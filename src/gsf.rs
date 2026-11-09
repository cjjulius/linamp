//! Game Boy Advance GSF music (.gsf/.minigsf) via libgsf (MPL-2.0), behind the `gsf` feature. libgsf
//! bundles a trimmed mGBA and plays ROM-less: mGBA's built-in HLE BIOS stands in for the GBA BIOS
//! (useBios=false), so no Nintendo BIOS is involved (the game's audio program lives inside the
//! .gsf/.minigsf file, which libgsf parses itself, resolving `.minigsf` `_lib` chains). Unlike the other
//! *SF decoders this one does not use psflib and supports native seeking, so `try_seek` seeks the
//! emulator directly rather than resetting and rendering forward. Output is 44.1 kHz stereo. See
//! src/gsf_shim.c.

use std::ffi::CString;
use std::os::raw::{c_char, c_int, c_long, c_void};
use std::path::Path;
use std::sync::Mutex;
use std::time::Duration;

use anyhow::{anyhow, Result};
use rodio::source::SeekError;
use rodio::Source;

const OUT_RATE: u32 = 44_100;
const OUT_CHANNELS: u16 = 2;
const BATCH: usize = 4096;
const DEFAULT_PLAY_SECS: u64 = 150;

/// Extensions routed here. `.minigsf` references a shared `.gsflib` base file (resolved by libgsf).
/// `.gsflib` itself is a support file, never played directly, so not listed.
pub const GSF_EXTS: &[&str] = &["gsf", "minigsf"];

pub fn is_gsf(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| GSF_EXTS.iter().any(|x| e.eq_ignore_ascii_case(x)))
        .unwrap_or(false)
}

extern "C" {
    fn gsf_shim_open(path: *const c_char) -> *mut c_void;
    fn gsf_shim_render(handle: *mut c_void, buf: *mut i16, frames: c_int) -> c_int;
    fn gsf_shim_seek(handle: *mut c_void, frame: c_long);
    fn gsf_shim_close(handle: *mut c_void);
}

// libgsf keeps all state in the handle, but LinAmp renders one source at a time; the lock keeps the
// test harness (which exercises several tunes) safe.
static GSF_LOCK: Mutex<()> = Mutex::new(());

/// A seekable rodio `Source` streaming a GSF tune as f32 stereo at 44.1 kHz.
pub struct GsfSource {
    handle: *mut c_void,
    total: Option<Duration>,
    cap_frames: u64,
    pos_frames: u64,
    raw: Vec<i16>,
    buf: Vec<f32>,
    idx: usize,
    ended: bool,
}

unsafe impl Send for GsfSource {}

impl Drop for GsfSource {
    fn drop(&mut self) {
        let _guard = GSF_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        unsafe { gsf_shim_close(self.handle) };
    }
}

impl GsfSource {
    fn fill(&mut self) -> bool {
        if self.ended || self.pos_frames >= self.cap_frames {
            self.ended = true;
            return false;
        }
        let want = BATCH.min((self.cap_frames - self.pos_frames) as usize);
        let got = {
            let _guard = GSF_LOCK.lock().unwrap_or_else(|e| e.into_inner());
            unsafe { gsf_shim_render(self.handle, self.raw.as_mut_ptr(), want as c_int) }
        };
        if got <= 0 {
            self.ended = true;
            return false;
        }
        let got = got as usize;
        self.buf.clear();
        self.buf
            .extend(self.raw[..got * OUT_CHANNELS as usize].iter().map(|&s| s as f32 / 32768.0));
        self.idx = 0;
        self.pos_frames += got as u64;
        true
    }
}

impl Iterator for GsfSource {
    type Item = f32;
    fn next(&mut self) -> Option<f32> {
        if self.idx >= self.buf.len() && !self.fill() {
            return None;
        }
        let s = self.buf[self.idx];
        self.idx += 1;
        Some(s)
    }
}

impl Source for GsfSource {
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
        let target = (pos.as_secs_f64() * OUT_RATE as f64).round() as u64;
        let _guard = GSF_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // libgsf seeks natively (it resets internally for backward seeks), so just seek the emulator.
        unsafe { gsf_shim_seek(self.handle, target.min(i64::MAX as u64) as c_long) };
        self.pos_frames = target;
        self.buf.clear();
        self.idx = 0;
        self.ended = self.pos_frames >= self.cap_frames;
        Ok(())
    }
}

