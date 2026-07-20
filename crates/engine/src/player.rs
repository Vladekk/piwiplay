//! High-level playback orchestration.
//!
//! [`Engine`] is the single seam every frontend uses: send [`Command`]s, receive
//! [`Event`]s. It owns the playlist, transport/volume state, the PipeWire
//! [`Sink`], and the waveform worker, and runs its own thread so the UI never
//! blocks. A WebUI would drive the exact same Command/Event API over a socket.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crossbeam_channel::{bounded, unbounded, Receiver, Sender};

use crate::audio::pipewire_sink::{Sink, SinkCmd, SinkEvent};
use crate::decode;
use crate::playlist::Playlist;
use crate::types::{DsdInfo, OutputMode, RepeatMode, Tags, TrackInfo, Transport};
use crate::waveform::{self};
use crate::WaveColumn;

/// Number of waveform columns computed per track; the UI subsamples to width.
const WAVE_BUCKETS: usize = 1600;
/// Volume step for VolumeStep(+/-).
const VOL_STEP: f64 = 0.05;

/// Commands accepted by the engine.
#[derive(Debug, Clone)]
pub enum Command {
    /// Clear the queue, enqueue this path (file or dir), and play it.
    OpenAndPlay(PathBuf),
    /// Add files/dirs to the queue without changing playback.
    Enqueue(Vec<PathBuf>),
    Play,
    Pause,
    TogglePlay,
    Stop,
    Next,
    Prev,
    /// Seek by ±seconds.
    SeekRelative(i64),
    /// Seek to a fraction 0.0..=1.0 of the track.
    SeekFraction(f64),
    SetVolume(f64),
    VolumeStep(f64),
    ToggleMute,
    CycleRepeat,
    ToggleShuffle,
    /// Select a playlist row and play it.
    SelectAndPlay(usize),
    RemoveTrack(usize),
    ClearPlaylist,
    SavePlaylist(PathBuf),
    LoadPlaylist(PathBuf),
    Quit,
}

/// Events emitted by the engine for a frontend to render.
#[derive(Debug, Clone)]
pub enum Event {
    Status { transport: Transport, track: Option<TrackInfo>, mode: OutputMode },
    Position { elapsed: Duration, total: Duration },
    Volume { level: f64, muted: bool, hardware: bool },
    Playlist { tracks: Vec<TrackInfo>, current: Option<usize> },
    Waveform(Arc<Vec<WaveColumn>>),
    Modes { repeat: RepeatMode, shuffle: bool },
    Message(String),
}

/// The engine handle held by a frontend.
pub struct Engine {
    cmd_tx: Sender<Command>,
    event_rx: Receiver<Event>,
    thread: Option<JoinHandle<()>>,
}

impl Engine {
    pub fn start() -> Self {
        let (cmd_tx, cmd_rx) = unbounded::<Command>();
        let (event_tx, event_rx) = unbounded::<Event>();
        let thread = thread::Builder::new()
            .name("piwiplay-player".into())
            .spawn(move || PlayerState::new(event_tx).run(cmd_rx))
            .expect("spawn player");
        Engine { cmd_tx, event_rx, thread: Some(thread) }
    }

    pub fn command(&self, cmd: Command) {
        let _ = self.cmd_tx.send(cmd);
    }

    pub fn events(&self) -> &Receiver<Event> {
        &self.event_rx
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        let _ = self.cmd_tx.send(Command::Quit);
        if let Some(h) = self.thread.take() {
            let _ = h.join();
        }
    }
}

/// A finished waveform computation, tagged with the path it was for.
struct WaveResult {
    path: PathBuf,
    columns: Vec<WaveColumn>,
}

struct PlayerState {
    events: Sender<Event>,
    sink: Sink,
    sink_rx: Receiver<SinkEvent>,
    wave_tx: Sender<WaveResult>,
    wave_rx: Receiver<WaveResult>,

    playlist: Playlist,
    transport: Transport,
    mode: OutputMode,
    volume: f64,
    muted: bool,
    hardware_volume: bool,

    current_info: Option<DsdInfo>,
    current_path: Option<PathBuf>,
    pos_bytes: u64,
}

impl PlayerState {
    fn new(events: Sender<Event>) -> Self {
        let (sink_tx, sink_rx) = unbounded::<SinkEvent>();
        let sink = Sink::spawn(sink_tx);
        let (wave_tx, wave_rx) = bounded::<WaveResult>(4);
        Self {
            events,
            sink,
            sink_rx,
            wave_tx,
            wave_rx,
            playlist: Playlist::new(),
            transport: Transport::Stopped,
            mode: OutputMode::Unknown,
            volume: 0.7,
            muted: false,
            hardware_volume: false, // v1: DSD is bit-perfect; volume is fixed (use DAC)
            current_info: None,
            current_path: None,
            pos_bytes: 0,
        }
    }

