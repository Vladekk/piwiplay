//! Application state: a view-model updated from engine [`Event`]s plus local UI
//! state (focus, browser, selections), and key handling that maps input to
//! engine [`Command`]s.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use piwiplay_engine::config::Config;
use piwiplay_engine::{Command, Engine, Event, OutputMode, RepeatMode, TrackInfo, Transport, WaveColumn};

use crate::fs_browser::Browser;
use crate::theme::Theme;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Browser,
    Playlist,
}

pub struct App {
    pub engine: Engine,
    pub cfg: Config,
    pub theme: Theme,
    pub playlists_dir: PathBuf,

    // view-model (from engine events)
    pub transport: Transport,
    pub track: Option<TrackInfo>,
    pub mode: OutputMode,
    pub elapsed: Duration,
    pub total: Duration,
    pub volume: f64,
    pub muted: bool,
    pub hw_volume: bool,
    pub playlist: Vec<TrackInfo>,
    pub playlist_cur: Option<usize>,
    pub waveform: Arc<Vec<WaveColumn>>,
    pub repeat: RepeatMode,
    pub shuffle: bool,

    // local UI
    pub focus: Focus,
    pub browser: Browser,
    pub playlist_sel: usize,
    pub show_help: bool,
    pub find: Option<String>,
    pub message: Option<(String, Instant)>,
    pub should_quit: bool,
}

impl App {
    pub fn new(engine: Engine, cfg: Config, theme: Theme, start_dir: PathBuf, playlists_dir: PathBuf) -> Self {
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
            hw_volume: false,
            playlist: Vec::new(),
            playlist_cur: None,
            waveform: Arc::new(Vec::new()),
            repeat: RepeatMode::Off,
            shuffle: false,
            focus: Focus::Browser,
            browser: Browser::new(&start_dir),
            playlist_sel: 0,
            show_help: false,
            find: None,
            message: None,
            should_quit: false,
        }
    }

    /// Fold an engine event into the view-model.
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
            Event::Volume { level, muted, hardware } => {
                self.volume = level;
                self.muted = muted;
                self.hw_volume = hardware;
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

    /// Drop a status message after a few seconds.
    pub fn tick(&mut self) {
        if let Some((_, t)) = &self.message {
            if t.elapsed() > Duration::from_secs(4) {
                self.message = None;
            }
        }
    }

    pub fn on_key(&mut self, key: KeyEvent) {
        // Find mode captures printable input.
        if let Some(buf) = self.find.as_mut() {
            match key.code {
                KeyCode::Esc | KeyCode::Enter => self.find = None,
                KeyCode::Backspace => {
                    buf.pop();
                    let needle = buf.clone();
                    self.apply_find(&needle);
                }
                KeyCode::Char(c) => {
                    buf.push(c);
                    let needle = buf.clone();
                    self.apply_find(&needle);
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

            KeyCode::Tab | KeyCode::BackTab => {
                self.focus = match self.focus {
                    Focus::Browser => Focus::Playlist,
                    Focus::Playlist => Focus::Browser,
                };
            }

            KeyCode::Up | KeyCode::Char('k') => self.move_sel(-1),
            KeyCode::Down | KeyCode::Char('j') => self.move_sel(1),
            KeyCode::Char('g') => self.sel_top(),
            KeyCode::Char('G') => self.sel_bottom(),

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
                if self.focus == Focus::Playlist && !self.playlist.is_empty() {
                    self.engine.command(Command::RemoveTrack(self.playlist_sel));
                }
            }
            KeyCode::Char('x') => self.save_playlist(),

            _ => {}
        }
    }

    fn apply_find(&mut self, needle: &str) {
        match self.focus {
            Focus::Browser => self.browser.find(needle),
            Focus::Playlist => {
                let n = needle.to_lowercase();
                if let Some(i) = self
                    .playlist
                    .iter()
                    .position(|t| t.display_title().to_lowercase().contains(&n))
                {
                    self.playlist_sel = i;
                }
            }
        }
    }

    fn move_sel(&mut self, delta: isize) {
        match self.focus {
            Focus::Browser => self.browser.move_by(delta),
            Focus::Playlist => {
                if self.playlist.is_empty() {
                    return;
                }
                let n = self.playlist.len() as isize;
                let s = (self.playlist_sel as isize + delta).clamp(0, n - 1);
                self.playlist_sel = s as usize;
            }
        }
    }

    fn sel_top(&mut self) {
        match self.focus {
            Focus::Browser => self.browser.to_top(),
            Focus::Playlist => self.playlist_sel = 0,
        }
    }

    fn sel_bottom(&mut self) {
        match self.focus {
            Focus::Browser => self.browser.to_bottom(),
            Focus::Playlist => self.playlist_sel = self.playlist.len().saturating_sub(1),
        }
    }

    fn activate(&mut self) {
        match self.focus {
            Focus::Browser => {
                let entry = self.browser.selected_entry().cloned();
                if let Some(e) = entry {
                    if e.is_dir {
                        self.browser.enter_dir();
                    } else if e.is_playlist {
                        self.engine.command(Command::LoadPlaylist(e.path));
                    } else {
                        self.engine.command(Command::OpenAndPlay(e.path));
                        self.focus = Focus::Playlist;
                    }
                }
            }
            Focus::Playlist => {
                if !self.playlist.is_empty() {
                    self.engine.command(Command::SelectAndPlay(self.playlist_sel));
                }
            }
        }
    }

    fn add_selected(&mut self) {
        if self.focus == Focus::Browser {
            if let Some(e) = self.browser.selected_entry() {
                if !e.is_parent {
                    self.engine.command(Command::Enqueue(vec![e.path.clone()]));
                }
            }
        }
    }

    fn save_playlist(&mut self) {
        let path = self.playlists_dir.join("piwiplay.m3u");
        self.engine.command(Command::SavePlaylist(path));
    }
}
