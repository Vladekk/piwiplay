//! DSD spike: parse a `.dsf`, offer a DSD format to PipeWire the way `pw-cat`
//! does (rate + channels + positions only, letting the sink choose interleave
//! and bitorder), then repack the planar DSF data to the negotiated layout and
//! play it bit-perfectly to a native-DSD sink.
//!
//! Verifies SPEC.md Milestone 0: pipewire-rs can express and negotiate DSD.
//!
//! Usage: play-dsd <file.dsf>

use std::cell::RefCell;
use std::rc::Rc;

use pipewire as pw;
use pw::{properties::properties, spa};
use spa::pod::serialize::PodSerializer;
use spa::pod::{Object, Pod, Property, Value, ValueArray};
use spa::sys;
use spa::utils::Id;

/// Planar per-channel DSD from a DSF file (LSB-first, as stored in DSF).
struct Planes {
    channels: u32,
    dsd_rate: u32, // 1-bit samples/sec (e.g. 2_822_400 for DSD64)
    planes: Vec<Vec<u8>>,
}

/// Playback buffer, repacked to the negotiated on-wire layout.
#[derive(Default)]
struct Play {
    payload: Vec<u8>,
    pos: usize,
    stride: usize,
    ready: bool,
}

fn le_u32(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(b[o..o + 4].try_into().unwrap())
}
fn le_u64(b: &[u8], o: usize) -> u64 {
    u64::from_le_bytes(b[o..o + 8].try_into().unwrap())
}

fn parse_dsf(bytes: &[u8]) -> Planes {
    assert!(&bytes[0..4] == b"DSD ", "not a DSF file");
    let fmt = 28;
    assert!(&bytes[fmt..fmt + 4] == b"fmt ", "missing fmt chunk");
    let channels = le_u32(bytes, fmt + 24);
    let dsd_rate = le_u32(bytes, fmt + 28);
    let block = le_u32(bytes, fmt + 44) as usize;
    let data_hdr = fmt + 52;
    assert!(&bytes[data_hdr..data_hdr + 4] == b"data", "missing data chunk");
    let data_size = le_u64(bytes, data_hdr + 4) as usize - 12;
    let data = &bytes[data_hdr + 12..data_hdr + 12 + data_size];

    let ch = channels as usize;
    let blocks = data.len() / (block * ch);
    let mut planes: Vec<Vec<u8>> = vec![Vec::with_capacity(blocks * block); ch];
    for b in 0..blocks {
        for (c, plane) in planes.iter_mut().enumerate() {
            let s = (b * ch + c) * block;
            plane.extend_from_slice(&data[s..s + block]);
        }
    }
    Planes { channels, dsd_rate, planes }
}

/// Repack planar (DSF, LSB-first) DSD into the sink's negotiated layout.
/// `interleave` bytes per channel per group; negative means the bytes within a
/// group are reversed. `dst_lsb` false means the sink wants MSB-first bits.
fn repack(planes: &[Vec<u8>], interleave: i32, dst_lsb: bool) -> (Vec<u8>, usize) {
    let n = interleave.unsigned_abs().max(1) as usize;
    let reverse_group = interleave < 0;
    let ch = planes.len();
    let per = planes[0].len();
    let mut out = Vec::with_capacity(per * ch);
    let mut i = 0;
    while i < per {
        for plane in planes {
            let end = (i + n).min(per);
            let mut group: Vec<u8> = plane[i..end].to_vec();
            if !dst_lsb {
                for b in group.iter_mut() {
                    *b = b.reverse_bits(); // DSF is LSB-first; flip to MSB
                }
            }
            if reverse_group {
                group.reverse();
            }
            out.extend_from_slice(&group);
        }
        i += n;
    }
    (out, n * ch)
}

/// Parse the negotiated Format POD for interleave + bitorder chosen by the sink.
fn parse_negotiated(param: &Pod) -> Option<(i32, bool)> {
    let obj = param.as_object().ok()?;
    let interleave = obj
        .find_prop(Id(sys::SPA_FORMAT_AUDIO_interleave))
        .and_then(|p| p.value().get_int().ok())
        .unwrap_or(1);
    let lsb = obj
        .find_prop(Id(sys::SPA_FORMAT_AUDIO_bitorder))
        .and_then(|p| p.value().get_id().ok())
        .map(|Id(b)| b != sys::SPA_PARAM_BITORDER_msb)
        .unwrap_or(true);
    Some((interleave, lsb))
}

