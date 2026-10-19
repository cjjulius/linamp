use std::path::{Path, PathBuf};
use std::time::Duration;

use walkdir::WalkDir;

/// Supported audio file extensions.
pub const SUPPORTED_EXTS: &[&str] = &[
    "mp3", "flac", "ogg", "oga", "wav", "m4a", "aac", "opus", "wv", "aiff", "aif",
];

/// A single track in the playlist with cached metadata.
#[derive(Clone, Debug)]
pub struct Track {
    pub path: PathBuf,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub duration: Option<Duration>,
    /// Audio properties for the classic info line.
    pub sample_rate: Option<u32>,
    pub bitrate: Option<u32>,
    pub channels: Option<u8>,
    /// Whether tags/properties have been read from disk yet (lazy loading).
    pub loaded: bool,
}

impl Track {
    /// A track with only its file name known; tags are read later by `load`.
    pub fn new(path: PathBuf) -> Self {
        let fallback_title = path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "Unknown".to_string());
        Track {
            path,
            title: fallback_title,
            artist: String::new(),
            album: String::new(),
            duration: None,
            sample_rate: None,
            bitrate: None,
            channels: None,
            loaded: false,
        }
    }

    /// Read tags/properties from disk (no-op if already loaded).
    pub fn load(&mut self) {
        if self.loaded {
            return;
        }
        self.loaded = true;
        if let Ok(tagged) = lofty::read_from_path(&self.path) {
            use lofty::file::{AudioFile, TaggedFileExt};
            use lofty::tag::Accessor;

            let props = tagged.properties();
            self.duration = Some(props.duration());
            self.sample_rate = props.sample_rate();
            self.bitrate = props.audio_bitrate();
            self.channels = props.channels();

            if let Some(tag) = tagged.primary_tag().or_else(|| tagged.first_tag()) {
                if let Some(t) = tag.title() {
                    if !t.trim().is_empty() {
                        self.title = t.to_string();
                    }
                }
                if let Some(a) = tag.artist() {
                    self.artist = a.to_string();
                }
                if let Some(al) = tag.album() {
                    self.album = al.to_string();
                }
            }
        }
    }

    /// Build a fully-loaded track (tags read immediately).
    pub fn from_path(path: PathBuf) -> Self {
        let mut t = Track::new(path);
        t.load();
        t
    }

    /// "Artist - Title" when an artist is known, otherwise just the title.
    pub fn display_name(&self) -> String {
        if self.artist.trim().is_empty() {
            self.title.clone()
        } else {
            format!("{} - {}", self.artist, self.title)
        }
    }
}

/// The ordered list of tracks plus the current selection/playback cursor.
#[derive(Default)]
pub struct Playlist {
    pub tracks: Vec<Track>,
    pub current: Option<usize>,
}

impl Playlist {
    pub fn is_empty(&self) -> bool {
        self.tracks.is_empty()
    }

    pub fn len(&self) -> usize {
        self.tracks.len()
    }

    pub fn clear(&mut self) {
        self.tracks.clear();
        self.current = None;
    }

    pub fn current_track(&self) -> Option<&Track> {
        self.current.and_then(|i| self.tracks.get(i))
    }

    /// Add a single file if it has a supported extension (tags read lazily).
    pub fn add_file(&mut self, path: PathBuf) {
        if is_supported(&path) {
            self.tracks.push(Track::new(path));
        }
    }

    /// Read tags for one track if not already loaded.
    pub fn ensure_loaded(&mut self, index: usize) {
        if let Some(t) = self.tracks.get_mut(index) {
            t.load();
        }
    }

    /// Read tags for every track.
    pub fn load_all(&mut self) {
        for t in &mut self.tracks {
            t.load();
        }
    }

    /// Recursively add every supported file found under `dir`.
    pub fn add_folder(&mut self, dir: &Path) {
        let mut found: Vec<PathBuf> = WalkDir::new(dir)
            .into_iter()
            .filter_map(Result::ok)
            .filter(|e| e.file_type().is_file())
            .map(|e| e.into_path())
            .filter(|p| is_supported(p))
            .collect();
        found.sort();
        for p in found {
            self.tracks.push(Track::new(p));
        }
    }

    pub fn remove(&mut self, index: usize) {
        if index >= self.tracks.len() {
            return;
        }
        self.tracks.remove(index);
        match self.current {
            Some(c) if c == index => self.current = None,
            Some(c) if c > index => self.current = Some(c - 1),
            _ => {}
        }
    }

    /// Index after `current`, honoring shuffle/repeat-all.
    pub fn next_index(&self, shuffle: bool, repeat_all: bool) -> Option<usize> {
        if self.tracks.is_empty() {
            return None;
        }
        if shuffle {
            return Some(pseudo_random(self.tracks.len()));
        }
        match self.current {
            Some(c) if c + 1 < self.tracks.len() => Some(c + 1),
            Some(_) => {
                if repeat_all {
                    Some(0)
                } else {
                    None
                }
            }
            None => Some(0),
        }
    }

    /// Index before `current`.
    pub fn prev_index(&self, shuffle: bool, repeat_all: bool) -> Option<usize> {
        if self.tracks.is_empty() {
            return None;
        }
        if shuffle {
            return Some(pseudo_random(self.tracks.len()));
        }
        match self.current {
            Some(c) if c > 0 => Some(c - 1),
            Some(_) => {
                if repeat_all {
                    Some(self.tracks.len() - 1)
                } else {
                    None
                }
            }
            None => Some(0),
        }
    }
}

