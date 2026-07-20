//! Rendering: layout with size breakpoints, colored/unicode widgets, overlays.

use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap};
use ratatui::Frame;

use piwiplay_engine::{OutputMode, Transport};

use crate::app::{App, Focus};
use crate::widgets::{bar_parts, braille_waveform, fmt_time};

pub fn draw(f: &mut Frame, app: &App) {
    let full = f.area();
    let min_c = app.cfg.ui.min_cols.max(20);
    let min_r = app.cfg.ui.min_rows.max(8);
    if full.width < min_c || full.height < min_r {
        draw_too_small(f, full, min_c, min_r);
        return;
    }

    // Clamp absurdly wide terminals to a readable width, centered.
    let area = clamp_width(full, app.cfg.ui.max_content_cols);

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // status
            Constraint::Min(6),    // main
            Constraint::Length(3), // transport
            Constraint::Length(1), // hints
        ])
        .split(area);

    render_status(f, rows[0], app);
    render_main(f, rows[1], app);
    render_transport(f, rows[2], app);
    render_hints(f, rows[3], app);

    if app.show_help {
        draw_help(f, area);
    }
    if let Some((msg, _)) = &app.message {
        draw_message(f, area, msg, app);
    }
}

fn clamp_width(area: Rect, max_cols: u16) -> Rect {
    if max_cols >= 20 && area.width > max_cols {
        let x = area.x + (area.width - max_cols) / 2;
        Rect { x, y: area.y, width: max_cols, height: area.height }
    } else {
        area
    }
}

fn draw_too_small(f: &mut Frame, area: Rect, min_c: u16, min_r: u16) {
    let msg = format!("Terminal too small\nneed ≥ {min_c}×{min_r}\n({}×{} now)", area.width, area.height);
    let p = Paragraph::new(msg).alignment(Alignment::Center).wrap(Wrap { trim: true });
    let inner = center_rect(area, 30, 3);
    f.render_widget(Clear, inner);
    f.render_widget(p, inner);
}

fn render_status(f: &mut Frame, area: Rect, app: &App) {
    let th = &app.theme;
    let badge = app
        .track
        .as_ref()
        .and_then(|t| t.info.as_ref())
        .map(|i| {
            format!(
                "{} · {:.2} MHz · {}ch · {}",
                i.rate_family().label(),
                i.sample_rate as f64 / 1_000_000.0,
                i.channels,
                app.mode.label()
            )
        })
        .unwrap_or_else(|| "—".into());

    let state = match app.transport {
        Transport::Playing => "▶ Playing",
        Transport::Paused => "⏸ Paused",
        Transport::Stopped => "■ Stopped",
    };

    let title = app.track.as_ref().map(|t| t.display_title()).unwrap_or_else(|| "no track".into());
    let mode_color = match app.mode {
        OutputMode::Native => th.meter_ok,
        OutputMode::Dop => th.meter_warn,
        OutputMode::Transcoded => th.accent,
        OutputMode::Unknown => th.text_dim,
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(th.border))
        .title(Span::styled(" piwiplay ", Style::default().fg(th.accent).add_modifier(Modifier::BOLD)))
        .title_top(Line::from(Span::styled(format!(" {badge} "), Style::default().fg(mode_color))).right_aligned());

    let inner = block.inner(area);
    f.render_widget(block, area);

    let line = Line::from(vec![
        Span::styled("♪ ", Style::default().fg(th.accent)),
        Span::styled(title, Style::default().add_modifier(Modifier::BOLD)),
        Span::raw("   "),
        Span::styled(format!("[{state}]"), Style::default().fg(th.text_dim)),
    ]);
    f.render_widget(Paragraph::new(line), inner);
}

fn render_main(f: &mut Frame, area: Rect, app: &App) {
    let two_pane = area.width >= 100 && area.height >= 24;
    if two_pane {
        let cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(48), Constraint::Percentage(52)])
            .split(area);
        render_list(f, cols[0], app);
        let right = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(4), Constraint::Length(4)])
            .split(cols[1]);
        render_waveform(f, right[0], app);
        render_levels(f, right[1], app);
    } else {
        render_list(f, area, app);
    }
}

