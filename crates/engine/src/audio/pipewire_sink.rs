//! PipeWire sink supporting two output paths (spec-v2):
//!
//! * **Native DSD** — bit-perfect 1-bit passthrough (v1), volume fixed.
//! * **PCM** — interleaved f32 from the ffmpeg decoder (any format, or DSD
//!   transcoded), with software volume applied in the feeder.
//!
//! Threads: a **sink thread** owning the PipeWire loop + stream (the RT
//! `process` callback only drains the ring), and a **feeder thread** that reads
//! the active source (DSD decoder or PCM subprocess) and fills the ring.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU8, AtomicUsize, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use pipewire as pw;
use pw::{properties::properties, spa};
use spa::param::audio::{AudioFormat, AudioInfoRaw, MAX_CHANNELS};
use spa::pod::serialize::PodSerializer;
use spa::pod::{Object, Pod, Property, Value, ValueArray};
use spa::sys;
use spa::utils::Id;

use super::{repack_planar, Layout};
use crate::audio::ring::Ring;
use crate::decode::Decoder;
use crate::pcm::{apply_gain_f32le, PcmInfo, PcmSource};
use crate::types::{DsdInfo, OutputMode, Transport};

const RING_CAPACITY: usize = 4 * 1024 * 1024;
const READ_CHUNK: usize = 64 * 1024;
const DSD_IDLE: u8 = 0x69;

const MODE_NONE: u8 = 0;
const MODE_DSD: u8 = 1;
const MODE_PCM: u8 = 2;

/// Commands accepted by the sink.
pub enum SinkCmd {
    /// Native DSD playback of a decoder.
    PlayDsd { decoder: Box<dyn Decoder>, info: DsdInfo },
    /// PCM playback from an ffmpeg source (any format / transcoded DSD).
    PlayPcm { source: Box<PcmSource>, info: PcmInfo, start_secs: f64 },
    Pause,
    Resume,
    Stop,
    /// DSD-path seek (per-channel byte offset). PCM seeks by re-issuing PlayPcm.
    SeekBytes(u64),
    /// Software volume 0.0..=1.0 (applied on the PCM path only).
    SetVolume(f32),
    Quit,
}

#[derive(Debug, Clone)]
pub enum SinkEvent {
    Negotiated { mode: OutputMode },
    /// Elapsed playback position in seconds (path-independent).
    PositionSecs(f64),
    TrackEnded,
    Transport(Transport),
    Error(String),
}

enum FeedCmd {
    LoadDsd { decoder: Box<dyn Decoder>, src_lsb: bool, channels: usize, spa_rate: u32, base_bytes: u64 },
    LoadPcm { source: Box<PcmSource>, base_secs: f64 },
    Seek(u64),
    Stop,
    Quit,
}

pub struct Sink {
    tx: pw::channel::Sender<SinkCmd>,
    sink_thread: Option<JoinHandle<()>>,
    feeder_thread: Option<JoinHandle<()>>,
}

/// State shared between the sink and feeder threads.
#[derive(Clone)]
struct Shared {
    ring: Ring,
    layout: Arc<Mutex<Option<Layout>>>,
    stride: Arc<AtomicUsize>,
    paused: Arc<AtomicBool>,
    channels: Arc<AtomicU32>,
    mode: Arc<AtomicU8>,
    volume: Arc<Mutex<f32>>,
}

impl Sink {
    pub fn spawn(events: crossbeam_channel::Sender<SinkEvent>) -> Self {
        let shared = Shared {
            ring: Ring::new(RING_CAPACITY),
            layout: Arc::new(Mutex::new(None)),
            stride: Arc::new(AtomicUsize::new(0)),
            paused: Arc::new(AtomicBool::new(true)),
            channels: Arc::new(AtomicU32::new(2)),
            mode: Arc::new(AtomicU8::new(MODE_NONE)),
            volume: Arc::new(Mutex::new(0.7)),
        };
        let (feed_tx, feed_rx) = mpsc::channel::<FeedCmd>();
        let (cmd_tx, cmd_rx) = pw::channel::channel::<SinkCmd>();

        let feeder_thread = {
            let shared = shared.clone();
            let events = events.clone();
            thread::Builder::new()
                .name("piwiplay-feeder".into())
                .spawn(move || feeder_loop(feed_rx, shared, events))
                .expect("spawn feeder")
        };
        let sink_thread = thread::Builder::new()
            .name("piwiplay-sink".into())
            .spawn(move || {
                if let Err(e) = sink_loop(cmd_rx, feed_tx, shared, events.clone()) {
                    let _ = events.send(SinkEvent::Error(e));
                }
            })
            .expect("spawn sink");

        Sink { tx: cmd_tx, sink_thread: Some(sink_thread), feeder_thread: Some(feeder_thread) }
    }

