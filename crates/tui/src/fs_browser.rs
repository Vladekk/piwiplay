//! Filesystem browser model: a directory listing with a selection cursor and a
//! multi-selection (marked) set. Shows a parent entry, then directories, then
//! supported audio files (and `.m3u` playlists).

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use piwiplay_engine::decode;

#[derive(Debug, Clone)]
pub struct Entry {
    pub path: PathBuf,
    pub name: String,
    pub is_dir: bool,
    pub is_parent: bool,
    pub is_playlist: bool,
}

pub struct Browser {
    pub cwd: PathBuf,
    pub entries: Vec<Entry>,
    pub selected: usize,
    /// Multi-selection: indices into `entries` marked for a bulk action.
    pub marked: BTreeSet<usize>,
    /// Anchor for range extension (Shift+move).
    anchor: usize,
    /// Only show `.m3u`/`.m3u8` entries (used for the saved-playlists pane).
    playlists_only: bool,
}

impl Browser {
    pub fn new(start: &Path) -> Self {
        Self::with_mode(start, false)
    }

    /// A browser restricted to playlist files (for the saved-playlists pane).
    pub fn playlists_at(dir: &Path) -> Self {
        Self::with_mode(dir, true)
    }

    fn with_mode(start: &Path, playlists_only: bool) -> Self {
        let cwd = if start.is_dir() {
            start.to_path_buf()
        } else {
            start.parent().map(|p| p.to_path_buf()).unwrap_or_else(|| PathBuf::from("."))
        };
        let mut b = Browser {
            cwd,
            entries: Vec::new(),
            selected: 0,
            marked: BTreeSet::new(),
            anchor: 0,
            playlists_only,
        };
        b.refresh();
        b
    }

    pub fn refresh(&mut self) {
        let mut dirs = Vec::new();
        let mut files = Vec::new();
        if let Ok(rd) = std::fs::read_dir(&self.cwd) {
            for e in rd.flatten() {
                let path = e.path();
                let name = e.file_name().to_string_lossy().into_owned();
                if name.starts_with('.') {
                    continue;
                }
                let is_dir = path.is_dir();
                let is_playlist = path
                    .extension()
                    .and_then(|x| x.to_str())
                    .map(|x| x.eq_ignore_ascii_case("m3u") || x.eq_ignore_ascii_case("m3u8"))
                    .unwrap_or(false);
                if is_dir {
                    dirs.push(Entry { path, name, is_dir, is_parent: false, is_playlist: false });
                } else if self.playlists_only {
                    if is_playlist {
                        files.push(Entry { path, name, is_dir, is_parent: false, is_playlist });
                    }
                } else if decode::is_supported(&path) || is_playlist {
                    files.push(Entry { path, name, is_dir, is_parent: false, is_playlist });
                }
            }
        }
        dirs.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
        files.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));

        let mut entries = Vec::new();
        if let Some(parent) = self.cwd.parent() {
            entries.push(Entry {
                path: parent.to_path_buf(),
                name: "..".into(),
                is_dir: true,
                is_parent: true,
                is_playlist: false,
            });
        }
        entries.extend(dirs);
        entries.extend(files);
        self.entries = entries;
        self.selected = self.selected.min(self.entries.len().saturating_sub(1));
        self.marked.clear();
        self.anchor = self.selected;
    }

    pub fn selected_entry(&self) -> Option<&Entry> {
        self.entries.get(self.selected)
    }

    /// Move the cursor. When `extend`, grow the marked range from the anchor
    /// (Shift+move multi-select).
    pub fn move_by(&mut self, delta: isize, extend: bool) {
        if self.entries.is_empty() {
            return;
        }
        let n = self.entries.len() as isize;
        if !extend {
            self.anchor = (self.selected as isize).clamp(0, n - 1) as usize;
            self.marked.clear();
        }
        let s = (self.selected as isize + delta).clamp(0, n - 1);
        self.selected = s as usize;
        if extend {
            self.mark_range();
        }
    }

    pub fn to_top(&mut self, extend: bool) {
        if !extend {
            self.marked.clear();
            self.anchor = 0;
        }
        self.selected = 0;
        if extend {
            self.mark_range();
        }
    }

    pub fn to_bottom(&mut self, extend: bool) {
        let last = self.entries.len().saturating_sub(1);
        if !extend {
            self.marked.clear();
            self.anchor = last;
        }
        self.selected = last;
        if extend {
            self.mark_range();
        }
    }

    fn mark_range(&mut self) {
        let (lo, hi) = (self.anchor.min(self.selected), self.anchor.max(self.selected));
        self.marked = (lo..=hi).filter(|&i| !self.entries.get(i).map(|e| e.is_parent).unwrap_or(true)).collect();
    }

    /// Set the cursor directly (e.g. mouse click), clearing any marks.
    pub fn set_selected(&mut self, i: usize) {
        if i < self.entries.len() {
            self.selected = i;
            self.anchor = i;
            self.marked.clear();
        }
    }

    /// Paths to act on: the marked set if any, else the single selected entry
    /// (never the ".." parent).
    pub fn action_paths(&self) -> Vec<PathBuf> {
        if !self.marked.is_empty() {
            return self.marked.iter().filter_map(|&i| self.entries.get(i)).map(|e| e.path.clone()).collect();
        }
        match self.selected_entry() {
            Some(e) if !e.is_parent => vec![e.path.clone()],
            _ => Vec::new(),
        }
    }

    pub fn has_marks(&self) -> bool {
        !self.marked.is_empty()
    }

    /// Enter the selected directory (or parent). Returns true if navigated.
    pub fn enter_dir(&mut self) -> bool {
        if let Some(e) = self.selected_entry() {
            if e.is_dir {
                self.goto(&e.path.clone());
                return true;
            }
        }
        false
    }

    /// Navigate to an arbitrary directory.
    pub fn goto(&mut self, dir: &Path) {
        self.cwd = dir.to_path_buf();
        self.selected = 0;
        self.refresh();
    }

    /// Jump the cursor to the first entry whose name contains `needle`.
    pub fn find(&mut self, needle: &str) {
        let n = needle.to_lowercase();
        if let Some(i) = self.entries.iter().position(|e| e.name.to_lowercase().contains(&n)) {
            self.set_selected(i);
        }
    }
}

