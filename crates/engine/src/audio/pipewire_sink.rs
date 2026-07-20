//! Native-DSD PipeWire sink.
//!
//! Two threads:
//! * **sink thread** — owns the PipeWire main loop + stream. The RT `process`
//!   callback only drains the [`Ring`] into the output buffer (a `memcpy`); it
//!   never touches disk or allocates on the hot path. Command handling and
//!   format negotiation also run here (loop thread).
//! * **feeder thread** — owns the [`Decoder`], reads planar DSD, repacks it into
//!   the negotiated layout in group-aligned chunks, and pushes to the ring. It
//!   also emits throttled position updates and end-of-track detection.
//!
//! Control flows in via [`pipewire::channel`]; events flow out via a
//! crossbeam channel. See `spike/RESULTS.md` for the negotiation details.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use pipewire as pw;
use pw::{properties::properties, spa};
use spa::pod::serialize::PodSerializer;
use spa::pod::{Object, Pod, Property, Value, ValueArray};
use spa::sys;
use spa::utils::Id;

use super::{repack_planar, Layout};
use crate::audio::ring::Ring;
use crate::decode::Decoder;
use crate::types::{DsdInfo, OutputMode, Transport};

/// Ring capacity: enough for ~0.5s of the highest supported rate (DSD512
/// stereo ≈ 2.8 MB/s). Fixed so the RT callback keeps a stable Arc.
const RING_CAPACITY: usize = 4 * 1024 * 1024;

/// Per-channel bytes to read from the decoder per feeder iteration.
const READ_CHUNK: usize = 64 * 1024;

/// Idle DSD byte used to pad the final partial group (keeps buffers stride-aligned).
const DSD_IDLE: u8 = 0x69;

/// Commands accepted by the sink (delivered into the loop thread).
pub enum SinkCmd {
    /// Load a decoder and begin playing it.
    Play { decoder: Box<dyn Decoder>, info: DsdInfo },
    Pause,
    Resume,
    Stop,
    SeekBytes(u64),
    Quit,
}

/// Events emitted by the sink.
#[derive(Debug, Clone)]
pub enum SinkEvent {
    Negotiated { layout: Layout, mode: OutputMode },
    /// Per-channel bytes played (maps to time via `DsdInfo::spa_rate`).
    PositionBytes(u64),
    TrackEnded,
    Transport(Transport),
    Error(String),
}

/// Commands from the sink thread to the feeder thread.
enum FeedCmd {
    Load { decoder: Box<dyn Decoder>, src_lsb: bool, channels: usize, base_per_chan: u64 },
    Seek(u64),
    Stop,
    Quit,
}

/// Handle owning both threads; drop stops them.
pub struct Sink {
    tx: pw::channel::Sender<SinkCmd>,
    sink_thread: Option<JoinHandle<()>>,
    feeder_thread: Option<JoinHandle<()>>,
}

