//! AdLib / OPL2/OPL3 FM-synthesis playback via AdPlug (LGPL-2.1+), behind the `adplug` feature.
//! AdPlug replays dozens of early-PC AdLib music formats (DOSBox OPL captures, Creative CMF,
//! Reality AdLib Tracker, HSC, Sierra, LucasArts, id/Apogee, and many tracker-style formats) by
//! driving an emulated OPL chip. It is C++-only with no C API, so a small C++ shim
//! (src/adplug_shim.cpp) exposes the handful of calls we need and we FFI to that, rather than to a
//! Rust binding crate. One AdPlug build plays every format below.
//!
//! Like the gme side-decoder this renders the whole song to an interleaved f32 stereo buffer up
//! front (AdLib synthesis is cheap) and wraps it as a seekable rodio `Source`, so seeking is an
//! exact index into the buffer. Many of these files hold subsongs; this plays the default one
//! (subsong selection in the UI is a later enhancement, as with gme).

use std::ffi::CString;
use std::os::raw::{c_char, c_int, c_long};
use std::path::Path;
use std::sync::Mutex;
use std::time::Duration;

use anyhow::{anyhow, Result};
use rodio::source::SeekError;
use rodio::Source;

const OUT_RATE: u32 = 44_100;
const OUT_CHANNELS: u16 = 2;
// Hard ceiling on how much audio we render from one song, so a file that never signals its end
// cannot render without bound. Well past any normal AdLib tune.
const MAX_RENDER_SECS: usize = 600;
const CHUNK_FRAMES: usize = 4096;

/// Extensions routed to AdPlug. This is the set verified to load and render audible output from
/// AdPlug's own test corpus as a standalone file. Deliberately excluded: extensions that collide
/// with other decoders (e.g. `.imf`/`.dtm` to libopenmpt, `.s3m` to xmrs, `.mid` to the MIDI synth,
/// `.vgm` to libgme), overly generic ones (`.raw`, `.dat`, `.m`, `.bnk`), and formats that need a
/// companion instrument-bank file beside the tune to play (`.ksm`, `.rol`, `.sci`), which a single
/// dropped file cannot satisfy. AdPlug detects format by content, so the playlist's add-time
/// decodability probe keeps out any file that merely shares one of these extensions.
pub const ADPLUG_EXTS: &[&str] = &[
    "a2m", "adl", "agd", "amd", "bam", "bmf", "cff", "cmf", "d00", "dfm", "dmo", "dro", "got",
    "ha2", "hsc", "hsp", "hsq", "ims", "jbm", "laa", "lds", "mad", "mdi", "mkj", "msc", "mus",
    "rad", "rix", "sa2", "sat", "sdb", "sng", "sop", "sqx", "wlf", "xad", "xsm",
];

// AdPlug's format factory and its global instrument database are shared mutable state that is not
// thread-safe. LinAmp decodes one track at a time, but the add-time duration probe and playback
// can touch AdPlug from different threads, so serialize every entry into the library with this
// lock. It is only ever held briefly for duration(), or for one up-front render in open().
static ADPLUG_LOCK: Mutex<()> = Mutex::new(());

pub fn is_adplug(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| ADPLUG_EXTS.iter().any(|x| e.eq_ignore_ascii_case(x)))
        .unwrap_or(false)
}

#[repr(C)]
struct AdplugHandle {
    _private: [u8; 0],
}

extern "C" {
    fn adplug_open(path: *const c_char, rate: c_int) -> *mut AdplugHandle;
    fn adplug_length_ms(h: *mut AdplugHandle) -> c_long;
    fn adplug_render(h: *mut AdplugHandle, out: *mut f32, frames: c_int) -> c_int;
    fn adplug_close(h: *mut AdplugHandle);
}

/// A seekable rodio `Source` over a fully-rendered AdLib song (interleaved f32, stereo).
pub struct AdplugSource {
    samples: Vec<f32>,
    pos: usize,
    total: Option<Duration>,
}

impl Iterator for AdplugSource {
    type Item = f32;
    fn next(&mut self) -> Option<f32> {
        let s = self.samples.get(self.pos).copied();
        if s.is_some() {
            self.pos += 1;
        }
        s
    }
}