/// The user's music directory, if the system defines one: `XDG_MUSIC_DIR`,
/// then `xdg-user-dir MUSIC`, then `~/Music`. Returns None if none exists.
pub fn media_library_dir() -> Option<PathBuf> {
    if let Ok(v) = std::env::var("XDG_MUSIC_DIR") {
        let p = PathBuf::from(v);
        if p.is_dir() {
            return Some(p);
        }
    }
    if let Ok(out) = std::process::Command::new("xdg-user-dir").arg("MUSIC").output() {
        if out.status.success() {
            let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
            let p = PathBuf::from(&s);
            // xdg-user-dir returns $HOME when unset; only accept a real Music subdir.
            if p.is_dir() && p.file_name().map(|n| n == "Music").unwrap_or(false) {
                return Some(p);
            }
        }
    }
    let home = std::env::var("HOME").ok()?;
    let p = PathBuf::from(home).join("Music");
    p.is_dir().then_some(p)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn lists_dirs_then_supported_files_and_parent() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join("sub")).unwrap();
        fs::write(dir.path().join("a.dsf"), b"x").unwrap();
        fs::write(dir.path().join("b.txt"), b"x").unwrap();
        fs::write(dir.path().join(".hidden.dsf"), b"x").unwrap();

        let b = Browser::new(dir.path());
        let names: Vec<&str> = b.entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names[0], "..");
        assert!(names.contains(&"sub"));
        assert!(names.contains(&"a.dsf"));
        assert!(!names.contains(&"b.txt"));
        assert!(!names.iter().any(|n| n.contains("hidden")));
    }

    #[test]
    fn shift_move_marks_range() {
        let dir = tempfile::tempdir().unwrap();
        for i in 0..5 {
            fs::write(dir.path().join(format!("f{i}.dsf")), b"x").unwrap();
        }
        let mut b = Browser::new(dir.path());
        // entries: [.., f0..f4] -> select first file
        b.set_selected(1);
        b.move_by(1, true); // extend to index 2
        b.move_by(1, true); // extend to index 3
        assert_eq!(b.marked.len(), 3, "marked f0,f1,f2");
        assert!(b.has_marks());
        assert_eq!(b.action_paths().len(), 3);
    }

    #[test]
    fn action_paths_falls_back_to_single_selection() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("only.dsf"), b"x").unwrap();
        let mut b = Browser::new(dir.path());
        b.set_selected(1);
        assert_eq!(b.action_paths().len(), 1);
        assert!(b.action_paths()[0].ends_with("only.dsf"));
    }

    #[test]
    fn parent_never_marked_or_actioned() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("a.dsf"), b"x").unwrap();
        let mut b = Browser::new(dir.path());
        b.set_selected(0); // ".."
        assert!(b.action_paths().is_empty());
    }

    #[test]
    fn playlists_only_hides_audio() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("a.dsf"), b"x").unwrap();
        fs::write(dir.path().join("mix.m3u"), b"#EXTM3U\n").unwrap();
        let b = Browser::playlists_at(dir.path());
        let names: Vec<&str> = b.entries.iter().map(|e| e.name.as_str()).collect();
        assert!(names.contains(&"mix.m3u"));
        assert!(!names.contains(&"a.dsf"));
    }
}