fn render_list(f: &mut Frame, area: Rect, app: &App) {
    let th = &app.theme;
    let (title, focused) = match app.focus {
        Focus::Browser => (" Browser ", true),
        Focus::Playlist => (" Playlist ", true),
    };
    let other = match app.focus {
        Focus::Browser => "Playlist",
        Focus::Playlist => "Browser",
    };
    let border_color = if focused { th.accent } else { th.border };

    let items: Vec<ListItem> = match app.focus {
        Focus::Browser => app
            .browser
            .entries
            .iter()
            .map(|e| {
                let (icon, style) = if e.is_parent {
                    ("▸ ", Style::default().fg(th.text_dim))
                } else if e.is_dir {
                    ("▾ ", Style::default().fg(th.accent))
                } else if e.is_playlist {
                    ("≣ ", Style::default().fg(th.text_dim))
                } else {
                    ("♫ ", Style::default())
                };
                ListItem::new(Line::from(vec![Span::styled(icon, style), Span::raw(e.name.clone())]))
            })
            .collect(),
        Focus::Playlist => app
            .playlist
            .iter()
            .enumerate()
            .map(|(i, t)| {
                let playing = Some(i) == app.playlist_cur;
                let icon = if playing { "♪ " } else { "  " };
                let mut style = Style::default();
                if t.missing {
                    style = style.fg(app.theme.meter_clip).add_modifier(Modifier::DIM);
                }
                if playing {
                    style = style.fg(app.theme.accent).add_modifier(Modifier::BOLD);
                }
                let label = if t.missing { format!("! {}", t.display_title()) } else { t.display_title() };
                ListItem::new(Line::from(vec![Span::raw(icon), Span::styled(label, style)]))
            })
            .collect(),
    };

    let hint = format!("{title}(Tab → {other})");
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border_color))
        .title(Span::styled(hint, Style::default().fg(th.accent)));

    let mut state = ListState::default();
    let sel = match app.focus {
        Focus::Browser => app.browser.selected,
        Focus::Playlist => app.playlist_sel,
    };
    state.select(if list_len(app) == 0 { None } else { Some(sel) });

    let list = List::new(items)
        .block(block)
        .highlight_style(Style::default().add_modifier(Modifier::REVERSED))
        .highlight_symbol("");
    f.render_stateful_widget(list, area, &mut state);
}

fn list_len(app: &App) -> usize {
    match app.focus {
        Focus::Browser => app.browser.entries.len(),
        Focus::Playlist => app.playlist.len(),
    }
}

fn render_waveform(f: &mut Frame, area: Rect, app: &App) {
    let th = &app.theme;
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(th.border))
        .title(Span::styled(" Waveform ", Style::default().fg(th.text_dim)));
    let inner = block.inner(area);
    f.render_widget(block, area);

    let w = inner.width as usize;
    let h = inner.height as usize;
    if w == 0 || h == 0 {
        return;
    }
    let wave = app.waveform.clone();
    let amp_at = |x: f64| -> f64 {
        if wave.is_empty() {
            return 0.0;
        }
        let idx = (x * (wave.len() - 1) as f64).round() as usize;
        wave[idx.min(wave.len() - 1)].peak as f64
    };
    let rows = braille_waveform(w, h, amp_at);

    let playhead_frac = frac(app.elapsed.as_secs_f64(), app.total.as_secs_f64());
    let played_cells = (playhead_frac * w as f64).round() as usize;

    let lines: Vec<Line> = rows
        .into_iter()
        .map(|s| {
            let chars: Vec<char> = s.chars().collect();
            let played: String = chars.iter().take(played_cells).collect();
            let rest: String = chars.iter().skip(played_cells).collect();
            Line::from(vec![
                Span::styled(played, Style::default().fg(th.played)),
                Span::styled(rest, Style::default().fg(th.unplayed)),
            ])
        })
        .collect();
    f.render_widget(Paragraph::new(lines), inner);
}

fn render_levels(f: &mut Frame, area: Rect, app: &App) {
    let th = &app.theme;
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(th.border))
        .title(Span::styled(" Level ", Style::default().fg(th.text_dim)));
    let inner = block.inner(area);
    f.render_widget(block, area);
    if inner.height == 0 {
        return;
    }

    // v1: single mono-derived level shown on L and R (see SPEC §8.3 limitation).
    let level = current_level(app);
    let bar_w = inner.width.saturating_sub(8) as usize;
    let mut lines = Vec::new();
    for label in ["L", "R"] {
        lines.push(meter_line(label, level, bar_w, th));
    }
    f.render_widget(Paragraph::new(lines), inner);
}

fn current_level(app: &App) -> f32 {
    if app.transport != Transport::Playing || app.waveform.is_empty() {
        return 0.0;
    }
    let frac = frac(app.elapsed.as_secs_f64(), app.total.as_secs_f64());
    let idx = (frac * (app.waveform.len() - 1) as f64).round() as usize;
    app.waveform[idx.min(app.waveform.len() - 1)].rms
}

fn meter_line<'a>(label: &'a str, level: f32, width: usize, th: &crate::theme::Theme) -> Line<'a> {
    let (full, partial, rest) = bar_parts(width, level as f64);
    let color = th.meter_color(level);
    let mut spans = vec![Span::styled(format!("{label} "), Style::default().fg(th.text_dim))];
    spans.push(Span::styled("▕", Style::default().fg(th.border)));
    spans.push(Span::styled("█".repeat(full), Style::default().fg(color)));
    if let Some(c) = partial {
        spans.push(Span::styled(c.to_string(), Style::default().fg(color)));
    }
    spans.push(Span::styled("░".repeat(rest), Style::default().fg(th.unplayed)));
    spans.push(Span::styled("▏", Style::default().fg(th.border)));
    Line::from(spans)
}

