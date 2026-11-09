//! Sega Saturn SSF and Dreamcast DSF program music (.ssf/.minissf/.dsf/.minidsf/.dcsf/.minidcsf) via
//! Highly_Theoretical (GPL-3) + psflib (MIT), behind the `ssf` feature. Highly_Theoretical emulates
//! only the Saturn SCSP and Dreamcast AICA sound chips, so it plays ROM-less: no console BIOS or game
//! ROM is needed or shipped (the driver program and samples live inside the .ssf/.dsf file, which
//! psflib unpacks, resolving `.minissf`/`.minidsf` `_lib` chains). The emulator has no random seek, so
//! seeking resets the state and renders forward, like the chip decoders. See src/ssf_shim.c.
//!
//! `.dsf` is ambiguous: it is also "DSD Stream File" (hi-res audio, handled by crate::dsd). We claim a
//! `.dsf` only when its bytes are a PSF v2 (0x12) container, so DSD `.dsf` files fall through.

use std::ffi::CString;
use std::io::Read;
use std::os::raw::{c_char, c_int, c_void};
use std::path::Path;
use std::sync::Mutex;
use std::time::Duration;

use anyhow::{anyhow, Result};
use rodio::source::SeekError;
use rodio::Source;

const OUT_RATE: u32 = 44_100;
const OUT_CHANNELS: u16 = 2;
/// Render batch, in stereo frames.
const BATCH: usize = 4096;
/// Play length for a tune that declares none. Matches the PSF/GME/SID default.
const DEFAULT_PLAY_SECS: u64 = 150;

/// Unambiguous extensions routed here. `.dsf` is handled separately (content-sniffed, see `is_ssf`)
/// because it collides with DSD Stream File, so it is deliberately NOT in this list - the router guard
/// and SUPPORTED_EXTS use this list for pure-extension routing. The `.ssflib`/`.dsflib`/`.dcsflib` base
/// files are support files pulled in by a `.mini*`, never played directly, so they are not listed.
pub const SSF_EXTS: &[&str] = &["ssf", "minissf", "minidsf", "dcsf", "minidcsf"];

/// True for an SSF/DSF file: any unambiguous `SSF_EXTS` extension, or a `.dsf` whose bytes are a PSF v2
/// (`PSF\x12`) container. The `.dsf` content check keeps us from stealing DSD Stream File `.dsf` files,
/// which route to the DSD decoder.
pub fn is_ssf(path: &Path) -> bool {
    let ext = match path.extension().and_then(|e| e.to_str()) {
        Some(e) => e.to_ascii_lowercase(),
        None => return false,
    };
    SSF_EXTS.contains(&ext.as_str()) || (ext == "dsf" && starts_with_psf_v2(path))
}

/// Whether the file begins with the PSF v2 signature `PSF\x12` (a Dreamcast DSF). Used to tell a
/// DSF `.dsf` from a DSD `.dsf`.
fn starts_with_psf_v2(path: &Path) -> bool {
    let mut buf = [0u8; 4];
    std::fs::File::open(path)
        .and_then(|mut f| f.read_exact(&mut buf))
        .is_ok()
        && &buf[..3] == b"PSF"
        && buf[3] == 0x12
}

extern "C" {
    fn ssf_shim_open(path: *const c_char) -> *mut c_void;
    fn ssf_shim_render(handle: *mut c_void, buf: *mut i16, frames: c_int) -> c_int;
    fn ssf_shim_reset(handle: *mut c_void);
    fn ssf_shim_close(handle: *mut c_void);
}

// Highly_Theoretical has a one-time global init (sega_init) and shared tables, so opens/renders on
// different threads can collide. LinAmp renders one source at a time; the lock just makes the test
// harness (which exercises several tunes) safe.
static SSF_LOCK: Mutex<()> = Mutex::new(());

/// A seekable rodio `Source` streaming an SSF/DSF tune as f32 stereo at 44.1 kHz.
pub struct SsfSource {
    handle: *mut c_void, // live decoder; closed on Drop
    total: Option<Duration>,
    cap_frames: u64, // stop here (the tune loops forever)
    pos_frames: u64,
    raw: Vec<i16>, // render scratch, interleaved stereo
    buf: Vec<f32>, // converted batch
    idx: usize,
    ended: bool,
}

// Only ever touched from the single thread that owns the source.
unsafe impl Send for SsfSource {}

impl Drop for SsfSource {
    fn drop(&mut self) {
        let _guard = SSF_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        unsafe { ssf_shim_close(self.handle) };
    }
}

