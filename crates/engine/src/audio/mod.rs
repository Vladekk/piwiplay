//! Audio output layer: PipeWire native-DSD sink plus the pure data-shaping
//! helpers it relies on (layout repacking and the feed ring buffer).

pub mod pipewire_sink;
pub mod ring;

/// The on-wire DSD layout the sink negotiated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Layout {
    /// Bytes per channel per interleave group; negative reverses bytes in a group.
    pub interleave: i32,
    /// Whether the sink wants LSB-first bits.
    pub dst_lsb: bool,
    pub channels: usize,
}

impl Layout {
    /// Bytes advanced per output frame (one group per channel).
    pub fn stride(&self) -> usize {
        self.interleave.unsigned_abs().max(1) as usize * self.channels
    }
}

/// Repack planar per-channel DSD into the sink's negotiated interleave/bitorder.
///
/// `src_lsb` is the bit order of the source container (DSF = true, DFF = false);
/// when it differs from `layout.dst_lsb`, each byte's bits are reversed.
pub fn repack_planar(planes: &[Vec<u8>], layout: Layout, src_lsb: bool) -> Vec<u8> {
    let n = layout.interleave.unsigned_abs().max(1) as usize;
    let reverse_group = layout.interleave < 0;
    let flip_bits = src_lsb != layout.dst_lsb;
    let per = planes.first().map(|p| p.len()).unwrap_or(0);
    let mut out = Vec::with_capacity(per * planes.len());

    let mut i = 0;
    while i < per {
        let end = (i + n).min(per);
        for plane in planes {
            let mut group: Vec<u8> = plane[i..end.min(plane.len())].to_vec();
            if flip_bits {
                for b in group.iter_mut() {
                    *b = b.reverse_bits();
                }
            }
            if reverse_group {
                group.reverse();
            }
            out.extend_from_slice(&group);
        }
        i += n;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interleave1_same_bitorder() {
        let planes = vec![vec![0xA0, 0xA1], vec![0xB0, 0xB1]];
        let layout = Layout { interleave: 1, dst_lsb: true, channels: 2 };
        // 1-byte groups: L0 R0 L1 R1
        assert_eq!(repack_planar(&planes, layout, true), vec![0xA0, 0xB0, 0xA1, 0xB1]);
    }

    #[test]
    fn interleave4_groups() {
        let planes = vec![(0..8).collect::<Vec<u8>>(), (100..108).collect::<Vec<u8>>()];
        let layout = Layout { interleave: 4, dst_lsb: true, channels: 2 };
        let out = repack_planar(&planes, layout, true);
        // L[0..4] R[0..4] L[4..8] R[4..8]
        assert_eq!(out[0..4], [0, 1, 2, 3]);
        assert_eq!(out[4..8], [100, 101, 102, 103]);
        assert_eq!(out[8..12], [4, 5, 6, 7]);
        assert_eq!(out[12..16], [104, 105, 106, 107]);
    }

    #[test]
    fn bit_flip_when_orders_differ() {
        let planes = vec![vec![0b0000_0001u8]]; // LSB-first source
        let layout = Layout { interleave: 1, dst_lsb: false, channels: 1 };
        // reversed bits -> 0b1000_0000
        assert_eq!(repack_planar(&planes, layout, true), vec![0b1000_0000]);
    }

    #[test]
    fn negative_interleave_reverses_group() {
        let planes = vec![vec![1, 2, 3, 4], vec![5, 6, 7, 8]];
        let layout = Layout { interleave: -2, dst_lsb: true, channels: 2 };
        let out = repack_planar(&planes, layout, true);
        // group of 2, reversed within group: L(2,1) R(6,5) L(4,3) R(8,7)
        assert_eq!(out, vec![2, 1, 6, 5, 4, 3, 8, 7]);
    }

    #[test]
    fn stride_matches_layout() {
        assert_eq!(Layout { interleave: 4, dst_lsb: true, channels: 2 }.stride(), 8);
        assert_eq!(Layout { interleave: -4, dst_lsb: true, channels: 2 }.stride(), 8);
        assert_eq!(Layout { interleave: 1, dst_lsb: true, channels: 1 }.stride(), 1);
    }
}
