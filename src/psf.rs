//! PlayStation PSF1 music (.psf/.minipsf) via libupse (GPL), behind the `psf`
//! feature. libupse ("Unix PlayStation Sound Emulator") emulates the PS1/PS2 sound hardware and runs
//! the driver program a PSF carries. It is C, but opening a module needs an I/O-callback struct and a
//! one-time global init, so build.rs compiles a tiny C shim (src/psf_shim.c) and we FFI to that, the
//! same side-decoder shape as the other native features.
//!
//! ROM-less: libupse ships its own HLE "FakeBIOS", so no PlayStation BIOS (which is copyrighted and we
//! will never ship) is needed or loaded. This is the whole reason we use libupse rather than the
//! Highly Experimental core, which requires a real 512 KB PS1 BIOS.
//!
//! libupse decodes to interleaved stereo s16 at 44.1 kHz and seeks natively (by milliseconds), so
//! this is a streaming source: it renders a batch at a time and a scrub is a direct library call. A
//! tune with a length tag ends on its own (libupse returns no more frames at length+fade); a tune
//! without one would loop forever, so we cap it at a fixed default, mirroring the chip decoders.

use std::ffi::CString;
use std::os::raw::{c_char, c_int, c_void};
use std::path::Path;
use std::sync::Mutex;
use std::time::Duration;

use anyhow::{anyhow, Result};
use rodio::source::SeekError;
use rodio::Source;

const OUT_RATE: u32 = 44_100;
const OUT_CHANNELS: u16 = 2;
/// Play length for a PSF that declares none (some loop forever). Matches the GME/SID default so chip
/// and console music behave consistently.
const DEFAULT_PLAY_SECS: u64 = 150;

/// Extensions routed to libupse. `.minipsf` references a shared `.psflib` base file that libupse
/// resolves. PSF2/.minipsf2 are deliberately NOT handled: libupse's PS2 IOP HLE is a stub
/// (`upse_ps2_iop_call` is empty), so PSF2 tunes load but decode to silence, and the only engine
/// that plays them correctly (Highly Experimental) needs the copyrighted PS2 BIOS, which we will
/// never ship. PSF2 files are rejected at the signature check in psf_shim.c.
pub const PSF_EXTS: &[&str] = &["psf", "minipsf"];

pub fn is_psf(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| PSF_EXTS.iter().any(|x| e.eq_ignore_ascii_case(x)))
        .unwrap_or(false)
}

extern "C" {
    fn psf_shim_open(path: *const c_char) -> *mut c_void;
    fn psf_shim_render(module: *mut c_void, out: *mut *mut i16) -> c_int;
    fn psf_shim_seek(module: *mut c_void, ms: u32) -> c_int;
    fn psf_shim_close(module: *mut c_void);
}

// libupse keeps process-global emulator state (the one-time init, shared tables), so two modules
// opening or rendering on different threads can collide. LinAmp only renders one source at a time, so
// this lock is uncontended in practice; it just makes the library safe when the test harness
// exercises several tunes in parallel.
static PSF_LOCK: Mutex<()> = Mutex::new(());

/// A seekable rodio `Source` that streams a PSF tune. libupse renders interleaved stereo s16 on
/// demand and seeks natively, so there is no buffering of the whole tune. Output is f32 stereo.
pub struct PsfSource {
    module: *mut c_void, // live libupse module; closed on Drop
    total: Option<Duration>,
    cap_frames: u64, // stop here (bounds a tune that loops forever)
    pos_frames: u64, // absolute frame position
    buf: Vec<f32>,   // current render batch, interleaved stereo
    idx: usize,
    ended: bool,
}

// The module is only ever touched from the single thread that owns the source (the audio thread,
// after it is moved there), never shared, so it is safe to send.
unsafe impl Send for PsfSource {}

impl Drop for PsfSource {
    fn drop(&mut self) {
        let _guard = PSF_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        unsafe { psf_shim_close(self.module) };
    }
}

