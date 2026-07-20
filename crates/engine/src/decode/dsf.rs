//! DSF (Sony) container decoder. DSF stores 1-bit samples **LSB-first** in
//! per-channel planar blocks (`block_size` bytes each), arranged as
//! `[ch0][ch1]…[ch0][ch1]…`. Optional trailing ID3v2 metadata.

use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::Path;

use super::{le_u32, le_u64, Decoder};
use crate::error::{EngineError, Result};
use crate::types::{BitOrder, DsdInfo, Tags};

pub struct DsfDecoder {
    file: File,
    info: DsdInfo,
    tags: Tags,
    channels: usize,
    block_size: usize,
    data_start: u64,
    /// True per-channel byte length (excludes block padding).
    total_per_chan: u64,
    pos_per_chan: u64,
    /// Buffered planar bytes from the current super-block.
    leftover: Vec<Vec<u8>>,
    leftover_off: usize,
}

impl DsfDecoder {
    pub fn open(path: &Path) -> Result<Self> {
        let mut file = File::open(path).map_err(|e| EngineError::io(path, e))?;
        // Header region: DSD (28) + fmt (52) + data header (12) = 92 bytes.
        let mut hdr = [0u8; 92];
        file.read_exact(&mut hdr).map_err(|e| EngineError::io(path, e))?;

        if &hdr[0..4] != b"DSD " {
            return Err(EngineError::BadFile("missing DSD chunk".into()));
        }
        let metadata_ptr = le_u64(&hdr, 20);

        let fmt = 28;
        if &hdr[fmt..fmt + 4] != b"fmt " {
            return Err(EngineError::BadFile("missing fmt chunk".into()));
        }
        let format_id = le_u32(&hdr, fmt + 16);
        if format_id != 0 {
            return Err(EngineError::BadFile(format!("unsupported DSF format id {format_id}")));
        }
        let channels = le_u32(&hdr, fmt + 24) as usize;
        let sample_rate = le_u32(&hdr, fmt + 28);
        let samples_per_channel = le_u64(&hdr, fmt + 36);
        let block_size = le_u32(&hdr, fmt + 44) as usize;

        if channels == 0 || channels > 8 || block_size == 0 {
            return Err(EngineError::BadFile("nonsensical channel/block values".into()));
        }

        let data_hdr = fmt + 52;
        if &hdr[data_hdr..data_hdr + 4] != b"data" {
            return Err(EngineError::BadFile("missing data chunk".into()));
        }
        let data_start = (data_hdr + 12) as u64;
        let total_per_chan = samples_per_channel / 8;

        let tags = if metadata_ptr != 0 {
            read_id3(&mut file, metadata_ptr).unwrap_or_default()
        } else {
            Tags::default()
        };

        file.seek(SeekFrom::Start(data_start)).map_err(|e| EngineError::io(path, e))?;

        Ok(Self {
            file,
            info: DsdInfo {
                channels: channels as u32,
                sample_rate,
                bit_order: BitOrder::Lsb,
                samples_per_channel,
            },
            tags,
            channels,
            block_size,
            data_start,
            total_per_chan,
            pos_per_chan: 0,
            leftover: vec![Vec::new(); channels],
            leftover_off: 0,
        })
    }

    /// Read the next super-block (`block_size * channels` bytes) into `leftover`.
    /// Returns false at EOF.
    fn fill_super_block(&mut self) -> io::Result<bool> {
        let want = self.block_size * self.channels;
        let mut buf = vec![0u8; want];
        let mut got = 0;
        while got < want {
            match self.file.read(&mut buf[got..])? {
                0 => break,
                n => got += n,
            }
        }
        if got == 0 {
            return Ok(false);
        }
        // Split into per-channel planes; a short final block yields fewer bytes.
        let per = got / self.channels;
        for c in 0..self.channels {
            let s = c * self.block_size;
            self.leftover[c].clear();
            self.leftover[c].extend_from_slice(&buf[s..s + per.min(self.block_size)]);
        }
        self.leftover_off = 0;
        Ok(true)
    }
}

