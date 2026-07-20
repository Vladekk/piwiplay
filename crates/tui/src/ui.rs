//! Rendering: layout with size breakpoints, colored/unicode widgets, overlays.
//! Also records mouse hit-areas (list rows, seek bar) into `app.hit`.

use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap};
use ratatui::Frame;

use piwiplay_engine::{OutputMode, Transport};

use crate::app::{App, Focus, VERSION};
use crate::fs_browser::Browser;
use crate::widgets::{bar_parts, braille_waveform, fmt_time};

pub fn draw(f: &mut Frame, app: &App) {
    let full = f.area();
    let min_c = app.cfg.ui.min_cols.max(20);
    let min_r = app.cfg.ui.min_rows.max(8);
    if full.width < min_c || full.height < min_r {
        draw_too_small(f, full, min_c, min_r);
        return;
    }
    let area = clamp_width(full, app.cfg.ui.max_content_cols);

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Min(6), Constraint::Length(3), Constraint::Length(1)])
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
        Rect { x: area.x + (area.width - max_cols) / 2, y: area.y, width: max_cols, height: area.height }
    } else {
        area
    }
}

fn draw_too_small(f: &mut Frame, area: Rect, min_c: u16, min_r: u16) {
    let msg = format!("Terminal too small\nneed ≥ {min_c}×{min_r}\n({}×{} now)", area.width, area.height);
    let inner = center_rect(area, 30, 3);
    f.render_widget(Clear, inner);
    f.render_widget(Paragraph::new(msg).alignment(Alignment::Center).wrap(Wrap { trim: true }), inner);
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
        .unwrap_or_else(|| format!("— · {}", app.mode.label()));

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
        render_focused_list(f, cols[0], app);
        let right = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(4), Constraint::Length(4)])
            .split(cols[1]);
        render_waveform(f, right[0], app);
        render_levels(f, right[1], app);
    } else {
        render_focused_list(f, area, app);
    }
}

fn render_focused_list(f: &mut Frame, area: Rect, app: &App) {
    match app.focus {
        Focus::Browser => render_browser(f, area, app, &app.browser),
        Focus::Saved => render_browser(f, area, app, &app.saved),
        Focus::Queue => render_queue(f, area, app),
    }
}

fn pane_title(app: &App) -> String {
    let cycle = match app.focus {
        Focus::Browser => "Tab→Playlist",
        Focus::Queue => "Tab→Saved",
        Focus::Saved => "Tab→Browser",
    };
    format!(" {} ({cycle}) ", app.focus.title())
}

fn render_browser(f: &mut Frame, area: Rect, app: &App, br: &Browser) {
    let th = &app.theme;
    let subtitle = format!("{}  [{}]", pane_title(app), br.cwd.display());
    let items: Vec<ListItem> = br
        .entries
        .iter()
        .enumerate()
        .map(|(i, e)| {
            let (icon, mut style) = if e.is_parent {
                ("▸ ", Style::default().fg(th.text_dim))
            } else if e.is_dir {
                ("▾ ", Style::default().fg(th.accent))
            } else if e.is_playlist {
                ("≣ ", Style::default().fg(th.text_dim))
            } else {
                ("♫ ", Style::default())
            };
            if br.marked.contains(&i) {
                style = style.add_modifier(Modifier::BOLD).fg(th.played);
            }
            let mark = if br.marked.contains(&i) { "●" } else { " " };
            ListItem::new(Line::from(vec![
                Span::styled(mark, Style::default().fg(th.played)),
                Span::styled(icon, style),
                Span::styled(e.name.clone(), style),
            ]))
        })
        .collect();

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(th.accent))
        .title(Span::styled(subtitle, Style::default().fg(th.accent)));
    let inner = block.inner(area);
    let mut state = ListState::default();
    state.select(if br.entries.is_empty() { None } else { Some(br.selected) });
    let list = List::new(items).block(block).highlight_style(Style::default().add_modifier(Modifier::REVERSED));
    f.render_stateful_widget(list, area, &mut state);

    let mut hit = app.hit.borrow_mut();
    hit.list = Some(inner);
    hit.list_offset = state.offset();
}

