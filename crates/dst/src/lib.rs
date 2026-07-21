//! DST (Direct Stream Transfer) lossless decoder.
//!
//! Decodes DST-compressed DSD frames back to the raw 1-bit DSD stream (planar,
//! MSB-first), which the caller can then play natively. This is a faithful Rust
//! port of FFmpeg's `libavcodec/dstdec.c` (LGPL-2.1) — the arithmetic coder,
//! prediction filters, and table reading follow ISO/IEC 14496-3 Part 3
//! Subpart 10. **This crate is LGPL-2.1** (see Cargo.toml); it does not decode
//! to PCM (that is intentionally left to the native path).

#![allow(clippy::needless_range_loop)]

const DST_MAX_CHANNELS: usize = 6;
const DST_MAX_ELEMENTS: usize = 2 * DST_MAX_CHANNELS;

const FSETS_CODE_PRED_COEFF: [[i8; 3]; 3] = [[-8, 0, 0], [-16, 8, 0], [-9, -5, 6]];
const PROBS_CODE_PRED_COEFF: [[i8; 3]; 3] = [[-8, 0, 0], [-16, 8, 0], [-24, 24, -8]];

#[derive(Debug)]
pub enum DstError {
    InvalidData,
    Unsupported(&'static str),
}

impl std::fmt::Display for DstError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DstError::InvalidData => write!(f, "invalid DST data"),
            DstError::Unsupported(s) => write!(f, "unsupported DST feature: {s}"),
        }
    }
}
impl std::error::Error for DstError {}

type Result<T> = std::result::Result<T, DstError>;

/// Number of 1-bit samples per channel per DST frame for a DSD bit-rate.
/// (`588 * DSD_FS44`, where `DSD_FS44 = rate / 44100`.)
pub fn samples_per_frame(dsd_bit_rate: u32) -> usize {
    588 * (dsd_bit_rate as usize / 44100)
}

// ---- MSB-first bit reader ----

struct BitReader<'a> {
    data: &'a [u8],
    /// Absolute bit position from the start.
    pos: usize,
}

impl<'a> BitReader<'a> {
    fn new(data: &'a [u8]) -> Self {
        BitReader { data, pos: 0 }
    }
    fn total(&self) -> usize {
        self.data.len() * 8
    }
    fn left(&self) -> usize {
        self.total().saturating_sub(self.pos)
    }
    #[inline]
    fn get_bits1(&mut self) -> u32 {
        if self.pos >= self.total() {
            self.pos += 1;
            return 0;
        }
        let byte = self.data[self.pos >> 3];
        let bit = (byte >> (7 - (self.pos & 7))) & 1;
        self.pos += 1;
        bit as u32
    }
    #[inline]
    fn get_bits(&mut self, n: u32) -> u32 {
        let mut v = 0u32;
        for _ in 0..n {
            v = (v << 1) | self.get_bits1();
        }
        v
    }
    #[inline]
    fn get_sbits(&mut self, n: u32) -> i32 {
        let v = self.get_bits(n);
        if n == 0 {
            return 0;
        }
        // sign-extend from n bits
        let shift = 32 - n;
        ((v << shift) as i32) >> shift
    }
    #[inline]
    fn skip_bits1(&mut self) {
        self.pos += 1;
    }
}

#[inline]
fn av_log2(x: u32) -> i32 {
    if x == 0 {
        0
    } else {
        31 - x.leading_zeros() as i32
    }
}

#[inline]
fn reverse8(mut x: u8) -> u8 {
    x = (x >> 4) | (x << 4);
    x = ((x & 0xCC) >> 2) | ((x & 0x33) << 2);
    x = ((x & 0xAA) >> 1) | ((x & 0x55) << 1);
    x
}

// ---- Golomb (JPEG-LS unsigned) as used by DST ----

