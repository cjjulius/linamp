//! The tracker "zoo" via libopenmpt (Tier 2b), behind the `openmpt` feature. libopenmpt
//! (BSD-3-Clause) plays roughly thirty module formats beyond the four the pure-Rust `xmrs`
//! path already covers (mod/xm/s3m/it): OctaMED, Farandole, Oktalyzer, Digital Symphony,
//! Scream Tracker variants, Imago Orpheus, and many more. We keep `xmrs` for its four (no C
//! build dependency there) and route only the extra formats here, the same split the roadmap
//! calls out, so the common cases stay pure Rust and only the long tail pulls in libopenmpt.
//!
//! Like the GME side-decoder this renders the whole module to an interleaved f32 stereo buffer
//! up front and wraps it as a seekable rodio `Source`, so seeking is an exact index into the
//! buffer. Modules are small and render far faster than real time, so the up-front cost is
//! negligible. libopenmpt is linked through a small C-API FFI and bundled beside the binary,
//! the same way the `ffmpeg` and `gme` features bundle their libraries; no Rust binding crate
//! is pulled in, matching the GME precedent.

use std::ffi::c_void;
use std::os::raw::{c_char, c_int};
use std::path::Path;
use std::time::Duration;

use anyhow::{anyhow, Result};
use rodio::source::SeekError;
use rodio::Source;

const OUT_RATE: u32 = 44_100;
const OUT_CHANNELS: u16 = 2;
// Hard ceiling on how much audio we render from one module, so a file that mis-reports its
// length (or a bug) cannot render without bound. Well past any normal module length.
const MAX_RENDER_SECS: f64 = 1800.0;
// Frames per read call (per channel); the buffer holds twice this many floats (stereo).
const CHUNK_FRAMES: usize = 4096;

/// Module formats routed to libopenmpt. Deliberately excludes the four `xmrs` already handles
/// (mod/xm/s3m/it), the compressed-sample formats that need extra codec libraries our minimal
/// build omits (mo3/oxm), the transparent packer wrappers libopenmpt unwraps on its own
/// (xpk/mmcmp/ppm), and a few extensions too generic to claim safely (fmt/sfx/ice/mus, the
/// last of which also belongs to the later AdLib/MUS work). What remains is the distinct
/// tracker "zoo": one libopenmpt build plays all of them.
pub const OPENMPT_EXTS: &[&str] = &[
    "mptm", "669", "amf", "ams", "c67", "dbm", "digi", "dmf", "dsm", "dtm", "far", "gdm",
    "imf", "j2b", "mdl", "med", "mt2", "mtm", "okt", "plm", "psm", "ptm", "stm", "stp",
    "stx", "symmod", "ult", "umx",
];

pub fn is_openmpt(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| OPENMPT_EXTS.iter().any(|x| e.eq_ignore_ascii_case(x)))
        .unwrap_or(false)
}

#[repr(C)]
struct OpenmptModule {
    _private: [u8; 0],
}

extern "C" {
    // The log/error function pointers and the ctls list are all nullable; we pass null for
    // each (default behaviour, no logging, no extra controls).
    fn openmpt_module_create_from_memory2(
        filedata: *const c_void,
        filesize: usize,
        logfunc: *const c_void,
        loguser: *mut c_void,
        errfunc: *const c_void,
        erruser: *mut c_void,
        error: *mut c_int,
        error_message: *mut *const c_char,
        ctls: *const c_void,
    ) -> *mut OpenmptModule;
    fn openmpt_module_get_duration_seconds(module: *mut OpenmptModule) -> f64;
    fn openmpt_module_set_position_seconds(module: *mut OpenmptModule, seconds: f64) -> f64;
    fn openmpt_module_set_repeat_count(module: *mut OpenmptModule, repeat_count: i32) -> c_int;
    fn openmpt_module_read_interleaved_float_stereo(
        module: *mut OpenmptModule,
        samplerate: i32,
        count: usize,
        interleaved_stereo: *mut f32,
    ) -> usize;
    fn openmpt_module_destroy(module: *mut OpenmptModule);
}

