//! Playlist model: ordered tracks, current selection, repeat/shuffle behavior,
//! and extended-M3U persistence. Pure logic — metadata population and file
//! decoding happen at the [`crate::player`] layer.

use std::io::Write;
use std::path::{Path, PathBuf};

use crate::error::{EngineError, Result};
use crate::types::{RepeatMode, TrackInfo};

#[derive(Default)]
pub struct Playlist {
    tracks: Vec<TrackInfo>,
    current: Option<usize>,
    repeat: RepeatMode,
    shuffle: bool,
    /// Shuffled visiting order (indices into `tracks`); identity when not shuffling.
    order: Vec<usize>,
}

impl Playlist {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn tracks(&self) -> &[TrackInfo] {
        &self.tracks
    }
    pub fn len(&self) -> usize {
        self.tracks.len()
    }
    pub fn is_empty(&self) -> bool {
        self.tracks.is_empty()
    }
    pub fn current(&self) -> Option<usize> {
        self.current
    }
    pub fn current_track(&self) -> Option<&TrackInfo> {
        self.current.and_then(|i| self.tracks.get(i))
    }
    pub fn get(&self, i: usize) -> Option<&TrackInfo> {
        self.tracks.get(i)
    }
    pub fn get_mut(&mut self, i: usize) -> Option<&mut TrackInfo> {
        self.tracks.get_mut(i)
    }
    pub fn repeat(&self) -> RepeatMode {
        self.repeat
    }
    pub fn shuffle(&self) -> bool {
        self.shuffle
    }

    pub fn push(&mut self, track: TrackInfo) {
        self.tracks.push(track);
        self.rebuild_order();
    }

    pub fn extend<I: IntoIterator<Item = TrackInfo>>(&mut self, it: I) {
        self.tracks.extend(it);
        self.rebuild_order();
    }

    pub fn remove(&mut self, i: usize) {
        if i >= self.tracks.len() {
            return;
        }
        self.tracks.remove(i);
        self.current = match self.current {
            Some(c) if c == i => None,
            Some(c) if c > i => Some(c - 1),
            other => other,
        };
        self.rebuild_order();
    }

    pub fn clear(&mut self) {
        self.tracks.clear();
        self.current = None;
        self.order.clear();
    }

    pub fn set_current(&mut self, i: usize) -> Option<&TrackInfo> {
        if i < self.tracks.len() {
            self.current = Some(i);
            self.tracks.get(i)
        } else {
            None
        }
    }

    pub fn set_repeat(&mut self, r: RepeatMode) {
        self.repeat = r;
    }
    pub fn cycle_repeat(&mut self) {
        self.repeat = self.repeat.next();
    }

    /// Toggle shuffle. `seed` drives the permutation when enabling.
    pub fn toggle_shuffle(&mut self, seed: u64) {
        self.shuffle = !self.shuffle;
        self.rebuild_order_seeded(seed);
    }

    fn rebuild_order(&mut self) {
        // Preserve identity ordering; shuffle re-seeds explicitly via toggle.
        if !self.shuffle {
            self.order = (0..self.tracks.len()).collect();
        } else if self.order.len() != self.tracks.len() {
            self.order = (0..self.tracks.len()).collect();
        }
    }

    fn rebuild_order_seeded(&mut self, seed: u64) {
        self.order = (0..self.tracks.len()).collect();
        if self.shuffle {
            shuffle_in_place(&mut self.order, seed);
        }
    }

    /// Position of `current` within the visiting order.
    fn order_pos(&self) -> Option<usize> {
        let cur = self.current?;
        self.order.iter().position(|&x| x == cur)
    }

    /// Index to play next. `natural` is true on track end (honors RepeatOne),
    /// false when the user explicitly skips.
    pub fn next_index(&self, natural: bool) -> Option<usize> {
        if self.tracks.is_empty() {
            return None;
        }
        if natural && self.repeat == RepeatMode::One {
            return self.current.or(Some(self.order[0]));
        }
        match self.order_pos() {
            None => self.order.first().copied(),
            Some(pos) if pos + 1 < self.order.len() => Some(self.order[pos + 1]),
            Some(_) => {
                if self.repeat == RepeatMode::All {
                    self.order.first().copied()
                } else {
                    None
                }
            }
        }
    }

    /// Index to play when going back.
    pub fn prev_index(&self) -> Option<usize> {
        if self.tracks.is_empty() {
            return None;
        }
        match self.order_pos() {
            None => self.order.first().copied(),
            Some(0) => {
                if self.repeat == RepeatMode::All {
                    self.order.last().copied()
                } else {
                    Some(self.order[0])
                }
            }
            Some(pos) => Some(self.order[pos - 1]),
        }
    }

