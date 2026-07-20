//! TUI integration tests: render real frames with ratatui's `TestBackend`
//! (no terminal needed) and assert on the resulting cell buffer.
//!
//! `Engine::start()` spawns the audio threads; when PipeWire is unreachable
//! (CI/sandbox) the sink simply errors out — rendering does not depend on it.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use piwiplay::app::{App, Focus, VERSION};
use piwiplay::theme::Theme;
use piwiplay::ui;
use piwiplay_engine::config::Config;
use piwiplay_engine::{BitOrder, DsdInfo, Engine, OutputMode, Tags, TrackInfo, Transport, WaveColumn};

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
    let theme = Theme::from_config(&cfg);
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

fn render(app: &App, w: u16, h: u16) -> String {
    let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
    term.draw(|f| ui::draw(f, app)).unwrap();
    buffer_text(&term)
}

#[test]
fn renders_full_layout_with_status_and_hints() {
    let mut app = test_app();
    app.transport = Transport::Playing;
    app.track = Some(a_track());
    app.mode = OutputMode::Native;
    app.elapsed = Duration::from_secs(30);
    app.total = Duration::from_secs(120);
    app.waveform = Arc::new((0..1600).map(|i| WaveColumn { peak: ((i % 100) as f32) / 100.0, rms: 0.3 }).collect());

    let text = render(&app, 120, 34);
    assert!(text.contains("piwiplay"));
    assert!(text.contains("Browser"));
    assert!(text.contains("DSD64"));
    assert!(text.contains("NATIVE"));
    assert!(text.contains("Opening"));
    assert!(text.contains("Waveform"));
    assert!(text.contains("Playing"));
}

#[test]
fn transcoded_mode_badge_shows_pcm_and_active_volume() {
    let mut app = test_app();
    app.transport = Transport::Playing;
    app.track = Some(a_track());
    app.mode = OutputMode::Transcoded;
    app.vol_effective = true;
    app.volume = 0.5;
    let text = render(&app, 120, 30);
    assert!(text.contains("PCM"), "transcoded badge is PCM");
    assert!(text.contains("50%"), "volume percentage shown");
    assert!(!text.contains("·fix"), "volume is active (no 'fix' note) on PCM path");
}

#[test]
fn native_mode_marks_volume_fixed() {
    let mut app = test_app();
    app.track = Some(a_track());
    app.mode = OutputMode::Native;
    app.vol_effective = false;
    let text = render(&app, 120, 30);
    assert!(text.contains("fix"), "native DSD marks volume as fixed / use DAC");
}

#[test]
fn multi_select_marks_render() {
    let dir = tempfile::tempdir().unwrap();
    for i in 0..4 {
        std::fs::write(dir.path().join(format!("f{i}.dsf")), b"x").unwrap();
    }
    let mut app = test_app();
    app.browser = piwiplay::fs_browser::Browser::new(dir.path());
    app.browser.set_selected(1);
    app.browser.move_by(1, true); // mark a range of two
    let text = render(&app, 120, 30);
    assert!(text.contains('●'), "marked rows show a bullet marker");
}

#[test]
fn saved_playlists_pane_renders() {
    let mut app = test_app();
    app.focus = Focus::Saved;
    let text = render(&app, 120, 30);
    assert!(text.contains("Saved playlists"), "saved-playlists pane title");
}

#[test]
fn help_overlay_shows_version_and_new_keys() {
    let mut app = test_app();
    app.show_help = true;
    let text = render(&app, 120, 34);
    assert!(text.contains(VERSION), "version shown in help");
    assert!(text.contains("transcode"), "transcode key documented");
    assert!(text.contains("music library"), "music-library key documented");
    assert!(text.contains("multi-select"), "multi-select documented");
}

#[test]
fn save_as_prompt_renders() {
    let mut app = test_app();
    app.prompt = Some(("Save playlist as".into(), "mymix".into()));
    let text = render(&app, 120, 30);
    assert!(text.contains("Save playlist as"));
    assert!(text.contains("mymix"));
}

#[test]
fn too_small_terminal_shows_message() {
    let app = test_app();
    let text = render(&app, 40, 10);
    assert!(text.contains("small"));
}

#[test]
fn single_column_below_two_pane_breakpoint() {
    let mut app = test_app();
    app.track = Some(a_track());
    let text = render(&app, 80, 28);
    assert!(text.contains("Browser"));
    assert!(!text.contains("Waveform"), "waveform pane hidden in single-column layout");
}