    pub fn send(&self, cmd: SinkCmd) {
        let _ = self.tx.send(cmd);
    }
}

impl Drop for Sink {
    fn drop(&mut self) {
        let _ = self.tx.send(SinkCmd::Quit);
        if let Some(h) = self.sink_thread.take() {
            let _ = h.join();
        }
        if let Some(h) = self.feeder_thread.take() {
            let _ = h.join();
        }
    }
}

fn parse_negotiated(param: &Pod, fallback_channels: usize) -> Option<Layout> {
    let obj = param.as_object().ok()?;
    let interleave = obj
        .find_prop(Id(sys::SPA_FORMAT_AUDIO_interleave))
        .and_then(|p| p.value().get_int().ok())
        .unwrap_or(1);
    let dst_lsb = obj
        .find_prop(Id(sys::SPA_FORMAT_AUDIO_bitorder))
        .and_then(|p| p.value().get_id().ok())
        .map(|Id(b)| b != sys::SPA_PARAM_BITORDER_msb)
        .unwrap_or(true);
    let channels = obj
        .find_prop(Id(sys::SPA_FORMAT_AUDIO_channels))
        .and_then(|p| p.value().get_int().ok())
        .map(|c| c as usize)
        .unwrap_or(fallback_channels);
    Some(Layout { interleave, dst_lsb, channels })
}

fn dsd_positions(channels: u32) -> Vec<Id> {
    match channels {
        1 => vec![Id(sys::SPA_AUDIO_CHANNEL_MONO)],
        _ => (0..channels)
            .map(|i| match i {
                0 => Id(sys::SPA_AUDIO_CHANNEL_FL),
                1 => Id(sys::SPA_AUDIO_CHANNEL_FR),
                n => Id(sys::SPA_AUDIO_CHANNEL_AUX0 + n),
            })
            .collect(),
    }
}

fn build_dsd_format(info: &DsdInfo) -> Vec<u8> {
    let obj = Object {
        type_: sys::SPA_TYPE_OBJECT_Format,
        id: sys::SPA_PARAM_EnumFormat,
        properties: vec![
            Property::new(sys::SPA_FORMAT_mediaType, Value::Id(Id(sys::SPA_MEDIA_TYPE_audio))),
            Property::new(sys::SPA_FORMAT_mediaSubtype, Value::Id(Id(sys::SPA_MEDIA_SUBTYPE_dsd))),
            Property::new(sys::SPA_FORMAT_AUDIO_rate, Value::Int(info.spa_rate() as i32)),
            Property::new(sys::SPA_FORMAT_AUDIO_channels, Value::Int(info.channels as i32)),
            Property::new(
                sys::SPA_FORMAT_AUDIO_position,
                Value::ValueArray(ValueArray::Id(dsd_positions(info.channels))),
            ),
        ],
    };
    PodSerializer::serialize(std::io::Cursor::new(Vec::new()), &Value::Object(obj)).unwrap().0.into_inner()
}

fn build_pcm_format(info: &PcmInfo) -> Vec<u8> {
    let mut ai = AudioInfoRaw::new();
    ai.set_format(AudioFormat::F32LE);
    ai.set_rate(info.rate);
    ai.set_channels(info.channels);
    let mut pos = [0u32; MAX_CHANNELS];
    if info.channels >= 2 {
        pos[0] = sys::SPA_AUDIO_CHANNEL_FL;
        pos[1] = sys::SPA_AUDIO_CHANNEL_FR;
    } else {
        pos[0] = sys::SPA_AUDIO_CHANNEL_MONO;
    }
    ai.set_position(pos);
    let obj = Object {
        type_: sys::SPA_TYPE_OBJECT_Format,
        id: sys::SPA_PARAM_EnumFormat,
        properties: ai.into(),
    };
    PodSerializer::serialize(std::io::Cursor::new(Vec::new()), &Value::Object(obj)).unwrap().0.into_inner()
}

