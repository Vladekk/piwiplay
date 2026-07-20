//! Filesystem browser model: a directory listing with a selection cursor.
//! Shows a parent entry, then directories, then supported audio files.

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
}

impl Browser {
    pub fn new(start: &Path) -> Self {
        let cwd = if start.is_dir() {
            start.to_path_buf()
        } else {
            start.parent().map(|p| p.to_path_buf()).unwrap_or_else(|| PathBuf::from("."))
        };
        let mut b = Browser { cwd, entries: Vec::new(), selected: 0 };
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
                    continue; // skip hidden
                }
                let is_dir = path.is_dir();
                let is_playlist = path
                    .extension()
                    .and_then(|x| x.to_str())
                    .map(|x| x.eq_ignore_ascii_case("m3u") || x.eq_ignore_ascii_case("m3u8"))
                    .unwrap_or(false);
                if is_dir {
                    dirs.push(Entry { path, name, is_dir, is_parent: false, is_playlist: false });
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
    }

    pub fn selected_entry(&self) -> Option<&Entry> {
        self.entries.get(self.selected)
    }

    pub fn move_by(&mut self, delta: isize) {
        if self.entries.is_empty() {
            return;
        }
        let n = self.entries.len() as isize;
        let mut s = self.selected as isize + delta;
        s = s.clamp(0, n - 1);
        self.selected = s as usize;
    }

    pub fn to_top(&mut self) {
        self.selected = 0;
    }
    pub fn to_bottom(&mut self) {
        self.selected = self.entries.len().saturating_sub(1);
    }

    /// Enter the selected directory (or parent). Returns true if navigated.
    pub fn enter_dir(&mut self) -> bool {
        if let Some(e) = self.selected_entry() {
            if e.is_dir {
                self.cwd = e.path.clone();
                self.selected = 0;
                self.refresh();
                return true;
            }
        }
        false
    }

    /// Jump the cursor to the first entry whose name contains `needle` (case-insensitive).
    pub fn find(&mut self, needle: &str) {
        let n = needle.to_lowercase();
        if let Some(i) = self.entries.iter().position(|e| e.name.to_lowercase().contains(&n)) {
            self.selected = i;
        }
    }
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
        fs::write(dir.path().join("b.txt"), b"x").unwrap(); // unsupported, skipped
        fs::write(dir.path().join(".hidden.dsf"), b"x").unwrap(); // hidden, skipped

        let b = Browser::new(dir.path());
        let names: Vec<&str> = b.entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names[0], ".."); // parent first
        assert!(names.contains(&"sub"));
        assert!(names.contains(&"a.dsf"));
        assert!(!names.contains(&"b.txt"));
        assert!(!names.iter().any(|n| n.contains("hidden")));
        // dir sorts before file
        let sub = names.iter().position(|n| *n == "sub").unwrap();
        let file = names.iter().position(|n| *n == "a.dsf").unwrap();
        assert!(sub < file);
    }

    #[test]
    fn find_jumps_selection() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("apple.dsf"), b"x").unwrap();
        fs::write(dir.path().join("banana.dsf"), b"x").unwrap();
        let mut b = Browser::new(dir.path());
        b.find("banana");
        assert_eq!(b.selected_entry().unwrap().name, "banana.dsf");
    }
}