impl SsfSource {
    fn fill(&mut self) -> bool {
        if self.ended || self.pos_frames >= self.cap_frames {
            self.ended = true;
            return false;
        }
        let want = BATCH.min((self.cap_frames - self.pos_frames) as usize);
        let got = {
            let _guard = SSF_LOCK.lock().unwrap_or_else(|e| e.into_inner());
            unsafe { ssf_shim_render(self.handle, self.raw.as_mut_ptr(), want as c_int) }
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

impl Iterator for SsfSource {
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

impl Source for SsfSource {
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
        let _guard = SSF_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        if target < self.pos_frames {
            unsafe { ssf_shim_reset(self.handle) };
            self.pos_frames = 0;
        }
        // No random seek: render-and-discard forward to the target.
        let mut scratch = vec![0i16; BATCH * OUT_CHANNELS as usize];
        while self.pos_frames < target {
            let want = ((target - self.pos_frames).min(BATCH as u64)) as c_int;
            let got = unsafe { ssf_shim_render(self.handle, scratch.as_mut_ptr(), want) };
            if got <= 0 {
                break;
            }
            self.pos_frames += got as u64;
        }
        self.buf.clear();
        self.idx = 0;
        self.ended = self.pos_frames >= self.cap_frames;
        Ok(())
    }
}

/// Load an SSF/DSF tune as a lazy, seekable stereo source. Errors cleanly when the file is not a
/// valid SSF/DSF (the shim checks the PSF signature + version), so the skip-unplayable path takes over.
pub fn open(path: &Path) -> Result<SsfSource> {
    let cpath = CString::new(path.as_os_str().to_string_lossy().as_bytes())
        .map_err(|_| anyhow!("path has interior NUL: {}", path.display()))?;
    let handle = {
        let _guard = SSF_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        unsafe { ssf_shim_open(cpath.as_ptr()) }
    };
    if handle.is_null() {
        return Err(anyhow!("Highly_Theoretical could not load {}", path.display()));
    }
    let secs = tagged_length_secs(path).filter(|s| *s > 0.0).unwrap_or(DEFAULT_PLAY_SECS as f64);
    Ok(SsfSource {
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

/// Length of an SSF/DSF tune for the playlist, from its `length` tag (or the default).
pub fn duration(path: &Path) -> Option<Duration> {
    if !is_ssf(path) {
        return None;
    }
    let secs = tagged_length_secs(path).filter(|s| *s > 0.0).unwrap_or(DEFAULT_PLAY_SECS as f64);
    Some(Duration::from_secs_f64(secs))
}

/// Parse a PSF time field (`[[HH:]MM:]SS[.frac]`) into seconds, parsing each field as a float so a
/// high-precision fraction like `.819682` is not misread as a huge integer (the PSF1 length-tag bug).
/// (Shared with the PSF1 path; a candidate to hoist into one `xsf_tags` helper for the later *SF cores.)
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

/// Play length in seconds from the file's `[TAG]` section (`length` tag), or `None`. The tag block
/// sits after the program, so the last `[TAG]` is the real one. Tag names are case-insensitive.
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
    fn is_ssf_matches_unambiguous_extensions() {
        for e in ["ssf", "SSF", "minissf", "minidsf", "dcsf", "minidcsf"] {
            assert!(is_ssf(&std::path::PathBuf::from(format!("x.{e}"))), ".{e}");
        }
        assert!(!is_ssf(std::path::Path::new("x.mp3")));
        assert!(!is_ssf(std::path::Path::new("noext")));
        // .dsf is content-sniffed, so a nonexistent/non-PSF path is not claimed.
        assert!(!is_ssf(std::path::Path::new("x.dsf")));
    }

    #[test]
    fn dsf_is_claimed_only_for_psf_v2_content() {
        let dsf = std::env::temp_dir().join("linamp_ssf_v2.dsf");
        std::fs::write(&dsf, b"PSF\x12rest").unwrap();
        assert!(is_ssf(&dsf));
        let _ = std::fs::remove_file(&dsf);
        // A DSD Stream File (magic "DSD ") with a .dsf extension is NOT claimed.
        let dsd = std::env::temp_dir().join("linamp_dsd.dsf");
        std::fs::write(&dsd, b"DSD \x00\x00\x00\x00").unwrap();
        assert!(!is_ssf(&dsd));
        let _ = std::fs::remove_file(&dsd);
    }

    #[test]
    fn time_parse_handles_fractional_seconds() {
        assert!((parse_psf_time("2:30.819682").unwrap() - 150.819_682).abs() < 1e-3);
        assert_eq!(parse_psf_time("1:02:03").unwrap(), 3723.0);
        assert!(parse_psf_time("abc").is_none());
    }

    /// Render every SSF/DSF sample present and confirm non-silent stereo audio. Highly_Theoretical is
    /// the only decoder, so this is a decode/render smoke test. (qa-music is gitignored, skips on CI.)
    #[test]
    fn renders_real_ssf_files() {
        let dir = std::path::Path::new("qa-music/format-samples");
        if !dir.exists() {
            return;
        }
        let mut checked = 0;
        for entry in std::fs::read_dir(dir).unwrap().flatten() {
            let path = entry.path();
            if !is_ssf(&path) {
                continue;
            }
            let src = open(&path).unwrap_or_else(|e| panic!("open {}: {e}", path.display()));
            assert_eq!(src.channels(), 2, "{}: stereo", path.display());
            let got: Vec<f32> = src.take(44_100 * 2 * 10).collect();
            assert!(got.len() > 44_100, "{}: too little audio", path.display());
            let peak = got.iter().fold(0.0f32, |m, &s| m.max(s.abs()));
            assert!(peak > 0.003, "{}: near-silence (peak {peak})", path.display());
            checked += 1;
        }
        assert!(checked > 0, "no SSF/DSF samples found to render");
    }
}
