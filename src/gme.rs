//! Chiptune / console music playback via Game Music Emu (libgme), behind the `gme`
//! feature. libgme (LGPL-2.1+) emulates the sound hardware of many consoles and home
//! computers and renders a register/music log to PCM: NSF/NSFE (NES), GBS (Game Boy),
//! VGM/VGZ (Genesis, Master System, arcade), SPC (SNES), AY (ZX Spectrum, Amstrad), SAP
//! (Atari), KSS (MSX), HES (PC Engine) and GYM (Genesis). None has a mature pure-Rust
//! player, so we link libgme through a small FFI and bundle it beside the binary, the same
//! way the `ffmpeg` feature bundles libav*.
//!
//! Like the other side-decoders this renders the whole track to an interleaved f32 stereo
//! buffer up front and wraps it as a seekable rodio `Source`, so seeking is an exact index
//! into the buffer. Many of these files hold several subsongs; this plays the first one
//! (subsong selection in the UI is a later enhancement). Length comes from the file when it
//! declares one, otherwise from libgme's intro-plus-two-loops estimate, and a fade is set at
//! the end so an endlessly looping tune stops cleanly instead of playing forever.

use std::ffi::CString;
use std::os::raw::{c_char, c_int};
use std::path::Path;
use std::time::Duration;

use anyhow::{anyhow, Result};
use rodio::source::SeekError;
use rodio::Source;

const OUT_RATE: u32 = 44_100;
const OUT_CHANNELS: u16 = 2;
// Hard ceiling on how much audio we render from one track, so a file that never reports
// "ended" cannot render without bound. Well past any normal chiptune length.
const MAX_RENDER_SECS: u32 = 600;
// Default play length for a track that declares none and does not loop (libgme's own
// fallback is 2.5 minutes).
const DEFAULT_PLAY_MS: c_int = 150_000;
// Extra audio rendered after the fade begins, so the fade-out is captured in full.
const FADE_TAIL_MS: i64 = 8_000;

/// Extensions handled by libgme. `.vgz` is gzip-compressed VGM, which libgme decompresses
/// itself. Formats with no test sample yet are still listed because one libgme build plays
/// all of them; the playlist's add-time decodability probe keeps out anything that fails.
const GME_EXTS: &[&str] = &[
    "nsf", "nsfe", "gbs", "vgm", "vgz", "spc", "ay", "sap", "kss", "hes", "gym",
];

pub fn is_gme(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| GME_EXTS.iter().any(|x| e.eq_ignore_ascii_case(x)))
        .unwrap_or(false)
}

#[repr(C)]
struct MusicEmu {
    _private: [u8; 0],
}

// Mirrors `gme_info_t` from gme.h. Only the four leading length fields are read; the rest
// are declared so the struct size and layout match what libgme writes.
#[repr(C)]
struct GmeInfo {
    length: c_int,
    intro_length: c_int,
    loop_length: c_int,
    play_length: c_int,
    reserved_ints: [c_int; 12],
    system: *const c_char,
    game: *const c_char,
    song: *const c_char,
    author: *const c_char,
    copyright: *const c_char,
    comment: *const c_char,
    dumper: *const c_char,
    reserved_ptrs: [*const c_char; 9],
}

extern "C" {
    fn gme_open_file(path: *const c_char, out: *mut *mut MusicEmu, sample_rate: c_int) -> *const c_char;
    fn gme_start_track(emu: *mut MusicEmu, index: c_int) -> *const c_char;
    fn gme_play(emu: *mut MusicEmu, count: c_int, out: *mut i16) -> *const c_char;
    fn gme_set_fade(emu: *mut MusicEmu, start_msec: c_int);
    fn gme_track_ended(emu: *const MusicEmu) -> c_int;
    fn gme_track_info(emu: *const MusicEmu, out: *mut *mut GmeInfo, track: c_int) -> *const c_char;
    fn gme_free_info(info: *mut GmeInfo);
    fn gme_delete(emu: *mut MusicEmu);
}

/// A libgme error is a C string: null means success, non-null is the message.
fn check(err: *const c_char, what: &str) -> Result<()> {
    if err.is_null() {
        return Ok(());
    }
    let msg = unsafe { std::ffi::CStr::from_ptr(err) }
        .to_string_lossy()
        .into_owned();
    Err(anyhow!("libgme {what}: {msg}"))
}

/// A seekable rodio `Source` over a fully-rendered chiptune (interleaved f32, stereo).
pub struct GmeSource {
    samples: Vec<f32>,
    pos: usize,
    total: Option<Duration>,
}

impl Iterator for GmeSource {
    type Item = f32;
    fn next(&mut self) -> Option<f32> {
        let s = self.samples.get(self.pos).copied();
        if s.is_some() {
            self.pos += 1;
        }
        s
    }
}