impl Sink {
    pub fn spawn(events: crossbeam_channel::Sender<SinkEvent>) -> Self {
        let ring = Ring::new(RING_CAPACITY);
        let layout: Arc<Mutex<Option<Layout>>> = Arc::new(Mutex::new(None));
        let paused = Arc::new(AtomicBool::new(true));
        let channels = Arc::new(AtomicU32::new(2));

        let (feed_tx, feed_rx) = mpsc::channel::<FeedCmd>();
        let (cmd_tx, cmd_rx) = pw::channel::channel::<SinkCmd>();

        let feeder_thread = {
            let ring = ring.clone();
            let layout = layout.clone();
            let events = events.clone();
            thread::Builder::new()
                .name("piwiplay-feeder".into())
                .spawn(move || feeder_loop(feed_rx, ring, layout, events))
                .expect("spawn feeder")
        };

        let sink_thread = thread::Builder::new()
            .name("piwiplay-sink".into())
            .spawn(move || {
                if let Err(e) = sink_loop(cmd_rx, feed_tx, ring, layout, paused, channels, events.clone()) {
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

/// Read interleave + bitorder chosen by the sink from a negotiated Format POD.
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

/// Build the DSD EnumFormat POD (no interleave/bitorder — the sink picks them).
fn build_format(info: &DsdInfo) -> Vec<u8> {
    let positions: Vec<Id> = match info.channels {
        1 => vec![Id(sys::SPA_AUDIO_CHANNEL_MONO)],
        _ => (0..info.channels)
            .map(|i| match i {
                0 => Id(sys::SPA_AUDIO_CHANNEL_FL),
                1 => Id(sys::SPA_AUDIO_CHANNEL_FR),
                n => Id(sys::SPA_AUDIO_CHANNEL_AUX0 + n),
            })
            .collect(),
    };
    let obj = Object {
        type_: sys::SPA_TYPE_OBJECT_Format,
        id: sys::SPA_PARAM_EnumFormat,
        properties: vec![
            Property::new(sys::SPA_FORMAT_mediaType, Value::Id(Id(sys::SPA_MEDIA_TYPE_audio))),
            Property::new(sys::SPA_FORMAT_mediaSubtype, Value::Id(Id(sys::SPA_MEDIA_SUBTYPE_dsd))),
            Property::new(sys::SPA_FORMAT_AUDIO_rate, Value::Int(info.spa_rate() as i32)),
            Property::new(sys::SPA_FORMAT_AUDIO_channels, Value::Int(info.channels as i32)),
            Property::new(sys::SPA_FORMAT_AUDIO_position, Value::ValueArray(ValueArray::Id(positions))),
        ],
    };
    PodSerializer::serialize(std::io::Cursor::new(Vec::new()), &Value::Object(obj))
        .expect("serialize dsd format")
        .0
        .into_inner()
}

#[allow(clippy::too_many_arguments)]
fn sink_loop(
    cmd_rx: pw::channel::Receiver<SinkCmd>,
    feed_tx: mpsc::Sender<FeedCmd>,
    ring: Ring,
    layout: Arc<Mutex<Option<Layout>>>,
    paused: Arc<AtomicBool>,
    channels: Arc<AtomicU32>,
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

    // Listener: process (RT drain), param_changed (negotiation), state_changed.
    let _listener = {
        let ring = ring.clone();
        let layout_p = layout.clone();
        let paused_p = paused.clone();
        let layout_pc = layout.clone();
        let channels_pc = channels.clone();
        let events_pc = events.clone();
        let events_sc = events.clone();
        stream
            .add_local_listener_with_user_data(())
            .process(move |stream, ()| {
                let Some(mut buffer) = stream.dequeue_buffer() else { return };
                let datas = buffer.datas_mut();
                let Some(data) = datas.first_mut() else { return };
                let stride = layout_p.lock().unwrap().map(|l| l.stride()).unwrap_or(1).max(1);
                let mut written = 0usize;
                if let Some(slice) = data.data() {
                    let cap = (slice.len() / stride) * stride;
                    if !paused_p.load(Ordering::Acquire) && cap > 0 {
                        written = ring.read_into(&mut slice[..cap]);
                    }
                }
                let chunk = data.chunk_mut();
                *chunk.offset_mut() = 0;
                *chunk.stride_mut() = stride as i32;
                *chunk.size_mut() = written as u32;
            })
            .param_changed(move |_, _, id, param| {
                if id != sys::SPA_PARAM_Format {
                    return;
                }
                let Some(param) = param else { return };
                let fallback = channels_pc.load(Ordering::Relaxed) as usize;
                if let Some(l) = parse_negotiated(param, fallback) {
                    *layout_pc.lock().unwrap() = Some(l);
                    // v1: we only ever offer DSD, so a negotiated DSD format is native.
                    let _ = events_pc.send(SinkEvent::Negotiated { layout: l, mode: OutputMode::Native });
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

    // Shared control state for the command handler.
    let connected = Rc::new(RefCell::new(false));

    let ml_for_cmd = mainloop.clone();
    let _attached = cmd_rx.attach(mainloop.loop_(), {
        let stream = stream.clone();
        let ring = ring.clone();
        let layout = layout.clone();
        let paused = paused.clone();
        let channels = channels.clone();
        let events = events.clone();
        let feed_tx = feed_tx.clone();
        move |cmd| match cmd {
            SinkCmd::Play { decoder, info } => {
                ring.reset();
                *layout.lock().unwrap() = None;
                channels.store(info.channels, Ordering::Relaxed);
                let src_lsb = matches!(info.bit_order, crate::types::BitOrder::Lsb);
                let _ = feed_tx.send(FeedCmd::Load {
                    decoder,
                    src_lsb,
                    channels: info.channels as usize,
                    base_per_chan: 0,
                });

                if *connected.borrow() {
                    let _ = stream.disconnect();
                }
                let values = build_format(&info);
                let mut params = [Pod::from_bytes(&values).unwrap()];
                if let Err(e) = stream.connect(
                    spa::utils::Direction::Output,
                    None,
                    pw::stream::StreamFlags::AUTOCONNECT
                        | pw::stream::StreamFlags::MAP_BUFFERS
                        | pw::stream::StreamFlags::RT_PROCESS,
                    &mut params,
                ) {
                    let _ = events.send(SinkEvent::Error(format!("connect failed: {e}")));
                    return;
                }
                *connected.borrow_mut() = true;
                paused.store(false, Ordering::Release);
                let _ = stream.set_active(true);
                let _ = events.send(SinkEvent::Transport(Transport::Playing));
            }
            SinkCmd::Pause => {
                paused.store(true, Ordering::Release);
                let _ = events.send(SinkEvent::Transport(Transport::Paused));
            }
            SinkCmd::Resume => {
                paused.store(false, Ordering::Release);
                let _ = events.send(SinkEvent::Transport(Transport::Playing));
            }
            SinkCmd::Stop => {
                paused.store(true, Ordering::Release);
                let _ = feed_tx.send(FeedCmd::Stop);
                ring.reset();
                let _ = events.send(SinkEvent::Transport(Transport::Stopped));
            }
            SinkCmd::SeekBytes(b) => {
                ring.reset();
                let _ = feed_tx.send(FeedCmd::Seek(b));
            }
            SinkCmd::Quit => {
                let _ = feed_tx.send(FeedCmd::Quit);
                ml_for_cmd.quit();
            }
        }
    });

    mainloop.run();
    Ok(())
}

fn feeder_loop(
    rx: mpsc::Receiver<FeedCmd>,
    ring: Ring,
    layout: Arc<Mutex<Option<Layout>>>,
    events: crossbeam_channel::Sender<SinkEvent>,
) {
    let mut decoder: Option<Box<dyn Decoder>> = None;
    let mut src_lsb = true;
    let mut channels = 2usize;
    let mut base_per_chan = 0u64;
    let mut pending: Vec<Vec<u8>> = Vec::new();
    let mut decoder_eof = false;
    let mut ended_sent = false;
    let mut last_pos = Instant::now();

    loop {
        // Drain control commands.
        loop {
            let cmd = if decoder.is_none() {
                // Nothing to do: block until a command arrives.
                match rx.recv() {
                    Ok(c) => Some(c),
                    Err(_) => return,
                }
            } else {
                rx.try_recv().ok()
            };
            match cmd {
                Some(FeedCmd::Load { decoder: d, src_lsb: s, channels: c, base_per_chan: b }) => {
                    decoder = Some(d);
                    src_lsb = s;
                    channels = c.max(1);
                    base_per_chan = b;
                    pending = vec![Vec::new(); channels];
                    decoder_eof = false;
                    ended_sent = false;
                }
                Some(FeedCmd::Seek(b)) => {
                    if let Some(d) = decoder.as_mut() {
                        let landed = d.seek_bytes(b).unwrap_or(b);
                        base_per_chan = landed;
                        pending = vec![Vec::new(); channels];
                        decoder_eof = false;
                        ended_sent = false;
                    }
                }
                Some(FeedCmd::Stop) => {
                    decoder = None;
                    pending.clear();
                    decoder_eof = false;
                }
                Some(FeedCmd::Quit) => return,
                None => break,
            }
        }

        let Some(dec) = decoder.as_mut() else { continue };
        let Some(layout) = *layout.lock().unwrap() else {
            thread::sleep(Duration::from_millis(3));
            continue;
        };
        let grp = layout.interleave.unsigned_abs().max(1) as usize;

        // Top up the pending planar buffer from the decoder.
        if !decoder_eof && pending.first().map(|p| p.len()).unwrap_or(0) < READ_CHUNK {
            let mut planes = Vec::new();
            match dec.read_planar(READ_CHUNK, &mut planes) {
                Ok(0) => decoder_eof = true,
                Ok(_n) => {
                    if pending.len() != planes.len() {
                        pending = vec![Vec::new(); planes.len()];
                    }
                    for (p, incoming) in pending.iter_mut().zip(planes) {
                        p.extend_from_slice(&incoming);
                    }
                }
                Err(e) => {
                    let _ = events.send(SinkEvent::Error(format!("read error: {e}")));
                    decoder_eof = true;
                }
            }
        }

        // Push group-aligned chunks while there is room in the ring.
        let avail = pending.first().map(|p| p.len()).unwrap_or(0);
        let free_per_chan = ring.free_space() / channels;
        let aligned = (avail.min(free_per_chan) / grp) * grp;
        if aligned > 0 {
            let slice: Vec<Vec<u8>> = pending.iter().map(|p| p[..aligned].to_vec()).collect();
            let packed = repack_planar(&slice, layout, src_lsb);
            ring.push(&packed);
            for p in pending.iter_mut() {
                p.drain(..aligned);
            }
        } else if decoder_eof && avail > 0 && ring.free_space() >= channels * grp {
            // Flush the final partial group, padded to keep stride alignment.
            let mut slice: Vec<Vec<u8>> = pending.iter().map(|p| p.clone()).collect();
            for p in slice.iter_mut() {
                while p.len() < grp {
                    p.push(DSD_IDLE);
                }
            }
            let packed = repack_planar(&slice, layout, src_lsb);
            ring.push(&packed);
            pending.iter_mut().for_each(|p| p.clear());
        } else if decoder_eof && avail == 0 {
            ring.set_eof(true);
        }

        // Throttled position + end-of-track.
        if last_pos.elapsed() >= Duration::from_millis(50) {
            let per_chan_played = base_per_chan + ring.consumed() / channels as u64;
            let _ = events.send(SinkEvent::PositionBytes(per_chan_played));
            last_pos = Instant::now();
        }
        if ring.is_drained() && !ended_sent {
            ended_sent = true;
            let _ = events.send(SinkEvent::TrackEnded);
            decoder = None;
        }

        thread::sleep(Duration::from_millis(2));
    }
}
