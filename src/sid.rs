//! Commodore 64 SID music (.sid: PSID/RSID) via libsidplayfp (GPL-2.0+), behind the `sid` feature.
//! libsidplayfp emulates the C64 and its MOS6581/8580 SID chip with the reSIDfp engine. It is
//! C++-only, so build.rs compiles a small C++ shim (src/sid_shim.cpp) via the `cc` crate and we FFI
//! to that, the same pattern as the AdPlug side-decoder. One libsidplayfp build plays PSID and RSID.
//!
//! ROM-less: we never ship or set the Commodore KERNAL/BASIC/CHARGEN ROMs (they are copyrighted),
//! which plays most PSID and the majority of RSID tunes. The SID is a mono chip, so the shim renders
//! mono and duplicates it to both channels. SID tunes loop forever and carry no length (the length
//! database, HVSC Songlengths, is not freely licensed), so we render a fixed default length with a
//! fade-out, mirroring the GME chiptune behavior, and play the default subsong (subsong selection in
//! the UI is a later enhancement). Rendered whole to a seekable buffer, like the other side-decoders.

use std::ffi::CString;
use std::os::raw::{c_char, c_int};
use std::path::Path;
use std::sync::Mutex;
use std::time::Duration;

use anyhow::{anyhow, Result};
use rodio::source::SeekError;
use rodio::Source;

const OUT_RATE: u32 = 44_100;
const OUT_CHANNELS: u16 = 2;
/// How long to render a tune that declares no length (SID tunes loop endlessly). Matches the GME
/// default so chip music behaves consistently.
const DEFAULT_PLAY_SECS: u32 = 150;
/// Fade-out applied over the end of the render so a looping tune stops cleanly.
const FADE_SECS: f32 = 4.0;
const CHUNK_FRAMES: usize = 8192;

/// Extensions routed to libsidplayfp. `.sid` is the near-universal container; `.psid`/`.rsid` are
/// rare explicit spellings of the same formats.
pub const SID_EXTS: &[&str] = &["sid", "psid", "rsid"];

pub fn is_sid(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| SID_EXTS.iter().any(|x| e.eq_ignore_ascii_case(x)))
        .unwrap_or(false)
}

#[repr(C)]
struct SidHandle {
    _private: [u8; 0],
}

extern "C" {
    fn sid_open(path: *const c_char, rate: c_int) -> *mut SidHandle;
    fn sid_render(h: *mut SidHandle, out: *mut f32, frames: c_int) -> c_int;
    fn sid_close(h: *mut SidHandle);
}

// libsidplayfp has process-global state (shared chip tables / lazy init) that is not thread-safe, so
// two tunes opening or rendering on different threads can crash. LinAmp only ever renders one source
// at a time (a single mixer thread, even across a crossfade), so this lock is uncontended in
// practice; it just makes the library safe when the test harness exercises several tunes in parallel.
static SID_LOCK: Mutex<()> = Mutex::new(());

/// A seekable rodio `Source` that streams a SID tune on demand and caches what it renders. Unlike a
/// fully-buffered decoder this does NOT render the whole tune up front: SID emulation runs at ~25x
/// real time, so rendering the fixed default length (~150s) in `open()` would block for several
/// seconds and freeze the UI (the slow start / unresponsive scrub). Instead it renders one chunk at
/// a time as playback (or a forward seek) needs it, appending to a growing cache so start is instant,
/// backward/replayed scrubbing is instant, and a forward seek only pays for the gap it skips over.
pub struct SidSource {
    h: *mut SidHandle, // live engine; destroyed on Drop
    cache: Vec<f32>,   // interleaved stereo, everything rendered so far (raw, fade applied on read)
    rendered_frames: usize,
    pos: usize, // interleaved index
    cap_frames: usize,
    fade_frames: usize,
    total: Option<Duration>,
    ended: bool, // the engine stopped before the cap
}

// The libsidplayfp handle is only ever touched from the single thread that owns the source (the
// audio thread, after it is moved there), never shared, so it is safe to send.
unsafe impl Send for SidSource {}

impl Drop for SidSource {
    fn drop(&mut self) {
        let _guard = SID_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        unsafe { sid_close(self.h) };
    }
}

impl SidSource {
    /// Render one more chunk from the live engine into the cache. Returns false at the cap or when
    /// the engine stops producing audio.
    fn render_more(&mut self) -> bool {
        if self.ended || self.rendered_frames >= self.cap_frames {
            return false;
        }
        let mut tmp = [0.0f32; CHUNK_FRAMES * OUT_CHANNELS as usize];
        let got = {
            let _guard = SID_LOCK.lock().unwrap_or_else(|e| e.into_inner());
            unsafe { sid_render(self.h, tmp.as_mut_ptr(), CHUNK_FRAMES as c_int) }
        };
        if got <= 0 {
            self.ended = true;
            return false;
        }
        let got = got as usize;
        self.cache.extend_from_slice(&tmp[..got * OUT_CHANNELS as usize]);
        self.rendered_frames += got;
        if got < CHUNK_FRAMES {
            self.ended = true;
        }
        true
    }

    /// Fade gain for a frame near the cap, so an endlessly-looping tune ends cleanly.
    fn fade_gain(&self, frame: usize) -> f32 {
        let fade_start = self.cap_frames.saturating_sub(self.fade_frames);
        if self.fade_frames == 0 || frame < fade_start {
            1.0
        } else {
            self.cap_frames.saturating_sub(frame) as f32 / self.fade_frames as f32
        }
    }
}

impl Iterator for SidSource {
    type Item = f32;
    fn next(&mut self) -> Option<f32> {
        if self.pos >= self.rendered_frames * OUT_CHANNELS as usize && !self.render_more() {
            return None; // hit the cap or the engine ended
        }
        let s = *self.cache.get(self.pos)?;
        let frame = self.pos / OUT_CHANNELS as usize;
        self.pos += 1;
        Some(s * self.fade_gain(frame))
    }
}