impl Decoder for DsfDecoder {
    fn info(&self) -> &DsdInfo {
        &self.info
    }
    fn tags(&self) -> &Tags {
        &self.tags
    }

    fn read_planar(&mut self, max_per_chan: usize, out: &mut Vec<Vec<u8>>) -> io::Result<usize> {
        if self.pos_per_chan >= self.total_per_chan {
            return Ok(0);
        }
        if self.leftover_off >= self.leftover[0].len() && !self.fill_super_block()? {
            return Ok(0);
        }
        let avail = self.leftover[0].len() - self.leftover_off;
        let remaining_true = (self.total_per_chan - self.pos_per_chan) as usize;
        let n = max_per_chan.min(avail).min(remaining_true);

        out.clear();
        for c in 0..self.channels {
            out.push(self.leftover[c][self.leftover_off..self.leftover_off + n].to_vec());
        }
        self.leftover_off += n;
        self.pos_per_chan += n as u64;
        Ok(n)
    }

    fn seek_bytes(&mut self, per_chan_byte: u64) -> io::Result<u64> {
        let target = per_chan_byte.min(self.total_per_chan);
        let block_index = target / self.block_size as u64;
        let within = (target % self.block_size as u64) as usize;
        let file_off = self.data_start + block_index * (self.block_size * self.channels) as u64;
        self.file.seek(SeekFrom::Start(file_off))?;
        self.leftover_off = 0;
        self.leftover.iter_mut().for_each(|p| p.clear());
        self.pos_per_chan = block_index * self.block_size as u64;
        if self.fill_super_block()? {
            let clamp = within.min(self.leftover[0].len());
            self.leftover_off = clamp;
            self.pos_per_chan += clamp as u64;
        }
        Ok(self.pos_per_chan)
    }

    fn position_bytes(&self) -> u64 {
        self.pos_per_chan
    }
}

/// Best-effort ID3v2 reader: extracts TIT2/TPE1/TALB. Handles v2.3 and v2.4
/// text frames with latin-1, UTF-8, and UTF-16 encodings. Ignores anything it
/// cannot parse rather than failing the file open.
fn read_id3(file: &mut File, offset: u64) -> Option<Tags> {
    file.seek(SeekFrom::Start(offset)).ok()?;
    let mut hdr = [0u8; 10];
    file.read_exact(&mut hdr).ok()?;
    if &hdr[0..3] != b"ID3" {
        return None;
    }
    let major = hdr[3];
    let size = syncsafe(&hdr[6..10]);
    let mut body = vec![0u8; size as usize];
    file.read_exact(&mut body).ok()?;

    let mut tags = Tags::default();
    let mut i = 0usize;
    while i + 10 <= body.len() {
        let id = &body[i..i + 4];
        if id == [0, 0, 0, 0] {
            break;
        }
        let fsize = if major >= 4 {
            syncsafe(&body[i + 4..i + 8]) as usize
        } else {
            u32::from_be_bytes(body[i + 4..i + 8].try_into().ok()?) as usize
        };
        let start = i + 10;
        let end = (start + fsize).min(body.len());
        if start > body.len() || fsize == 0 {
            break;
        }
        let frame = &body[start..end];
        let text = decode_text_frame(frame);
        match id {
            b"TIT2" => tags.title = text,
            b"TPE1" => tags.artist = text,
            b"TALB" => tags.album = text,
            _ => {}
        }
        i = end;
    }
    Some(tags)
}

fn syncsafe(b: &[u8]) -> u32 {
    ((b[0] as u32 & 0x7f) << 21)
        | ((b[1] as u32 & 0x7f) << 14)
        | ((b[2] as u32 & 0x7f) << 7)
        | (b[3] as u32 & 0x7f)
}

