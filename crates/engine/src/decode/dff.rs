//! DFF / DSDIFF (Philips) container decoder. DFF uses big-endian IFF-style
//! chunks. 1-bit samples are **MSB-first**, byte-interleaved across channels
//! (`[ch0][ch1][ch0][ch1]…` at 1-byte granularity). DST (compressed) is
//! rejected — v1 plays only raw DSD.

use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::Path;

use super::{be_u32, be_u64, Decoder};
use crate::error::{EngineError, Result};
use crate::types::{BitOrder, DsdInfo, Tags};

pub struct DffDecoder {
    file: File,
    info: DsdInfo,
    tags: Tags,
    channels: usize,
    data_start: u64,
    total_per_chan: u64,
    pos_per_chan: u64,
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
        let mut data_start = 0u64;
        let mut data_len = 0u64;

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
                    return Err(EngineError::DstUnsupported);
                }
                _ => {}
            }
            // advance, IFF pads odd-sized chunk bodies to an even boundary
            pos = body_start + size + (size & 1);
        }

        if compressed {
            return Err(EngineError::DstUnsupported);
        }
        if channels == 0 || sample_rate == 0 || data_start == 0 {
            return Err(EngineError::BadFile("missing PROP/DSD chunk".into()));
        }

        let total_per_chan = data_len / channels as u64;
        let samples_per_channel = total_per_chan * 8;
        file.seek(SeekFrom::Start(data_start)).map_err(|e| EngineError::io(path, e))?;

        Ok(Self {
            file,
            info: DsdInfo {
                channels: channels as u32,
                sample_rate,
                bit_order: BitOrder::Msb,
                samples_per_channel,
            },
            tags: Tags::default(), // DFF DIIN metadata is rarely present; skipped in v1
            channels,
            data_start,
            total_per_chan,
            pos_per_chan: 0,
        })
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
            b"CHNL" if end >= start + 2 => {
                *channels = u16::from_be_bytes([body[start], body[start + 1]]) as usize;
            }
            b"CMPR" if end >= start + 4 => {
                *compressed = &body[start..start + 4] != b"DSD ";
            }
            _ => {}
        }
        i = start + size + (size & 1);
    }
    Ok(())
}

impl Decoder for DffDecoder {
    fn info(&self) -> &DsdInfo {
        &self.info
    }
    fn tags(&self) -> &Tags {
        &self.tags
    }

    fn read_planar(&mut self, max_per_chan: usize, out: &mut Vec<Vec<u8>>) -> io::Result<usize> {
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
    use crate::testgen::{dff_bytes, dff_dst_bytes, plane_byte};
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

    #[test]
    fn rejects_dst() {
        let f = write_tmp(&dff_dst_bytes(2, 2_822_400));
        assert!(matches!(DffDecoder::open(f.path()), Err(EngineError::DstUnsupported)));
    }
}