impl Source for SidSource {
    fn current_frame_len(&self) -> Option<usize> {
        None
    }
    fn channels(&self) -> u16 {
        OUT_CHANNELS
    }
    fn sample_rate(&self) -> u32 {
        OUT_RATE
    }
    fn total_duration(&self) -> Option<Duration> {
        self.total
    }
    fn try_seek(&mut self, pos: Duration) -> Result<(), SeekError> {
        let target = ((pos.as_secs_f64() * OUT_RATE as f64).round() as usize).min(self.cap_frames);
        // A backward seek (or into already-rendered audio) is an instant index; a forward seek past
        // the rendered edge renders the gap first, since a SID cannot be randomly seeked.
        while self.rendered_frames < target && self.render_more() {}
        self.pos = (target * OUT_CHANNELS as usize).min(self.cache.len());
        Ok(())
    }
}

/// Load a SID tune and expose its default subsong (ROM-less) as a lazy, seekable stereo source.
/// Rendering happens on demand, so this is cheap. Errors cleanly when the file is not a valid SID
/// tune, so the skip-unplayable path takes over.
pub fn open(path: &Path) -> Result<SidSource> {
    let cpath = CString::new(path.as_os_str().to_string_lossy().as_bytes())
        .map_err(|_| anyhow!("path has an interior NUL: {}", path.display()))?;
    let h = {
        let _guard = SID_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        unsafe { sid_open(cpath.as_ptr(), OUT_RATE as c_int) }
    };
    if h.is_null() {
        return Err(anyhow!("libsidplayfp could not load {}", path.display()));
    }
    Ok(SidSource {
        h,
        cache: Vec::new(),
        rendered_frames: 0,
        pos: 0,
        cap_frames: DEFAULT_PLAY_SECS as usize * OUT_RATE as usize,
        fade_frames: (FADE_SECS * OUT_RATE as f32) as usize,
        total: Some(Duration::from_secs(DEFAULT_PLAY_SECS as u64)),
        ended: false,
    })
}

/// Length of a SID tune for the playlist. SID tunes carry no length and loop forever, so this is the
/// same fixed default the render uses (no free song-length database exists). `None` when the file
/// isn't a SID.
pub fn duration(path: &Path) -> Option<Duration> {
    is_sid(path).then(|| Duration::from_secs(DEFAULT_PLAY_SECS as u64))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_sid_matches_the_sid_extensions() {
        for e in ["sid", "SID", "psid", "rsid"] {
            assert!(is_sid(&std::path::PathBuf::from(format!("x.{e}"))), ".{e}");
        }
        assert!(!is_sid(std::path::Path::new("x.mp3")));
        assert!(!is_sid(std::path::Path::new("noext")));
    }

    #[test]
    fn open_errors_cleanly_on_non_sid() {
        let p = std::env::temp_dir().join("linamp_not_a_sid.sid");
        std::fs::write(&p, b"this is definitely not a SID tune").unwrap();
        assert!(open(&p).is_err());
        let _ = std::fs::remove_file(&p);
    }

    /// Render every SID sample present and confirm it produces non-silent stereo audio, ROM-less.
    /// libsidplayfp is the only decoder, so this is a decode/render smoke test. (qa-music is
    /// gitignored, so it skips on CI.)
    #[test]
    fn renders_real_sid_tunes() {
        let dir = std::path::Path::new("qa-music/format-samples");
        if !dir.exists() {
            return;
        }
        let mut checked = 0;
        for entry in std::fs::read_dir(dir).unwrap().flatten() {
            let path = entry.path();
            if !is_sid(&path) {
                continue;
            }
            let src = open(&path).unwrap_or_else(|e| panic!("open {}: {e}", path.display()));
            assert_eq!(src.channels(), 2, "{}: stereo", path.display());
            assert_eq!(src.sample_rate(), 44_100, "{}: 44.1k", path.display());
            let got: Vec<f32> = src.take(44_100 * 2 * 10).collect();
            assert!(got.len() > 44_100, "{}: too little audio", path.display());
            let peak = got.iter().fold(0.0f32, |m, &s| m.max(s.abs()));
            assert!(peak > 0.003, "{}: near-silence (peak {peak})", path.display());
            checked += 1;
        }
        assert!(checked > 0, "no SID samples found to render");
    }

    /// Seeking the streaming source: a forward seek renders the gap and lands near the target, a
    /// backward seek is instant (served from the cache), and total_duration is the shown length so
    /// the scrub bar maps to the buffer. Local sample only.
    #[test]
    fn seeking_forward_then_back_produces_audio() {
        let p = std::path::Path::new("qa-music/format-samples/sid-sample.sid");
        if !p.exists() {
            return;
        }
        let mut src = open(p).expect("load sid");
        assert_eq!(src.total_duration(), Some(Duration::from_secs(DEFAULT_PLAY_SECS as u64)));
        // Forward seek to 20s, then read ~0.5s: must be audible (the gap was rendered).
        src.try_seek(Duration::from_secs(20)).unwrap();
        let fwd: Vec<f32> = (&mut src).take(44_100).collect();
        assert!(fwd.len() > 1000, "no audio after forward seek");
        assert!(fwd.iter().any(|&s| s.abs() > 0.003), "silence after forward seek");
        // Backward seek to 2s (into already-rendered audio) is instant and still audible.
        src.try_seek(Duration::from_secs(2)).unwrap();
        let back: Vec<f32> = (&mut src).take(44_100).collect();
        assert!(back.iter().any(|&s| s.abs() > 0.003), "silence after backward seek");
    }
}
