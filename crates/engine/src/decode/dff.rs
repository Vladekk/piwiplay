//! DFF / DSDIFF (Philips) container decoder. DFF uses big-endian IFF-style
//! chunks. 1-bit samples are **MSB-first**, byte-interleaved across channels.
//!
//! Two payload kinds are supported natively:
//! * **`DSD ` (uncompressed)** — raw byte-interleaved DSD.
//! * **`DST ` (compressed)** — DST-compressed frames, decoded back to raw DSD by
//!   [`piwiplay_dst`], so DST also plays through the native DSD path.

use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::Path;

use piwiplay_dst::{samples_per_frame, DstDecoder};

use super::{be_u16, be_u32, be_u64, Decoder};
use crate::error::{EngineError, Result};
use crate::types::{BitOrder, DsdInfo, Tags};

/// A DST frame's location within the file.
#[derive(Clone, Copy)]
struct FrameLoc {
    offset: u64,
    size: usize,
}

/// State for decoding a DST (compressed) payload on the fly.
struct DstState {
    decoder: DstDecoder,
    frames: Vec<FrameLoc>,
    /// Bytes per channel per fully-decoded frame.
    frame_bytes: usize,
    /// Samples (bits) per channel per frame.
    frame_samples: usize,
    /// Next frame to decode.
    frame_idx: usize,
    /// Currently-decoded frame's planar bytes and how much has been served.
    buf: Vec<Vec<u8>>,
    buf_off: usize,
}

pub struct DffDecoder {
    file: File,
    info: DsdInfo,
    tags: Tags,
    channels: usize,
    /// Uncompressed data start (raw mode only).
    data_start: u64,
    total_per_chan: u64,
    pos_per_chan: u64,
    dst: Option<DstState>,
}

impl DffDecoder {
    pub fn open(path: &Path) -> Result<Self> {
        let mut file = File::open(path).map_err(|e| EngineError::io(path, e))?;
        let mut form = [0u8; 16];
        file.read_exact(&mut form).map_err(|e| EngineError::io(path, e))?;
        if &form[0..4] != b"FRM8" || &form[12..16] != b"DSD " {
            return Err(EngineError::BadFile("not a DSDIFF FRM8/DSD file".into()));
        }

        let mut channels = 0usize;
        let mut sample_rate = 0u32;
        let mut compressed = false;
        let mut data_start = 0u64; // uncompressed DSD payload
        let mut data_len = 0u64;
        let mut dst_body: Option<(u64, u64)> = None; // (start, len) of DST chunk body

        // Scan top-level chunks (id[4] + size[BE u64] + body, even-padded).
        let mut pos = 16u64;
        loop {
            file.seek(SeekFrom::Start(pos)).map_err(|e| EngineError::io(path, e))?;
            let mut ch = [0u8; 12];
            if file.read_exact(&mut ch).is_err() {
                break;
            }
            let id = [ch[0], ch[1], ch[2], ch[3]];
            let size = be_u64(&ch, 4);
            let body_start = pos + 12;
            match &id {
                b"PROP" => {
                    let mut body = vec![0u8; size as usize];
                    file.read_exact(&mut body).map_err(|e| EngineError::io(path, e))?;
                    parse_prop(&body, &mut channels, &mut sample_rate, &mut compressed)?;
                }
                b"DSD " => {
                    data_start = body_start;
                    data_len = size;
                }
                b"DST " => {
                    dst_body = Some((body_start, size));
                }
                _ => {}
            }
            pos = body_start + size + (size & 1);
        }

        if channels == 0 || sample_rate == 0 {
            return Err(EngineError::BadFile("missing PROP chunk".into()));
        }

        // ---- DST (compressed) payload ----
        if compressed || dst_body.is_some() {
            let (start, len) = dst_body.ok_or_else(|| EngineError::BadFile("DST flagged but no DST chunk".into()))?;
            let frames = scan_dst_frames(&mut file, start, len).map_err(|e| EngineError::io(path, e))?;
            if frames.is_empty() {
                return Err(EngineError::BadFile("no DST frames".into()));
            }
            let frame_samples = samples_per_frame(sample_rate);
            if frame_samples == 0 {
                return Err(EngineError::BadFile("nonstandard DSD rate for DST".into()));
            }
            let frame_bytes = frame_samples / 8;
            let total_per_chan = frames.len() as u64 * frame_bytes as u64;
            let samples_per_channel = total_per_chan * 8;
            return Ok(Self {
                file,
                info: DsdInfo { channels: channels as u32, sample_rate, bit_order: BitOrder::Msb, samples_per_channel },
                tags: Tags::default(),
                channels,
                data_start: 0,
                total_per_chan,
                pos_per_chan: 0,
                dst: Some(DstState {
                    decoder: DstDecoder::new(),
                    frames,
                    frame_bytes,
                    frame_samples,
                    frame_idx: 0,
                    buf: Vec::new(),
                    buf_off: 0,
                }),
            });
        }

        // ---- uncompressed DSD payload ----
        if data_start == 0 {
            return Err(EngineError::BadFile("missing DSD/DST data chunk".into()));
        }
        let total_per_chan = data_len / channels as u64;
        let samples_per_channel = total_per_chan * 8;
        file.seek(SeekFrom::Start(data_start)).map_err(|e| EngineError::io(path, e))?;

        Ok(Self {
            file,
            info: DsdInfo { channels: channels as u32, sample_rate, bit_order: BitOrder::Msb, samples_per_channel },
            tags: Tags::default(),
            channels,
            data_start,
            total_per_chan,
            pos_per_chan: 0,
            dst: None,
        })
    }