fn get_ur_golomb_jpegls(br: &mut BitReader, k: u32, limit: usize) -> i32 {
    let mut q = 0usize;
    while br.get_bits1() == 0 {
        q += 1;
        if q >= limit {
            break;
        }
    }
    let rem = if k > 0 { br.get_bits(k) } else { 0 };
    ((q as u32) << k | rem) as i32
}

fn get_sr_golomb_dst(br: &mut BitReader, k: u32) -> i32 {
    let left = br.left();
    let v = get_ur_golomb_jpegls(br, k, left);
    if v != 0 && br.get_bits1() != 0 {
        -v
    } else {
        v
    }
}

// ---- Tables (filter coefficient sets & probability tables) ----

struct Table {
    elements: usize,
    length: [usize; DST_MAX_ELEMENTS],
    coeff: [[i32; 128]; DST_MAX_ELEMENTS],
}

impl Table {
    fn new() -> Self {
        Table { elements: 0, length: [0; DST_MAX_ELEMENTS], coeff: [[0; 128]; DST_MAX_ELEMENTS] }
    }
}

fn read_map(br: &mut BitReader, t: &mut Table, map: &mut [usize; DST_MAX_CHANNELS], channels: usize) -> Result<()> {
    t.elements = 1;
    map[0] = 0;
    if br.get_bits1() == 0 {
        for ch in 1..channels {
            let bits = av_log2(t.elements as u32) + 1;
            map[ch] = br.get_bits(bits as u32) as usize;
            if map[ch] == t.elements {
                t.elements += 1;
                if t.elements >= DST_MAX_ELEMENTS {
                    return Err(DstError::InvalidData);
                }
            } else if map[ch] > t.elements {
                return Err(DstError::InvalidData);
            }
        }
    } else {
        map.iter_mut().for_each(|m| *m = 0);
    }
    Ok(())
}

fn read_uncoded_coeff(br: &mut BitReader, dst: &mut [i32], elements: usize, coeff_bits: u32, is_signed: bool, offset: i32) {
    for i in 0..elements {
        dst[i] = if is_signed { br.get_sbits(coeff_bits) } else { br.get_bits(coeff_bits) as i32 } + offset;
    }
}

fn read_table(
    br: &mut BitReader,
    t: &mut Table,
    code_pred_coeff: &[[i8; 3]; 3],
    length_bits: u32,
    coeff_bits: u32,
    is_signed: bool,
    offset: i32,
) -> Result<()> {
    for i in 0..t.elements {
        t.length[i] = br.get_bits(length_bits) as usize + 1;
        if br.get_bits1() == 0 {
            let len = t.length[i];
            read_uncoded_coeff(br, &mut t.coeff[i], len, coeff_bits, is_signed, offset);
        } else {
            let method = br.get_bits(2) as usize;
            if method == 3 {
                return Err(DstError::InvalidData);
            }
            read_uncoded_coeff(br, &mut t.coeff[i], method + 1, coeff_bits, is_signed, offset);
            let lsb_size = br.get_bits(3);
            for j in (method + 1)..t.length[i] {
                let mut x: i32 = 0;
                for k in 0..(method + 1) {
                    x = x.wrapping_add(code_pred_coeff[method][k] as i32 * t.coeff[i][j - k - 1]);
                }
                let mut c = get_sr_golomb_dst(br, lsb_size);
                if x >= 0 {
                    c -= (x + 4) / 8;
                } else {
                    c += (-x + 3) / 8;
                }
                if !is_signed && (c < offset || c >= offset + (1 << coeff_bits)) {
                    return Err(DstError::InvalidData);
                }
                t.coeff[i][j] = c;
            }
        }
    }
    Ok(())
}

// ---- Arithmetic coder ----

struct ArithCoder {
    a: u32,
    c: u32,
}

