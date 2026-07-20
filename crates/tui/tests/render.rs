//! TUI integration tests: render real frames with ratatui's `TestBackend`
//! (no terminal needed) and assert on the resulting cell buffer. This is how
//! the layout/widgets are regression-tested end to end.
//!
//! `Engine::start()` spawns the audio threads; when PipeWire is unreachable
//! (CI/sandbox) the sink simply errors out — rendering does not depend on it,
//! so these tests are hermetic.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use piwiplay::app::{App, Focus};
use piwiplay::theme::Theme;
use piwiplay::ui;
use piwiplay_engine::config::Config;
use piwiplay_engine::{BitOrder, DsdInfo, Engine, Tags, TrackInfo, Transport, WaveColumn};

use ratatui::backend::TestBackend;
use ratatui::Terminal;

fn buffer_text(t: &Terminal<TestBackend>) -> String {
    let buf = t.backend().buffer();
    let w = buf.area.width;
    let mut s = String::new();
    for (i, cell) in buf.content.iter().enumerate() {
        if i as u16 % w == 0 && i != 0 {
            s.push('\n');
        }
        s.push_str(cell.symbol());
    }
    s
}

fn test_app() -> App {
    let cfg = Config::default();
    let theme = Theme::from_config(&cfg.theme);
    let engine = Engine::start();
    App::new(engine, cfg, theme, PathBuf::from("."), PathBuf::from("."))
}

fn a_track() -> TrackInfo {
    TrackInfo {
        path: PathBuf::from("/music/Opening.dsf"),
        tags: Tags { title: Some("Opening".into()), artist: Some("Artist".into()), album: None },
        info: Some(DsdInfo {
            channels: 2,
            sample_rate: 2_822_400,
            bit_order: BitOrder::Lsb,
            samples_per_channel: 2_822_400 * 120,
        }),
        missing: false,
    }
}

#[test]
fn renders_full_layout_with_status_and_hints() {
    let mut app = test_app();
    app.transport = Transport::Playing;
    app.track = Some(a_track());
    app.mode = piwiplay_engine::OutputMode::Native;
    app.elapsed = Duration::from_secs(30);
    app.total = Duration::from_secs(120);
    app.waveform = Arc::new((0..1600).map(|i| WaveColumn { peak: ((i % 100) as f32) / 100.0, rms: 0.3 }).collect());

    let mut term = Terminal::new(TestBackend::new(120, 34)).unwrap();
    term.draw(|f| ui::draw(f, &app)).unwrap();
    let text = buffer_text(&term);

    assert!(text.contains("piwiplay"), "title present");
    assert!(text.contains("Browser"), "browser pane present");
    assert!(text.contains("DSD64"), "format badge present");
    assert!(text.contains("NATIVE"), "output mode present");
    assert!(text.contains("Opening"), "track title present");
    assert!(text.contains("Waveform"), "waveform pane present (two-pane at 120 wide)");
    assert!(text.contains("Playing"), "transport state present");
}

#[test]
fn playlist_focus_shows_now_playing_marker() {
    let mut app = test_app();
    app.focus = Focus::Playlist;
    app.playlist = vec![a_track(), a_track()];
    app.playlist_cur = Some(0);
    app.playlist_sel = 1;

    let mut term = Terminal::new(TestBackend::new(120, 34)).unwrap();
    term.draw(|f| ui::draw(f, &app)).unwrap();
    let text = buffer_text(&term);
    assert!(text.contains("Playlist"), "playlist pane title");
    assert!(text.contains('♪'), "now-playing marker rendered");
}

#[test]
fn too_small_terminal_shows_message() {
    let app = test_app();
    let mut term = Terminal::new(TestBackend::new(40, 10)).unwrap();
    term.draw(|f| ui::draw(f, &app)).unwrap();
    let text = buffer_text(&term);
    assert!(text.contains("too small") || text.contains("small"), "small-terminal notice shown");
}

#[test]
fn single_column_below_two_pane_breakpoint() {
    let mut app = test_app();
    app.track = Some(a_track());
    // 80 wide is >= min (60) but < two-pane (100): waveform pane hidden.
    let mut term = Terminal::new(TestBackend::new(80, 28)).unwrap();
    term.draw(|f| ui::draw(f, &app)).unwrap();
    let text = buffer_text(&term);
    assert!(text.contains("Browser"), "list still present");
    assert!(!text.contains("Waveform"), "waveform pane hidden in single-column layout");
}

#[test]
fn help_overlay_renders() {
    let mut app = test_app();
    app.show_help = true;
    let mut term = Terminal::new(TestBackend::new(120, 34)).unwrap();
    term.draw(|f| ui::draw(f, &app)).unwrap();
    let text = buffer_text(&term);
    assert!(text.contains("Help"), "help overlay title");
    assert!(text.contains("play / pause"), "help lists keys");
}
