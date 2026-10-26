//! Tracker module playback (.mod/.xm/.s3m/.it) via xmrs + xmrsplayer (#158).
//!
//! Classic Winamp played tracker modules natively. We load the module with xmrs
//! and render it with xmrsplayer, both pure Rust, so unlike libopenmpt this adds
//! no C build dependency and compiles everywhere. The player borrows the module,
//! so a `self_cell` holds the two together and we expose them as a lazy, seekable
//! rodio `Source` that renders on demand (no up-front decode of the whole song).

use std::path::Path;
use std::time::Duration;

use anyhow::{anyhow, Result};
use rodio::source::SeekError;
use rodio::Source;
use xmrs::prelude::Module;
use xmrsplayer::xmrsplayer::XmrsPlayer;

/// Render rate; the rodio mixer resamples to the output device.
const SAMPLE_RATE: u32 = 44_100;

/// Tracker extensions handled here (pure-Rust formats via xmrs).
pub const TRACKER_EXTS: &[&str] = &["mod", "xm", "s3m", "it"];

/// True when `path` is a tracker module routed through this module.
pub fn is_tracker(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| TRACKER_EXTS.contains(&e.to_ascii_lowercase().as_str()))
        .unwrap_or(false)
}

// The player borrows the module; keep both alive together. not_covariant keeps it
// simple, we only call methods that return owned values (next / seek / duration).
self_cell::self_cell!(
    struct ModuleCell {
        owner: Module,
        #[not_covariant]
        dependent: XmrsPlayer,
    }
);

/// A lazy, seekable rodio `Source` that renders a tracker module on demand.
pub struct TrackerSource {
    cell: ModuleCell,
    total: Option<Duration>,
    /// Interleaved samples emitted since the start (2 per frame).
    samples_out: u64,
    /// Hard stop at the reported length (interleaved samples). xmrsplayer plays a
    /// module's song length but then follows its loop/jump effects a little further
    /// before `set_max_loop_count(1)` halts it, so the audio ran past the time the
    /// playlist shows. Capping at the reported duration keeps playback == the shown
    /// length (and stops at the natural pattern end, where a loop would restart).
    cap_samples: Option<u64>,
}

impl Iterator for TrackerSource {
    type Item = f32;

    fn next(&mut self) -> Option<f32> {
        // Stop once we've emitted the reported duration, so a looping module doesn't
        // play past the time shown in the playlist.
        if let Some(cap) = self.cap_samples {
            if self.samples_out >= cap {
                return None;
            }
        }
        // xmrsplayer yields interleaved stereo i16 one sample per call; it also ends
        // (returns None) after one pass via set_max_loop_count(1) below.
        let s = self
            .cell
            .with_dependent_mut(|_owner, player| player.next())
            .map(|s| s as f32 / 32768.0)?;
        self.samples_out += 1;
        Some(s)
    }
}

impl Source for TrackerSource {
    fn current_frame_len(&self) -> Option<usize> {
        None
    }
    fn channels(&self) -> u16 {
        2
    }
    fn sample_rate(&self) -> u32 {
        SAMPLE_RATE
    }
    fn total_duration(&self) -> Option<Duration> {
        self.total
    }
    fn try_seek(&mut self, pos: Duration) -> Result<(), SeekError> {
        let ok = self
            .cell
            .with_dependent_mut(|_owner, player| player.seek_seconds(pos.as_secs_f64()));
        if ok {
            // Keep the cap relative to the new position (2 samples per frame).
            self.samples_out = (pos.as_secs_f64() * SAMPLE_RATE as f64 * 2.0) as u64;
            Ok(())
        } else {
            Err(SeekError::NotSupported {
                underlying_source: "tracker",
            })
        }
    }
}

/// The length of a tracker module, so the playlist can show a time for it (lofty
/// doesn't read these). `None` when it isn't a module or can't be loaded.
pub fn duration(path: &Path) -> Option<Duration> {
    if !is_tracker(path) {
        return None;
    }
    open_tracker(path).ok().and_then(|s| s.total_duration())
}

/// Load a tracker module and expose it as a lazy, seekable source. Errors cleanly
/// on an unrecognized or corrupt module (caught by the skip-unplayable guard).
pub fn open_tracker(path: &Path) -> Result<TrackerSource> {
    let data = std::fs::read(path)?;
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let module = match ext.as_str() {
        "mod" => Module::load_mod(&data),
        "xm" => Module::load_xm(&data),
        "s3m" => Module::load_s3m(&data),
        "it" => Module::load_it(&data),
        other => return Err(anyhow!("not a tracker module: .{other}")),
    }
    .map_err(|e| anyhow!("couldn't load {}: {e:?}", path.display()))?;

    let cell = ModuleCell::new(module, |m| {
        let mut p = XmrsPlayer::new(m, SAMPLE_RATE, 0);
        p.set_max_loop_count(1); // play once, so the source ends for auto-advance
        p
    });
    let secs = cell.with_dependent(|_owner, player| player.duration_seconds());
    let total = (secs > 0.0).then(|| Duration::from_secs_f64(secs));
    // Interleaved-sample cap at the reported length (2 samples per frame). None when
    // the module reports no length, so it just plays to its natural end.
    let cap_samples = total.map(|d| (d.as_secs_f64() * SAMPLE_RATE as f64 * 2.0) as u64);
    Ok(TrackerSource { cell, total, samples_out: 0, cap_samples })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_tracker_matches_the_module_extensions() {
        for p in ["song.mod", "SONG.XM", "tune.s3m", "x.it"] {
            assert!(is_tracker(Path::new(p)), "{p} should be a tracker");
        }
        assert!(!is_tracker(Path::new("song.mp3")));
        assert!(!is_tracker(Path::new("song.mid")));
    }

    #[test]
    fn open_tracker_errors_cleanly_on_non_module() {
        // Garbage bytes written to a .mod: a clean error, not a panic.
        let p = std::env::temp_dir().join("linamp_not_a_module.mod");
        std::fs::write(&p, b"this is definitely not a tracker module").unwrap();
        assert!(open_tracker(&p).is_err());
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn open_tracker_renders_a_module() {
        // End-to-end render check against the xmrs example module, when present
        // (qa-music is local/gitignored, so this skips on a bare checkout/CI).
        let p = Path::new("qa-music/note.xm");
        if !p.exists() {
            return;
        }
        let mut src = open_tracker(p).expect("should load the .xm");
        assert_eq!(src.channels(), 2);
        assert_eq!(src.sample_rate(), SAMPLE_RATE);
        let out: Vec<f32> = (&mut src).take(8192).collect();
        assert!(!out.is_empty(), "tracker render produced no samples");
    }

    #[test]
    fn playback_stops_at_the_reported_duration() {
        // The user-facing guarantee: a module plays for exactly the time the playlist
        // shows, never past it (looping modules used to run on past the reported
        // length). Uses the local sample when present.
        let p = Path::new("qa-music/Generated_tests/sample_scale.mod");
        if !p.exists() {
            return;
        }
        let mut src = open_tracker(p).expect("should load the .mod");
        let reported = src.total_duration().expect("has a duration");
        let frames: usize = (&mut src).count() / 2; // emitted stereo frames
        let played = frames as f64 / SAMPLE_RATE as f64;
        // Actual playback matches the reported length within a few milliseconds
        // (never longer, that was the bug).
        let diff = played - reported.as_secs_f64();
        assert!(
            diff <= 0.05 && diff >= -0.05,
            "played {played:.3}s vs reported {:.3}s (must match, never over)",
            reported.as_secs_f64()
        );
    }
}
