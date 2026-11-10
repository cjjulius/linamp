//! CUE sheets (`.cue`): split a single-file album (one big `.flac`/`.ape`/`.wv` plus a `.cue`) into
//! real per-track playlist entries. Parsing is the pure-Rust `cuna` crate (MIT). Each track becomes a
//! virtual playlist entry with a pseudo-path `cue://<start_ms>:<len_ms>/<abs-audio-path>` (the same
//! trick the audio-CD code uses with `cdda://`), carrying its title/performer/length from the sheet.
//! Playing one opens the underlying audio file through the normal decode dispatch, seeks to the track
//! start, and caps output to the track length - a thin [`SubTrackSource`] wrapper that also re-bases
//! `try_seek` so scrubbing within a track lands correctly.

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{anyhow, Result};
use cuna::Cuna;
use rodio::source::SeekError;
use rodio::Source;

/// A parsed CUE track, ready to become a playlist `Track`.
pub struct CueEntry {
    /// `cue://<start_ms>:<len_ms>/<abs-audio-path>` (len_ms 0 = to end of file).
    pub path: String,
    pub title: String,
    pub performer: String,
    pub album: String,
    pub duration: Option<Duration>,
}

pub fn is_cue(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| e.eq_ignore_ascii_case("cue"))
        .unwrap_or(false)
}

pub fn is_cue_path(s: &str) -> bool {
    s.starts_with("cue://")
}

/// `cue://<start_ms>:<len_ms>/<abs-path>`. The audio path is absolute (leading `/`), so the first `/`
/// after the times splits cleanly and the path keeps its leading slash.
fn make_cue_path(start_ms: u64, len_ms: u64, audio: &Path) -> String {
    format!("cue://{start_ms}:{len_ms}/{}", audio.display())
}

/// Parse a `cue://` pseudo-path into (start_ms, len_ms, audio_path).
pub fn parse_cue_path(s: &str) -> Option<(u64, u64, PathBuf)> {
    let rest = s.strip_prefix("cue://")?;
    let (times, path) = rest.split_once('/')?;
    let (start, len) = times.split_once(':')?;
    Some((start.trim().parse().ok()?, len.trim().parse().ok()?, PathBuf::from(path)))
}

/// A track's start time: INDEX 01 (the audio start), falling back to the first index present.
fn track_start(tk: &cuna::track::Track) -> Option<Duration> {
    let idx = tk.get_index(1).or_else(|| tk.index.first())?;
    Some(Duration::from(idx.begin_time()))
}

/// Expand a `.cue` into per-track entries. Resolves the `FILE` reference(s) relative to the `.cue`'s
/// own directory, computes each track's length as the gap to the next track's start (the last track of
/// a file runs to end-of-file, len 0).
pub fn expand(cue_path: &Path) -> Result<Vec<CueEntry>> {
    let sheet = Cuna::open(cue_path).map_err(|e| anyhow!("parse {}: {e}", cue_path.display()))?;
    let dir = cue_path.parent().unwrap_or_else(|| Path::new("."));
    let album = sheet.title().first().cloned().unwrap_or_default();
    let album_artist = sheet.performer().first().cloned().unwrap_or_default();

    let mut out = Vec::new();
    for disc in sheet.files() {
        let audio = dir.join(&disc.name);
        let audio = audio.canonicalize().unwrap_or(audio);
        let tracks = &disc.tracks;
        for (i, tk) in tracks.iter().enumerate() {
            let start = match track_start(tk) {
                Some(s) => s,
                None => continue, // a track with no index is unplayable; skip it
            };
            let next = tracks.get(i + 1).and_then(track_start);
            let len = next.map(|n| n.saturating_sub(start));
            let start_ms = start.as_millis() as u64;
            let len_ms = len.map(|d| d.as_millis() as u64).unwrap_or(0);
            let title = tk.title.first().cloned().unwrap_or_else(|| format!("Track {}", i + 1));
            let performer =
                tk.performer.first().cloned().unwrap_or_else(|| album_artist.clone());
            out.push(CueEntry {
                path: make_cue_path(start_ms, len_ms, &audio),
                title,
                performer,
                album: album.clone(),
                duration: len,
            });
        }
    }
    if out.is_empty() {
        return Err(anyhow!("no playable tracks in {}", cue_path.display()));
    }
    Ok(out)
}

/// Open a `cue://` virtual track: decode the underlying audio file through the normal dispatch, seek to
/// the track start, and cap it to the track length.
pub fn open(pseudo_path: &str) -> Result<SubTrackSource> {
    let (start_ms, len_ms, audio) =
        parse_cue_path(pseudo_path).ok_or_else(|| anyhow!("bad cue path: {pseudo_path}"))?;
    let inner = crate::player::open_source(&audio)?;
    let start = Duration::from_millis(start_ms);
    let len = if len_ms == 0 { None } else { Some(Duration::from_millis(len_ms)) };
    Ok(SubTrackSource::new(inner, start, len))
}