impl Playlist {
    /// Append all tracks referenced by an .m3u/.m3u8/.pls file.
    pub fn load_file(&mut self, path: &Path) -> usize {
        let before = self.tracks.len();
        for p in parse_playlist_file(path) {
            if p.exists() && is_supported(&p) {
                self.tracks.push(Track::new(p));
            }
        }
        self.tracks.len() - before
    }
}

/// Write the playlist to disk as M3U (extended) or PLS, chosen by extension.
pub fn save_playlist_file(path: &Path, tracks: &[Track]) -> std::io::Result<()> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_lowercase())
        .unwrap_or_default();

    let mut out = String::new();
    if ext == "pls" {
        out.push_str("[playlist]\n");
        for (i, t) in tracks.iter().enumerate() {
            let n = i + 1;
            out.push_str(&format!("File{n}={}\n", t.path.display()));
            out.push_str(&format!("Title{n}={}\n", t.display_name()));
            let secs = t.duration.map(|d| d.as_secs() as i64).unwrap_or(-1);
            out.push_str(&format!("Length{n}={secs}\n"));
        }
        out.push_str(&format!("NumberOfEntries={}\n", tracks.len()));
        out.push_str("Version=2\n");
    } else {
        out.push_str("#EXTM3U\n");
        for t in tracks {
            let secs = t.duration.map(|d| d.as_secs() as i64).unwrap_or(-1);
            out.push_str(&format!("#EXTINF:{secs},{}\n", t.display_name()));
            out.push_str(&format!("{}\n", t.path.display()));
        }
    }
    std::fs::write(path, out)
}

/// Return the file paths referenced by an .m3u/.m3u8/.pls file, resolving
/// relative paths against the playlist file's own directory.
pub fn parse_playlist_file(path: &Path) -> Vec<PathBuf> {
    let content = std::fs::read_to_string(path).unwrap_or_default();
    let base = path.parent();
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_lowercase())
        .unwrap_or_default();

    let mut out = Vec::new();
    if ext == "pls" {
        for line in content.lines() {
            let line = line.trim();
            if let Some(rest) = line.strip_prefix("File") {
                if let Some(eq) = rest.find('=') {
                    out.push(resolve(base, rest[eq + 1..].trim()));
                }
            }
        }
    } else {
        for line in content.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            out.push(resolve(base, line));
        }
    }
    out
}

fn resolve(base: Option<&Path>, value: &str) -> PathBuf {
    let p = PathBuf::from(value);
    if p.is_absolute() {
        p
    } else if let Some(b) = base {
        b.join(p)
    } else {
        p
    }
}

/// Extract the first embedded cover image (raw encoded bytes) from a file, if any.
pub fn read_cover(path: &Path) -> Option<Vec<u8>> {
    use lofty::file::TaggedFileExt;
    let tagged = lofty::read_from_path(path).ok()?;
    let tag = tagged.primary_tag().or_else(|| tagged.first_tag())?;
    let pic = tag.pictures().first()?;
    Some(pic.data().to_vec())
}

pub fn is_supported(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| SUPPORTED_EXTS.contains(&e.to_lowercase().as_str()))
        .unwrap_or(false)
}

/// Tiny time-seeded index picker so we don't pull in the `rand` crate for a
/// shuffle that doesn't need to be cryptographically anything.
fn pseudo_random(len: usize) -> usize {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.subsec_nanos() as usize)
        .unwrap_or(0);
    nanos % len.max(1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};

    #[test]
    fn supported_exts() {
        assert!(is_supported(Path::new("a.mp3")));
        assert!(is_supported(Path::new("A.FLAC")));
        assert!(!is_supported(Path::new("a.txt")));
        assert!(!is_supported(Path::new("noext")));
    }

    #[test]
    fn lazy_track_uses_filename() {
        let t = Track::new(PathBuf::from("/music/Song.mp3"));
        assert!(!t.loaded);
        assert_eq!(t.title, "Song");
        assert_eq!(t.display_name(), "Song");
    }

    #[test]
    fn display_with_artist() {
        let mut t = Track::new(PathBuf::from("x.mp3"));
        t.artist = "Band".into();
        t.title = "Hit".into();
        assert_eq!(t.display_name(), "Band - Hit");
    }

    #[test]
    fn next_prev_indices() {
        let mut pl = Playlist::default();
        for n in ["a.mp3", "b.mp3", "c.mp3"] {
            pl.add_file(PathBuf::from(n));
        }
        pl.current = Some(0);
        assert_eq!(pl.next_index(false, false), Some(1));
        pl.current = Some(2);
        assert_eq!(pl.next_index(false, false), None);
        assert_eq!(pl.next_index(false, true), Some(0));
        pl.current = Some(0);
        assert_eq!(pl.prev_index(false, false), None);
        pl.current = Some(1);
        assert_eq!(pl.prev_index(false, false), Some(0));
    }

    #[test]
    fn remove_adjusts_current() {
        let mut pl = Playlist::default();
        for n in ["a.mp3", "b.mp3", "c.mp3"] {
            pl.add_file(PathBuf::from(n));
        }
        pl.current = Some(2);
        pl.remove(0);
        assert_eq!(pl.len(), 2);
        assert_eq!(pl.current, Some(1));
    }

    #[test]
    fn m3u_roundtrip() {
        let dir = std::env::temp_dir();
        let p = dir.join("linamp_test_list.m3u");
        let mut pl = Playlist::default();
        pl.tracks.push(Track::new(dir.join("one.mp3")));
        pl.tracks.push(Track::new(dir.join("two.flac")));
        save_playlist_file(&p, &pl.tracks).unwrap();
        let paths = parse_playlist_file(&p);
        assert_eq!(paths.len(), 2);
        assert!(paths[0].ends_with("one.mp3"));
        let _ = std::fs::remove_file(&p);
    }
}
