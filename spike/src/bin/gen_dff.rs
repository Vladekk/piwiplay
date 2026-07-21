//! Generate a valid uncompressed stereo DSD64 `.dff` (DSDIFF) file with a sine
//! tone (1st-order sigma-delta), to verify native DFF playback.
//!
//! DFF stores 1-bit samples MSB-first, byte-interleaved across channels.
//!
//! Usage: gen-dff <out.dff> [freq_hz] [seconds]

use std::env;
use std::fs::File;
use std::io::{BufWriter, Write};

const DSD64_RATE: u32 = 2_822_400;
const CHANNELS: usize = 2;

fn chunk(id: &[u8; 4], body: &[u8]) -> Vec<u8> {
    let mut v = Vec::with_capacity(12 + body.len() + 1);
    v.extend_from_slice(id);
    v.extend_from_slice(&(body.len() as u64).to_be_bytes());
    v.extend_from_slice(body);
    if body.len() % 2 == 1 {
        v.push(0); // IFF even-byte padding
    }
    v
}

fn main() {
    let args: Vec<String> = env::args().collect();
    let out = args.get(1).cloned().unwrap_or_else(|| "test.dff".into());
    let freq: f64 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(500.0);
    let seconds: f64 = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(3.0);

    let bytes_per_chan = (DSD64_RATE as f64 * seconds / 8.0) as usize;
    let plane = modulate_sine(freq, 0.5, bytes_per_chan * 8);

    // Interleave 1 byte per channel: [ch0][ch1][ch0][ch1]…
    let mut snd = Vec::with_capacity(bytes_per_chan * CHANNELS);
    for i in 0..bytes_per_chan {
        for _c in 0..CHANNELS {
            snd.push(plane[i]); // same tone both channels
        }
    }

    // PROP/SND: FS, CHNL, CMPR(uncompressed)
    let mut prop = Vec::new();
    prop.extend_from_slice(b"SND ");
    prop.extend_from_slice(&chunk(b"FS  ", &DSD64_RATE.to_be_bytes()));
    let mut chnl = Vec::new();
    chnl.extend_from_slice(&(CHANNELS as u16).to_be_bytes());
    chnl.extend_from_slice(b"SLFT");
    chnl.extend_from_slice(b"SRGT");
    prop.extend_from_slice(&chunk(b"CHNL", &chnl));
    prop.extend_from_slice(&chunk(b"CMPR", b"DSD \0\x0Cnot compressed"));

    let mut form_body = Vec::new();
    form_body.extend_from_slice(b"DSD "); // FRM8 form type
    form_body.extend_from_slice(&chunk(b"FVER", &[1, 5, 0, 0]));
    form_body.extend_from_slice(&chunk(b"PROP", &prop));
    form_body.extend_from_slice(&chunk(b"DSD ", &snd));

    let f = File::create(&out).expect("create dff");
    let mut w = BufWriter::new(f);
    w.write_all(b"FRM8").unwrap();
    w.write_all(&(form_body.len() as u64).to_be_bytes()).unwrap();
    w.write_all(&form_body).unwrap();
    w.flush().unwrap();

    println!(
        "wrote {out}: DSD64 stereo DFF, {freq} Hz, {:.2}s, {} bytes sound data",
        seconds,
        snd.len()
    );
}

/// 1st-order sigma-delta modulator, packed MSB-first (bit 7 = earliest sample),
/// matching DFF bit order.
fn modulate_sine(freq: f64, amp: f64, samples: usize) -> Vec<u8> {
    let mut out = vec![0u8; samples / 8];
    let mut integ = 0.0f64;
    let step = std::f64::consts::TAU * freq / DSD64_RATE as f64;
    for n in 0..samples {
        let x = amp * (step * n as f64).sin();
        let q = if integ >= 0.0 { 1.0 } else { -1.0 };
        integ += x - q;
        if q > 0.0 {
            out[n / 8] |= 1 << (7 - (n % 8)); // MSB-first
        }
    }
    out
}