/// A rodio `Source` that plays a sub-range (offset + optional length) of another source: it seeks the
/// inner source to `start` on construction, stops after `len`, and re-bases `try_seek` so a scrub maps
/// to an absolute position inside the inner source.
pub struct SubTrackSource {
    inner: Box<dyn Source<Item = f32> + Send>,
    start: Duration,
    cap: Option<u64>, // max interleaved samples to emit (None = to end of inner)
    channels: u16,
    sample_rate: u32,
    emitted: u64,
}

impl SubTrackSource {
    fn new(mut inner: Box<dyn Source<Item = f32> + Send>, start: Duration, len: Option<Duration>) -> Self {
        let channels = inner.channels().max(1);
        let sample_rate = inner.sample_rate().max(1);
        let _ = inner.try_seek(start); // position to the track start (ignored if the inner can't seek)
        let cap = len.map(|d| {
            (d.as_secs_f64() * sample_rate as f64).round() as u64 * channels as u64
        });
        Self { inner, start, cap, channels, sample_rate, emitted: 0 }
    }
}

impl Iterator for SubTrackSource {
    type Item = f32;
    fn next(&mut self) -> Option<f32> {
        if let Some(cap) = self.cap {
            if self.emitted >= cap {
                return None;
            }
        }
        let s = self.inner.next()?;
        self.emitted += 1;
        Some(s)
    }
}

impl Source for SubTrackSource {
    fn current_frame_len(&self) -> Option<usize> {
        None
    }
    fn channels(&self) -> u16 {
        self.channels
    }
    fn sample_rate(&self) -> u32 {
        self.sample_rate
    }
    fn total_duration(&self) -> Option<Duration> {
        self.cap
            .map(|c| Duration::from_secs_f64(c as f64 / (self.sample_rate as f64 * self.channels as f64)))
    }
    fn try_seek(&mut self, pos: Duration) -> Result<(), SeekError> {
        self.inner.try_seek(self.start + pos)?;
        self.emitted =
            (pos.as_secs_f64() * self.sample_rate as f64).round() as u64 * self.channels as u64;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_cue_matches_the_extension() {
        assert!(is_cue(Path::new("album.cue")));
        assert!(is_cue(Path::new("ALBUM.CUE")));
        assert!(!is_cue(Path::new("album.flac")));
    }

    #[test]
    fn cue_pseudo_path_roundtrips_absolute_paths() {
        let p = make_cue_path(12_000, 180_500, Path::new("/music/My Album/cd.flac"));
        assert!(is_cue_path(&p));
        let (start, len, audio) = parse_cue_path(&p).unwrap();
        assert_eq!(start, 12_000);
        assert_eq!(len, 180_500);
        assert_eq!(audio, PathBuf::from("/music/My Album/cd.flac"));
    }

    /// End-to-end: expand a real CUE + single-file album and decode each virtual track, confirming it
    /// yields its own non-silent audio (SubTrackSource seeks + caps correctly). Uses the gitignored QA
    /// sample if present; skips on CI.
    #[test]
    fn decodes_real_cue_album_tracks() {
        let cue = std::path::Path::new("qa-music/format-samples/cue-test-album.cue");
        if !cue.exists() {
            return;
        }
        let entries = expand(cue).unwrap_or_else(|e| panic!("expand: {e}"));
        assert!(entries.len() >= 2, "expected multiple tracks, got {}", entries.len());
        for e in &entries {
            let src = open(&e.path).unwrap_or_else(|err| panic!("open {}: {err}", e.path));
            let ch = src.channels().max(1) as usize;
            let sr = src.sample_rate() as usize;
            // Decode up to ~2s of this track and confirm it is not silence.
            let got: Vec<f32> = src.take(sr * ch * 2).collect();
            assert!(got.len() > sr, "{}: too little audio", e.title);
            let peak = got.iter().fold(0.0f32, |m, &s| m.max(s.abs()));
            assert!(peak > 0.003, "{}: near-silence (peak {peak})", e.title);
        }
    }

    #[test]
    fn expands_a_single_file_album_cue() {
        // A minimal single-file-album sheet; the audio file need not exist to parse + expand.
        let dir = std::env::temp_dir().join(format!("linamp-cue-test-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let cue = dir.join("album.cue");
        std::fs::write(
            &cue,
            "PERFORMER \"The Band\"\nTITLE \"The Album\"\nFILE \"album.flac\" WAVE\n\
             \x20 TRACK 01 AUDIO\n    TITLE \"First\"\n    INDEX 01 00:00:00\n\
             \x20 TRACK 02 AUDIO\n    TITLE \"Second\"\n    INDEX 01 02:30:00\n",
        )
        .unwrap();
        let entries = expand(&cue).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].title, "First");
        assert_eq!(entries[0].album, "The Album");
        assert_eq!(entries[0].performer, "The Band");
        // First track length = 2:30 = 150s; second runs to end (len 0 -> None).
        assert_eq!(entries[0].duration, Some(Duration::from_secs(150)));
        assert_eq!(entries[1].duration, None);
        let (start, len, _) = parse_cue_path(&entries[1].path).unwrap();
        assert_eq!(start, 150_000);
        assert_eq!(len, 0);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
