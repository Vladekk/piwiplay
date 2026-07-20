//! PCM decoding via the `ffmpeg` CLI (spec-v2).
//!
//! Rather than link libav* (whose API churns across major versions), we shell
//! out to the `ffmpeg` binary and read raw interleaved `f32le` from its stdout.
//! This literally uses ffmpeg, supports every format ffmpeg demuxes (FLAC, ALAC,
//! WAV/AIFF, MP3, AAC, Opus, Vorbis, …) **and** DSD (`.dsf`/`.dff`, including
//! DST) decoded to PCM. `ffprobe` supplies stream metadata.
//!
//! This path is used for: non-DSD files, and DSD files when the user switches a
//! track from native to transcoded (so software volume can apply).

use std::io::{self, Read};
use std::path::Path;
use std::process::{Child, ChildStdout, Command, Stdio};
use std::time::Duration;

/// Interleaved f32 PCM description.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PcmInfo {
    pub rate: u32,
    pub channels: u32,
}

impl PcmInfo {
    /// Bytes per interleaved frame (f32 per channel).
    pub fn stride(&self) -> usize {
        self.channels as usize * 4
    }
}

/// Result of probing a file with ffprobe.
#[derive(Debug, Clone)]
pub struct ProbeInfo {
    pub codec: String,
    pub rate: u32,
    pub channels: u32,
    pub duration: Duration,
    pub is_dsd: bool,
}

impl ProbeInfo {
    /// Target PCM rate for playback, clamped to a sane DAC range. ffprobe
    /// already reports a DSD stream's rate as its decimated PCM rate (DSD64 →
    /// 352 800 Hz), so no extra division is needed.
    pub fn target_pcm(&self) -> PcmInfo {
        PcmInfo { rate: self.rate.clamp(8_000, 384_000), channels: self.channels.clamp(1, 8) }
    }
}

/// Common audio extensions ffmpeg can decode (fast filter for the browser /
/// folder scan; the authoritative check is [`probe`]).
pub fn is_supported_ext(path: &Path) -> bool {
    const EXTS: &[&str] = &[
        "flac", "wav", "aiff", "aif", "mp3", "aac", "m4a", "mp4", "ogg", "oga", "opus", "wv",
        "ape", "alac", "wma", "mka", "mpc", "tta", "ac3", "dts",
    ];
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .map(|e| EXTS.contains(&e.as_str()))
        .unwrap_or(false)
}

/// Whether the `ffmpeg` and `ffprobe` binaries are available.
pub fn available() -> bool {
    which("ffmpeg") && which("ffprobe")
}