/// Load a GSF tune as a lazy, seekable stereo source. Errors cleanly when the file is not a valid GSF
/// (the shim checks the PSF signature + version 0x22), so the skip-unplayable path takes over.
pub fn open(path: &Path) -> Result<GsfSource> {
    let cpath = CString::new(path.as_os_str().to_string_lossy().as_bytes())
        .map_err(|_| anyhow!("path has interior NUL: {}", path.display()))?;
    let handle = {
        let _guard = GSF_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        unsafe { gsf_shim_open(cpath.as_ptr()) }
    };
    if handle.is_null() {
        return Err(anyhow!("libgsf could not load {}", path.display()));
    }
    let secs = tagged_length_secs(path).filter(|s| *s > 0.0).unwrap_or(DEFAULT_PLAY_SECS as f64);
    Ok(GsfSource {
        handle,
        total: Some(Duration::from_secs_f64(secs)),
        cap_frames: (secs * OUT_RATE as f64).round() as u64,
        pos_frames: 0,
        raw: vec![0i16; BATCH * OUT_CHANNELS as usize],
        buf: Vec::new(),
        idx: 0,
        ended: false,
    })
}

/// Length of a GSF tune for the playlist, from its `length` tag (or the default).
pub fn duration(path: &Path) -> Option<Duration> {
    if !is_gsf(path) {
        return None;
    }
    let secs = tagged_length_secs(path).filter(|s| *s > 0.0).unwrap_or(DEFAULT_PLAY_SECS as f64);
    Some(Duration::from_secs_f64(secs))
}

/// Parse a PSF time field (`[[HH:]MM:]SS[.frac]`) into seconds, parsing each field as a float (the
/// PSF1 fractional-seconds fix). Shared with the other *SF cores; a candidate to hoist into one helper.
fn parse_psf_time(s: &str) -> Option<f64> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    let mut total = 0.0f64;
    for (i, field) in s.rsplit(':').enumerate() {
        let v: f64 = field.trim().parse().ok()?;
        total += match i {
            0 => v,
            1 => v * 60.0,
            2 => v * 3600.0,
            _ => return None,
        };
    }
    Some(total)
}

/// Play length in seconds from the file's `[TAG]` section (`length` tag), or `None`.
fn tagged_length_secs(path: &Path) -> Option<f64> {
    let data = std::fs::read(path).ok()?;
    let pos = data.windows(5).rposition(|w| w == b"[TAG]")? + 5;
    let text = String::from_utf8_lossy(&data[pos..]);
    for line in text.split(['\n', '\r']) {
        if let Some((key, value)) = line.split_once('=') {
            if key.trim().eq_ignore_ascii_case("length") {
                return parse_psf_time(value);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_gsf_matches_the_extensions() {
        for e in ["gsf", "GSF", "minigsf"] {
            assert!(is_gsf(&std::path::PathBuf::from(format!("x.{e}"))), ".{e}");
        }
        assert!(!is_gsf(std::path::Path::new("x.mp3")));
        assert!(!is_gsf(std::path::Path::new("x.gsflib"))); // support file, not played directly
        assert!(!is_gsf(std::path::Path::new("noext")));
    }

    #[test]
    fn time_parse_handles_fractional_seconds() {
        assert!((parse_psf_time("2:30.819682").unwrap() - 150.819_682).abs() < 1e-3);
        assert_eq!(parse_psf_time("01:32").unwrap(), 92.0);
        assert!(parse_psf_time("abc").is_none());
    }

    /// Render every GSF sample present and confirm non-silent stereo audio. libgsf is the only
    /// decoder, so this is a decode/render smoke test. (qa-music is gitignored, skips on CI.)
    #[test]
    fn renders_real_gsf_files() {
        let dir = std::path::Path::new("qa-music/format-samples");
        if !dir.exists() {
            return;
        }
        let mut checked = 0;
        for entry in std::fs::read_dir(dir).unwrap().flatten() {
            let path = entry.path();
            if !is_gsf(&path) {
                continue;
            }
            let src = open(&path).unwrap_or_else(|e| panic!("open {}: {e}", path.display()));
            assert_eq!(src.channels(), 2, "{}: stereo", path.display());
            let got: Vec<f32> = src.take(OUT_RATE as usize * 2 * 10).collect();
            assert!(got.len() > OUT_RATE as usize, "{}: too little audio", path.display());
            let peak = got.iter().fold(0.0f32, |m, &s| m.max(s.abs()));
            assert!(peak > 0.003, "{}: near-silence (peak {peak})", path.display());
            checked += 1;
        }
        assert!(checked > 0, "no GSF samples found to render");
    }
}