    /// Write an extended-M3U playlist. Track paths are written **absolute** so
    /// the playlist plays regardless of where it is loaded from.
    pub fn save_m3u(&self, path: &Path) -> Result<()> {
        let mut f = std::fs::File::create(path).map_err(|e| EngineError::io(path, e))?;
        writeln!(f, "#EXTM3U").map_err(|e| EngineError::io(path, e))?;
        for t in &self.tracks {
            let secs = t.info.as_ref().map(|i| i.duration().as_secs()).unwrap_or(0);
            writeln!(f, "#EXTINF:{},{}", secs, t.display_title())
                .map_err(|e| EngineError::io(path, e))?;
            writeln!(f, "{}", absolutize(&t.path).display()).map_err(|e| EngineError::io(path, e))?;
        }
        Ok(())
    }

    /// Parse an M3U into its list of file paths (comments ignored).
    pub fn load_m3u_paths(path: &Path) -> Result<Vec<PathBuf>> {
        let text = std::fs::read_to_string(path).map_err(|e| EngineError::io(path, e))?;
        let base = path.parent();
        let mut out = Vec::new();
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let p = PathBuf::from(line);
            let p = if p.is_absolute() {
                p
            } else if let Some(b) = base {
                b.join(p)
            } else {
                p
            };
            out.push(p);
        }
        Ok(out)
    }
}

/// Make a path absolute (without requiring it to exist): absolute paths are
/// returned as-is; relative paths are joined onto the current directory.
pub fn absolutize(p: &Path) -> PathBuf {
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        std::env::current_dir().map(|c| c.join(p)).unwrap_or_else(|_| p.to_path_buf())
    }
}

/// In-place Fisher–Yates using a small xorshift PRNG (no external deps; the
/// determinism is handy for tests, and the seed comes from wall-clock at runtime).
fn shuffle_in_place<T>(v: &mut [T], seed: u64) {
    let mut state = seed | 1;
    let mut next = || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    for i in (1..v.len()).rev() {
        let j = (next() % (i as u64 + 1)) as usize;
        v.swap(i, j);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Tags;

    fn track(name: &str) -> TrackInfo {
        TrackInfo { path: PathBuf::from(name), tags: Tags::default(), info: None, missing: false }
    }

    fn list(n: usize) -> Playlist {
        let mut p = Playlist::new();
        for i in 0..n {
            p.push(track(&format!("t{i}.dsf")));
        }
        p
    }

    #[test]
    fn sequential_next_prev() {
        let mut p = list(3);
        p.set_current(0);
        assert_eq!(p.next_index(false), Some(1));
        p.set_current(1);
        assert_eq!(p.next_index(false), Some(2));
        p.set_current(2);
        assert_eq!(p.next_index(false), None); // repeat off -> stop
        assert_eq!(p.prev_index(), Some(1));
    }

    #[test]
    fn repeat_all_wraps() {
        let mut p = list(3);
        p.set_repeat(RepeatMode::All);
        p.set_current(2);
        assert_eq!(p.next_index(false), Some(0));
        p.set_current(0);
        assert_eq!(p.prev_index(), Some(2));
    }

    #[test]
    fn repeat_one_holds_on_natural_end_but_skips_on_manual() {
        let mut p = list(3);
        p.set_repeat(RepeatMode::One);
        p.set_current(1);
        assert_eq!(p.next_index(true), Some(1)); // natural end repeats
        assert_eq!(p.next_index(false), Some(2)); // manual skip advances
    }

    #[test]
    fn remove_adjusts_current() {
        let mut p = list(3);
        p.set_current(2);
        p.remove(0);
        assert_eq!(p.current(), Some(1));
        p.remove(1);
        assert_eq!(p.current(), None);
    }

    #[test]
    fn shuffle_permutes_but_keeps_all_indices() {
        let mut p = list(6);
        p.toggle_shuffle(0xDEAD_BEEF);
        assert!(p.shuffle());
        let mut seen = p.order.clone();
        seen.sort_unstable();
        assert_eq!(seen, (0..6).collect::<Vec<_>>());
    }

    #[test]
    fn m3u_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pl.m3u");
        let p = list(2);
        p.save_m3u(&path).unwrap();
        let paths = Playlist::load_m3u_paths(&path).unwrap();
        assert_eq!(paths.len(), 2);
        assert!(paths[0].ends_with("t0.dsf"));
    }

    #[test]
    fn save_m3u_writes_absolute_paths() {
        // A filename-only track must be saved as an absolute path so the
        // playlist plays when loaded from a different directory.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pl.m3u");
        let mut p = Playlist::new();
        p.push(track("song.dsf"));
        p.save_m3u(&path).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        let entry = text.lines().find(|l| !l.starts_with('#') && l.ends_with("song.dsf")).unwrap();
        assert!(Path::new(entry).is_absolute(), "entry must be absolute, got {entry:?}");
    }
}
