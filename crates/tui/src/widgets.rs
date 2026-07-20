//! Pure rendering helpers: sub-cell fractional bars (eighth blocks) and the
//! braille waveform rasterizer. Kept free of ratatui state so they are unit
//! testable; `ui.rs` turns their output into styled spans.

/// Eighth-block ramp for sub-cell horizontal fill.
const EIGHTHS: [char; 8] = ['▏', '▎', '▍', '▌', '▋', '▊', '▉', '█'];

/// Split a bar of `width` cells at `frac` (0.0..=1.0) into full cells, an
/// optional fractional cap character, and the number of empty trailing cells.
/// Gives ~8× the effective resolution of a plain `#`/`=` bar.
pub fn bar_parts(width: usize, frac: f64) -> (usize, Option<char>, usize) {
    if width == 0 {
        return (0, None, 0);
    }
    let frac = frac.clamp(0.0, 1.0);
    let total_eighths = (frac * width as f64 * 8.0).round() as usize;
    let mut full = total_eighths / 8;
    let mut part_idx = total_eighths % 8;
    if full >= width {
        full = width;
        part_idx = 0;
    }
    let partial = if part_idx == 0 { None } else { Some(EIGHTHS[part_idx - 1]) };
    let used = full + usize::from(partial.is_some());
    (full, partial, width.saturating_sub(used))
}

/// Braille dot bit for a dot at (`x` in 0..2, `y` in 0..4) within a cell.
#[inline]
fn dot_bit(x: usize, y: usize) -> u8 {
    // Unicode braille dot numbering.
    const COL0: [u8; 4] = [0x01, 0x02, 0x04, 0x40];
    const COL1: [u8; 4] = [0x08, 0x10, 0x20, 0x80];
    if x == 0 {
        COL0[y]
    } else {
        COL1[y]
    }
}

/// Rasterize a mirrored waveform into `height` rows × `width` cells of braille.
/// `amp_at(col_fraction)` returns the normalized amplitude (0.0..=1.0) for a
/// horizontal position; the envelope is centered vertically and mirrored.
pub fn braille_waveform<F>(width: usize, height: usize, amp_at: F) -> Vec<String>
where
    F: Fn(f64) -> f64,
{
    if width == 0 || height == 0 {
        return Vec::new();
    }
    let dot_w = width * 2;
    let dot_h = height * 4;
    let center = dot_h as f64 / 2.0;

    // dot grid
    let mut grid = vec![vec![false; dot_w]; dot_h];
    for dx in 0..dot_w {
        let frac = dx as f64 / (dot_w.max(1) - 1).max(1) as f64;
        let amp = amp_at(frac).clamp(0.0, 1.0);
        let reach = amp * center;
        for (dy, row) in grid.iter_mut().enumerate() {
            let dist = (dy as f64 + 0.5 - center).abs();
            if dist <= reach {
                row[dx] = true;
            }
        }
    }

    // fold 2×4 dot blocks into braille chars
    let mut rows = Vec::with_capacity(height);
    for cy in 0..height {
        let mut line = String::with_capacity(width);
        for cx in 0..width {
            let mut bits = 0u8;
            for x in 0..2 {
                for y in 0..4 {
                    let gx = cx * 2 + x;
                    let gy = cy * 4 + y;
                    if grid[gy][gx] {
                        bits |= dot_bit(x, y);
                    }
                }
            }
            line.push(char::from_u32(0x2800 + bits as u32).unwrap_or(' '));
        }
        rows.push(line);
    }
    rows
}

/// Format a duration as `m:ss` or `h:mm:ss`.
pub fn fmt_time(d: std::time::Duration) -> String {
    let s = d.as_secs();
    let (h, m, sec) = (s / 3600, (s % 3600) / 60, s % 60);
    if h > 0 {
        format!("{h}:{m:02}:{sec:02}")
    } else {
        format!("{m}:{sec:02}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bar_empty_and_full() {
        assert_eq!(bar_parts(10, 0.0), (0, None, 10));
        assert_eq!(bar_parts(10, 1.0), (10, None, 0));
    }

    #[test]
    fn bar_half_is_five_cells() {
        let (full, partial, rest) = bar_parts(10, 0.5);
        assert_eq!(full, 5);
        assert_eq!(partial, None);
        assert_eq!(rest, 5);
    }

    #[test]
    fn bar_fractional_uses_eighth_cap() {
        // 0.55 of 10 cells = 5.5 cells => 5 full + a 4/8 cap
        let (full, partial, rest) = bar_parts(10, 0.55);
        assert_eq!(full, 5);
        assert_eq!(partial, Some('▌'));
        assert_eq!(rest, 4);
        assert_eq!(full + 1 + rest, 10);
    }

    #[test]
    fn bar_never_exceeds_width() {
        for f in [0.0, 0.1, 0.33, 0.5, 0.99, 1.0, 1.5] {
            let (full, partial, rest) = bar_parts(8, f);
            assert_eq!(full + usize::from(partial.is_some()) + rest, 8, "f={f}");
        }
    }

    #[test]
    fn waveform_dimensions_and_braille_range() {
        let rows = braille_waveform(20, 3, |x| x); // ramp
        assert_eq!(rows.len(), 3);
        for r in &rows {
            assert_eq!(r.chars().count(), 20);
            for c in r.chars() {
                let u = c as u32;
                assert!((0x2800..=0x28FF).contains(&u), "not braille: {c}");
            }
        }
    }

    #[test]
    fn silent_waveform_is_blank_braille() {
        let rows = braille_waveform(10, 2, |_| 0.0);
        // amplitude 0 -> only the center-most dots may light; corners must be blank
        assert_eq!(rows[0].chars().next(), Some('\u{2800}'));
    }

    #[test]
    fn time_formats() {
        assert_eq!(fmt_time(std::time::Duration::from_secs(65)), "1:05");
        assert_eq!(fmt_time(std::time::Duration::from_secs(3725)), "1:02:05");
    }
}
