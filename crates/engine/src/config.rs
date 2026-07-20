//! Configuration and XDG path resolution.
//!
//! Config lives at `$XDG_CONFIG_HOME/piwiplay/config.toml`; playlists under
//! `$XDG_DATA_HOME/piwiplay/playlists`; logs/state under
//! `$XDG_STATE_HOME/piwiplay`. Missing/invalid config falls back to defaults.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Config {
    pub audio: AudioConfig,
    pub ui: UiConfig,
    pub theme: ThemeConfig,
    pub keymap: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct AudioConfig {
    /// Keep DSD bit-perfect: never attenuate in software (see SPEC §5.4).
    pub allow_pcm_volume: bool,
    /// Empty = default PipeWire sink; otherwise a node name to target.
    pub target_sink: String,
    /// Ring fill target in milliseconds.
    pub buffer_ms: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct UiConfig {
    pub theme: String,
    pub fps: u32,
    /// "braille" | "blocks" | "off"
    pub waveform: String,
    pub min_cols: u16,
    pub min_rows: u16,
    pub max_content_cols: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct ThemeConfig {
    pub accent: String,
    pub played: String,
    pub unplayed: String,
    pub meter_ok: String,
    pub meter_warn: String,
    pub meter_clip: String,
    pub border: String,
    pub text_dim: String,
}

impl Default for AudioConfig {
    fn default() -> Self {
        Self { allow_pcm_volume: false, target_sink: String::new(), buffer_ms: 400 }
    }
}

impl Default for UiConfig {
    fn default() -> Self {
        Self {
            theme: "dark".into(),
            fps: 30,
            waveform: "braille".into(),
            min_cols: 60,
            min_rows: 20,
            max_content_cols: 200,
        }
    }
}

impl Default for ThemeConfig {
    fn default() -> Self {
        Self {
            accent: "#8ec07c".into(),
            played: "#83a598".into(),
            unplayed: "#504945".into(),
            meter_ok: "#b8bb26".into(),
            meter_warn: "#fabd2f".into(),
            meter_clip: "#fb4934".into(),
            border: "#665c54".into(),
            text_dim: "#928374".into(),
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            audio: AudioConfig::default(),
            ui: UiConfig::default(),
            theme: ThemeConfig::default(),
            keymap: BTreeMap::new(),
        }
    }
}

impl Config {
    /// Parse from a TOML string, filling any missing fields with defaults.
    pub fn from_toml(s: &str) -> Result<Self, toml::de::Error> {
        toml::from_str(s)
    }

    pub fn to_toml(&self) -> String {
        toml::to_string_pretty(self).unwrap_or_default()
    }

    /// Load from the standard path, or defaults (logging a warning) if absent
    /// or unparseable.
    pub fn load() -> Self {
        let path = Paths::get().config_file;
        match std::fs::read_to_string(&path) {
            Ok(s) => match Self::from_toml(&s) {
                Ok(c) => c,
                Err(e) => {
                    tracing::warn!("invalid config at {}: {e}; using defaults", path.display());
                    Self::default()
                }
            },
            Err(_) => Self::default(),
        }
    }
}

/// Resolved XDG paths for the app.
pub struct Paths {
    pub config_file: PathBuf,
    pub state_dir: PathBuf,
    pub data_dir: PathBuf,
    pub playlists_dir: PathBuf,
    pub cache_dir: PathBuf,
}

impl Paths {
    pub fn get() -> Self {
        if let Some(pd) = directories::ProjectDirs::from("", "", "piwiplay") {
            let data_dir = pd.data_dir().to_path_buf();
            Self {
                config_file: pd.config_dir().join("config.toml"),
                state_dir: pd.state_dir().map(|p| p.to_path_buf()).unwrap_or_else(|| data_dir.clone()),
                playlists_dir: data_dir.join("playlists"),
                cache_dir: pd.cache_dir().to_path_buf(),
                data_dir,
            }
        } else {
            let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
            let base = PathBuf::from(home).join(".piwiplay");
            Self {
                config_file: base.join("config.toml"),
                state_dir: base.join("state"),
                playlists_dir: base.join("playlists"),
                cache_dir: base.join("cache"),
                data_dir: base,
            }
        }
    }

    /// Create the state/data/playlist directories (best effort).
    pub fn ensure_dirs(&self) {
        for d in [&self.state_dir, &self.data_dir, &self.playlists_dir, &self.cache_dir] {
            let _ = std::fs::create_dir_all(d);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_bit_perfect() {
        let c = Config::default();
        assert!(!c.audio.allow_pcm_volume);
        assert_eq!(c.audio.buffer_ms, 400);
        assert_eq!(c.ui.waveform, "braille");
        assert_eq!(c.ui.min_cols, 60);
    }

    #[test]
    fn partial_toml_fills_defaults() {
        let c = Config::from_toml("[ui]\nfps = 60\n").unwrap();
        assert_eq!(c.ui.fps, 60);
        assert_eq!(c.ui.theme, "dark"); // default retained
        assert!(!c.audio.allow_pcm_volume);
    }

    #[test]
    fn round_trips_through_toml() {
        let mut c = Config::default();
        c.ui.theme = "light".into();
        c.keymap.insert("seek_forward_small".into(), "Right".into());
        let s = c.to_toml();
        let back = Config::from_toml(&s).unwrap();
        assert_eq!(c, back);
    }
}