fn sink_loop(
    cmd_rx: pw::channel::Receiver<SinkCmd>,
    feed_tx: mpsc::Sender<FeedCmd>,
    shared: Shared,
    events: crossbeam_channel::Sender<SinkEvent>,
) -> Result<(), String> {
    pw::init();
    let mainloop = pw::main_loop::MainLoopRc::new(None).map_err(|e| e.to_string())?;
    let context = pw::context::ContextRc::new(&mainloop, None).map_err(|e| e.to_string())?;
    let core = context.connect_rc(None).map_err(|e| e.to_string())?;

    let stream = pw::stream::StreamRc::new(
        core,
        "piwiplay",
        properties! {
            *pw::keys::MEDIA_TYPE => "Audio",
            *pw::keys::MEDIA_ROLE => "Music",
            *pw::keys::MEDIA_CATEGORY => "Playback",
            *pw::keys::MEDIA_CLASS => "Stream/Output/Audio",
        },
    )
    .map_err(|e| e.to_string())?;

    let _listener = {
        let s = shared.clone();
        let s_pc = shared.clone();
        let events_pc = events.clone();
        let events_sc = events.clone();
        stream
            .add_local_listener_with_user_data(())
            .process(move |stream, ()| {
                let Some(mut buffer) = stream.dequeue_buffer() else { return };
                let datas = buffer.datas_mut();
                let Some(data) = datas.first_mut() else { return };
                let stride = s.stride.load(Ordering::Acquire).max(1);
                let mut written = 0usize;
                if let Some(slice) = data.data() {
                    let cap = (slice.len() / stride) * stride;
                    if !s.paused.load(Ordering::Acquire) && cap > 0 {
                        written = s.ring.read_into(&mut slice[..cap]);
                    }
                }
                let chunk = data.chunk_mut();
                *chunk.offset_mut() = 0;
                *chunk.stride_mut() = stride as i32;
                *chunk.size_mut() = written as u32;
            })
            .param_changed(move |_, _, id, param| {
                if id != sys::SPA_PARAM_Format || s_pc.mode.load(Ordering::Acquire) != MODE_DSD {
                    return;
                }
                let Some(param) = param else { return };
                let fallback = s_pc.channels.load(Ordering::Relaxed) as usize;
                if let Some(l) = parse_negotiated(param, fallback) {
                    *s_pc.layout.lock().unwrap() = Some(l);
                    s_pc.stride.store(l.stride(), Ordering::Release);
                    let _ = events_pc.send(SinkEvent::Negotiated { mode: OutputMode::Native });
                }
            })
            .state_changed(move |_, _, _old, new| {
                use pw::stream::StreamState;
                if let StreamState::Error(msg) = new {
                    let _ = events_sc.send(SinkEvent::Error(msg));
                }
            })
            .register()
            .map_err(|e| e.to_string())?
    };

    let connected = Rc::new(RefCell::new(false));
    let ml_for_cmd = mainloop.clone();

    let _attached = cmd_rx.attach(mainloop.loop_(), {
        let stream = stream.clone();
        let shared = shared.clone();
        let events = events.clone();
        let feed_tx = feed_tx.clone();
        move |cmd| {
            let connect = |values: Vec<u8>| -> Result<(), pw::Error> {
                if *connected.borrow() {
                    let _ = stream.disconnect();
                }
                let mut params = [Pod::from_bytes(&values).unwrap()];
                stream.connect(
                    spa::utils::Direction::Output,
                    None,
                    pw::stream::StreamFlags::AUTOCONNECT
                        | pw::stream::StreamFlags::MAP_BUFFERS
                        | pw::stream::StreamFlags::RT_PROCESS,
                    &mut params,
                )?;
                *connected.borrow_mut() = true;
                Ok(())
            };
            match cmd {
                SinkCmd::PlayDsd { decoder, info } => {
                    shared.ring.reset();
                    *shared.layout.lock().unwrap() = None;
                    shared.stride.store(0, Ordering::Release);
                    shared.channels.store(info.channels, Ordering::Relaxed);
                    shared.mode.store(MODE_DSD, Ordering::Release);
                    let src_lsb = matches!(info.bit_order, crate::types::BitOrder::Lsb);
                    let _ = feed_tx.send(FeedCmd::LoadDsd {
                        decoder,
                        src_lsb,
                        channels: info.channels as usize,
                        spa_rate: info.spa_rate(),
                        base_bytes: 0,
                    });
                    if let Err(e) = connect(build_dsd_format(&info)) {
                        let _ = events.send(SinkEvent::Error(format!("connect failed: {e}")));
                        return;
                    }
                    shared.paused.store(false, Ordering::Release);
                    let _ = stream.set_active(true);
                    let _ = events.send(SinkEvent::Transport(Transport::Playing));
                }
                SinkCmd::PlayPcm { source, info, start_secs } => {
                    shared.ring.reset();
                    *shared.layout.lock().unwrap() = None;
                    shared.channels.store(info.channels, Ordering::Relaxed);
                    shared.stride.store(info.stride(), Ordering::Release);
                    shared.mode.store(MODE_PCM, Ordering::Release);
                    let _ = feed_tx.send(FeedCmd::LoadPcm { source, base_secs: start_secs });
                    if let Err(e) = connect(build_pcm_format(&info)) {
                        let _ = events.send(SinkEvent::Error(format!("connect failed: {e}")));
                        return;
                    }
                    shared.paused.store(false, Ordering::Release);
                    let _ = stream.set_active(true);
                    let _ = events.send(SinkEvent::Negotiated { mode: OutputMode::Transcoded });
                    let _ = events.send(SinkEvent::Transport(Transport::Playing));
                }
                SinkCmd::Pause => {
                    shared.paused.store(true, Ordering::Release);
                    let _ = events.send(SinkEvent::Transport(Transport::Paused));
                }
                SinkCmd::Resume => {
                    shared.paused.store(false, Ordering::Release);
                    let _ = events.send(SinkEvent::Transport(Transport::Playing));
                }
                SinkCmd::Stop => {
                    shared.paused.store(true, Ordering::Release);
                    let _ = feed_tx.send(FeedCmd::Stop);
                    shared.ring.reset();
                    shared.mode.store(MODE_NONE, Ordering::Release);
                    let _ = events.send(SinkEvent::Transport(Transport::Stopped));
                }
                SinkCmd::SeekBytes(b) => {
                    shared.ring.reset();
                    let _ = feed_tx.send(FeedCmd::Seek(b));
                }
                SinkCmd::SetVolume(v) => {
                    *shared.volume.lock().unwrap() = v.clamp(0.0, 1.0);
                }
                SinkCmd::Quit => {
                    let _ = feed_tx.send(FeedCmd::Quit);
                    ml_for_cmd.quit();
                }
            }
        }
    });

    mainloop.run();
    Ok(())
}