fn main() -> Result<(), pw::Error> {
    let path = std::env::args().nth(1).expect("usage: play-dsd <file.dsf>");
    let bytes = std::fs::read(&path).expect("read dsf");
    let src = parse_dsf(&bytes);
    let spa_rate = (src.dsd_rate / 8) as i32; // SPA DSD rate is in bytes/sec
    let channels = src.channels;
    println!(
        "playing {path}: {channels}ch, DSD{}, spa_rate={spa_rate} B/s, {} bytes/plane",
        src.dsd_rate / 44100,
        src.planes[0].len()
    );

    pw::init();
    let mainloop = pw::main_loop::MainLoopRc::new(None)?;
    let context = pw::context::ContextRc::new(&mainloop, None)?;
    let core = context.connect_rc(None)?;

    let stream = pw::stream::StreamBox::new(
        &core,
        "piwiplay-spike",
        properties! {
            *pw::keys::MEDIA_TYPE => "Audio",
            *pw::keys::MEDIA_ROLE => "Music",
            *pw::keys::MEDIA_CATEGORY => "Playback",
            *pw::keys::MEDIA_CLASS => "Stream/Output/Audio",
        },
    )?;

    let planes = Rc::new(src.planes);
    let play = Rc::new(RefCell::new(Play::default()));
    let ml = mainloop.clone();

    let planes_pc = planes.clone();
    let play_pc = play.clone();
    let play_proc = play.clone();

    let _listener = stream
        .add_local_listener_with_user_data(())
        .state_changed(|_, _, old, new| eprintln!("state: {old:?} -> {new:?}"))
        .param_changed(move |_, _, id, param| {
            if id != sys::SPA_PARAM_Format {
                return;
            }
            let Some(param) = param else { return };
            if let Some((interleave, lsb)) = parse_negotiated(param) {
                let (payload, stride) = repack(&planes_pc, interleave, lsb);
                eprintln!(
                    "negotiated: interleave={interleave} bitorder={} stride={stride} payload={}",
                    if lsb { "lsb" } else { "msb" },
                    payload.len()
                );
                let mut p = play_pc.borrow_mut();
                p.payload = payload;
                p.stride = stride;
                p.pos = 0;
                p.ready = true;
            }
        })
        .process(move |stream, ()| {
            let mut p = play_proc.borrow_mut();
            if !p.ready {
                return;
            }
            let Some(mut buffer) = stream.dequeue_buffer() else { return };
            let datas = buffer.datas_mut();
            let data = &mut datas[0];
            let stride = p.stride.max(1);
            let total = p.payload.len();
            let mut written = 0usize;
            if let Some(slice) = data.data() {
                let cap = (slice.len() / stride) * stride;
                let remaining = total - p.pos;
                let n = remaining.min(cap);
                let pos = p.pos;
                slice[..n].copy_from_slice(&p.payload[pos..pos + n]);
                p.pos += n;
                written = n;
                if p.pos >= total {
                    ml.quit();
                }
            }
            let chunk = data.chunk_mut();
            *chunk.offset_mut() = 0;
            *chunk.stride_mut() = stride as i32;
            *chunk.size_mut() = written as u32;
        })
        .register()?;

    // Build the DSD EnumFormat POD the way pw-cat does: no interleave/bitorder,
    // so the sink chooses them. Include channel positions.
    let positions: Vec<Id> = match channels {
        1 => vec![Id(sys::SPA_AUDIO_CHANNEL_MONO)],
        _ => vec![Id(sys::SPA_AUDIO_CHANNEL_FL), Id(sys::SPA_AUDIO_CHANNEL_FR)],
    };

    let obj = Object {
        type_: sys::SPA_TYPE_OBJECT_Format,
        id: sys::SPA_PARAM_EnumFormat,
        properties: vec![
            Property::new(sys::SPA_FORMAT_mediaType, Value::Id(Id(sys::SPA_MEDIA_TYPE_audio))),
            Property::new(sys::SPA_FORMAT_mediaSubtype, Value::Id(Id(sys::SPA_MEDIA_SUBTYPE_dsd))),
            Property::new(sys::SPA_FORMAT_AUDIO_rate, Value::Int(spa_rate)),
            Property::new(sys::SPA_FORMAT_AUDIO_channels, Value::Int(channels as i32)),
            Property::new(sys::SPA_FORMAT_AUDIO_position, Value::ValueArray(ValueArray::Id(positions))),
        ],
    };

    let values: Vec<u8> = PodSerializer::serialize(std::io::Cursor::new(Vec::new()), &Value::Object(obj))
        .unwrap()
        .0
        .into_inner();
    let mut params = [Pod::from_bytes(&values).unwrap()];

    stream.connect(
        spa::utils::Direction::Output,
        None,
        pw::stream::StreamFlags::AUTOCONNECT
            | pw::stream::StreamFlags::MAP_BUFFERS
            | pw::stream::StreamFlags::RT_PROCESS,
        &mut params,
    )?;

    mainloop.run();
    println!("done");
    Ok(())
}