fn render_transport(f: &mut Frame, area: Rect, app: &App) {
    let th = &app.theme;
    let block = Block::default().borders(Borders::ALL).border_style(Style::default().fg(th.border));
    let inner = block.inner(area);
    f.render_widget(block, area);

    // Left: elapsed  seekbar  total.  Right: volume.
    let vol_w = 18u16.min(inner.width / 3);
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Min(10), Constraint::Length(vol_w)])
        .split(inner);

    // seek bar
    let seek_frac = frac(app.elapsed.as_secs_f64(), app.total.as_secs_f64());
    let labels_w = 14usize;
    let bar_w = (cols[0].width as usize).saturating_sub(labels_w);
    let (full, partial, rest) = bar_parts(bar_w, seek_frac);
    let mut spans = vec![
        Span::styled(format!("{:>5} ", fmt_time(app.elapsed)), Style::default().fg(th.text_dim)),
        Span::styled("▕", Style::default().fg(th.border)),
        Span::styled("█".repeat(full), Style::default().fg(th.played)),
    ];
    if let Some(c) = partial {
        spans.push(Span::styled(c.to_string(), Style::default().fg(th.played)));
    }
    spans.push(Span::styled("░".repeat(rest), Style::default().fg(th.unplayed)));
    spans.push(Span::styled("▏", Style::default().fg(th.border)));
    spans.push(Span::styled(format!(" {:>5}", fmt_time(app.total)), Style::default().fg(th.text_dim)));
    f.render_widget(Paragraph::new(Line::from(spans)), cols[0]);

    // volume
    let vlabel = if app.muted { "mute".to_string() } else { format!("{:>3}%", (app.volume * 100.0) as u32) };
    let vbar_w = (cols[1].width as usize).saturating_sub(7);
    let (vf, vp, vr) = bar_parts(vbar_w, if app.muted { 0.0 } else { app.volume });
    let mut vspans = vec![Span::styled("♪", Style::default().fg(th.accent))];
    vspans.push(Span::styled("▕", Style::default().fg(th.border)));
    vspans.push(Span::styled("█".repeat(vf), Style::default().fg(th.accent)));
    if let Some(c) = vp {
        vspans.push(Span::styled(c.to_string(), Style::default().fg(th.accent)));
    }
    vspans.push(Span::styled("░".repeat(vr), Style::default().fg(th.unplayed)));
    vspans.push(Span::styled(format!(" {vlabel}"), Style::default().fg(th.text_dim)));
    f.render_widget(Paragraph::new(Line::from(vspans)), cols[1]);
}

fn render_hints(f: &mut Frame, area: Rect, app: &App) {
    let th = &app.theme;
    if let Some(buf) = &app.find {
        let p = Paragraph::new(Line::from(vec![
            Span::styled("/", Style::default().fg(th.accent)),
            Span::raw(buf.clone()),
            Span::styled("  (Esc to cancel)", Style::default().fg(th.text_dim)),
        ]));
        f.render_widget(p, area);
        return;
    }
    let extra = format!("  repeat:{} shuffle:{}", app.repeat.label(), if app.shuffle { "on" } else { "off" });
    let hints = "space play/pause  ←→ seek  n/p track  ⏎ open  a add  d del  x save  z r  / find  ? help  q quit";
    let line = Line::from(vec![
        Span::styled(hints, Style::default().fg(th.text_dim)),
        Span::styled(extra, Style::default().fg(th.accent)),
    ]);
    f.render_widget(Paragraph::new(line), area);
}

fn draw_help(f: &mut Frame, area: Rect) {
    let text = "\
 piwiplay — keys

 space   play / pause          ⏎     open dir / play file
 S       stop                  a     add selection to playlist
 n / p   next / previous       d     remove from playlist
 ← →     seek ∓5s              x     save playlist
 Shift←→ seek ∓30s             o/⏎   load .m3u (browser)
 [ ] -+  volume                Tab   switch Browser/Playlist
 m       mute                  ↑↓ kj move   g/G top/bottom
 r       repeat cycle          /     find
 z       shuffle               ? close help   q quit
";
    let inner = center_rect(area, 64, 16);
    f.render_widget(Clear, inner);
    let block = Block::default().borders(Borders::ALL).title(" Help ");
    let p = Paragraph::new(text).block(block);
    f.render_widget(p, inner);
}

fn draw_message(f: &mut Frame, area: Rect, msg: &str, app: &App) {
    let w = (msg.len() as u16 + 4).min(area.width.saturating_sub(2));
    let rect = Rect { x: area.x + 1, y: area.y + area.height.saturating_sub(5), width: w, height: 3 };
    f.render_widget(Clear, rect);
    let block = Block::default().borders(Borders::ALL).border_style(Style::default().fg(app.theme.accent));
    f.render_widget(Paragraph::new(msg.to_string()).block(block), rect);
}

fn center_rect(area: Rect, w: u16, h: u16) -> Rect {
    let w = w.min(area.width);
    let h = h.min(area.height);
    Rect { x: area.x + (area.width - w) / 2, y: area.y + (area.height - h) / 2, width: w, height: h }
}

fn frac(num: f64, den: f64) -> f64 {
    if den <= 0.0 {
        0.0
    } else {
        (num / den).clamp(0.0, 1.0)
    }
}