fn feeder_loop(rx: mpsc::Receiver<FeedCmd>, shared: Shared, events: crossbeam_channel::Sender<SinkEvent>) {
    // Active source state.
    enum Active {
        Dsd { decoder: Box<dyn Decoder>, src_lsb: bool, channels: usize, spa_rate: u32, base_bytes: u64, pending: Vec<Vec<u8>>, eof: bool },
        Pcm { source: Box<PcmSource>, base_secs: f64, carry: Vec<u8>, eof: bool },
    }
    let mut active: Option<Active> = None;
    let mut ended_sent = false;
    let mut last_pos = Instant::now();

    loop {
        loop {
            let cmd = if active.is_none() {
                match rx.recv() {
                    Ok(c) => Some(c),
                    Err(_) => return,
                }
            } else {
                rx.try_recv().ok()
            };
            match cmd {
                Some(FeedCmd::LoadDsd { decoder, src_lsb, channels, spa_rate, base_bytes }) => {
                    active = Some(Active::Dsd {
                        decoder, src_lsb, channels: channels.max(1), spa_rate,
                        base_bytes, pending: vec![Vec::new(); channels.max(1)], eof: false,
                    });
                    ended_sent = false;
                }
                Some(FeedCmd::LoadPcm { source, base_secs }) => {
                    active = Some(Active::Pcm { source, base_secs, carry: Vec::new(), eof: false });
                    ended_sent = false;
                }
                Some(FeedCmd::Seek(b)) => {
                    if let Some(Active::Dsd { decoder, channels, base_bytes, pending, eof, .. }) = active.as_mut() {
                        let landed = decoder.seek_bytes(b).unwrap_or(b);
                        *base_bytes = landed;
                        *pending = vec![Vec::new(); *channels];
                        *eof = false;
                        ended_sent = false;
                    }
                }
                Some(FeedCmd::Stop) => active = None,
                Some(FeedCmd::Quit) => return,
                None => break,
            }
        }

        let Some(act) = active.as_mut() else { continue };
        let elapsed_secs = match act {
            Active::Dsd { decoder, src_lsb, channels, spa_rate, base_bytes, pending, eof } => {
                feed_dsd(&shared, decoder, *src_lsb, *channels, pending, eof);
                if *spa_rate > 0 {
                    *base_bytes as f64 / *spa_rate as f64
                        + (shared.ring.consumed() / *channels as u64) as f64 / *spa_rate as f64
                } else {
                    0.0
                }
            }
            Active::Pcm { source, base_secs, carry, eof } => {
                feed_pcm(&shared, source, carry, eof);
                let stride = shared.stride.load(Ordering::Relaxed).max(1) as u64;
                let ch = shared.channels.load(Ordering::Relaxed).max(1) as u64;
                let rate = source.info.rate.max(1) as f64;
                let frames = shared.ring.consumed() / stride.max(ch * 4);
                *base_secs + frames as f64 / rate
            }
        };

        if last_pos.elapsed() >= Duration::from_millis(50) {
            let _ = events.send(SinkEvent::PositionSecs(elapsed_secs));
            last_pos = Instant::now();
        }
        if shared.ring.is_drained() && !ended_sent {
            ended_sent = true;
            let _ = events.send(SinkEvent::TrackEnded);
            active = None;
        }
        thread::sleep(Duration::from_millis(2));
    }
}