fn decode_text_frame(frame: &[u8]) -> Option<String> {
    let (&enc, rest) = frame.split_first()?;
    let s = match enc {
        0 => rest.iter().map(|&b| b as char).collect::<String>(), // latin-1
        3 => String::from_utf8_lossy(rest).into_owned(),          // utf-8
        1 | 2 => decode_utf16(rest),                              // utf-16 (bom or be)
        _ => String::from_utf8_lossy(rest).into_owned(),
    };
    let s = s.trim_end_matches('\0').trim().to_string();
    (!s.is_empty()).then_some(s)
}

fn decode_utf16(bytes: &[u8]) -> String {
    let (le, data) = match bytes {
        [0xff, 0xfe, rest @ ..] => (true, rest),
        [0xfe, 0xff, rest @ ..] => (false, rest),
        rest => (false, rest),
    };
    let units: Vec<u16> = data
        .chunks_exact(2)
        .map(|c| if le { u16::from_le_bytes([c[0], c[1]]) } else { u16::from_be_bytes([c[0], c[1]]) })
        .collect();
    String::from_utf16_lossy(&units)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testgen::{dsf_bytes, plane_byte};
    use std::io::Write;

    fn write_tmp(bytes: &[u8]) -> tempfile::NamedTempFile {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        f.write_all(bytes).unwrap();
        f.flush().unwrap();
        f
    }

    /// Read the whole stream via many small reads; return concatenated planes.
    fn drain(dec: &mut DsfDecoder, chunk: usize) -> Vec<Vec<u8>> {
        let ch = dec.info().channels as usize;
        let mut acc = vec![Vec::new(); ch];
        let mut out = Vec::new();
        loop {
            let n = dec.read_planar(chunk, &mut out).unwrap();
            if n == 0 {
                break;
            }
            for c in 0..ch {
                acc[c].extend_from_slice(&out[c]);
            }
        }
        acc
    }

    #[test]
    fn parses_header_fields() {
        let f = write_tmp(&dsf_bytes(2, 2_822_400, 50, 16));
        let dec = DsfDecoder::open(f.path()).unwrap();
        assert_eq!(dec.info().channels, 2);
        assert_eq!(dec.info().sample_rate, 2_822_400);
        assert_eq!(dec.info().bit_order, BitOrder::Lsb);
        assert_eq!(dec.info().samples_per_channel, 400);
        assert_eq!(dec.total_bytes(), 50);
    }

    #[test]
    fn round_trip_planar_bytes_across_blocks() {
        // per_chan=50 spans 4 blocks of 16 (last padded); ensure padding is dropped.
        let f = write_tmp(&dsf_bytes(2, 2_822_400, 50, 16));
        let mut dec = DsfDecoder::open(f.path()).unwrap();
        let acc = drain(&mut dec, 7); // odd chunk size crosses block boundaries
        for c in 0..2 {
            assert_eq!(acc[c].len(), 50, "channel {c} length");
            for i in 0..50 {
                assert_eq!(acc[c][i], plane_byte(c, i), "ch{c} byte{i}");
            }
        }
    }

    #[test]
    fn seek_lands_on_requested_byte() {
        let f = write_tmp(&dsf_bytes(2, 2_822_400, 100, 16));
        let mut dec = DsfDecoder::open(f.path()).unwrap();
        let landed = dec.seek_bytes(37).unwrap();
        assert_eq!(landed, 37);
        assert_eq!(dec.position_bytes(), 37);
        let mut out = Vec::new();
        dec.read_planar(4, &mut out).unwrap();
        assert_eq!(out[0][0], plane_byte(0, 37));
        assert_eq!(out[1][0], plane_byte(1, 37));
    }

    #[test]
    fn rejects_non_dsf() {
        let f = write_tmp(b"NOTDSDATALL............");
        assert!(DsfDecoder::open(f.path()).is_err());
    }
}
