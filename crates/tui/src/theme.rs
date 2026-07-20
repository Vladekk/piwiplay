//! Color theme: parses the `[theme]` hex colors from config into ratatui
//! colors, honoring `NO_COLOR` (disables color entirely).

use piwiplay_engine::config::ThemeConfig;
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
    pub colored: bool,
}

impl Theme {
    pub fn from_config(c: &ThemeConfig) -> Self {
        let colored = std::env::var_os("NO_COLOR").is_none();
        let p = |hex: &str, fallback: Color| {
            if !colored {
                Color::Reset
            } else {
                parse_hex(hex).unwrap_or(fallback)
            }
        };
        Theme {
            accent: p(&c.accent, Color::Green),
            played: p(&c.played, Color::Cyan),
            unplayed: p(&c.unplayed, Color::DarkGray),
            meter_ok: p(&c.meter_ok, Color::Green),
            meter_warn: p(&c.meter_warn, Color::Yellow),
            meter_clip: p(&c.meter_clip, Color::Red),
            border: p(&c.border, Color::DarkGray),
            text_dim: p(&c.text_dim, Color::Gray),
            colored,
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
        assert_eq!(parse_hex("#12345"), None);
    }
}