    /// Decode the DST frame at `frame_idx` into `buf` (planar).
    fn decode_dst_frame(&mut self, frame_idx: usize) -> io::Result<()> {
        let dst = self.dst.as_mut().expect("dst mode");
        let loc = dst.frames[frame_idx];
        let mut pkt = vec![0u8; loc.size];
        self.file.seek(SeekFrom::Start(loc.offset))?;
        self.file.read_exact(&mut pkt)?;
        match dst.decoder.decode_frame(&pkt, self.channels, dst.frame_samples) {
            Ok(planes) => {
                dst.buf = planes;
                dst.buf_off = 0;
                Ok(())
            }
            Err(e) => Err(io::Error::new(io::ErrorKind::InvalidData, e.to_string())),
        }
    }

    fn read_planar_dst(&mut self, max_per_chan: usize, out: &mut Vec<Vec<u8>>) -> io::Result<usize> {
        // Ensure the current decoded frame has bytes available.
        loop {
            let (empty, more) = {
                let dst = self.dst.as_ref().unwrap();
                (dst.buf.is_empty() || dst.buf_off >= dst.frame_bytes, dst.frame_idx < dst.frames.len())
            };
            if !empty {
                break;
            }
            if !more {
                return Ok(0);
            }
            let idx = self.dst.as_ref().unwrap().frame_idx;
            self.decode_dst_frame(idx)?;
            self.dst.as_mut().unwrap().frame_idx += 1;
        }

        let dst = self.dst.as_mut().unwrap();
        let avail = dst.frame_bytes - dst.buf_off;
        let n = max_per_chan.min(avail);
        out.clear();
        for c in 0..self.channels {
            out.push(dst.buf[c][dst.buf_off..dst.buf_off + n].to_vec());
        }
        dst.buf_off += n;
        self.pos_per_chan += n as u64;
        Ok(n)
    }
}

fn parse_prop(body: &[u8], channels: &mut usize, rate: &mut u32, compressed: &mut bool) -> Result<()> {
    if body.len() < 4 || &body[0..4] != b"SND " {
        return Err(EngineError::BadFile("PROP is not SND type".into()));
    }
    let mut i = 4usize;
    while i + 12 <= body.len() {
        let id = &body[i..i + 4];
        let size = be_u64(body, i + 4) as usize;
        let start = i + 12;
        let end = (start + size).min(body.len());
        match id {
            b"FS  " if end >= start + 4 => *rate = be_u32(body, start),
            b"CHNL" if end >= start + 2 => *channels = be_u16(body, start) as usize,
            b"CMPR" if end >= start + 4 => *compressed = &body[start..start + 4] != b"DSD ",
            _ => {}
        }
        i = start + size + (size & 1);
    }
    Ok(())
}

/// Parse the DST sound-data chunk into its list of DSTF frame locations.
/// The `DST ` body contains an `FRTE` info chunk then `DSTF` (+ optional `DSTC`).
fn scan_dst_frames(file: &mut File, start: u64, len: u64) -> io::Result<Vec<FrameLoc>> {
    let mut frames = Vec::new();
    let mut pos = start;
    let end = start + len;
    while pos + 12 <= end {
        file.seek(SeekFrom::Start(pos))?;
        let mut hdr = [0u8; 12];
        if file.read_exact(&mut hdr).is_err() {
            break;
        }
        let id = [hdr[0], hdr[1], hdr[2], hdr[3]];
        let size = be_u64(&hdr, 4);
        let body = pos + 12;
        if &id == b"DSTF" {
            frames.push(FrameLoc { offset: body, size: size as usize });
        }
        pos = body + size + (size & 1);
    }
    Ok(frames)
}