    fn run(mut self, cmd_rx: Receiver<Command>) {
        self.emit_volume();
        self.emit_modes();
        loop {
            crossbeam_channel::select! {
                recv(cmd_rx) -> msg => match msg {
                    Ok(Command::Quit) | Err(_) => break,
                    Ok(cmd) => self.handle_command(cmd),
                },
                recv(self.sink_rx) -> msg => if let Ok(ev) = msg { self.handle_sink(ev); },
                recv(self.wave_rx) -> msg => if let Ok(w) = msg { self.handle_wave(w); },
            }
        }
        // Engine dropping Sink stops sink/feeder threads.
    }

    // ---- command handling ----

    fn handle_command(&mut self, cmd: Command) {
        match cmd {
            Command::OpenAndPlay(path) => {
                self.playlist.clear();
                self.enqueue_paths(&[path]);
                if let Some(i) = self.playlist.next_index(false).or(Some(0)) {
                    self.play_index(i);
                }
                self.emit_playlist();
            }
            Command::Enqueue(paths) => {
                self.enqueue_paths(&paths);
                self.emit_playlist();
                if self.transport == Transport::Stopped {
                    if let Some(i) = self.playlist.current().or(Some(0)) {
                        if self.playlist.get(i).is_some() {
                            self.play_index(i);
                        }
                    }
                }
            }
            Command::Play => self.resume(),
            Command::Pause => self.pause(),
            Command::TogglePlay => match self.transport {
                Transport::Playing => self.pause(),
                Transport::Paused => self.resume(),
                Transport::Stopped => {
                    if let Some(i) = self.playlist.current().or(Some(0)) {
                        if self.playlist.get(i).is_some() {
                            self.play_index(i);
                        }
                    }
                }
            },
            Command::Stop => self.stop(),
            Command::Next => self.skip(false),
            Command::Prev => self.prev(),
            Command::SeekRelative(secs) => self.seek_relative(secs),
            Command::SeekFraction(f) => self.seek_fraction(f),
            Command::SetVolume(v) => {
                self.volume = v.clamp(0.0, 1.0);
                self.emit_volume();
            }
            Command::VolumeStep(d) => {
                let step = if d == 0.0 { 0.0 } else { d.signum() * VOL_STEP };
                self.volume = (self.volume + step).clamp(0.0, 1.0);
                self.emit_volume();
            }
            Command::ToggleMute => {
                self.muted = !self.muted;
                self.emit_volume();
            }
            Command::CycleRepeat => {
                self.playlist.cycle_repeat();
                self.emit_modes();
            }
            Command::ToggleShuffle => {
                self.playlist.toggle_shuffle(seed());
                self.emit_modes();
            }
            Command::SelectAndPlay(i) => self.play_index(i),
            Command::RemoveTrack(i) => {
                self.playlist.remove(i);
                self.emit_playlist();
            }
            Command::ClearPlaylist => {
                self.stop();
                self.playlist.clear();
                self.emit_playlist();
            }
            Command::SavePlaylist(p) => match self.playlist.save_m3u(&p) {
                Ok(()) => self.msg(format!("saved playlist: {}", p.display())),
                Err(e) => self.msg(format!("save failed: {e}")),
            },
            Command::LoadPlaylist(p) => match Playlist::load_m3u_paths(&p) {
                Ok(paths) => {
                    self.playlist.clear();
                    self.enqueue_paths(&paths);
                    self.emit_playlist();
                    self.msg(format!("loaded {} tracks", self.playlist.len()));
                }
                Err(e) => self.msg(format!("load failed: {e}")),
            },
            Command::Quit => {}
        }
    }

    fn enqueue_paths(&mut self, paths: &[PathBuf]) {
        let mut files = Vec::new();
        for p in paths {
            if p.is_dir() {
                collect_dir(p, &mut files);
            } else if decode::is_supported(p) {
                files.push(p.clone());
            }
        }
        files.sort();
        for f in files {
            self.playlist.push(track_info(&f));
        }
    }

    fn play_index(&mut self, i: usize) {
        let Some(track) = self.playlist.set_current(i).cloned() else { return };
        match decode::open(&track.path) {
            Ok(dec) => {
                let info = dec.info().clone();
                self.current_info = Some(info.clone());
                self.current_path = Some(track.path.clone());
                self.pos_bytes = 0;
                self.mode = OutputMode::Unknown;
                self.transport = Transport::Playing;
                self.sink.send(SinkCmd::Play { decoder: dec, info });
                self.spawn_waveform(track.path.clone());
                self.emit_status();
                self.emit_position();
                self.emit_playlist();
            }
            Err(e) => {
                if let Some(t) = self.playlist.get_mut(i) {
                    t.missing = true;
                }
                self.msg(format!("cannot play {}: {e}", track.path.display()));
                self.skip(false);
            }
        }
    }

    fn resume(&mut self) {
        if self.transport == Transport::Paused {
            self.sink.send(SinkCmd::Resume);
            self.transport = Transport::Playing;
            self.emit_status();
        }
    }

    fn pause(&mut self) {
        if self.transport == Transport::Playing {
            self.sink.send(SinkCmd::Pause);
            self.transport = Transport::Paused;
            self.emit_status();
        }
    }