fn render_queue(f: &mut Frame, area: Rect, app: &App) {
    let th = &app.theme;
    let items: Vec<ListItem> = app
        .playlist
        .iter()
        .enumerate()
        .map(|(i, t)| {
            let playing = Some(i) == app.playlist_cur;
            let icon = if playing { "♪ " } else { "  " };
            let mut style = Style::default();
            if t.missing {
                style = style.fg(th.meter_clip).add_modifier(Modifier::DIM);
            }
            if playing {
                style = style.fg(th.accent).add_modifier(Modifier::BOLD);
            }
            let label = if t.missing { format!("! {}", t.display_title()) } else { t.display_title() };
            ListItem::new(Line::from(vec![Span::raw(icon), Span::styled(label, style)]))
        })
        .collect();

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(th.accent))
        .title(Span::styled(pane_title(app), Style::default().fg(th.accent)));
    let inner = block.inner(area);
    let mut state = ListState::default();
    state.select(if app.playlist.is_empty() { None } else { Some(app.playlist_sel) });
    let list = List::new(items).block(block).highlight_style(Style::default().add_modifier(Modifier::REVERSED));
    f.render_stateful_widget(list, area, &mut state);

    let mut hit = app.hit.borrow_mut();
    hit.list = Some(inner);
    hit.list_offset = state.offset();
}

