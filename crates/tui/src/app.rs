//! Application state: a view-model updated from engine [`Event`]s plus local UI
//! state (focus, browsers, selections, prompts), key handling, and mouse
//! handling. Key/mouse input is translated to engine [`Command`]s.

use std::cell::RefCell;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;

use piwiplay_engine::config::Config;
use piwiplay_engine::{Command, Engine, Event, OutputMode, RepeatMode, TrackInfo, Transport, WaveColumn};

use crate::fs_browser::{self, Browser};
use crate::theme::Theme;

/// The version string, shown in the help overlay.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Browser,
    Queue,
    /// Saved playlists in the default playlists directory.
    Saved,
}

impl Focus {
    fn next(self) -> Self {
        match self {
            Focus::Browser => Focus::Queue,
            Focus::Queue => Focus::Saved,
            Focus::Saved => Focus::Browser,
        }
    }
    fn prev(self) -> Self {
        match self {
            Focus::Browser => Focus::Saved,
            Focus::Queue => Focus::Browser,
            Focus::Saved => Focus::Queue,
        }
    }
    pub fn title(self) -> &'static str {
        match self {
            Focus::Browser => "Browser",
            Focus::Queue => "Playlist",
            Focus::Saved => "Saved playlists",
        }
    }
}

/// Rects from the last render, used for mouse hit-testing (interior mutability
/// so `ui::draw(&App)` can populate them).
#[derive(Default, Clone)]
pub struct Hit {
    pub list: Option<Rect>,
    pub list_offset: usize,
    pub seek: Option<Rect>,
}

pub struct App {
    pub engine: Engine,
    pub cfg: Config,
    pub theme: Theme,
    pub playlists_dir: PathBuf,

    // view-model
    pub transport: Transport,
    pub track: Option<TrackInfo>,
    pub mode: OutputMode,
    pub elapsed: Duration,
    pub total: Duration,
    pub volume: f64,
    pub muted: bool,
    pub vol_effective: bool,
    pub playlist: Vec<TrackInfo>,
    pub playlist_cur: Option<usize>,
    pub waveform: Arc<Vec<WaveColumn>>,
    pub repeat: RepeatMode,
    pub shuffle: bool,

    // local UI
    pub focus: Focus,
    pub browser: Browser,
    pub saved: Browser,
    pub playlist_sel: usize,
    pub show_help: bool,
    pub find: Option<String>,
    /// Active text prompt (label, buffer) — used for "save playlist as".
    pub prompt: Option<(String, String)>,
    pub message: Option<(String, Instant)>,
    pub should_quit: bool,
    pub hit: RefCell<Hit>,
}

impl App {
    pub fn new(engine: Engine, cfg: Config, theme: Theme, start_dir: PathBuf, playlists_dir: PathBuf) -> Self {
        let saved = Browser::playlists_at(&playlists_dir);
        App {
            engine,
            cfg,
            theme,
            playlists_dir,
            transport: Transport::Stopped,
            track: None,
            mode: OutputMode::Unknown,
            elapsed: Duration::ZERO,
            total: Duration::ZERO,
            volume: 0.7,
            muted: false,
            vol_effective: false,
            playlist: Vec::new(),
            playlist_cur: None,
            waveform: Arc::new(Vec::new()),
            repeat: RepeatMode::Off,
            shuffle: false,
            focus: Focus::Browser,
            browser: Browser::new(&start_dir),
            saved,
            playlist_sel: 0,
            show_help: false,
            find: None,
            prompt: None,
            message: None,
            should_quit: false,
            hit: RefCell::new(Hit::default()),
        }
    }

    /// A concise title for the terminal window: track + status.
    pub fn window_title(&self) -> String {
        let state = match self.transport {
            Transport::Playing => "▶",
            Transport::Paused => "⏸",
            Transport::Stopped => "■",
        };
        match &self.track {
            Some(t) => format!("piwiplay {state} {}", t.display_title()),
            None => "piwiplay".into(),
        }
    }

    pub fn apply_event(&mut self, ev: Event) {
        match ev {
            Event::Status { transport, track, mode } => {
                self.transport = transport;
                self.track = track;
                self.mode = mode;
            }
            Event::Position { elapsed, total } => {
                self.elapsed = elapsed;
                self.total = total;
            }
            Event::Volume { level, muted, effective } => {
                self.volume = level;
                self.muted = muted;
                self.vol_effective = effective;
            }
            Event::Playlist { tracks, current } => {
                self.playlist = tracks;
                self.playlist_cur = current;
                self.playlist_sel = self.playlist_sel.min(self.playlist.len().saturating_sub(1));
            }
            Event::Waveform(w) => self.waveform = w,
            Event::Modes { repeat, shuffle } => {
                self.repeat = repeat;
                self.shuffle = shuffle;
            }
            Event::Message(m) => self.message = Some((m, Instant::now())),
        }
    }