impl PsfSource {
    /// Render the next batch into `buf`. Returns false at the cap or when libupse stops producing.
    fn fill(&mut self) -> bool {
        if self.ended || self.pos_frames >= self.cap_frames {
            self.ended = true;
            return false;
        }
        let mut ptr: *mut i16 = std::ptr::null_mut();
        let frames = {
            let _guard = PSF_LOCK.lock().unwrap_or_else(|e| e.into_inner());
            unsafe { psf_shim_render(self.module, &mut ptr) }
        };
        if frames <= 0 || ptr.is_null() {
            self.ended = true;
            return false;
        }
        let frames = frames as usize;
        let samples = unsafe { std::slice::from_raw_parts(ptr, frames * OUT_CHANNELS as usize) };
        self.buf.clear();
        self.buf.extend(samples.iter().map(|&s| s as f32 / 32768.0));
        self.idx = 0;
        self.pos_frames += frames as u64;
        true
    }
}

impl Iterator for PsfSource {
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

impl Source for PsfSource {
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
        let ms = pos.as_millis().min(u32::MAX as u128) as u32;
        {
            let _guard = PSF_LOCK.lock().unwrap_or_else(|e| e.into_inner());
            unsafe { psf_shim_seek(self.module, ms) };
        }
        self.buf.clear();
        self.idx = 0;
        self.pos_frames = (pos.as_secs_f64() * OUT_RATE as f64).round() as u64;
        self.ended = self.pos_frames >= self.cap_frames;
        Ok(())
    }
}

/// Parse a PSF time field (`[[HH:]MM:]SS[.frac]`, e.g. `2:30.819682`) into seconds. We parse the
/// `length` tag ourselves rather than use libupse's length, because libupse's time parser mishandles
/// fractional seconds with more than two digits: it reads `.819682` as the integer 819682 and so
/// inflates a `length=2:30.819682` tune to ~22 hours, which then drives both the shown duration and
/// the play cap. Returns `None` on an unparseable field.
fn parse_psf_time(s: &str) -> Option<f64> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    // Colon-separated, least significant last: seconds (possibly fractional), then minutes, then hours.
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

/// Play length in seconds from the PSF's `[TAG]` section (`length` tag), or `None` when the tune
/// declares none (callers fall back to `DEFAULT_PLAY_SECS`). The tag block is `[TAG]` followed by
/// `key=value` lines and sits after the compressed program, so the last `[TAG]` in the file is the
/// real one. Tag names are case-insensitive.
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

/// Load a PSF tune as a lazy, seekable stereo source. Errors cleanly when the file is not a valid
/// PSF (the shim checks the signature before touching libupse, which crashes on malformed input), so
/// the skip-unplayable path takes over.
pub fn open(path: &Path) -> Result<PsfSource> {
    let cpath = CString::new(path.as_os_str().to_string_lossy().as_bytes())
        .map_err(|_| anyhow!("path has an interior NUL: {}", path.display()))?;
    let module = {
        let _guard = PSF_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        unsafe { psf_shim_open(cpath.as_ptr()) }
    };
    if module.is_null() {
        return Err(anyhow!("libupse could not load {}", path.display()));
    }
    // Length comes from the PSF's own `length` tag, parsed in Rust (see tagged_length_secs); libupse's
    // length is unreliable for high-precision fractional tags.
    let total_secs = match tagged_length_secs(path) {
        Some(secs) if secs > 0.0 => secs,
        _ => DEFAULT_PLAY_SECS as f64,
    };
    Ok(PsfSource {
        module,
        total: Some(Duration::from_secs_f64(total_secs)),
        cap_frames: (total_secs * OUT_RATE as f64) as u64,
        pos_frames: 0,
        buf: Vec::new(),
        idx: 0,
        ended: false,
    })
}