impl Decoder for DffDecoder {
    fn info(&self) -> &DsdInfo {
        &self.info
    }
    fn tags(&self) -> &Tags {
        &self.tags
    }

    fn read_planar(&mut self, max_per_chan: usize, out: &mut Vec<Vec<u8>>) -> io::Result<usize> {
        if self.dst.is_some() {
            return self.read_planar_dst(max_per_chan, out);
        }
        // uncompressed byte-interleaved
        let remaining = (self.total_per_chan - self.pos_per_chan) as usize;
        let n = max_per_chan.min(remaining);
        if n == 0 {
            return Ok(0);
        }
        let mut buf = vec![0u8; n * self.channels];
        let mut got = 0;
        while got < buf.len() {
            match self.file.read(&mut buf[got..])? {
                0 => break,
                k => got += k,
            }
        }
        let frames = got / self.channels;
        out.clear();
        for c in 0..self.channels {
            let mut plane = Vec::with_capacity(frames);
            for f in 0..frames {
                plane.push(buf[f * self.channels + c]);
            }
            out.push(plane);
        }
        self.pos_per_chan += frames as u64;
        Ok(frames)
    }

    fn seek_bytes(&mut self, per_chan_byte: u64) -> io::Result<u64> {
        let target = per_chan_byte.min(self.total_per_chan);
        if let Some(dst) = self.dst.as_ref() {
            let fb = dst.frame_bytes.max(1) as u64;
            let frame = (target / fb) as usize;
            let within = (target % fb) as usize;
            self.dst.as_mut().unwrap().frame_idx = frame;
            self.dst.as_mut().unwrap().buf.clear();
            self.pos_per_chan = frame as u64 * fb;
            if frame < self.dst.as_ref().unwrap().frames.len() {
                self.decode_dst_frame(frame)?;
                self.dst.as_mut().unwrap().frame_idx = frame + 1;
                let clamp = within.min(self.dst.as_ref().unwrap().frame_bytes);
                self.dst.as_mut().unwrap().buf_off = clamp;
                self.pos_per_chan += clamp as u64;
            }
            return Ok(self.pos_per_chan);
        }
        let file_off = self.data_start + target * self.channels as u64;
        self.file.seek(SeekFrom::Start(file_off))?;
        self.pos_per_chan = target;
        Ok(target)
    }

    fn position_bytes(&self) -> u64 {
        self.pos_per_chan
    }

    fn total_bytes(&self) -> u64 {
        self.total_per_chan
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testgen::{dff_bytes, plane_byte};
    use std::io::Write;

    fn write_tmp(bytes: &[u8]) -> tempfile::NamedTempFile {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        f.write_all(bytes).unwrap();
        f.flush().unwrap();
        f
    }

    #[test]
    fn parses_and_deinterleaves() {
        let f = write_tmp(&dff_bytes(2, 5_644_800, 40));
        let mut dec = DffDecoder::open(f.path()).unwrap();
        assert_eq!(dec.info().channels, 2);
        assert_eq!(dec.info().sample_rate, 5_644_800);
        assert_eq!(dec.info().bit_order, BitOrder::Msb);
        assert_eq!(dec.total_bytes(), 40);

        let mut acc = vec![Vec::new(); 2];
        let mut out = Vec::new();
        loop {
            let n = dec.read_planar(9, &mut out).unwrap();
            if n == 0 {
                break;
            }
            for c in 0..2 {
                acc[c].extend_from_slice(&out[c]);
            }
        }
        for c in 0..2 {
            assert_eq!(acc[c].len(), 40);
            for i in 0..40 {
                assert_eq!(acc[c][i], plane_byte(c, i), "ch{c} byte{i}");
            }
        }
    }

    #[test]
    fn seek_deinterleaved() {
        let f = write_tmp(&dff_bytes(2, 5_644_800, 64));
        let mut dec = DffDecoder::open(f.path()).unwrap();
        assert_eq!(dec.seek_bytes(20).unwrap(), 20);
        let mut out = Vec::new();
        dec.read_planar(2, &mut out).unwrap();
        assert_eq!(out[0][0], plane_byte(0, 20));
        assert_eq!(out[1][0], plane_byte(1, 20));
    }
}
