//! # piwiplay-engine
//!
//! Headless playback engine for piwiplay. It decodes DSD (`.dsf`/`.dff`),
//! outputs bit-perfect native DSD to a PipeWire sink, and manages a playlist,
//! transport, volume, and waveform extraction — all behind a single
//! [`Engine`] driven by [`Command`]s and [`Event`]s.
//!
//! The engine has **no UI dependencies**: the TUI is one frontend, and the
//! same Command/Event seam is what a future WebUI/Electron frontend would use.

pub mod audio;
pub mod config;
pub mod decode;
pub mod error;
pub mod player;
pub mod playlist;
pub mod types;
pub mod waveform;

#[cfg(test)]
mod testgen;

pub use config::{Config, Paths};
pub use error::{EngineError, Result};
pub use player::{Command, Engine, Event};
pub use types::{
    BitOrder, DsdInfo, DsdRate, OutputMode, RepeatMode, Tags, TrackInfo, Transport, WaveColumn,
};