impl Source for AdplugSource {
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

/// Open an AdLib file and render its default subsong to a seekable stereo f32 source. Errors
/// cleanly when AdPlug recognizes no format in the file, so the skip-unplayable path takes over.
pub fn open(path: &Path) -> Result<AdplugSource> {
    let cpath = CString::new(path.as_os_str().to_string_lossy().as_bytes())
        .map_err(|_| anyhow!("path has an interior NUL: {}", path.display()))?;
    let _guard = ADPLUG_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let h = unsafe { adplug_open(cpath.as_ptr(), OUT_RATE as c_int) };
    if h.is_null() {
        return Err(anyhow!("AdPlug could not load {}", path.display()));
    }
    // From here on the handle must be freed on every path.
    let result = render(h);
    unsafe { adplug_close(h) };
    result.map_err(|e| anyhow!("{e} ({})", path.display()))
}

fn render(h: *mut AdplugHandle) -> Result<AdplugSource> {
    let cap_samples = MAX_RENDER_SECS * OUT_RATE as usize * OUT_CHANNELS as usize;
    let mut chunk = vec![0.0f32; CHUNK_FRAMES * OUT_CHANNELS as usize];
    let mut samples: Vec<f32> = Vec::new();
    loop {
        let got = unsafe { adplug_render(h, chunk.as_mut_ptr(), CHUNK_FRAMES as c_int) };
        if got <= 0 {
            break;
        }
        let got = got as usize;
        samples.extend_from_slice(&chunk[..got * OUT_CHANNELS as usize]);
        if got < CHUNK_FRAMES {
            break; // the song ended (one pass, no loop)
        }
        if samples.len() >= cap_samples {
            break;
        }
    }
    if samples.is_empty() {
        return Err(anyhow!("rendered no audio"));
    }
    let frames = samples.len() / OUT_CHANNELS as usize;
    let total = Some(Duration::from_secs_f64(frames as f64 / OUT_RATE as f64));
    Ok(AdplugSource { samples, pos: 0, total })
}

/// Length of an AdLib song for the playlist, without keeping the rendered audio. AdPlug simulates
/// the song to measure it (no synthesis of the final PCM is retained), cheap enough for the
/// add-time scan. `None` when the file isn't an AdPlug format or declares no length.
pub fn duration(path: &Path) -> Option<Duration> {
    if !is_adplug(path) {
        return None;
    }
    let cpath = CString::new(path.as_os_str().to_string_lossy().as_bytes()).ok()?;
    let _guard = ADPLUG_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let h = unsafe { adplug_open(cpath.as_ptr(), OUT_RATE as c_int) };
    if h.is_null() {
        return None;
    }
    let ms = unsafe { adplug_length_ms(h) };
    unsafe { adplug_close(h) };
    (ms > 0).then(|| Duration::from_millis(ms as u64))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_adplug_matches_the_adlib_extensions() {
        for e in ADPLUG_EXTS {
            assert!(is_adplug(&std::path::PathBuf::from(format!("x.{e}"))), ".{e}");
            assert!(
                is_adplug(&std::path::PathBuf::from(format!("x.{}", e.to_uppercase()))),
                ".{e} uppercase"
            );
        }
        // These belong to other decoders; AdPlug must not claim them.
        for e in ["imf", "dtm", "s3m", "mid", "vgm", "mp3"] {
            assert!(!is_adplug(&std::path::PathBuf::from(format!("x.{e}"))), ".{e} is not adplug");
        }
    }

    #[test]
    fn open_errors_cleanly_on_non_adlib() {
        let p = std::env::temp_dir().join("linamp_not_adlib.cmf");
        std::fs::write(&p, b"this is definitely not an AdLib tune").unwrap();
        assert!(open(&p).is_err());
        let _ = std::fs::remove_file(&p);
    }

    /// Render every AdLib sample present and confirm it produces a plausible amount of non-silent
    /// stereo audio. AdPlug is the only decoder, so there is no independent oracle; this is a
    /// decode/render smoke test. (qa-music is gitignored, so it skips on CI.)
    #[test]
    fn renders_real_adlib_tunes() {
        let dir = std::path::Path::new("qa-music/format-samples");
        if !dir.exists() {
            return;
        }
        let mut checked = 0;
        for entry in std::fs::read_dir(dir).unwrap().flatten() {
            let path = entry.path();
            if !is_adplug(&path) {
                continue;
            }
            let src = open(&path).unwrap_or_else(|e| panic!("open {}: {e}", path.display()));
            assert_eq!(src.channels(), 2, "{}: stereo", path.display());
            assert_eq!(src.sample_rate(), 44_100, "{}: 44.1k", path.display());
            // AdLib tunes are short; scan the whole render for audio.
            let got: Vec<f32> = src.take(44_100 * 2 * 30).collect();
            assert!(got.len() > 4_410, "{}: too little audio ({})", path.display(), got.len());
            let peak = got.iter().fold(0.0f32, |m, &s| m.max(s.abs()));
            assert!(peak > 0.003, "{}: near-silence (peak {peak})", path.display());
            checked += 1;
        }
        assert!(checked > 0, "no AdLib samples found to render");
    }
}
