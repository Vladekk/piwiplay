//! Waveform envelope extraction for DSD.
//!
//! DSD carries no PCM samples, so an amplitude envelope is *derived*: the moving
//! average of the 1-bit stream tracks the analog signal. We use each byte's
//! popcount as a cheap 8×-decimated sample — `value = (ones*2 - 8) / 8` in
//! `-1.0..=1.0` — which is bit-order independent, then bucket those samples into
//! per-column peak/RMS.
//!
//! Columns hold **absolute** amplitude (not per-track peak-normalized): a
//! half-scale signal renders at half height and transients spike above the body,
//! which keeps the display varied instead of a solid block. The renderer applies
//! any cosmetic shaping.

use std::path::Path;

use crate::decode;
use crate::error::Result;
use crate::types::WaveColumn;

/// Per-byte DSD→amplitude sample in `-1.0..=1.0` (popcount based).
#[inline]
fn byte_sample(b: u8) -> f32 {
    (b.count_ones() as f32 * 2.0 - 8.0) / 8.0
}

/// Compute a `buckets`-column envelope from planar per-channel DSD bytes.
/// Channels are mono-mixed. Returned columns are peak-normalized.
pub fn envelope(planar: &[Vec<u8>], buckets: usize) -> Vec<WaveColumn> {
    let buckets = buckets.max(1);
    let per = planar.first().map(|p| p.len()).unwrap_or(0);
    if per == 0 {
        return vec![WaveColumn::default(); buckets];
    }
    let channels = planar.len().max(1);
    let mut cols = vec![WaveColumn::default(); buckets];
    let mut sumsq = vec![0f64; buckets];
    let mut counts = vec![0u64; buckets];

    for i in 0..per {
        // mono mix of this byte position across channels
        let mut mix = 0f32;
        for plane in planar {
            mix += byte_sample(plane[i]);
        }
        mix /= channels as f32;

        let bucket = (i * buckets) / per;
        let c = &mut cols[bucket];
        let a = mix.abs();
        if a > c.peak {
            c.peak = a;
        }
        sumsq[bucket] += (mix as f64) * (mix as f64);
        counts[bucket] += 1;
    }

    for b in 0..buckets {
        if counts[b] > 0 {
            cols[b].rms = (sumsq[b] / counts[b] as f64).sqrt() as f32;
        }
    }
    cols
}

/// Stream a file from disk and compute its envelope. Intended to run on a
/// background worker thread (it can read the whole file).
pub fn compute(path: &Path, buckets: usize) -> Result<Vec<WaveColumn>> {
    let mut dec = decode::open(path)?;
    let total = dec.total_bytes().max(1);
    let buckets = buckets.max(1);

    let mut sum_peak = vec![0f32; buckets];
    let mut sumsq = vec![0f64; buckets];
    let mut counts = vec![0u64; buckets];

    let mut base = 0u64;
    let mut planes = Vec::new();
    loop {
        let n = dec
            .read_planar(64 * 1024, &mut planes)
            .map_err(|e| crate::error::EngineError::io(path, e))?;
        if n == 0 {
            break;
        }
        let channels = planes.len().max(1);
        for i in 0..n {
            let mut mix = 0f32;
            for plane in &planes {
                mix += byte_sample(plane[i]);
            }
            mix /= channels as f32;
            let global = base + i as u64;
            let bucket = ((global * buckets as u64) / total) as usize;
            let bucket = bucket.min(buckets - 1);
            let a = mix.abs();
            if a > sum_peak[bucket] {
                sum_peak[bucket] = a;
            }
            sumsq[bucket] += (mix as f64) * (mix as f64);
            counts[bucket] += 1;
        }
        base += n as u64;
    }

    let cols: Vec<WaveColumn> = (0..buckets)
        .map(|b| WaveColumn {
            peak: sum_peak[b],
            rms: if counts[b] > 0 { (sumsq[b] / counts[b] as f64).sqrt() as f32 } else { 0.0 },
        })
        .collect();
    Ok(cols)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn byte_sample_extremes() {
        assert_eq!(byte_sample(0x00), -1.0); // all zeros
        assert_eq!(byte_sample(0xFF), 1.0); // all ones
        assert_eq!(byte_sample(0x0F), 0.0); // balanced
    }

    #[test]
    fn envelope_bucket_count_and_range() {
        let planar = vec![(0..=255u8).cycle().take(1024).collect::<Vec<u8>>()];
        let cols = envelope(&planar, 32);
        assert_eq!(cols.len(), 32);
        for c in &cols {
            assert!((0.0..=1.0).contains(&c.peak));
            assert!((0.0..=1.0).contains(&c.rms));
            assert!(c.rms <= c.peak + 1e-6);
        }
    }

    #[test]
    fn absolute_amplitude_not_renormalized() {
        // Half-scale-ish first half vs full-scale second half. Absolute amplitude
        // means the quiet part stays low and is NOT stretched to fill height.
        let mut bytes = vec![0x0Fu8; 512]; // balanced -> ~0.0
        bytes.extend(std::iter::repeat(0xFF).take(512)); // all ones -> 1.0
        let cols = envelope(&vec![bytes], 2);
        assert!(cols[0].peak < 0.01, "quiet bucket stays ~0 (not normalized up)");
        assert!((cols[1].peak - 1.0).abs() < 1e-6, "full-scale bucket ~1");
    }

    #[test]
    fn half_scale_stays_half_height() {
        // A steady half-density pattern must render around half, not full.
        // 0b11001100 has 4 ones -> byte_sample 0.0; use 0b11101110 (6 ones -> 0.5).
        let cols = envelope(&vec![vec![0b1110_1110u8; 256]], 4);
        for c in &cols {
            assert!((c.peak - 0.5).abs() < 1e-6, "steady 0.5 amplitude, got {}", c.peak);
        }
    }

    #[test]
    fn empty_yields_zeroed_columns() {
        let cols = envelope(&[vec![]], 8);
        assert_eq!(cols.len(), 8);
        assert!(cols.iter().all(|c| c.peak == 0.0 && c.rms == 0.0));
    }
}