    pub fn tick(&mut self) {
        if let Some((_, t)) = &self.message {
            if t.elapsed() > Duration::from_secs(4) {
                self.message = None;
            }
        }
    }

    pub fn on_key(&mut self, key: KeyEvent) {
        // Text prompt (save-as) captures input first.
        if let Some((_, buf)) = self.prompt.as_mut() {
            match key.code {
                KeyCode::Esc => self.prompt = None,
                KeyCode::Enter => {
                    let name = buf.trim().to_string();
                    self.prompt = None;
                    if !name.is_empty() {
                        let fname = if name.ends_with(".m3u") { name } else { format!("{name}.m3u") };
                        self.engine.command(Command::SavePlaylist(self.playlists_dir.join(fname)));
                    }
                }
                KeyCode::Backspace => {
                    buf.pop();
                }
                KeyCode::Char(c) => buf.push(c),
                _ => {}
            }
            return;
        }

        if let Some(buf) = self.find.as_mut() {
            match key.code {
                KeyCode::Esc | KeyCode::Enter => self.find = None,
                KeyCode::Backspace => {
                    buf.pop();
                    let n = buf.clone();
                    self.apply_find(&n);
                }
                KeyCode::Char(c) => {
                    buf.push(c);
                    let n = buf.clone();
                    self.apply_find(&n);
                }
                _ => {}
            }
            return;
        }

        if self.show_help {
            self.show_help = false;
            return;
        }

        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let shift = key.modifiers.contains(KeyModifiers::SHIFT);

        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => self.should_quit = true,
            KeyCode::Char('c') if ctrl => self.should_quit = true,
            KeyCode::Char('?') => self.show_help = true,
            KeyCode::Char('/') => self.find = Some(String::new()),

            KeyCode::Char(' ') => self.engine.command(Command::TogglePlay),
            KeyCode::Char('S') => self.engine.command(Command::Stop),
            KeyCode::Char('n') => self.engine.command(Command::Next),
            KeyCode::Char('p') => self.engine.command(Command::Prev),
            KeyCode::Char('t') => self.engine.command(Command::ToggleTranscode),

            KeyCode::Tab => self.focus = self.focus.next(),
            KeyCode::BackTab => self.focus = self.focus.prev(),

            KeyCode::Up | KeyCode::Char('k') => self.move_sel(-1, shift),
            KeyCode::Down | KeyCode::Char('j') => self.move_sel(1, shift),
            KeyCode::PageUp | KeyCode::Char('g') => self.sel_top(shift),
            KeyCode::PageDown | KeyCode::Char('G') => self.sel_bottom(shift),

            KeyCode::Left | KeyCode::Char('h') => {
                self.engine.command(Command::SeekRelative(if shift { -30 } else { -5 }));
            }
            KeyCode::Right | KeyCode::Char('l') => {
                self.engine.command(Command::SeekRelative(if shift { 30 } else { 5 }));
            }

            KeyCode::Char('[') | KeyCode::Char('-') => self.engine.command(Command::VolumeStep(-1.0)),
            KeyCode::Char(']') | KeyCode::Char('+') | KeyCode::Char('=') => {
                self.engine.command(Command::VolumeStep(1.0));
            }
            KeyCode::Char('m') => self.engine.command(Command::ToggleMute),
            KeyCode::Char('r') => self.engine.command(Command::CycleRepeat),
            KeyCode::Char('z') => self.engine.command(Command::ToggleShuffle),

            KeyCode::Enter => self.activate(),
            KeyCode::Char('a') => self.add_selected(),
            KeyCode::Char('d') => {
                if self.focus == Focus::Queue && !self.playlist.is_empty() {
                    self.engine.command(Command::RemoveTrack(self.playlist_sel));
                }
            }
            KeyCode::Char('X') => self.prompt = Some(("Save playlist as".into(), String::new())),
            KeyCode::Char('x') => {
                self.engine.command(Command::SavePlaylist(self.playlists_dir.join("piwiplay.m3u")));
            }
            KeyCode::Char('L') => self.open_media_library(),

            _ => {}
        }
    }

    pub fn on_mouse(&mut self, ev: MouseEvent) {
        if !self.cfg.ui.mouse {
            return;
        }
        match ev.kind {
            MouseEventKind::ScrollUp => self.move_sel(-1, false),
            MouseEventKind::ScrollDown => self.move_sel(1, false),
            MouseEventKind::Down(MouseButton::Left) => self.on_click(ev.column, ev.row),
            _ => {}
        }
    }

    fn on_click(&mut self, col: u16, row: u16) {
        let hit = self.hit.borrow().clone();
        // Click on the seek bar → seek to that fraction.
        if let Some(seek) = hit.seek {
            if contains(seek, col, row) && seek.width > 0 {
                let frac = (col - seek.x) as f64 / seek.width as f64;
                self.engine.command(Command::SeekFraction(frac));
                return;
            }
        }
        // Click on a list row → select it (a click on the already-selected row
        // activates it).
        if let Some(list) = hit.list {
            if contains(list, col, row) {
                let idx = hit.list_offset + (row - list.y) as usize;
                match self.focus {
                    Focus::Browser => {
                        let same = self.browser.selected == idx;
                        self.browser.set_selected(idx);
                        if same {
                            self.activate();
                        }
                    }
                    Focus::Saved => {
                        let same = self.saved.selected == idx;
                        self.saved.set_selected(idx);
                        if same {
                            self.activate();
                        }
                    }
                    Focus::Queue => {
                        if idx < self.playlist.len() {
                            let same = self.playlist_sel == idx;
                            self.playlist_sel = idx;
                            if same {
                                self.engine.command(Command::SelectAndPlay(idx));
                            }
                        }
                    }
                }
            }
        }
    }

    fn apply_find(&mut self, needle: &str) {
        match self.focus {
            Focus::Browser => self.browser.find(needle),
            Focus::Saved => self.saved.find(needle),
            Focus::Queue => {
                let n = needle.to_lowercase();
                if let Some(i) = self.playlist.iter().position(|t| t.display_title().to_lowercase().contains(&n)) {
                    self.playlist_sel = i;
                }
            }
        }
    }

    fn move_sel(&mut self, delta: isize, extend: bool) {
        match self.focus {
            Focus::Browser => self.browser.move_by(delta, extend),
            Focus::Saved => self.saved.move_by(delta, false),
            Focus::Queue => {
                if !self.playlist.is_empty() {
                    let n = self.playlist.len() as isize;
                    self.playlist_sel = (self.playlist_sel as isize + delta).clamp(0, n - 1) as usize;
                }
            }
        }
    }

    fn sel_top(&mut self, extend: bool) {
        match self.focus {
            Focus::Browser => self.browser.to_top(extend),
            Focus::Saved => self.saved.to_top(false),
            Focus::Queue => self.playlist_sel = 0,
        }
    }

    fn sel_bottom(&mut self, extend: bool) {
        match self.focus {
            Focus::Browser => self.browser.to_bottom(extend),
            Focus::Saved => self.saved.to_bottom(false),
            Focus::Queue => self.playlist_sel = self.playlist.len().saturating_sub(1),
        }
    }

    fn activate(&mut self) {
        match self.focus {
            Focus::Browser => {
                // Marked multi-selection → add all to the current playlist.
                if self.browser.has_marks() {
                    let paths = self.browser.action_paths();
                    let n = paths.len();
                    self.engine.command(Command::Enqueue(paths));
                    self.message = Some((format!("added {n} items to playlist"), Instant::now()));
                    return;
                }
                let entry = self.browser.selected_entry().cloned();
                if let Some(e) = entry {
                    if e.is_dir {
                        self.browser.enter_dir();
                    } else if e.is_playlist {
                        self.engine.command(Command::LoadPlaylist(e.path));
                        self.focus = Focus::Queue;
                    } else {
                        self.engine.command(Command::OpenAndPlay(e.path));
                        self.focus = Focus::Queue;
                    }
                }
            }
            Focus::Saved => {
                let entry = self.saved.selected_entry().cloned();
                if let Some(e) = entry {
                    if e.is_dir {
                        self.saved.enter_dir();
                    } else if e.is_playlist {
                        self.engine.command(Command::LoadPlaylist(e.path));
                        self.focus = Focus::Queue;
                    }
                }
            }
            Focus::Queue => {
                if !self.playlist.is_empty() {
                    self.engine.command(Command::SelectAndPlay(self.playlist_sel));
                }
            }
        }
    }

    fn add_selected(&mut self) {
        if self.focus == Focus::Browser {
            let paths = self.browser.action_paths();
            if !paths.is_empty() {
                let n = paths.len();
                self.engine.command(Command::Enqueue(paths));
                self.message = Some((format!("added {n} item(s)"), Instant::now()));
            }
        }
    }

    fn open_media_library(&mut self) {
        match fs_browser::media_library_dir() {
            Some(dir) => {
                self.browser.goto(&dir);
                self.focus = Focus::Browser;
                self.message = Some((format!("music library: {}", dir.display()), Instant::now()));
            }
            None => self.message = Some(("no system music library (XDG MUSIC) found".into(), Instant::now())),
        }
    }
}

fn contains(r: Rect, col: u16, row: u16) -> bool {
    col >= r.x && col < r.x + r.width && row >= r.y && row < r.y + r.height
}
