//! High-level playback orchestration.
//!
//! [`Engine`] is the single seam every frontend uses: send [`Command`]s, receive
//! [`Event`]s. It owns the playlist, transport/volume state, the PipeWire
//! [`Sink`], and the waveform worker, and runs its own thread.
//!
//! v2 routes each track to one of two paths: **native DSD** (bit-perfect, volume
//! fixed) or **PCM via ffmpeg** (any format, or DSD transcoded on request — with
//! working software volume). The chosen [`OutputMode`] is reported to the UI.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crossbeam_channel::{bounded, unbounded, Receiver, Sender};

use crate::audio::pipewire_sink::{Sink, SinkCmd, SinkEvent};
use crate::pcm::{self, PcmSource};
use crate::playlist::Playlist;
use crate::types::{DsdInfo, OutputMode, RepeatMode, Tags, TrackInfo, Transport};
use crate::waveform;
use crate::WaveColumn;

const WAVE_BUCKETS: usize = 1600;
const VOL_STEP: f64 = 0.05;

/// Commands accepted by the engine.
#[derive(Debug, Clone)]
pub enum Command {
    OpenAndPlay(PathBuf),
    Enqueue(Vec<PathBuf>),
    Play,
    Pause,
    TogglePlay,
    Stop,
    Next,
    Prev,
    SeekRelative(i64),
    SeekFraction(f64),
    SetVolume(f64),
    VolumeStep(f64),
    ToggleMute,
    CycleRepeat,
    ToggleShuffle,
    /// Toggle the current track between native DSD and ffmpeg/PCM (so volume
    /// applies). No-op for non-DSD tracks (already PCM).
    ToggleTranscode,
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
    /// `effective` is true when volume actually changes loudness (PCM path);
    /// false on the bit-perfect DSD path (use the DAC).
    Volume { level: f64, muted: bool, effective: bool },
    Playlist { tracks: Vec<TrackInfo>, current: Option<usize> },
    Waveform(Arc<Vec<WaveColumn>>),
    Modes { repeat: RepeatMode, shuffle: bool },
    Message(String),
}

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
    force_transcode: bool,

    current_path: Option<PathBuf>,
    current_dsd: Option<DsdInfo>, // Some if the file is DSD
    playing_pcm: bool,            // true if the active path is PCM
    total: Duration,
    elapsed: Duration,
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
            force_transcode: false,
            current_path: None,
            current_dsd: None,
            playing_pcm: false,
            total: Duration::ZERO,
            elapsed: Duration::ZERO,
        }
    }

    fn run(mut self, cmd_rx: Receiver<Command>) {
        self.push_volume();
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
    }

    fn handle_command(&mut self, cmd: Command) {
        match cmd {
            Command::OpenAndPlay(path) => {
                self.playlist.clear();
                self.enqueue_paths(&[path]);
                if self.playlist.get(0).is_some() {
                    self.play_track(0, 0.0);
                }
                self.emit_playlist();
            }
            Command::Enqueue(paths) => {
                let was_empty = self.playlist.is_empty();
                self.enqueue_paths(&paths);
                self.emit_playlist();
                if was_empty && self.transport == Transport::Stopped && self.playlist.get(0).is_some() {
                    self.play_track(0, 0.0);
                }
            }
            Command::Play => self.resume(),
            Command::Pause => self.pause(),
            Command::TogglePlay => match self.transport {
                Transport::Playing => self.pause(),
                Transport::Paused => self.resume(),
                Transport::Stopped => {
                    if self.playlist.get(self.playlist.current().unwrap_or(0)).is_some() {
                        self.play_track(self.playlist.current().unwrap_or(0), 0.0);
                    }
                }
            },
            Command::Stop => self.stop(),
            Command::Next => self.skip(false),
            Command::Prev => self.prev(),
            Command::SeekRelative(secs) => {
                let target = (self.elapsed.as_secs_f64() + secs as f64).max(0.0);
                self.seek_secs(target);
            }
            Command::SeekFraction(f) => {
                let target = f.clamp(0.0, 1.0) * self.total.as_secs_f64();
                self.seek_secs(target);
            }
            Command::SetVolume(v) => {
                self.volume = v.clamp(0.0, 1.0);
                self.push_volume();
            }
            Command::VolumeStep(d) => {
                let step = if d == 0.0 { 0.0 } else { d.signum() * VOL_STEP };
                self.volume = (self.volume + step).clamp(0.0, 1.0);
                self.push_volume();
            }
            Command::ToggleMute => {
                self.muted = !self.muted;
                self.push_volume();
            }
            Command::CycleRepeat => {
                self.playlist.cycle_repeat();
                self.emit_modes();
            }
            Command::ToggleShuffle => {
                self.playlist.toggle_shuffle(seed());
                self.emit_modes();
            }
            Command::ToggleTranscode => self.toggle_transcode(),
            Command::SelectAndPlay(i) => self.play_track(i, 0.0),
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
            } else if crate::decode::is_supported(p) || pcm::is_supported_ext(p) {
                files.push(p.clone());
            }
        }
        files.sort();
        for f in files {
            // Store absolute paths so the queue plays and saved playlists are
            // portable (canonicalize when the file exists, else fall back).
            let abs = std::fs::canonicalize(&f).unwrap_or_else(|_| crate::playlist::absolutize(&f));
            self.playlist.push(track_info(&abs));
        }
    }

    /// Play playlist entry `i`, starting at `start_secs`.
    fn play_track(&mut self, i: usize, start_secs: f64) {
        let Some(track) = self.playlist.set_current(i).cloned() else { return };
        self.current_path = Some(track.path.clone());
        self.mode = OutputMode::Unknown;
        self.transport = Transport::Playing;
        self.elapsed = Duration::from_secs_f64(start_secs);

        match crate::decode::open(&track.path) {
            Ok(dec) => {
                let info = dec.info().clone();
                self.current_dsd = Some(info.clone());
                self.total = info.duration();
                if self.force_transcode {
                    drop(dec);
                    if !self.start_pcm(&track.path, start_secs, Some(info)) {
                        self.skip(false);
                        return;
                    }
                } else {
                    self.playing_pcm = false;
                    self.mode = OutputMode::Native;
                    self.sink.send(SinkCmd::PlayDsd { decoder: dec, info: info.clone() });
                    if start_secs > 0.0 {
                        let bytes = (start_secs * info.spa_rate() as f64) as u64;
                        self.sink.send(SinkCmd::SeekBytes(bytes));
                    }
                }
                self.spawn_waveform(track.path.clone());
            }
            Err(_) => {
                self.current_dsd = None;
                if !self.start_pcm(&track.path, start_secs, None) {
                    if let Some(t) = self.playlist.get_mut(i) {
                        t.missing = true;
                    }
                    self.msg(format!("cannot play {}", track.path.display()));
                    self.skip(false);
                    return;
                }
            }
        }
        self.push_volume();
        self.emit_status();
        self.emit_position();
        self.emit_playlist();
    }

    /// Start the PCM (ffmpeg) path. `dsd` carries the DSD info when transcoding a
    /// DSD file (for a fallback duration). Returns false if it can't start.
    fn start_pcm(&mut self, path: &Path, start_secs: f64, dsd: Option<DsdInfo>) -> bool {
        if !pcm::available() {
            self.msg("ffmpeg not found — install it for non-DSD / transcoded playback".into());
            return false;
        }
        let Some(probe) = pcm::probe(path) else {
            return false;
        };
        let info = probe.target_pcm();
        let src = match PcmSource::open(path, info, Duration::from_secs_f64(start_secs)) {
            Ok(s) => s,
            Err(e) => {
                self.msg(format!("ffmpeg failed: {e}"));
                return false;
            }
        };
        self.total = if probe.duration > Duration::ZERO {
            probe.duration
        } else {
            dsd.map(|d| d.duration()).unwrap_or(Duration::ZERO)
        };
        self.playing_pcm = true;
        self.mode = OutputMode::Transcoded;
        self.sink.send(SinkCmd::PlayPcm { source: Box::new(src), info, start_secs });
        true
    }

    fn toggle_transcode(&mut self) {
        self.force_transcode = !self.force_transcode;
        // Only DSD tracks can switch path; non-DSD is always PCM.
        if self.current_dsd.is_some() && self.transport != Transport::Stopped {
            let i = self.playlist.current().unwrap_or(0);
            let at = self.elapsed.as_secs_f64();
            self.play_track(i, at);
        }
        let m = if self.force_transcode { "on (PCM, volume active)" } else { "off (native DSD)" };
        self.msg(format!("transcode: {m}"));
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
        self.playing_pcm = false;
        self.elapsed = Duration::ZERO;
        self.emit_status();
        self.emit_position();
    }

    fn skip(&mut self, natural: bool) {
        match self.playlist.next_index(natural) {
            Some(i) => self.play_track(i, 0.0),
            None => self.stop(),
        }
    }

    fn prev(&mut self) {
        if self.elapsed.as_secs() > 3 {
            self.seek_secs(0.0);
            return;
        }
        match self.playlist.prev_index() {
            Some(i) => self.play_track(i, 0.0),
            None => self.seek_secs(0.0),
        }
    }

    fn seek_secs(&mut self, target: f64) {
        let target = target.clamp(0.0, self.total.as_secs_f64().max(0.0));
        self.elapsed = Duration::from_secs_f64(target);
        if self.playing_pcm {
            // Re-open the ffmpeg source at the new offset.
            if let Some(path) = self.current_path.clone() {
                let dsd = self.current_dsd.clone();
                self.start_pcm(&path, target, dsd);
            }
        } else if let Some(info) = &self.current_dsd {
            let bytes = (target * info.spa_rate() as f64) as u64;
            self.sink.send(SinkCmd::SeekBytes(bytes));
        }
        self.emit_position();
    }

    fn handle_sink(&mut self, ev: SinkEvent) {
        match ev {
            SinkEvent::Negotiated { mode } => {
                self.mode = mode;
                self.emit_status();
            }
            SinkEvent::PositionSecs(s) => {
                self.elapsed = Duration::from_secs_f64(s.max(0.0));
                self.emit_position();
            }
            SinkEvent::TrackEnded => self.skip(true),
            SinkEvent::Transport(t) => {
                if self.transport != Transport::Stopped || t == Transport::Playing {
                    self.transport = t;
                    self.emit_status();
                }
            }
            SinkEvent::Error(msg) => self.msg(format!("audio: {msg}")),
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

    fn push_volume(&self) {
        let gain = if self.muted { 0.0 } else { self.volume as f32 };
        self.sink.send(SinkCmd::SetVolume(gain));
        let _ = self.events.send(Event::Volume {
            level: self.volume,
            muted: self.muted,
            effective: self.playing_pcm,
        });
    }

    fn emit_status(&self) {
        let _ = self.events.send(Event::Status {
            transport: self.transport,
            track: self.playlist.current_track().cloned(),
            mode: self.mode,
        });
    }

    fn emit_position(&self) {
        let _ = self.events.send(Event::Position { elapsed: self.elapsed, total: self.total });
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

/// Build a [`TrackInfo`] by reading the container header, falling back to an
/// ffprobe for non-DSD files (best effort).
fn track_info(path: &Path) -> TrackInfo {
    if let Ok(dec) = crate::decode::open(path) {
        return TrackInfo {
            path: path.to_path_buf(),
            tags: dec.tags().clone(),
            info: Some(dec.info().clone()),
            missing: false,
        };
    }
    TrackInfo { path: path.to_path_buf(), tags: Tags::default(), info: None, missing: false }
}

fn collect_dir(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    let mut entries: Vec<PathBuf> = rd.filter_map(|e| e.ok().map(|e| e.path())).collect();
    entries.sort();
    for p in entries {
        if p.is_dir() {
            collect_dir(&p, out);
        } else if crate::decode::is_supported(&p) || pcm::is_supported_ext(&p) {
            out.push(p);
        }
    }
}

fn seed() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos() as u64).unwrap_or(0x9E37_79B9)
}