/// Length of a PSF for the playlist, read from its tags without emulating it (cheap). Falls back to
/// the fixed default for a tune with no length tag. `None` when the file isn't a PSF.
pub fn duration(path: &Path) -> Option<Duration> {
    if !is_psf(path) {
        return None;
    }
    let secs = match tagged_length_secs(path) {
        Some(secs) if secs > 0.0 => secs,
        _ => DEFAULT_PLAY_SECS as f64,
    };
    Some(Duration::from_secs_f64(secs))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_psf_matches_the_psf_extensions() {
        for e in ["psf", "PSF", "minipsf"] {
            assert!(is_psf(&std::path::PathBuf::from(format!("x.{e}"))), ".{e}");
        }
        // PSF2 is intentionally unsupported (libupse's PS2 IOP HLE is a stub -> silence).
        for e in ["psf2", "minipsf2"] {
            assert!(!is_psf(&std::path::PathBuf::from(format!("x.{e}"))), ".{e}");
        }
        assert!(!is_psf(std::path::Path::new("x.mp3")));
        assert!(!is_psf(std::path::Path::new("noext")));
    }

    #[test]
    fn psf_time_parses_high_precision_fractional_seconds() {
        // Regression: libupse's parser read "2:30.819682" as ~22 hours; the real length is ~150.8s.
        assert!((parse_psf_time("2:30.819682").unwrap() - 150.819_682).abs() < 1e-3);
        assert_eq!(parse_psf_time("90").unwrap(), 90.0);
        assert_eq!(parse_psf_time("2:30").unwrap(), 150.0);
        assert_eq!(parse_psf_time("1:02:03").unwrap(), 3723.0);
        assert!((parse_psf_time("0:05.5").unwrap() - 5.5).abs() < 1e-9);
        assert!(parse_psf_time("").is_none());
        assert!(parse_psf_time("abc").is_none());
    }

    #[test]
    fn tagged_length_reads_the_length_tag() {
        // A minimal file with a [TAG] block (tagged_length_secs only scans for the tag, not a real PSF).
        let p = std::env::temp_dir().join("linamp_psf_length.psf");
        std::fs::write(&p, b"PSF\x01....junk....[TAG]utf8=1\nlength=2:30.819682\nfade=10\n").unwrap();
        assert!((tagged_length_secs(&p).unwrap() - 150.819_682).abs() < 1e-3);
        let _ = std::fs::remove_file(&p);
        // No length tag -> None, so callers fall back to the default.
        let q = std::env::temp_dir().join("linamp_psf_nolength.psf");
        std::fs::write(&q, b"PSF\x01....junk....[TAG]utf8=1\ntitle=x\n").unwrap();
        assert!(tagged_length_secs(&q).is_none());
        let _ = std::fs::remove_file(&q);
    }

    #[test]
    fn open_errors_cleanly_on_non_psf() {
        let p = std::env::temp_dir().join("linamp_not_a_psf.psf");
        std::fs::write(&p, b"this is definitely not a PSF tune").unwrap();
        assert!(open(&p).is_err());
        let _ = std::fs::remove_file(&p);
    }

    /// Render every PSF sample present and confirm it produces non-silent stereo audio, ROM-less.
    /// libupse is the only decoder, so this is a decode/render smoke test. (qa-music is gitignored,
    /// so it skips on CI.)
    #[test]
    fn renders_real_psf_tunes() {
        let dir = std::path::Path::new("qa-music/format-samples");
        if !dir.exists() {
            return;
        }
        let mut checked = 0;
        for entry in std::fs::read_dir(dir).unwrap().flatten() {
            let path = entry.path();
            if !is_psf(&path) {
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
        assert!(checked > 0, "no PSF samples found to render");
    }

    /// Streaming seek: a forward seek lands with audio, a backward seek replays earlier audio.
    /// libupse seeks natively, so both are direct library calls. Local sample only.
    #[test]
    fn seeking_forward_then_back_produces_audio() {
        let p = std::path::Path::new("qa-music/format-samples/psf-sample.psf");
        if !p.exists() {
            return;
        }
        let mut src = open(p).expect("load psf");
        src.try_seek(Duration::from_secs(10)).unwrap();
        let fwd: Vec<f32> = (&mut src).take(44_100).collect();
        assert!(fwd.iter().any(|&s| s.abs() > 0.003), "silence after forward seek");
        src.try_seek(Duration::from_secs(2)).unwrap();
        let back: Vec<f32> = (&mut src).take(44_100).collect();
        assert!(back.iter().any(|&s| s.abs() > 0.003), "silence after backward seek");
    }
}