// (DSD/PCM feed helpers below.)

/// Feed the DSD path: decode planar bytes, repack to the negotiated layout in
/// group-aligned chunks, push to the ring.
fn feed_dsd(
    shared: &Shared,
    decoder: &mut Box<dyn Decoder>,
    src_lsb: bool,
    channels: usize,
    pending: &mut Vec<Vec<u8>>,
    eof: &mut bool,
) {
    let Some(layout) = *shared.layout.lock().unwrap() else {
        thread::sleep(Duration::from_millis(3));
        return;
    };
    let grp = layout.interleave.unsigned_abs().max(1) as usize;

    if !*eof && pending.first().map(|p| p.len()).unwrap_or(0) < READ_CHUNK {
        let mut planes = Vec::new();
        match decoder.read_planar(READ_CHUNK, &mut planes) {
            Ok(0) => *eof = true,
            Ok(_) => {
                if pending.len() != planes.len() {
                    *pending = vec![Vec::new(); planes.len()];
                }
                for (p, incoming) in pending.iter_mut().zip(planes) {
                    p.extend_from_slice(&incoming);
                }
            }
            Err(_) => *eof = true,
        }
    }

    let avail = pending.first().map(|p| p.len()).unwrap_or(0);
    let free_per_chan = shared.ring.free_space() / channels.max(1);
    let aligned = (avail.min(free_per_chan) / grp) * grp;
    if aligned > 0 {
        let slice: Vec<Vec<u8>> = pending.iter().map(|p| p[..aligned].to_vec()).collect();
        shared.ring.push(&repack_planar(&slice, layout, src_lsb));
        for p in pending.iter_mut() {
            p.drain(..aligned);
        }
    } else if *eof && avail > 0 && shared.ring.free_space() >= channels * grp {
        let mut slice: Vec<Vec<u8>> = pending.iter().map(|p| p.clone()).collect();
        for p in slice.iter_mut() {
            while p.len() < grp {
                p.push(DSD_IDLE);
            }
        }
        shared.ring.push(&repack_planar(&slice, layout, src_lsb));
        pending.iter_mut().for_each(|p| p.clear());
    } else if *eof && avail == 0 {
        shared.ring.set_eof(true);
    }
}

/// Feed the PCM path: read f32le from ffmpeg, apply volume, push frame-aligned
/// bytes to the ring.
fn feed_pcm(shared: &Shared, source: &mut Box<PcmSource>, carry: &mut Vec<u8>, eof: &mut bool) {
    let stride = source.info.stride().max(1);
    if *eof {
        if carry.is_empty() {
            shared.ring.set_eof(true);
        }
        return;
    }
    let free = shared.ring.free_space();
    if free < stride {
        thread::sleep(Duration::from_millis(3));
        return;
    }
    let want = free.min(READ_CHUNK);
    let mut buf = vec![0u8; want];
    match source.read(&mut buf) {
        Ok(0) => {
            *eof = true;
            if !carry.is_empty() {
                let gain = *shared.volume.lock().unwrap();
                apply_gain_f32le(carry, gain);
                let n = carry.len() / stride * stride;
                shared.ring.push(&carry[..n]);
                carry.clear();
            }
        }
        Ok(n) => {
            carry.extend_from_slice(&buf[..n]);
            let aligned = carry.len() / stride * stride;
            if aligned > 0 {
                let mut out = carry[..aligned].to_vec();
                let gain = *shared.volume.lock().unwrap();
                apply_gain_f32le(&mut out, gain);
                shared.ring.push(&out);
                carry.drain(..aligned);
            }
        }
        Err(_) => *eof = true,
    }
}