/// A no-op log callback. libopenmpt's default logger writes to stderr, which would spam the
/// terminal when the add-time duration probe or the skip-unplayable path opens a file that
/// isn't actually a module; routing logs here keeps it quiet. Matches `openmpt_log_func`:
/// `void(*)(const char* message, void* user)`.
extern "C" fn silent_log(_message: *const c_char, _user: *mut c_void) {}

/// A lazy, seekable rodio `Source` that streams a module from a live libopenmpt handle. Unlike
/// the GME side-decoder this does not render the whole song up front: modules are often several
/// minutes long, so a full render would cost seconds of latency and hundreds of megabytes. It
/// refills a small buffer on demand and seeks through libopenmpt natively.
pub struct OpenmptSource {
    module: *mut OpenmptModule, // owned; destroyed on Drop
    buf: Vec<f32>,             // one interleaved-stereo read, reused each refill
    filled: usize,             // valid floats currently in `buf`
    pos: usize,                // next float to emit from `buf`
    total: Option<Duration>,
    /// Hard stop at the reported length (interleaved samples), so a module whose loop/jump
    /// effects would otherwise run past its song length stops where the playlist says it ends.
    /// `None` when the module reports no length, so it just plays to its natural end.
    cap_samples: Option<u64>,
    emitted: u64,
}

// The libopenmpt handle is only ever touched from the single thread that owns the source (the
// audio thread, after the source is moved there), never shared, so it is safe to send.
unsafe impl Send for OpenmptSource {}

impl Drop for OpenmptSource {
    fn drop(&mut self) {
        unsafe { openmpt_module_destroy(self.module) };
    }
}

impl Iterator for OpenmptSource {
    type Item = f32;
    fn next(&mut self) -> Option<f32> {
        if let Some(cap) = self.cap_samples {
            if self.emitted >= cap {
                return None;
            }
        }
        if self.pos >= self.filled {
            // Refill: one read of up to CHUNK_FRAMES stereo frames. 0 frames means the song
            // ended (repeat count is 0, so it does not loop).
            let frames = unsafe {
                openmpt_module_read_interleaved_float_stereo(
                    self.module,
                    OUT_RATE as i32,
                    CHUNK_FRAMES,
                    self.buf.as_mut_ptr(),
                )
            };
            if frames == 0 {
                return None;
            }
            self.filled = frames * OUT_CHANNELS as usize;
            self.pos = 0;
        }
        let s = self.buf[self.pos];
        self.pos += 1;
        self.emitted += 1;
        Some(s)
    }
}

impl Source for OpenmptSource {
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
        unsafe { openmpt_module_set_position_seconds(self.module, pos.as_secs_f64()) };
        // Invalidate the buffered chunk and resync the cap to the new position.
        self.filled = 0;
        self.pos = 0;
        self.emitted = (pos.as_secs_f64() * OUT_RATE as f64 * OUT_CHANNELS as f64) as u64;
        Ok(())
    }
}

/// Load a module and expose it as a lazy, seekable stereo source that plays its song once.
/// Errors cleanly on an unrecognized or corrupt module, so the skip-unplayable path takes over.
pub fn open(path: &Path) -> Result<OpenmptSource> {
    let data = std::fs::read(path)?;
    if data.is_empty() {
        return Err(anyhow!("empty file: {}", path.display()));
    }
    let module = create_module(&data);
    if module.is_null() {
        return Err(anyhow!("libopenmpt could not load {}", path.display()));
    }
    // Play once (no loop) so the source ends and the playlist auto-advances.
    unsafe { openmpt_module_set_repeat_count(module, 0) };
    let secs = unsafe { openmpt_module_get_duration_seconds(module) };
    // get_duration_seconds steps the module to measure it, so rewind to the start before play.
    unsafe { openmpt_module_set_position_seconds(module, 0.0) };
    let total = (secs > 0.0 && secs < MAX_RENDER_SECS).then(|| Duration::from_secs_f64(secs));
    let cap_samples =
        total.map(|d| (d.as_secs_f64() * OUT_RATE as f64 * OUT_CHANNELS as f64) as u64);
    Ok(OpenmptSource {
        module,
        buf: vec![0.0; CHUNK_FRAMES * OUT_CHANNELS as usize],
        filled: 0,
        pos: 0,
        total,
        cap_samples,
        emitted: 0,
    })
}

