//! Shared value types used across the engine and exposed to any frontend
//! (TUI today, a WebUI tomorrow). These are deliberately UI-agnostic.

use std::path::PathBuf;
use std::time::Duration;

/// DSD sub-rate family, derived from the 1-bit sample rate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DsdRate {
    Dsd64,
    Dsd128,
    Dsd256,
    Dsd512,
    /// Any other multiple of 44100 (or 48000) not covered above.
    Other(u32),
}

impl DsdRate {
    /// Classify a 1-bit sample rate (bits/sec/channel).
    pub fn from_hz(hz: u32) -> Self {
        match hz {
            2_822_400 => Self::Dsd64,
            5_644_800 => Self::Dsd128,
            11_289_600 => Self::Dsd256,
            22_579_200 => Self::Dsd512,
            other => Self::Other(other),
        }
    }

    /// Short label, e.g. "DSD64".
    pub fn label(&self) -> String {
        match self {
            Self::Dsd64 => "DSD64".into(),
            Self::Dsd128 => "DSD128".into(),
            Self::Dsd256 => "DSD256".into(),
            Self::Dsd512 => "DSD512".into(),
            Self::Other(hz) => format!("DSD~{}", hz / 44_100),
        }
    }
}

/// Bit order of a DSD stream as stored in its container.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BitOrder {
    /// Least-significant bit is the earliest sample (DSF).
    Lsb,
    /// Most-significant bit is the earliest sample (DFF/DSDIFF).
    Msb,
}

/// Immutable description of a decoded DSD stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DsdInfo {
    pub channels: u32,
    /// 1-bit sample rate in Hz (e.g. 2_822_400 for DSD64).
    pub sample_rate: u32,
    pub bit_order: BitOrder,
    /// Total 1-bit samples per channel (excludes container padding).
    pub samples_per_channel: u64,
}

impl DsdInfo {
    pub fn rate_family(&self) -> DsdRate {
        DsdRate::from_hz(self.sample_rate)
    }

    /// SPA DSD rate = bytes/sec (1-bit rate / 8).
    pub fn spa_rate(&self) -> u32 {
        self.sample_rate / 8
    }

    pub fn duration(&self) -> Duration {
        if self.sample_rate == 0 {
            return Duration::ZERO;
        }
        Duration::from_secs_f64(self.samples_per_channel as f64 / self.sample_rate as f64)
    }

    /// Total per-channel bytes (excludes container padding).
    pub fn total_bytes(&self) -> u64 {
        self.samples_per_channel / 8
    }
}

/// Best-effort tags read from a container.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Tags {
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
}

/// A playlist entry with cached metadata.
#[derive(Debug, Clone, PartialEq)]
pub struct TrackInfo {
    pub path: PathBuf,
    pub tags: Tags,
    pub info: Option<DsdInfo>,
    /// Set when the file went missing after being enqueued.
    pub missing: bool,
}

impl TrackInfo {
    pub fn display_title(&self) -> String {
        if let Some(t) = &self.tags.title {
            if let Some(a) = &self.tags.artist {
                return format!("{a} — {t}");
            }
            return t.clone();
        }
        self.path
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.path.display().to_string())
    }
}

/// High-level transport state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transport {
    Stopped,
    Playing,
    Paused,
}

/// How the current stream is reaching the DAC. v1 only ever reports `Native`
/// (or `Unknown` before negotiation); v2 adds `Dop` and `Transcoded`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputMode {
    Unknown,
    /// Bit-perfect DSD to a DSD-capable sink.
    Native,
    /// DSD-over-PCM (v2).
    Dop,
    /// Decoded/resampled to PCM (v2).
    Transcoded,
}

impl OutputMode {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Unknown => "—",
            Self::Native => "NATIVE",
            Self::Dop => "DoP",
            Self::Transcoded => "PCM",
        }
    }
}

/// Playlist repeat behavior.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RepeatMode {
    #[default]
    Off,
    One,
    All,
}

impl RepeatMode {
    pub fn next(self) -> Self {
        match self {
            Self::Off => Self::All,
            Self::All => Self::One,
            Self::One => Self::Off,
        }
    }
    pub fn label(&self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::One => "one",
            Self::All => "all",
        }
    }
}

/// A single column of the waveform: normalized peak and RMS in `0.0..=1.0`.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct WaveColumn {
    pub peak: f32,
    pub rms: f32,
}
