use std::path::{Path, PathBuf};
use std::time::Duration;

use walkdir::WalkDir;

/// Supported audio file extensions.
pub const SUPPORTED_EXTS: &[&str] = &[
    "mp3", "flac", "ogg", "oga", "wav", "m4a", "aac", "aiff", "aif",
    // Not Opus or WavPack: symphonia 0.5 ships no decoder for either, so listing
    // them would accept the file then fail at play time (decode::tests guards this).
    // MPEG Layer I/II and the rest of the ISO-MP4 family, decoded via a symphonia
    // hint in crate::decode (rodio 0.20 can't open these directly) (#157/#164).
    "mp1", "mp2", "mpa", "mp4", "m4b",
    // MIDI, synthesized via crate::midi (rustysynth + a bundled SoundFont) (#159).
    "mid", "midi",
    // Tracker modules, rendered via crate::tracker (xmrs/xmrsplayer, pure Rust) (#158).
    "mod", "xm", "s3m", "it",
];

/// Repair "mojibake": text that is really UTF-8 but was decoded as Latin-1. This
/// happens with ID3v2 frames whose encoding byte says ISO-8859-1 while the bytes
/// are actually UTF-8 (common in the wild, example: zing.vn MP3s), so an accented
/// title like "Bằng Kiều" comes back as "Bá»±ng Kiá»u".
///
/// When every char is U+00..=U+FF the original bytes are recoverable: map each
/// char back to its byte and re-decode as UTF-8. Only used when that yields valid
/// UTF-8 that actually changed, so correctly-tagged text (and plain ASCII/Latin-1
/// that isn't disguised UTF-8) is left untouched. Idempotent, repaired text has
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
        // Formats lofty can't read (MIDI, tracker modules) still have a known
        // length; fill it in so the playlist shows a time for them too.
        if self.duration.is_none() {
            self.duration = crate::midi::duration(&self.path)
                .or_else(|| crate::tracker::duration(&self.path));
        }
    }

    /// Build a fully-loaded track (tags read immediately).
    pub fn from_path(path: PathBuf) -> Self {
        let mut t = Track::new(path);
        t.load();
        t
    }

    /// A playlist entry for an audio-CD track (#160). There is no file on disk, so
    /// the title and length come from the disc's table of contents and it is marked
    /// loaded (nothing to read lazily). The path is the `cdda://<device>/<track>`
    /// pseudo-path the player recognises. Linux-only in this build (the only caller,
    /// the Open-Audio-CD action, is Linux-gated).
    #[cfg(target_os = "linux")]
    pub fn cd_track(device: &str, t: &crate::cdda::CdTrack) -> Track {
        Track {
            path: PathBuf::from(crate::cdda::track_path(device, t.number)),
            title: format!("Track {}", t.number),
            artist: String::new(),
            album: "Audio CD".to_string(),
            duration: Some(t.duration()),
            sample_rate: Some(crate::cdda::CDDA_SAMPLE_RATE),
            // CDDA is a constant 1411 kbit/s (44100 × 16 × 2).
            bitrate: Some(1411),
            channels: Some(crate::cdda::CDDA_CHANNELS as u8),
            loaded: true,
        }
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
    /// Shuffle "bag": a shuffled permutation of track indices and the position
    /// of the next entry to play, so shuffle plays every track once per cycle
    /// (classic Winamp behaviour) rather than picking at random with replacement.
    /// Rebuilt lazily when the track count changes or the cycle is exhausted.
    shuffle_order: Vec<usize>,
    shuffle_pos: usize,
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

    /// After appending `count` tracks (now the last `count` in the list), move them
    /// to position `at`, so a drag-drop lands where it was released instead of only
    /// at the end. `at` is clamped to the pre-existing length; the now-playing cursor
    /// is kept on the same track. Returns the effective insertion index when it
    /// moved (so the caller can shift row-based state like the selection), or `None`
    /// when `at` is already the end (plain append, nothing moved).
    pub fn move_tail_to(&mut self, count: usize, at: usize) -> Option<usize> {
        let len = self.tracks.len();
        if count == 0 || count > len {
            return None;
        }
        let before = len - count; // first index of the appended block
        let at = at.min(before);
        if at == before {
            return None; // dropped at/after the end: it's already there
        }
        // Rotate the sub-slice [at..] right by `count` so the appended block (its
        // last `count` elements) moves to `at` and the rest shifts down.
        self.tracks[at..].rotate_right(count);
        if let Some(cur) = self.current {
            if cur >= at && cur < before {
                self.current = Some(cur + count);
            }
        }
        Some(at)
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

    /// Target index for a relative "jump N tracks", classic Winamp's numpad
    /// 1 and 3, which skip 10 songs back and forward. Counts from the current
    /// track (or the top when nothing is playing), clamps at the list ends and
    /// does not wrap. `None` for an empty list.
    pub fn jump_target(&self, delta: i32) -> Option<usize> {
        let len = self.tracks.len();
        if len == 0 {
            return None;
        }
        let base = self.current.unwrap_or(0) as i32;
        Some((base + delta).clamp(0, len as i32 - 1) as usize)
    }

    /// Shuffle the playlist into a random order, keeping the current track
    /// selected. The order is not reproducible (seeded from the clock).
    pub fn randomize(&mut self) {
        let cur = self.current_path();
        // xorshift64, a playlist shuffle needs no cryptographic quality.
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
    pub fn next_index(&mut self, shuffle: bool, repeat_all: bool) -> Option<usize> {
        if self.tracks.is_empty() {
            return None;
        }
        if shuffle {
            return self.next_shuffle();
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
    pub fn prev_index(&mut self, shuffle: bool, repeat_all: bool) -> Option<usize> {
        if self.tracks.is_empty() {
            return None;
        }
        if shuffle {
            return self.prev_shuffle();
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

    /// Next track from the shuffle bag, each track plays once per cycle; a new
    /// shuffled cycle starts when the current one is exhausted, never repeating
    /// the just-played track back-to-back.
    fn next_shuffle(&mut self) -> Option<usize> {
        let len = self.tracks.len();
        if len == 1 {
            return Some(0);
        }
        if self.shuffle_order.len() != len || self.shuffle_pos >= self.shuffle_order.len() {
            self.refill_shuffle();
        }
        let idx = self.shuffle_order[self.shuffle_pos];
        self.shuffle_pos += 1;
        Some(idx)
    }

    /// Step back through the shuffle bag (the previously-played entry).
    fn prev_shuffle(&mut self) -> Option<usize> {
        let len = self.tracks.len();
        if len == 1 {
            return Some(0);
        }
        if self.shuffle_order.len() != len {
            self.refill_shuffle();
        }
        if self.shuffle_pos > 1 {
            self.shuffle_pos -= 1;
            Some(self.shuffle_order[self.shuffle_pos - 1])
        } else {
            Some(*self.shuffle_order.first().unwrap_or(&0))
        }
    }

    /// Rebuild the shuffle bag: a fresh random permutation of all track indices,
    /// arranged so the first entry isn't the current track (no back-to-back repeat
    /// across cycle boundaries). Clock-seeded; a playlist shuffle needs no
    /// cryptographic quality.
    fn refill_shuffle(&mut self) {
        let len = self.tracks.len();
        let mut order: Vec<usize> = (0..len).collect();
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
        for i in (1..len).rev() {
            let j = (next() % (i as u64 + 1)) as usize;
            order.swap(i, j);
        }
        // Avoid immediately replaying the current track at the start of a new cycle.
        if len > 1 {
            if let Some(c) = self.current {
                if order[0] == c {
                    order.swap(0, 1);
                }
            }
        }
        self.shuffle_order = order;
        self.shuffle_pos = 0;
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
        // PLS entries are keyed "File<N>=path"; the number N defines the order,
        // not the line position, and only keys that are actually File<N> count
        // (so a stray "FileFoo=" is ignored rather than ingested as a track).
        let mut entries: Vec<(u32, PathBuf)> = Vec::new();
        for line in content.lines() {
            let line = line.trim();
            if let Some(rest) = line.strip_prefix("File") {
                if let Some(eq) = rest.find('=') {
                    if let Ok(n) = rest[..eq].trim().parse::<u32>() {
                        entries.push((n, resolve(base, rest[eq + 1..].trim())));
                    }
                }
            }
        }
        entries.sort_by_key(|(n, _)| *n);
        out.extend(entries.into_iter().map(|(_, p)| p));
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
    // A native-absolute entry is used as-is (a Windows "C:\..." path has no
    // meaning here anyway and is left untouched).
    let p = PathBuf::from(value);
    if p.is_absolute() {
        return p;
    }
    // Classic playlists are overwhelmingly Windows-authored, where relative
    // entries use backslash separators (example: "Music\Song.mp3"). Treat those as
    // path separators so the playlist resolves on Linux too.
    let p = PathBuf::from(value.replace('\\', "/"));
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
    fn track_load_fills_midi_length_via_the_fallback() {
        // lofty can't read MIDI, so Track::load must fall back to midi::duration
        // and show a time in the playlist (no SoundFont needed for the length).
        let track: [u8; 12] = [
            0x00, 0x90, 0x3C, 0x64, 0x60, 0x80, 0x3C, 0x00, 0x00, 0xFF, 0x2F, 0x00,
        ];
        let mut mid = Vec::new();
        mid.extend_from_slice(b"MThd");
        mid.extend_from_slice(&6u32.to_be_bytes());
        mid.extend_from_slice(&0u16.to_be_bytes());
        mid.extend_from_slice(&1u16.to_be_bytes());
        mid.extend_from_slice(&96u16.to_be_bytes());
        mid.extend_from_slice(b"MTrk");
        mid.extend_from_slice(&(track.len() as u32).to_be_bytes());
        mid.extend_from_slice(&track);
        let p = std::env::temp_dir().join("linamp_playlist_midi_len.mid");
        std::fs::write(&p, &mid).unwrap();

        let t = Track::from_path(p.clone());
        assert!(t.duration.is_some(), "MIDI track should have a length in the playlist");
        let _ = std::fs::remove_file(&p);
    }

    fn names(pl: &Playlist) -> Vec<String> {
        pl.tracks.iter().map(|t| t.path.to_string_lossy().into_owned()).collect()
    }

    #[test]
    fn move_tail_to_inserts_dropped_block_at_the_row() {
        // Drag-drop: files are appended, then moved to the drop row.
        let mut pl = Playlist::default();
        for n in ["a.mp3", "b.mp3", "c.mp3"] {
            pl.add_file(PathBuf::from(n));
        }
        pl.current = Some(2); // playing "c"
        pl.add_file(PathBuf::from("x.mp3"));
        pl.add_file(PathBuf::from("y.mp3"));
        assert_eq!(pl.move_tail_to(2, 1), Some(1)); // dropped between a and b
        assert_eq!(names(&pl), ["a.mp3", "x.mp3", "y.mp3", "b.mp3", "c.mp3"]);
        assert_eq!(pl.current, Some(4), "now-playing track stays the same one");
    }

    #[test]
    fn move_tail_to_at_the_top_and_past_the_end() {
        let mut pl = Playlist::default();
        for n in ["a.mp3", "b.mp3"] {
            pl.add_file(PathBuf::from(n));
        }
        pl.add_file(PathBuf::from("x.mp3"));
        assert_eq!(pl.move_tail_to(1, 0), Some(0)); // drop at the very top
        assert_eq!(names(&pl), ["x.mp3", "a.mp3", "b.mp3"]);
        // Dropping past the end just appends (clamped, no move).
        pl.add_file(PathBuf::from("z.mp3"));
        assert_eq!(pl.move_tail_to(1, 99), None);
        assert_eq!(names(&pl), ["x.mp3", "a.mp3", "b.mp3", "z.mp3"]);
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
        // Plain ASCII, already-correct Unicode and real Latin-1 that doesn't
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
            ..Default::default()
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
            ..Default::default()
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
        data.extend_from_slice(b"#EXTM3U\na.mp3\n");
        data.push(0xFF); // invalid UTF-8 byte on its own line
        data.extend_from_slice(b"\nb.mp3\n");
        std::fs::write(&m3u, &data).unwrap();

        // Assert on file names, not full paths: a bare "/music/a.mp3" is absolute
        // on Unix but drive-relative on Windows, so it resolves differently per
        // platform. Relative basenames resolve the same on both, keeping this a
        // test of BOM/bad-byte tolerance rather than path syntax.
        let got = parse_playlist_file(&m3u);
        let names: Vec<String> = got
            .iter()
            .filter_map(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
            .collect();
        assert!(names.iter().any(|n| n == "a.mp3"), "got {got:?}");
        assert!(names.iter().any(|n| n == "b.mp3"), "got {got:?}");
        // The BOM must be stripped so the header is still treated as a comment,
        // not pushed as a bogus path.
        assert!(
            !got.iter().any(|p| p.to_string_lossy().contains("EXTM3U")),
            "header leaked as a path: {got:?}"
        );
        let _ = std::fs::remove_file(&m3u);
    }

    #[test]
    fn parse_pls_orders_by_index_and_ignores_non_file_keys() {
        let dir = std::env::temp_dir();
        let pls = dir.join(format!("linamp_pls_{}.pls", std::process::id()));
        // Entries out of line order, plus a decoy key that starts with "File".
        let body = "[playlist]\n\
                    NumberOfEntries=3\n\
                    File2=b.mp3\n\
                    FileFoo=decoy.mp3\n\
                    File10=j.mp3\n\
                    File1=a.mp3\n\
                    Version=2\n";
        std::fs::write(&pls, body).unwrap();

        // Assert on file names, not full paths, so the expectation holds on both
        // platforms (a bare "/music/x" is absolute on Unix, drive-relative on
        // Windows). What's under test is the order (by N: 1, 2, 10) and that the
        // non-File<N> "decoy" key is dropped.
        let got = parse_playlist_file(&pls);
        let names: Vec<String> = got
            .iter()
            .filter_map(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
            .collect();
        assert_eq!(names, vec!["a.mp3", "b.mp3", "j.mp3"]);
        let _ = std::fs::remove_file(&pls);
    }

    #[test]
    fn resolve_handles_windows_relative_separators() {
        let base = PathBuf::from("/pl");
        // Windows-style relative entries resolve against the playlist directory.
        assert_eq!(
            resolve(Some(&base), "Music\\Song.mp3"),
            PathBuf::from("/pl/Music/Song.mp3")
        );
        assert_eq!(
            resolve(Some(&base), "sub\\dir\\x.mp3"),
            PathBuf::from("/pl/sub/dir/x.mp3")
        );
        // Forward-slash relative entries are unaffected.
        assert_eq!(
            resolve(Some(&base), "a/b.mp3"),
            PathBuf::from("/pl/a/b.mp3")
        );
        // Native-absolute entries are passed through untouched.
        assert_eq!(
            resolve(Some(&base), "/abs/x.mp3"),
            PathBuf::from("/abs/x.mp3")
        );
    }

    #[test]
    fn shuffle_plays_through_all_before_repeating() {
        let mut pl = pl_of(&["/0.mp3", "/1.mp3", "/2.mp3", "/3.mp3"], Some(2));
        // The first pick must not be the current track (no back-to-back repeat).
        let first = pl.next_index(true, false).unwrap();
        assert_ne!(first, 2, "shuffle must not replay the current track immediately");
        // One cycle (the remaining 3) must cover every other index exactly once,
        // ex: all 4 tracks play once before any repeats.
        let mut seen = vec![first];
        for _ in 0..3 {
            let i = pl.next_index(true, false).unwrap();
            pl.current = Some(i);
            seen.push(i);
        }
        seen.sort();
        assert_eq!(seen, vec![0, 1, 2, 3], "every track plays once per shuffle cycle");
        // All indices stay in range across many cycles.
        for _ in 0..64 {
            let i = pl.next_index(true, false).unwrap();
            pl.current = Some(i);
            assert!(i < 4);
        }
        // A single-track list can only return that track.
        let mut one = pl_of(&["/only.mp3"], Some(0));
        assert_eq!(one.next_index(true, false), Some(0));
    }

    #[test]
    fn jump_target_clamps_without_wrapping() {
        let pl = pl_of(&["/0.mp3", "/1.mp3", "/2.mp3", "/3.mp3", "/4.mp3"], Some(1));
        assert_eq!(pl.jump_target(2), Some(3));
        assert_eq!(pl.jump_target(10), Some(4)); // clamps at the end, no wrap
        assert_eq!(pl.jump_target(-10), Some(0)); // clamps at the start, no wrap
        assert_eq!(pl.jump_target(0), Some(1));
        // Nothing playing yet counts from the top of the list.
        let none = pl_of(&["/0.mp3", "/1.mp3"], None);
        assert_eq!(none.jump_target(1), Some(1));
        assert_eq!(none.jump_target(-1), Some(0));
        // An empty list has no target.
        assert_eq!(Playlist::default().jump_target(5), None);
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
        let mut empty = Playlist { tracks: vec![], current: None, ..Default::default() };
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