/// Create a libopenmpt module from in-memory file data, or null on failure. The module copies
/// the data it needs, so `data` can be freed afterwards.
fn create_module(data: &[u8]) -> *mut OpenmptModule {
    unsafe {
        openmpt_module_create_from_memory2(
            data.as_ptr() as *const c_void,
            data.len(),
            silent_log as *const c_void,
            std::ptr::null_mut(),
            std::ptr::null(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null(),
        )
    }
}

/// The length of a module for the playlist, without rendering audio. libopenmpt has to parse
/// the whole module to report this, but not synthesize it, so it is cheap enough for the
/// add-time scan. `None` when the file isn't one of our module formats or can't be loaded.
pub fn duration(path: &Path) -> Option<Duration> {
    if !is_openmpt(path) {
        return None;
    }
    let data = std::fs::read(path).ok()?;
    if data.is_empty() {
        return None;
    }
    let module = create_module(&data);
    if module.is_null() {
        return None;
    }
    let secs = unsafe { openmpt_module_get_duration_seconds(module) };
    unsafe { openmpt_module_destroy(module) };
    (secs > 0.0 && secs < MAX_RENDER_SECS).then(|| Duration::from_secs_f64(secs))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_openmpt_matches_the_module_zoo() {
        for e in OPENMPT_EXTS {
            assert!(is_openmpt(&std::path::PathBuf::from(format!("x.{e}"))), ".{e}");
            assert!(
                is_openmpt(&std::path::PathBuf::from(format!("x.{}", e.to_uppercase()))),
                ".{e} uppercase"
            );
        }
        // xmrs keeps these four, so libopenmpt must not claim them.
        for e in ["mod", "xm", "s3m", "it"] {
            assert!(!is_openmpt(&std::path::PathBuf::from(format!("x.{e}"))), ".{e} is xmrs");
        }
        assert!(!is_openmpt(std::path::Path::new("x.mp3")));
        assert!(!is_openmpt(std::path::Path::new("noext")));
    }

    #[test]
    fn open_errors_cleanly_on_non_module() {
        // Garbage bytes in one of our extensions: a clean error, not a panic.
        let p = std::env::temp_dir().join("linamp_not_a_module.far");
        std::fs::write(&p, b"this is definitely not a module").unwrap();
        assert!(open(&p).is_err());
        let _ = std::fs::remove_file(&p);
    }

    /// Render every module sample present and confirm it produces a plausible amount of
    /// non-silent stereo audio. libopenmpt is the only decoder, so there is no independent
    /// oracle; this is a decode/render smoke test. (qa-music is gitignored, so it skips on CI.)
    #[test]
    fn renders_real_modules() {
        let dir = std::path::Path::new("qa-music/format-samples");
        if !dir.exists() {
            return;
        }
        let mut checked = 0;
        for entry in std::fs::read_dir(dir).unwrap().flatten() {
            let path = entry.path();
            if !is_openmpt(&path) {
                continue;
            }
            let src = open(&path).unwrap_or_else(|e| panic!("open {}: {e}", path.display()));
            assert_eq!(src.channels(), 2, "{}: stereo", path.display());
            assert_eq!(src.sample_rate(), 44_100, "{}: 44.1k", path.display());
            // Modules can run several minutes; the source streams, so scan only the first ~20s
            // for audio instead of rendering the whole song.
            let scanned: Vec<f32> = src.take(44_100 * 2 * 20).collect();
            assert!(scanned.len() > 44_100, "{}: too little audio ({})", path.display(), scanned.len());
            let peak = scanned.iter().fold(0.0f32, |m, &s| m.max(s.abs()));
            assert!(peak > 0.01, "{}: near-silence (peak {peak})", path.display());
            checked += 1;
        }
        assert!(checked > 0, "no module samples found to render");
    }
}
