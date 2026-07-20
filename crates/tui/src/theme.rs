//! Color theme.
//!
//! Two palettes:
//! * **terminal** (default) — uses the 16 named ANSI colors, so the *terminal's*
//!   own theme (e.g. Konsole "Vapor", "Breeze", base16 schemes) recolors the app.
//!   Nothing is hard-coded to RGB, so switching the terminal theme restyles
//!   piwiplay live.
//! * **custom** — uses the explicit `[theme]` hex colors from config.
//!
//! `NO_COLOR` disables color entirely regardless of palette.

use piwiplay_engine::config::{Config, ThemeConfig};
use ratatui::style::Color;

#[derive(Debug, Clone)]
pub struct Theme {
    pub accent: Color,
    pub played: Color,
    pub unplayed: Color,
    pub meter_ok: Color,
    pub meter_warn: Color,
    pub meter_clip: Color,
    pub border: Color,
    pub text_dim: Color,
    #[allow(dead_code)]
    pub colored: bool,
}

impl Theme {
    pub fn from_config(cfg: &Config) -> Self {
        let colored = std::env::var_os("NO_COLOR").is_none();
        if !colored {
            return Self::mono();
        }
        match cfg.ui.palette.as_str() {
            "custom" => Self::from_hex(&cfg.theme),
            _ => Self::terminal(),
        }
    }

    /// Palette built from the terminal's own 16 ANSI colors (themeable by the
    /// terminal emulator). These map to palette slots that schemes recolor.
    pub fn terminal() -> Self {
        Theme {
            accent: Color::Green,
            played: Color::Cyan,
            unplayed: Color::DarkGray,
            meter_ok: Color::Green,
            meter_warn: Color::Yellow,
            meter_clip: Color::Red,
            border: Color::DarkGray,
            text_dim: Color::Gray,
            colored: true,
        }
    }

    fn mono() -> Self {
        Theme {
            accent: Color::Reset,
            played: Color::Reset,
            unplayed: Color::Reset,
            meter_ok: Color::Reset,
            meter_warn: Color::Reset,
            meter_clip: Color::Reset,
            border: Color::Reset,
            text_dim: Color::Reset,
            colored: false,
        }
    }

    fn from_hex(c: &ThemeConfig) -> Self {
        let p = |hex: &str, fallback: Color| parse_hex(hex).unwrap_or(fallback);
        Theme {
            accent: p(&c.accent, Color::Green),
            played: p(&c.played, Color::Cyan),
            unplayed: p(&c.unplayed, Color::DarkGray),
            meter_ok: p(&c.meter_ok, Color::Green),
            meter_warn: p(&c.meter_warn, Color::Yellow),
            meter_clip: p(&c.meter_clip, Color::Red),
            border: p(&c.border, Color::DarkGray),
            text_dim: p(&c.text_dim, Color::Gray),
            colored: true,
        }
    }

    /// Meter color for a normalized level (green → yellow → red).
    pub fn meter_color(&self, level: f32) -> Color {
        if level >= 0.92 {
            self.meter_clip
        } else if level >= 0.72 {
            self.meter_warn
        } else {
            self.meter_ok
        }
    }
}

fn parse_hex(s: &str) -> Option<Color> {
    let s = s.trim().trim_start_matches('#');
    if s.len() != 6 {
        return None;
    }
    let r = u8::from_str_radix(&s[0..2], 16).ok()?;
    let g = u8::from_str_radix(&s[2..4], 16).ok()?;
    let b = u8::from_str_radix(&s[4..6], 16).ok()?;
    Some(Color::Rgb(r, g, b))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_hex_colors() {
        assert_eq!(parse_hex("#8ec07c"), Some(Color::Rgb(0x8e, 0xc0, 0x7c)));
        assert_eq!(parse_hex("ff0000"), Some(Color::Rgb(255, 0, 0)));
        assert_eq!(parse_hex("#zzz"), None);
    }

    #[test]
    fn terminal_palette_uses_named_ansi_colors() {
        // Named ANSI colors (not Rgb) so the terminal's own theme controls them.
        let t = Theme::terminal();
        assert_eq!(t.accent, Color::Green);
        assert_eq!(t.played, Color::Cyan);
        assert!(!matches!(t.border, Color::Rgb(..)));
    }

    #[test]
    fn custom_palette_uses_hex() {
        let mut cfg = Config::default();
        cfg.ui.palette = "custom".into();
        let t = Theme::from_config(&cfg);
        assert!(matches!(t.accent, Color::Rgb(..)));
    }

    #[test]
    fn default_palette_is_terminal() {
        let cfg = Config::default();
        let t = Theme::from_config(&cfg);
        assert_eq!(t.accent, Color::Green); // named, terminal-themeable
    }
}
