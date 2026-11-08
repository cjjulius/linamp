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

/// A seekable rodio `Source` over a fully-rendered SID tune (interleaved f32, stereo).
pub struct SidSource {
    samples: Vec<f32>,
    pos: usize,
    total: Option<Duration>,
}

impl Iterator for SidSource {
    type Item = f32;
    fn next(&mut self) -> Option<f32> {
        let s = self.samples.get(self.pos).copied();
        if s.is_some() {
            self.pos += 1;
        }
        s
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
        let frame = (pos.as_secs_f64() * OUT_RATE as f64).round() as usize;
        self.pos = (frame * OUT_CHANNELS as usize).min(self.samples.len());
        Ok(())
    }
}

/// Load a SID tune and render its default subsong (ROM-less) to a seekable stereo f32 source.
/// Errors cleanly when the file is not a valid SID tune, so the skip-unplayable path takes over.
pub fn open(path: &Path) -> Result<SidSource> {
    let cpath = CString::new(path.as_os_str().to_string_lossy().as_bytes())
        .map_err(|_| anyhow!("path has an interior NUL: {}", path.display()))?;
    let h = unsafe { sid_open(cpath.as_ptr(), OUT_RATE as c_int) };
    if h.is_null() {
        return Err(anyhow!("libsidplayfp could not load {}", path.display()));
    }
    let result = render(h);
    unsafe { sid_close(h) };
    result.map_err(|e| anyhow!("{e} ({})", path.display()))
}

fn render(h: *mut SidHandle) -> Result<SidSource> {
    let target_frames = DEFAULT_PLAY_SECS as usize * OUT_RATE as usize;
    let mut chunk = vec![0.0f32; CHUNK_FRAMES * OUT_CHANNELS as usize];
    let mut samples: Vec<f32> = Vec::with_capacity(target_frames * OUT_CHANNELS as usize);
    while samples.len() < target_frames * OUT_CHANNELS as usize {
        let got = unsafe { sid_render(h, chunk.as_mut_ptr(), CHUNK_FRAMES as c_int) };
        if got <= 0 {
            break;
        }
        samples.extend_from_slice(&chunk[..got as usize * OUT_CHANNELS as usize]);
        if (got as usize) < CHUNK_FRAMES {
            break; // engine stopped early
        }
    }
    if samples.is_empty() {
        return Err(anyhow!("rendered no audio"));
    }
    apply_fade_out(&mut samples);
    let frames = samples.len() / OUT_CHANNELS as usize;
    let total = Some(Duration::from_secs_f64(frames as f64 / OUT_RATE as f64));
    Ok(SidSource { samples, pos: 0, total })
}

/// Linear fade over the last FADE_SECS of the rendered buffer, so an endlessly-looping tune ends
/// cleanly instead of being hard-cut.
fn apply_fade_out(samples: &mut [f32]) {
    let fade_frames = (FADE_SECS * OUT_RATE as f32) as usize;
    let total_frames = samples.len() / OUT_CHANNELS as usize;
    if fade_frames == 0 || total_frames == 0 {
        return;
    }
    let start = total_frames.saturating_sub(fade_frames);
    for f in start..total_frames {
        let gain = (total_frames - f) as f32 / fade_frames as f32;
        samples[f * 2] *= gain;
        samples[f * 2 + 1] *= gain;
    }
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
}