impl ArithCoder {
    fn init(br: &mut BitReader) -> Self {
        ArithCoder { a: 4095, c: br.get_bits(12) }
    }
    #[inline]
    fn get(&mut self, br: &mut BitReader, p: u32) -> i32 {
        let k = (self.a >> 8) | ((self.a >> 7) & 1);
        let q = k * p;
        let a_q = self.a - q;
        let e = (self.c < a_q) as i32;
        if e != 0 {
            self.a = a_q;
        } else {
            self.a = q;
            self.c -= a_q;
        }
        if self.a < 2048 {
            let n = 11 - av_log2(self.a);
            self.a <<= n;
            self.c = (self.c << n) | br.get_bits(n as u32);
        }
        e
    }
}

fn prob_dst_x_bit(c: i32) -> u32 {
    ((reverse8((c & 127) as u8) >> 1) + 1) as u32
}

fn build_filter(fsets: &Table) -> Result<Vec<[[i16; 256]; 16]>> {
    let mut table = vec![[[0i16; 256]; 16]; fsets.elements];
    for i in 0..fsets.elements {
        let length = fsets.length[i] as i32;
        for j in 0..16 {
            let total = (length - j as i32 * 8).clamp(0, 8);
            for k in 0..256 {
                let mut v: i64 = 0;
                for l in 0..total as usize {
                    let bit = ((k >> l) & 1) as i64; // +1 for a set bit, -1 otherwise
                    v += (bit * 2 - 1) * fsets.coeff[i][j * 8 + l] as i64;
                }
                if v as i16 as i64 != v {
                    return Err(DstError::InvalidData);
                }
                table[i][j][k] = v as i16;
            }
        }
    }
    Ok(table)
}

/// A reusable DST frame decoder. Each `decode_frame` call is self-contained
/// (tables are re-read from the packet), matching the FFmpeg design.
#[derive(Default)]
pub struct DstDecoder;

impl DstDecoder {
    pub fn new() -> Self {
        DstDecoder
    }