    fn stop(&mut self) {
        self.sink.send(SinkCmd::Stop);
        self.transport = Transport::Stopped;
        self.pos_bytes = 0;
        self.emit_status();
        self.emit_position();
    }

    fn skip(&mut self, natural: bool) {
        match self.playlist.next_index(natural) {
            Some(i) => self.play_index(i),
            None => self.stop(),
        }
    }

    fn prev(&mut self) {
        // Restart current track if we're past the first few seconds.
        if self.elapsed().as_secs() > 3 {
            self.seek_fraction(0.0);
            return;
        }
        match self.playlist.prev_index() {
            Some(i) => self.play_index(i),
            None => self.seek_fraction(0.0),
        }
    }

    fn seek_relative(&mut self, secs: i64) {
        let Some(info) = &self.current_info else { return };
        let rate = info.spa_rate() as i64;
        let cur = self.pos_bytes as i64;
        let target = (cur + secs * rate).clamp(0, info.total_bytes() as i64) as u64;
        self.pos_bytes = target;
        self.sink.send(SinkCmd::SeekBytes(target));
        self.emit_position();
    }

    fn seek_fraction(&mut self, f: f64) {
        let Some(info) = &self.current_info else { return };
        let target = (f.clamp(0.0, 1.0) * info.total_bytes() as f64) as u64;
        self.pos_bytes = target;
        self.sink.send(SinkCmd::SeekBytes(target));
        self.emit_position();
    }

    // ---- sink / waveform events ----

    fn handle_sink(&mut self, ev: SinkEvent) {
        match ev {
            SinkEvent::Negotiated { mode, .. } => {
                self.mode = mode;
                self.emit_status();
            }
            SinkEvent::PositionBytes(b) => {
                self.pos_bytes = b;
                self.emit_position();
            }
            SinkEvent::TrackEnded => self.skip(true),
            SinkEvent::Transport(t) => {
                // Sink is authoritative for Playing/Paused transitions it drives.
                if self.transport != Transport::Stopped || t == Transport::Playing {
                    self.transport = t;
                    self.emit_status();
                }
            }
            SinkEvent::Error(msg) => {
                self.msg(format!("audio: {msg}"));
            }
        }
    }

    fn handle_wave(&mut self, w: WaveResult) {
        if self.current_path.as_deref() == Some(w.path.as_path()) {
            let _ = self.events.send(Event::Waveform(Arc::new(w.columns)));
        }
    }

    fn spawn_waveform(&self, path: PathBuf) {
        let tx = self.wave_tx.clone();
        thread::Builder::new()
            .name("piwiplay-wave".into())
            .spawn(move || {
                if let Ok(columns) = waveform::compute(&path, WAVE_BUCKETS) {
                    let _ = tx.send(WaveResult { path, columns });
                }
            })
            .ok();
    }

    // ---- emit helpers ----

    fn elapsed(&self) -> Duration {
        match &self.current_info {
            Some(i) if i.spa_rate() > 0 => Duration::from_secs_f64(self.pos_bytes as f64 / i.spa_rate() as f64),
            _ => Duration::ZERO,
        }
    }

    fn emit_status(&self) {
        let _ = self.events.send(Event::Status {
            transport: self.transport,
            track: self.playlist.current_track().cloned(),
            mode: self.mode,
        });
    }

    fn emit_position(&self) {
        let total = self.current_info.as_ref().map(|i| i.duration()).unwrap_or(Duration::ZERO);
        let _ = self.events.send(Event::Position { elapsed: self.elapsed(), total });
    }

    fn emit_volume(&self) {
        let _ = self.events.send(Event::Volume {
            level: self.volume,
            muted: self.muted,
            hardware: self.hardware_volume,
        });
    }

    fn emit_modes(&self) {
        let _ = self.events.send(Event::Modes {
            repeat: self.playlist.repeat(),
            shuffle: self.playlist.shuffle(),
        });
    }

    fn emit_playlist(&self) {
        let _ = self.events.send(Event::Playlist {
            tracks: self.playlist.tracks().to_vec(),
            current: self.playlist.current(),
        });
    }

    fn msg(&self, m: String) {
        let _ = self.events.send(Event::Message(m));
    }
}

/// Build a [`TrackInfo`] by reading the container header (best effort).
fn track_info(path: &Path) -> TrackInfo {
    match decode::open(path) {
        Ok(dec) => TrackInfo {
            path: path.to_path_buf(),
            tags: dec.tags().clone(),
            info: Some(dec.info().clone()),
            missing: false,
        },
        Err(_) => TrackInfo {
            path: path.to_path_buf(),
            tags: Tags::default(),
            info: None,
            missing: true,
        },
    }
}

/// Recursively collect supported files from a directory.
fn collect_dir(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    let mut entries: Vec<PathBuf> = rd.filter_map(|e| e.ok().map(|e| e.path())).collect();
    entries.sort();
    for p in entries {
        if p.is_dir() {
            collect_dir(&p, out);
        } else if decode::is_supported(&p) {
            out.push(p);
        }
    }
}

fn seed() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos() as u64).unwrap_or(0x9E37_79B9)
}
