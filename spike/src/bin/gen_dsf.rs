//! Generate a valid stereo DSD64 `.dsf` file containing a sine tone, produced by
//! a 1st-order sigma-delta modulator so that a real DAC's reconstruction filter
//! plays an audible tone. Used to verify bit-perfect native DSD playback.
//!
//! Usage: gen-dsf <out.dsf> [freq_hz] [seconds]

use std::env;
use std::fs::File;
use std::io::{BufWriter, Write};

const DSD64_RATE: u32 = 2_822_400; // 1-bit samples per second per channel
const BLOCK: usize = 4096; // DSF block size per channel (bytes)
const CHANNELS: u32 = 2;

fn main() {
    let args: Vec<String> = env::args().collect();
    let out = args.get(1).cloned().unwrap_or_else(|| "test.dsf".into());
    let freq: f64 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(500.0);
    let seconds: f64 = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(3.0);

    // Round the per-channel byte count up to a whole number of blocks.
    let approx_bytes = (DSD64_RATE as f64 * seconds / 8.0) as usize;
    let n_blocks = approx_bytes.div_ceil(BLOCK).max(1);
    let bytes_per_chan = n_blocks * BLOCK;
    let samples_per_chan = bytes_per_chan * 8;

    // Sigma-delta modulate one sine per channel (same tone both channels).
    let plane_l = modulate_sine(freq, 0.5, samples_per_chan);
    let plane_r = plane_l.clone();

    // DSF stores interleaved *blocks*: [L block][R block][L block][R block]...
    let mut data = Vec::with_capacity(bytes_per_chan * 2);
    for b in 0..n_blocks {
        let s = b * BLOCK;
        data.extend_from_slice(&plane_l[s..s + BLOCK]);
        data.extend_from_slice(&plane_r[s..s + BLOCK]);
    }

    let data_chunk_size = 12u64 + data.len() as u64;
    let total_size = 28u64 + 52u64 + data_chunk_size;

    let f = File::create(&out).expect("create dsf");
    let mut w = BufWriter::new(f);

    // ---- DSD chunk (28 bytes) ----
    w.write_all(b"DSD ").unwrap();
    w.write_all(&28u64.to_le_bytes()).unwrap();
    w.write_all(&total_size.to_le_bytes()).unwrap();
    w.write_all(&0u64.to_le_bytes()).unwrap(); // metadata pointer: none

    // ---- fmt chunk (52 bytes) ----
    w.write_all(b"fmt ").unwrap();
    w.write_all(&52u64.to_le_bytes()).unwrap();
    w.write_all(&1u32.to_le_bytes()).unwrap(); // format version
    w.write_all(&0u32.to_le_bytes()).unwrap(); // format id: 0 = DSD raw
    w.write_all(&2u32.to_le_bytes()).unwrap(); // channel type: 2 = stereo
    w.write_all(&CHANNELS.to_le_bytes()).unwrap(); // channel num
    w.write_all(&DSD64_RATE.to_le_bytes()).unwrap(); // sampling frequency
    w.write_all(&1u32.to_le_bytes()).unwrap(); // bits per sample
    w.write_all(&(samples_per_chan as u64).to_le_bytes()).unwrap(); // sample count
    w.write_all(&(BLOCK as u32).to_le_bytes()).unwrap(); // block size per channel
    w.write_all(&0u32.to_le_bytes()).unwrap(); // reserved

    // ---- data chunk ----
    w.write_all(b"data").unwrap();
    w.write_all(&data_chunk_size.to_le_bytes()).unwrap();
    w.write_all(&data).unwrap();
    w.flush().unwrap();

    println!(
        "wrote {out}: DSD64 stereo, {freq} Hz, {:.2}s, {} bytes data ({} blocks/chan)",
        samples_per_chan as f64 / DSD64_RATE as f64,
        data.len(),
        n_blocks
    );
}

/// 1st-order sigma-delta modulator. Returns LSB-first packed bytes
/// (bit 0 of each byte = earliest sample), matching the DSF bit order.
fn modulate_sine(freq: f64, amp: f64, samples: usize) -> Vec<u8> {
    let mut out = vec![0u8; samples / 8];
    let mut integ = 0.0f64;
    let step = std::f64::consts::TAU * freq / DSD64_RATE as f64;
    for n in 0..samples {
        let x = amp * (step * n as f64).sin();
        let q = if integ >= 0.0 { 1.0 } else { -1.0 };
        integ += x - q;
        if q > 0.0 {
            out[n / 8] |= 1 << (n % 8); // LSB-first
        }
    }
    out
}