impl Source for GmeSource {
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

/// Decide how long to render, in milliseconds, from a track's info: an explicit length if the
/// file gives one, otherwise the intro plus two loops, otherwise a default. Clamped so a bad
/// value cannot drive an unbounded render.
fn play_length_ms(info: &GmeInfo) -> c_int {
    let ms = if info.length > 0 {
        info.length
    } else if info.loop_length > 0 {
        info.intro_length.max(0).saturating_add(info.loop_length.saturating_mul(2))
    } else if info.play_length > 0 {
        info.play_length
    } else {
        DEFAULT_PLAY_MS
    };
    ms.clamp(1, MAX_RENDER_SECS as c_int * 1000)
}

/// Open a chiptune and render its first subsong to a seekable stereo f32 source.
pub fn open(path: &Path) -> Result<GmeSource> {
    let cpath = CString::new(path.as_os_str().to_string_lossy().as_bytes())
        .map_err(|_| anyhow!("path has an interior NUL: {}", path.display()))?;

    let mut emu: *mut MusicEmu = std::ptr::null_mut();
    check(
        unsafe { gme_open_file(cpath.as_ptr(), &mut emu, OUT_RATE as c_int) },
        "open",
    )?;
    if emu.is_null() {
        return Err(anyhow!("libgme returned no emulator for {}", path.display()));
    }
    // From here on emu must be freed on every path.
    let result = render_first_track(emu);
    unsafe { gme_delete(emu) };
    result.map_err(|e| anyhow!("{e} ({})", path.display()))
}

fn render_first_track(emu: *mut MusicEmu) -> Result<GmeSource> {
    // Length of subsong 0, then start it and set the fade at the end.
    let mut info: *mut GmeInfo = std::ptr::null_mut();
    check(unsafe { gme_track_info(emu, &mut info, 0) }, "track_info")?;
    if info.is_null() {
        return Err(anyhow!("no track info"));
    }
    let play_ms = play_length_ms(unsafe { &*info });
    unsafe { gme_free_info(info) };

    check(unsafe { gme_start_track(emu, 0) }, "start_track")?;
    unsafe { gme_set_fade(emu, play_ms) };

    // Render until the track ends (the fade has played out) or we hit the ceiling. gme_play
    // takes a sample count (shorts), interleaved stereo, so it must be even.
    let frame_cap = ((play_ms as i64 + FADE_TAIL_MS).max(0) as u32 / 1000 + 1)
        .saturating_mul(OUT_RATE)
        .min(MAX_RENDER_SECS.saturating_mul(OUT_RATE)) as usize;
    let sample_cap = frame_cap * OUT_CHANNELS as usize;

    const CHUNK: usize = 8192; // shorts per gme_play call (4096 stereo frames)
    let mut buf = [0i16; CHUNK];
    let mut samples: Vec<f32> = Vec::new();
    while unsafe { gme_track_ended(emu) } == 0 && samples.len() < sample_cap {
        check(unsafe { gme_play(emu, CHUNK as c_int, buf.as_mut_ptr()) }, "play")?;
        samples.extend(buf.iter().map(|&s| s as f32 / 32768.0));
    }

    if samples.is_empty() {
        return Err(anyhow!("rendered no audio"));
    }
    let frames = samples.len() / OUT_CHANNELS as usize;
    let total = Some(Duration::from_secs_f64(frames as f64 / OUT_RATE as f64));
    Ok(GmeSource {
        samples,
        pos: 0,
        total,
    })
}

/// Length of a chiptune for the playlist, from its first subsong, without rendering audio.
pub fn duration(path: &Path) -> Option<Duration> {
    if !is_gme(path) {
        return None;
    }
    let cpath = CString::new(path.as_os_str().to_string_lossy().as_bytes()).ok()?;
    let mut emu: *mut MusicEmu = std::ptr::null_mut();
    if !unsafe { gme_open_file(cpath.as_ptr(), &mut emu, OUT_RATE as c_int) }.is_null()
        || emu.is_null()
    {
        return None;
    }
    let mut info: *mut GmeInfo = std::ptr::null_mut();
    let ms = if unsafe { gme_track_info(emu, &mut info, 0) }.is_null() && !info.is_null() {
        let ms = play_length_ms(unsafe { &*info });
        unsafe { gme_free_info(info) };
        Some(ms)
    } else {
        None
    };
    unsafe { gme_delete(emu) };
    ms.map(|ms| Duration::from_millis(ms as u64))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_gme_matches_chiptune_extensions() {
        for e in ["nsf", "NSF", "gbs", "vgm", "vgz", "spc", "ay", "sap", "kss", "hes", "gym"] {
            assert!(is_gme(&std::path::PathBuf::from(format!("x.{e}"))), ".{e}");
        }
        assert!(!is_gme(std::path::Path::new("x.mp3")));
        assert!(!is_gme(std::path::Path::new("noext")));
    }

    /// Render the first subsong of a real NSF and confirm it produces a plausible amount of
    /// non-silent stereo audio. libgme is the only decoder, so there is no independent oracle;
    /// this is a decode/render smoke test. (qa-music is gitignored, so it skips on CI.)
    #[test]
    fn renders_a_real_nsf() {
        let nsf = std::path::PathBuf::from("qa-music/format-samples/nsf-sample.nsf");
        if !nsf.exists() {
            return;
        }
        let src = open(&nsf).unwrap();
        assert_eq!(src.channels(), 2, "stereo output");
        assert_eq!(src.sample_rate(), 44_100);
        let got: Vec<f32> = src.collect();
        assert!(got.len() > 44_100 * 2, "too little audio: {}", got.len());
        let peak = got.iter().fold(0.0f32, |m, &s| m.max(s.abs()));
        assert!(peak > 0.01, "rendered near-silence (peak {peak})");
    }
}
