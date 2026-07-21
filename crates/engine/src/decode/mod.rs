//! Container decoding: turn a `.dsf`/`.dff` file into a streaming source of
//! **planar** per-channel DSD bytes plus a [`DsdInfo`] description. The audio
//! layer repacks these planes into whatever interleave/bitorder the sink picks.

use std::io;
use std::path::Path;

use crate::error::{EngineError, Result};
use crate::types::{DsdInfo, Tags};

pub mod dff;
pub mod dsf;

/// A streaming DSD source. Implementations read block-aligned chunks lazily so
/// large files never need to be held fully in memory.
pub trait Decoder: Send {
    fn info(&self) -> &DsdInfo;
    fn tags(&self) -> &Tags;

    /// Read up to `max_per_chan` bytes per channel into `out` (one Vec per
    /// channel, cleared and refilled). Returns the number of per-channel bytes
    /// produced; `0` means end of stream.
    fn read_planar(&mut self, max_per_chan: usize, out: &mut Vec<Vec<u8>>) -> io::Result<usize>;

    /// Seek to a per-channel byte offset (implementations align as required).
    /// Returns the actual per-channel byte offset landed on.
    fn seek_bytes(&mut self, per_chan_byte: u64) -> io::Result<u64>;

    /// Current per-channel byte position.
    fn position_bytes(&self) -> u64;

    /// Total per-channel bytes in the stream (excludes container padding).
    fn total_bytes(&self) -> u64 {
        self.info().samples_per_channel / 8
    }
}

/// Open a DSD file, dispatching by extension then magic bytes.
pub fn open(path: &Path) -> Result<Box<dyn Decoder>> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "dsf" => Ok(Box::new(dsf::DsfDecoder::open(path)?)),
        "dff" | "dsdiff" => Ok(Box::new(dff::DffDecoder::open(path)?)),
        _ => Err(EngineError::BadFile(format!("unrecognized extension: .{ext}"))),
    }
}

/// True if the extension is one this engine can (attempt to) play.
pub fn is_supported(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|e| e.to_str()).map(|e| e.to_ascii_lowercase()).as_deref(),
        Some("dsf") | Some("dff") | Some("dsdiff")
    )
}

// ---- shared little-endian / big-endian readers over an in-memory header ----

pub(crate) fn le_u32(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(b[o..o + 4].try_into().unwrap())
}
pub(crate) fn le_u64(b: &[u8], o: usize) -> u64 {
    u64::from_le_bytes(b[o..o + 8].try_into().unwrap())
}
pub(crate) fn be_u16(b: &[u8], o: usize) -> u16 {
    u16::from_be_bytes(b[o..o + 2].try_into().unwrap())
}
pub(crate) fn be_u32(b: &[u8], o: usize) -> u32 {
    u32::from_be_bytes(b[o..o + 4].try_into().unwrap())
}
pub(crate) fn be_u64(b: &[u8], o: usize) -> u64 {
    u64::from_be_bytes(b[o..o + 8].try_into().unwrap())
}
