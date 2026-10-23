use std::path::{Path, PathBuf};
use std::time::Duration;

use walkdir::WalkDir;

/// Supported audio file extensions.
pub const SUPPORTED_EXTS: &[&str] = &[
    "mp3", "flac", "ogg", "oga", "wav", "m4a", "aac", "opus", "wv", "aiff", "aif",
];

/// Repair "mojibake": text that is really UTF-8 but was decoded as Latin-1. This
/// happens with ID3v2 frames whose encoding byte says ISO-8859-1 while the bytes
/// are actually UTF-8 (common in the wild, e.g. zing.vn MP3s), so an accented
/// title like "Bằng Kiều" comes back as "Bá»±ng Kiá»u".
///
/// When every char is U+00..=U+FF the original bytes are recoverable: map each
/// char back to its byte and re-decode as UTF-8. Only used when that yields valid
/// UTF-8 that actually changed, so correctly-tagged text (and plain ASCII/Latin-1
/// that isn't disguised UTF-8) is left untouched. Idempotent — repaired text has
/// chars above U+00FF, so a second pass is a no-op.
fn repair_mojibake(s: &str) -> String {
    if s.is_empty() || !s.chars().all(|c| (c as u32) <= 0xFF) {
        return s.to_string();
    }
    let bytes: Vec<u8> = s.chars().map(|c| c as u8).collect();
    match std::str::from_utf8(&bytes) {
        Ok(fixed) if fixed != s => fixed.to_string(),
        _ => s.to_string(),
    }
}

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
                    let t = repair_mojibake(&t);
                    if !t.trim().is_empty() {
                        self.title = t;
                    }
                }
                if let Some(a) = tag.artist() {
                    self.artist = repair_mojibake(&a);
                }
                if let Some(al) = tag.album() {
                    self.album = repair_mojibake(&al);
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

    /// Remove tracks whose file no longer exists. Returns how many were removed.
    /// The currently-playing track stays selected if it survives (else `current`
    /// clears). Classic Winamp's "remove missing files".
    pub fn remove_missing(&mut self) -> usize {
        let before = self.tracks.len();
        let cur = self.current_path();
        self.tracks.retain(|t| t.path.exists());
        self.reselect(cur);
        before - self.tracks.len()
    }

    /// Reverse the playlist order, keeping the current track selected.
    pub fn reverse(&mut self) {
        let cur = self.current_path();
        self.tracks.reverse();
        self.reselect(cur);
    }

    /// Shuffle the playlist into a random order, keeping the current track
    /// selected. The order is not reproducible (seeded from the clock).
    pub fn randomize(&mut self) {
        let cur = self.current_path();
        // xorshift64 — a playlist shuffle needs no cryptographic quality.
        let mut s = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0x9E37_79B9_7F4A_7C15)
            | 1;
        let mut next = || {
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
            s
        };
        // Fisher-Yates.
        for i in (1..self.tracks.len()).rev() {
            let j = (next() % (i as u64 + 1)) as usize;
            self.tracks.swap(i, j);
        }
        self.reselect(cur);
    }

    /// Path of the currently-playing track, if any.
    fn current_path(&self) -> Option<PathBuf> {
        self.current.and_then(|i| self.tracks.get(i)).map(|t| t.path.clone())
    }

    /// Point `current` at the track with `path` after a reorder/removal.
    fn reselect(&mut self, path: Option<PathBuf>) {
        self.current = path.and_then(|p| self.tracks.iter().position(|t| t.path == p));
    }

    /// Index after `current`, honoring shuffle/repeat-all.
    pub fn next_index(&self, shuffle: bool, repeat_all: bool) -> Option<usize> {
        if self.tracks.is_empty() {
            return None;
        }
        if shuffle {
            return Some(random_other(self.tracks.len(), self.current));
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
            return Some(random_other(self.tracks.len(), self.current));
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
    // Decode leniently: a single non-UTF-8 byte (or a Latin-1 playlist) must not
    // make the whole file load as empty the way read_to_string would. Strip a
    // leading UTF-8 BOM so the first line still parses.
    let bytes = std::fs::read(path).unwrap_or_default();
    let text = String::from_utf8_lossy(&bytes);
    let content = text.strip_prefix('\u{feff}').unwrap_or(text.as_ref());
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

/// A random index in `0..len` that isn't `current`, so shuffle never replays the
/// current track back-to-back (classic Winamp behaviour). Falls back to any index
/// when there's no current track or only one track.
fn random_other(len: usize, current: Option<usize>) -> usize {
    match current {
        Some(c) if len > 1 && c < len => {
            let r = pseudo_random(len - 1); // 0..len-1, then skip over c
            if r >= c {
                r + 1
            } else {
                r
            }
        }
        _ => pseudo_random(len),
    }
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
    fn repair_mojibake_fixes_utf8_read_as_latin1() {
        // "Bằng Kiều" / "Những Ngày Đẹp Trời" encoded UTF-8 then (wrongly)
        // decoded as Latin-1 is what lofty hands back for these tags.
        for correct in ["Bằng Kiều", "Những Ngày Đẹp Trời", "Café del Mar", "日本語"] {
            let garbled: String = correct.bytes().map(|b| b as char).collect();
            assert_eq!(repair_mojibake(&garbled), correct, "should recover {correct:?}");
        }
    }

    #[test]
    fn repair_mojibake_leaves_clean_text_alone() {
        // Plain ASCII, already-correct Unicode, and real Latin-1 that doesn't
        // happen to form valid UTF-8 must pass through unchanged.
        for s in ["Hello World", "Bằng Kiều", "café", "naïve", "Mötley Crüe"] {
            assert_eq!(repair_mojibake(s), s, "should not alter {s:?}");
        }
    }

    #[test]
    fn lazy_track_uses_filename() {
        let t = Track::new(PathBuf::from("/music/Song.mp3"));
        assert!(!t.loaded);
        assert_eq!(t.title, "Song");
        assert_eq!(t.display_name(), "Song");
    }

    fn pl_of(paths: &[&str], current: Option<usize>) -> Playlist {
        Playlist {
            tracks: paths.iter().map(|p| Track::new(PathBuf::from(p))).collect(),
            current,
        }
    }

    #[test]
    fn reverse_keeps_current_track() {
        let mut pl = pl_of(&["/a.mp3", "/b.mp3", "/c.mp3"], Some(0));
        pl.reverse();
        assert_eq!(pl.tracks[0].path, PathBuf::from("/c.mp3"));
        assert_eq!(pl.tracks[2].path, PathBuf::from("/a.mp3"));
        assert_eq!(pl.current, Some(2)); // the once-first track is now last
    }

    #[test]
    fn randomize_is_a_permutation_keeping_current() {
        let paths: Vec<String> = (0..20).map(|i| format!("/{i}.mp3")).collect();
        let refs: Vec<&str> = paths.iter().map(String::as_str).collect();
        let mut pl = pl_of(&refs, Some(5));
        pl.randomize();
        assert_eq!(pl.tracks.len(), 20);
        let mut got: Vec<_> = pl.tracks.iter().map(|t| t.path.clone()).collect();
        got.sort();
        let mut want: Vec<_> = paths.iter().map(PathBuf::from).collect();
        want.sort();
        assert_eq!(got, want, "same set of tracks");
        assert_eq!(pl.tracks[pl.current.unwrap()].path, PathBuf::from("/5.mp3"));
    }

    #[test]
    fn remove_missing_drops_nonexistent_files() {
        let dir = std::env::temp_dir();
        let real = dir.join(format!("linamp_rm_{}.mp3", std::process::id()));
        std::fs::write(&real, b"x").unwrap();
        let gone = dir.join("linamp_rm_zzz_does_not_exist.mp3");
        let _ = std::fs::remove_file(&gone);
        let mut pl = Playlist {
            tracks: vec![Track::new(gone), Track::new(real.clone())],
            current: Some(1),
        };
        assert_eq!(pl.remove_missing(), 1);
        assert_eq!(pl.tracks.len(), 1);
        assert_eq!(pl.tracks[0].path, real);
        assert_eq!(pl.current, Some(0)); // current followed its surviving track
        let _ = std::fs::remove_file(&real);
    }

    #[test]
    fn parse_playlist_tolerates_bom_and_bad_bytes() {
        // A leading UTF-8 BOM plus a stray non-UTF-8 byte must not discard the
        // whole file; the valid entries still parse.
        let dir = std::env::temp_dir();
        let m3u = dir.join(format!("linamp_bom_{}.m3u", std::process::id()));
        let mut data = vec![0xEF, 0xBB, 0xBF]; // UTF-8 BOM
        data.extend_from_slice(b"#EXTM3U\n/music/a.mp3\n");
        data.push(0xFF); // invalid UTF-8 byte on its own line
        data.extend_from_slice(b"\n/music/b.mp3\n");
        std::fs::write(&m3u, &data).unwrap();

        let got = parse_playlist_file(&m3u);
        assert!(got.contains(&PathBuf::from("/music/a.mp3")), "got {got:?}");
        assert!(got.contains(&PathBuf::from("/music/b.mp3")), "got {got:?}");
        // The BOM must be stripped so the header is still treated as a comment,
        // not pushed as a bogus path.
        assert!(
            !got.iter().any(|p| p.to_string_lossy().contains("EXTM3U")),
            "header leaked as a path: {got:?}"
        );
        let _ = std::fs::remove_file(&m3u);
    }

    #[test]
    fn shuffle_never_repeats_current_track() {
        let pl = pl_of(&["/0.mp3", "/1.mp3", "/2.mp3", "/3.mp3"], Some(2));
        for _ in 0..64 {
            assert_ne!(pl.next_index(true, false), Some(2));
            assert_ne!(pl.prev_index(true, false), Some(2));
            assert!(pl.next_index(true, false).unwrap() < 4);
        }
        // A single-track list can only return that track.
        let one = pl_of(&["/only.mp3"], Some(0));
        assert_eq!(one.next_index(true, false), Some(0));
    }

    #[test]
    fn next_prev_index_navigation() {
        let pl = |cur| pl_of(&["/0.mp3", "/1.mp3", "/2.mp3"], cur);
        assert_eq!(pl(Some(0)).next_index(false, false), Some(1));
        assert_eq!(pl(Some(2)).next_index(false, false), None); // last, no repeat
        assert_eq!(pl(Some(2)).next_index(false, true), Some(0)); // last, repeat wraps
        assert_eq!(pl(None).next_index(false, false), Some(0)); // nothing playing
        assert_eq!(pl(Some(1)).prev_index(false, false), Some(0));
        assert_eq!(pl(Some(0)).prev_index(false, false), None); // first, no repeat
        assert_eq!(pl(Some(0)).prev_index(false, true), Some(2)); // first, repeat wraps
        let empty = Playlist { tracks: vec![], current: None };
        assert_eq!(empty.next_index(false, false), None);
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