fn which(bin: &str) -> bool {
    Command::new(bin)
        .arg("-version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Probe a file's first audio stream. Returns None if ffprobe is unavailable
/// or the file has no audio stream.
pub fn probe(path: &Path) -> Option<ProbeInfo> {
    let out = Command::new("ffprobe")
        .args([
            "-v", "error",
            "-select_streams", "a:0",
            "-show_entries", "stream=codec_name,sample_rate,channels:format=duration",
            "-of", "default=noprint_wrappers=1:nokey=0",
        ])
        .arg(path)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let mut codec = String::new();
    let mut rate = 0u32;
    let mut channels = 0u32;
    let mut duration = 0f64;
    for line in text.lines() {
        let (k, v) = line.split_once('=')?;
        match k.trim() {
            "codec_name" => codec = v.trim().to_string(),
            "sample_rate" => rate = v.trim().parse().unwrap_or(0),
            "channels" => channels = v.trim().parse().unwrap_or(0),
            "duration" => duration = v.trim().parse().unwrap_or(0.0),
            _ => {}
        }
    }
    if rate == 0 || channels == 0 {
        return None;
    }
    let is_dsd = codec.starts_with("dsd");
    Some(ProbeInfo {
        codec,
        rate,
        channels,
        duration: Duration::from_secs_f64(duration.max(0.0)),
        is_dsd,
    })
}

/// A running ffmpeg decode: raw interleaved f32le on stdout.
pub struct PcmSource {
    child: Child,
    out: ChildStdout,
    pub info: PcmInfo,
}

impl PcmSource {
    /// Spawn ffmpeg decoding `path` to `info` (rate/channels) as f32le, seeking
    /// to `start` first.
    pub fn open(path: &Path, info: PcmInfo, start: Duration) -> io::Result<Self> {
        let mut cmd = Command::new("ffmpeg");
        cmd.arg("-v").arg("error").arg("-nostdin");
        if start > Duration::ZERO {
            cmd.arg("-ss").arg(format!("{:.3}", start.as_secs_f64()));
        }
        cmd.arg("-i").arg(path);
        cmd.arg("-f").arg("f32le").arg("-acodec").arg("pcm_f32le");
        cmd.arg("-ar").arg(info.rate.to_string());
        cmd.arg("-ac").arg(info.channels.to_string());
        cmd.arg("-");
        cmd.stdout(Stdio::piped()).stderr(Stdio::null()).stdin(Stdio::null());
        let mut child = cmd.spawn()?;
        let out = child.stdout.take().expect("piped stdout");
        Ok(PcmSource { child, out, info })
    }

    /// Read raw f32le bytes; returns 0 at end of stream.
    pub fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.out.read(buf)
    }
}

impl Drop for PcmSource {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Apply a linear gain to a buffer of little-endian f32 samples in place.
/// `carry` holds up to 3 trailing bytes that did not complete a sample; it is
/// prepended on the next call so scaling never splits a sample.
pub fn apply_gain_f32le(buf: &mut Vec<u8>, gain: f32) {
    let n = buf.len() / 4 * 4;
    for chunk in buf[..n].chunks_exact_mut(4) {
        let s = f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
        let scaled = (s * gain).clamp(-1.0, 1.0);
        chunk.copy_from_slice(&scaled.to_le_bytes());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn target_pcm_for_normal_audio_is_source_rate() {
        let p = ProbeInfo { codec: "flac".into(), rate: 96_000, channels: 2, duration: Duration::ZERO, is_dsd: false };
        assert_eq!(p.target_pcm(), PcmInfo { rate: 96_000, channels: 2 });
    }

    #[test]
    fn target_pcm_for_dsd_uses_ffprobe_reported_rate() {
        // ffprobe reports DSD64 as 352_800 Hz already (decimated PCM rate).
        let p = ProbeInfo { codec: "dsd_lsbf_planar".into(), rate: 352_800, channels: 2, duration: Duration::ZERO, is_dsd: true };
        assert_eq!(p.target_pcm(), PcmInfo { rate: 352_800, channels: 2 });
    }

    #[test]
    fn target_pcm_clamps_extreme_rates() {
        let p = ProbeInfo { codec: "pcm".into(), rate: 768_000, channels: 2, duration: Duration::ZERO, is_dsd: false };
        assert_eq!(p.target_pcm().rate, 384_000);
    }

    #[test]
    fn stride_is_four_bytes_per_channel() {
        assert_eq!(PcmInfo { rate: 44_100, channels: 2 }.stride(), 8);
    }

    #[test]
    fn gain_scales_full_samples_and_ignores_partial_tail() {
        // two f32 = 1.0 plus a stray byte
        let mut buf = Vec::new();
        buf.extend_from_slice(&1.0f32.to_le_bytes());
        buf.extend_from_slice(&(-1.0f32).to_le_bytes());
        buf.push(0x7f); // partial sample, must be left untouched
        apply_gain_f32le(&mut buf, 0.5);
        assert_eq!(f32::from_le_bytes(buf[0..4].try_into().unwrap()), 0.5);
        assert_eq!(f32::from_le_bytes(buf[4..8].try_into().unwrap()), -0.5);
        assert_eq!(buf[8], 0x7f);
    }

    #[test]
    fn gain_clamps_to_unit_range() {
        let mut buf = 2.0f32.to_le_bytes().to_vec();
        apply_gain_f32le(&mut buf, 4.0);
        assert_eq!(f32::from_le_bytes(buf[0..4].try_into().unwrap()), 1.0);
    }
}
