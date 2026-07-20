//! A bounded byte ring shared between the decode/feeder thread (producer) and
//! the PipeWire RT callback (consumer).
//!
//! v1 uses a `Mutex<VecDeque<u8>>`; the RT side takes the lock only for a short
//! `memcpy` and falls back to idle bytes if it is momentarily contended, which
//! keeps the callback from ever blocking on the producer. A lock-free SPSC
//! upgrade is tracked in SPEC.md as future work.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

#[derive(Clone)]
pub struct Ring {
    buf: Arc<Mutex<VecDeque<u8>>>,
    capacity: usize,
    /// Producer has delivered the last byte of the current track.
    eof: Arc<AtomicBool>,
    /// Total bytes consumed by the RT side since the last `reset`.
    consumed: Arc<AtomicU64>,
}

impl Ring {
    pub fn new(capacity: usize) -> Self {
        Self {
            buf: Arc::new(Mutex::new(VecDeque::with_capacity(capacity))),
            capacity,
            eof: Arc::new(AtomicBool::new(false)),
            consumed: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Bytes that can still be pushed without exceeding capacity.
    pub fn free_space(&self) -> usize {
        let len = self.buf.lock().unwrap().len();
        self.capacity.saturating_sub(len)
    }

    pub fn len(&self) -> usize {
        self.buf.lock().unwrap().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Producer: append bytes (caller should respect [`Ring::free_space`]).
    pub fn push(&self, bytes: &[u8]) {
        let mut b = self.buf.lock().unwrap();
        b.extend(bytes.iter().copied());
    }

    /// Consumer (RT): fill `dst`, returning bytes written. Never blocks; if the
    /// lock is contended it writes nothing (caller emits idle bytes instead).
    pub fn read_into(&self, dst: &mut [u8]) -> usize {
        let Ok(mut b) = self.buf.try_lock() else { return 0 };
        let n = dst.len().min(b.len());
        for (slot, byte) in dst[..n].iter_mut().zip(b.drain(..n)) {
            *slot = byte;
        }
        self.consumed.fetch_add(n as u64, Ordering::Relaxed);
        n
    }

    pub fn set_eof(&self, v: bool) {
        self.eof.store(v, Ordering::Release);
    }

    pub fn is_eof(&self) -> bool {
        self.eof.load(Ordering::Acquire)
    }

    /// EOF has been signaled and the buffer is fully drained.
    pub fn is_drained(&self) -> bool {
        self.is_eof() && self.is_empty()
    }

    pub fn consumed(&self) -> u64 {
        self.consumed.load(Ordering::Relaxed)
    }

    /// Clear on stop/seek: drop buffered bytes, reset counters and EOF.
    pub fn reset(&self) {
        self.buf.lock().unwrap().clear();
        self.eof.store(false, Ordering::Release);
        self.consumed.store(0, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_read_roundtrip_and_counters() {
        let r = Ring::new(16);
        assert_eq!(r.free_space(), 16);
        r.push(&[1, 2, 3, 4]);
        assert_eq!(r.len(), 4);
        assert_eq!(r.free_space(), 12);

        let mut dst = [0u8; 3];
        assert_eq!(r.read_into(&mut dst), 3);
        assert_eq!(dst, [1, 2, 3]);
        assert_eq!(r.consumed(), 3);
        assert_eq!(r.len(), 1);
    }

    #[test]
    fn eof_and_drain() {
        let r = Ring::new(8);
        r.push(&[9]);
        r.set_eof(true);
        assert!(r.is_eof());
        assert!(!r.is_drained());
        let mut d = [0u8; 4];
        r.read_into(&mut d);
        assert!(r.is_drained());
    }

    #[test]
    fn reset_clears() {
        let r = Ring::new(8);
        r.push(&[1, 2, 3]);
        r.set_eof(true);
        r.reset();
        assert!(r.is_empty());
        assert!(!r.is_eof());
        assert_eq!(r.consumed(), 0);
    }
}