fn render_waveform(f: &mut Frame, area: Rect, app: &App) {
    let th = &app.theme;
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(th.border))
        .title(Span::styled(" Waveform ", Style::default().fg(th.text_dim)));
    let inner = block.inner(area);
    f.render_widget(block, area);
    let (w, h) = (inner.width as usize, inner.height as usize);
    if w == 0 || h == 0 {
        return;
    }
    let wave = app.waveform.clone();
    // Absolute amplitude with a gentle curve for low-level visibility and a
    // little headroom so peaks/transients stand out instead of a solid block.
    let amp_at = |x: f64| -> f64 {
        if wave.is_empty() {
            return 0.0;
        }
        let idx = (x * (wave.len() - 1) as f64).round() as usize;
        let a = wave[idx.min(wave.len() - 1)].peak as f64;
        a.powf(0.85).min(0.96)
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
    let level = current_level(app);
    let bar_w = inner.width.saturating_sub(8) as usize;
    let lines: Vec<Line> = ["L", "R"].iter().map(|l| meter_line(l, level, bar_w, th)).collect();
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
    let mut spans = vec![
        Span::styled(format!("{label} "), Style::default().fg(th.text_dim)),
        Span::styled("▕", Style::default().fg(th.border)),
        Span::styled("█".repeat(full), Style::default().fg(color)),
    ];
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

    let vol_w = 20u16.min(inner.width / 3);
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Min(10), Constraint::Length(vol_w)])
        .split(inner);

    // seek bar
    let seek_frac = frac(app.elapsed.as_secs_f64(), app.total.as_secs_f64());
    let elapsed_lbl = format!("{:>5} ", fmt_time(app.elapsed));
    let total_lbl = format!(" {:>5}", fmt_time(app.total));
    let bar_w = (cols[0].width as usize).saturating_sub(elapsed_lbl.len() + total_lbl.len() + 2);
    let (full, partial, rest) = bar_parts(bar_w, seek_frac);
    let mut spans = vec![
        Span::styled(elapsed_lbl.clone(), Style::default().fg(th.text_dim)),
        Span::styled("▕", Style::default().fg(th.border)),
        Span::styled("█".repeat(full), Style::default().fg(th.played)),
    ];
    if let Some(c) = partial {
        spans.push(Span::styled(c.to_string(), Style::default().fg(th.played)));
    }
    spans.push(Span::styled("░".repeat(rest), Style::default().fg(th.unplayed)));
    spans.push(Span::styled("▏", Style::default().fg(th.border)));
    spans.push(Span::styled(total_lbl, Style::default().fg(th.text_dim)));
    f.render_widget(Paragraph::new(Line::from(spans)), cols[0]);

    // record seek-bar hit region (the filled track between the ▕ ▏ guards)
    let seek_x = cols[0].x + elapsed_lbl.len() as u16 + 1;
    app.hit.borrow_mut().seek = Some(Rect { x: seek_x, y: cols[0].y, width: bar_w as u16, height: 1 });

    // volume
    let vlabel = if app.muted {
        "mute".to_string()
    } else if app.vol_effective {
        format!("{:>3}%", (app.volume * 100.0) as u32)
    } else {
        format!("{:>3}%·fix", (app.volume * 100.0) as u32) // bit-perfect: fixed, use DAC
    };
    // Reserve room for the "♪▕" prefix (2), a space, and the label.
    let vbar_w = (cols[1].width as usize).saturating_sub(vlabel.len() + 4);
    let (vf, vp, vr) = bar_parts(vbar_w, if app.muted { 0.0 } else { app.volume });
    let mut vspans = vec![Span::styled("♪", Style::default().fg(th.accent)), Span::styled("▕", Style::default().fg(th.border))];
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
    if let Some((label, buf)) = &app.prompt {
        let p = Paragraph::new(Line::from(vec![
            Span::styled(format!("{label}: "), Style::default().fg(th.accent)),
            Span::raw(buf.clone()),
            Span::styled("▏  (Enter save · Esc cancel)", Style::default().fg(th.text_dim)),
        ]));
        f.render_widget(p, area);
        return;
    }
    if let Some(buf) = &app.find {
        let p = Paragraph::new(Line::from(vec![
            Span::styled("/", Style::default().fg(th.accent)),
            Span::raw(buf.clone()),
            Span::styled("  (Esc)", Style::default().fg(th.text_dim)),
        ]));
        f.render_widget(p, area);
        return;
    }
    let extra = format!("  repeat:{} shuffle:{}", app.repeat.label(), if app.shuffle { "on" } else { "off" });
    let hints = "space ⏎ ←→seek n/p  t transcode  a add  X save-as  L music  Tab panes  / find  ? help  q quit";
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(hints, Style::default().fg(th.text_dim)),
            Span::styled(extra, Style::default().fg(th.accent)),
        ])),
        area,
    );
}

fn draw_help(f: &mut Frame, area: Rect) {
    let text = format!(
        "\
 piwiplay {VERSION} — keys

 space   play / pause          ⏎     open / play / add marked
 S       stop                  a     add selection to playlist
 n / p   next / previous       d     remove from playlist (Playlist)
 ← →     seek ∓5s              x     save playlist (default name)
 Shift←→ seek ∓30s             X     save playlist as… (Shift+x)
 [ ] - + volume                t     toggle native DSD ⇄ transcode(PCM)
 m       mute                        (PCM enables software volume)
 r       repeat cycle          L     jump to system music library
 z       shuffle               Tab   cycle Browser→Playlist→Saved
 ↑↓ k j  move selection        S-Tab cycle panes backward
 Shift+↑↓ multi-select         PgUp/PgDn  first / last item
 /       find                  mouse click=select, dblclick=play,
 ?       close help  q quit           wheel=scroll, click bar=seek
"
    );
    let inner = center_rect(area, 70, 20);
    f.render_widget(Clear, inner);
    let block = Block::default().borders(Borders::ALL).title(format!(" Help — piwiplay {VERSION} "));
    f.render_widget(Paragraph::new(text).block(block), inner);
}

fn draw_message(f: &mut Frame, area: Rect, msg: &str, app: &App) {
    let w = (msg.chars().count() as u16 + 4).min(area.width.saturating_sub(2));
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