    /// Decode one DST frame packet into planar DSD (one `Vec<u8>` per channel,
    /// each `samples_per_frame / 8` bytes, MSB-first).
    pub fn decode_frame(&mut self, packet: &[u8], channels: usize, samples_per_frame: usize) -> Result<Vec<Vec<u8>>> {
        if channels == 0 || channels > DST_MAX_CHANNELS {
            return Err(DstError::Unsupported("channel count"));
        }
        if packet.len() <= 1 || samples_per_frame & 7 != 0 {
            return Err(DstError::InvalidData);
        }
        let nb_bytes = samples_per_frame / 8;
        let mut planes = vec![vec![0u8; nb_bytes]; channels];

        let mut br = BitReader::new(packet);

        // Uncoded frame: raw byte-interleaved DSD after a 1-byte header.
        if br.get_bits1() == 0 {
            br.skip_bits1();
            if br.get_bits(6) != 0 {
                return Err(DstError::InvalidData);
            }
            let body = &packet[1..];
            for b in 0..nb_bytes {
                for ch in 0..channels {
                    let idx = b * channels + ch;
                    if idx < body.len() {
                        planes[ch][b] = body[idx];
                    }
                }
            }
            return Ok(planes);
        }

        // Segmentation (must be "same" — the only case seen in practice / SACD).
        for _ in 0..3 {
            if br.get_bits1() == 0 {
                return Err(DstError::Unsupported("non-default segmentation"));
            }
        }

        let same_map = br.get_bits1() != 0;
        let mut fsets = Table::new();
        let mut probs = Table::new();
        let mut map_felem = [0usize; DST_MAX_CHANNELS];
        let mut map_pelem = [0usize; DST_MAX_CHANNELS];
        read_map(&mut br, &mut fsets, &mut map_felem, channels)?;
        if same_map {
            probs.elements = fsets.elements;
            map_pelem = map_felem;
        } else {
            read_map(&mut br, &mut probs, &mut map_pelem, channels)?;
        }

        let mut half_prob = [0u32; DST_MAX_CHANNELS];
        for ch in 0..channels {
            half_prob[ch] = br.get_bits1();
        }

        read_table(&mut br, &mut fsets, &FSETS_CODE_PRED_COEFF, 7, 9, true, 0)?;
        read_table(&mut br, &mut probs, &PROBS_CODE_PRED_COEFF, 6, 7, false, 1)?;

        if br.get_bits1() != 0 {
            return Err(DstError::InvalidData);
        }
        let mut ac = ArithCoder::init(&mut br);
        let filter = build_filter(&fsets)?;

        // 128-bit shift register per channel, init 0xAA bytes.
        let mut status = [0u128; DST_MAX_CHANNELS];
        let init = {
            let mut v = 0u128;
            for _ in 0..16 {
                v = (v << 8) | 0xAA;
            }
            v
        };
        for ch in 0..channels {
            status[ch] = init;
        }

        // Prime the coder (result unused, but consumes state — matches FFmpeg).
        let _ = ac.get(&mut br, prob_dst_x_bit(fsets.coeff[0][0]));

        for i in 0..samples_per_frame {
            for ch in 0..channels {
                let felem = map_felem[ch];
                let f = &filter[felem];
                let st = status[ch];
                let mut predict: i32 = 0;
                for x in 0..16 {
                    let byte = ((st >> (8 * x)) & 0xff) as usize;
                    predict = predict.wrapping_add(f[x][byte] as i32);
                }
                let predict = predict as i16; // FFmpeg truncates the sum to int16_t

                let prob = if half_prob[ch] == 0 || i >= fsets.length[felem] {
                    let pelem = map_pelem[ch];
                    let index = (predict.unsigned_abs() as usize) >> 3;
                    probs.coeff[pelem][index.min(probs.length[pelem] - 1)] as u32
                } else {
                    128
                };

                let residual = ac.get(&mut br, prob);
                let v = (((predict >> 15) as i32) ^ residual) & 1;
                planes[ch][i >> 3] |= (v as u8) << (7 - (i & 7));

                status[ch] = (st << 1) | (v as u128);
            }
        }

        Ok(planes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bit_reader_msb_first() {
        let mut br = BitReader::new(&[0b1011_0010, 0b0100_0000]);
        assert_eq!(br.get_bits1(), 1);
        assert_eq!(br.get_bits(3), 0b011);
        assert_eq!(br.get_bits(4), 0b0010);
        assert_eq!(br.get_bits1(), 0);
    }

    #[test]
    fn sign_extend() {
        let mut br = BitReader::new(&[0b1111_1110]); // 8-bit: -2
        assert_eq!(br.get_sbits(8), -2);
    }

    #[test]
    fn reverse_and_log2() {
        assert_eq!(reverse8(0b0000_0001), 0b1000_0000);
        assert_eq!(reverse8(0b0000_0011), 0b1100_0000);
        assert_eq!(av_log2(1), 0);
        assert_eq!(av_log2(255), 7);
        assert_eq!(av_log2(256), 8);
    }

    #[test]
    fn samples_per_frame_dsd64() {
        // DSD64 bit rate 2_822_400 -> 588 * 64 = 37632 bits/ch/frame
        assert_eq!(samples_per_frame(2_822_400), 37632);
    }

    #[test]
    fn uncoded_frame_deinterleaves() {
        // header byte 0b0X000000 then byte-interleaved DSD for 1 byte/ch (8 samples)
        let spf = 8; // 1 byte per channel
        let mut pkt = vec![0u8; 1 + 2]; // header + L0 R0
        pkt[0] = 0b0000_0000;
        pkt[1] = 0xC3; // L
        pkt[2] = 0x3C; // R
        let planes = DstDecoder::new().decode_frame(&pkt, 2, spf).unwrap();
        assert_eq!(planes[0][0], 0xC3);
        assert_eq!(planes[1][0], 0x3C);
    }
}
