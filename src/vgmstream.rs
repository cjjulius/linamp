//! Streamed game-engine audio via libvgmstream (ISC), behind the `vgmstream` feature. vgmstream plays
//! hundreds of console and game-engine stream formats (GameCube/Wii DSP, PS-ADPCM VAG, CRI HCA,
//! Nintendo BRSTM/BCSTM/BFSTM, FMOD FSB, and many more). It exposes a C API, so unlike the AdPlug and
//! SID side-decoders there is no C++ shim: we bind libvgmstream directly through a hand-rolled FFI,
//! the same way the gme and openmpt features do.
//!
//! We build and link the *core-only* libvgmstream (no ffmpeg/vorbis/mpg123 externals), which is fully
//! permissive (ISC plus BSD/public-domain in-tree decoders) and self-contained (depends only on
//! libc/libm). That covers the large majority of formats, whose payloads are in-tree codecs
//! (DSP/ADPCM, PS-ADPCM, HCA, ATRAC9, PCM). Formats whose payload needs an external codec (e.g. a
//! Vorbis-encoded FSB) simply fail to open in this build and the file is skipped, same as any other
//! unplayable file; a later build tier can add those codecs.
//!
//! Routing is by an explicit allowlist (`VGMSTREAM_EXTS`): vgmstream's own extension table is huge and
//! aggressively claims generic names (.ogg/.wav/.aac/...), so we never ask it about those. We route
//! only game-specific extensions that nothing else in LinAmp handles, and run after every other
//! decoder in the dispatch chain, so the pure-Rust and ffmpeg paths always win a tie.
//!
//! vgmstream seeks natively (sample-accurate), so this is a true streaming source: it renders on
//! demand with no buffering of the whole song, and a seek is a direct library call. The library
//! outputs any channel count; we cap multichannel to stereo (`auto_downmix_channels = 2`) and
//! duplicate mono to both channels, so every track reaches the mixer as stereo, matching the other
//! decoders. Looping game streams are played once with looping disabled (`ignore_loop`) so a track has
//! a finite length and ends on its own, mirroring the fixed-length behavior of the chip decoders.

use std::ffi::CString;
use std::os::raw::{c_char, c_int, c_void};
use std::path::Path;
use std::time::Duration;

use anyhow::{anyhow, Result};
use rodio::source::SeekError;
use rodio::Source;

const OUT_CHANNELS: u16 = 2;
const SFMT_FLOAT: c_int = 4; // LIBVGMSTREAM_SFMT_FLOAT

/// Extensions routed to libvgmstream: game-specific stream formats that nothing else in LinAmp
/// handles. Deliberately an allowlist, not "ask the library" - libvgmstream also claims generic
/// container names (.ogg/.wav/.aac/...) that the pure-Rust and ffmpeg paths own, and we must never
/// let it seize those. ADX/AT3/ATRAC already route to ffdec, so they are not here.
pub const VGMSTREAM_EXTS: &[&str] = &[
    "genh", // vgmstream generic header (wraps raw PCM/ADPCM)
    "dsp", "idsp", // Nintendo GameCube/Wii DSP ADPCM
    "vag", "svag", // Sony PS-ADPCM
    "hca", // CRI HCA
    "brstm", "bcstm", "bfstm", "bfwav", "bwav", // Nintendo stream/wave
    "at9",  // Sony ATRAC9
    "xvag", // Sony XVAG
    "fsb",  // FMOD sample bank
    "xwb",  // Microsoft XACT wave bank
    "hps",  // HAL (GameCube)
    "ktss", // Koei Tecmo KTSS
];
// Note: CRI AWB/ACB containers are not routed - .awb already belongs to AMR-WB in LinAmp, and .acb
// is only a cue sheet that points at the .awb, so neither is useful here.

pub fn is_vgmstream(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| VGMSTREAM_EXTS.iter().any(|x| e.eq_ignore_ascii_case(x)))
        .unwrap_or(false)
}

// ---- FFI: libvgmstream.h / libvgmstream_streamfile.h (core API, version 1.1.0) ----
// These structs mirror the C header field-for-field (order and types) because libvgmstream reads the
// config we pass and we read the format/decoder it fills, both by offset. #[repr(C)] reproduces the C
// layout and padding.

#[repr(C)]
struct LibStreamfile {
    _private: [u8; 0],
}

#[repr(C)]
struct VgmFormat {
    channels: c_int,
    sample_rate: c_int,
    sample_format: c_int, // libvgmstream_sfmt_t
    sample_size: c_int,
    channel_layout: u32,
    subsong_index: c_int,
    subsong_count: c_int,
    input_channels: c_int,
    stream_samples: i64,
    loop_start: i64,
    loop_end: i64,
    loop_flag: bool,
    play_forever: bool,
    play_samples: i64,
    stream_bitrate: c_int,
    codec_name: [u8; 128],
    layout_name: [u8; 128],
    meta_name: [u8; 128],
    stream_name: [u8; 256],
    format_id: c_int,
}

#[repr(C)]
struct VgmDecoder {
    buf: *mut c_void, // interleaved f32 (we force FLOAT); valid until the next render
    buf_samples: c_int, // frames in buf
    buf_bytes: c_int,
    done: bool,
}

#[repr(C)]
struct LibVgmstream {
    priv_: *mut c_void,
    format: *const VgmFormat,
    decoder: *mut VgmDecoder,
}

#[repr(C)]
struct VgmConfig {
    disable_config_override: bool,
    allow_play_forever: bool,
    play_forever: bool,
    ignore_loop: bool,
    force_loop: bool,
    really_force_loop: bool,
    ignore_fade: bool,
    loop_count: f64,
    fade_time: f64,
    fade_delay: f64,
    stereo_track: c_int,
    auto_downmix_channels: c_int,
    force_sfmt: c_int, // libvgmstream_sfmt_t
}

impl VgmConfig {
    /// Play once with loops disabled (finite length), cap multichannel to stereo, output f32.
    fn player_defaults() -> Self {
        VgmConfig {
            disable_config_override: false,
            allow_play_forever: false,
            play_forever: false,
            ignore_loop: true,
            force_loop: false,
            really_force_loop: false,
            ignore_fade: false,
            loop_count: 0.0,
            fade_time: 0.0,
            fade_delay: 0.0,
            stereo_track: 0,
            auto_downmix_channels: OUT_CHANNELS as c_int,
            force_sfmt: SFMT_FLOAT,
        }
    }
}

extern "C" {
    fn libstreamfile_open_from_stdio(filename: *const c_char) -> *mut LibStreamfile;
    fn libstreamfile_close(sf: *mut LibStreamfile);
    fn libvgmstream_create(
        sf: *mut LibStreamfile,
        subsong: c_int,
        cfg: *mut VgmConfig,
    ) -> *mut LibVgmstream;
    fn libvgmstream_free(v: *mut LibVgmstream);
    fn libvgmstream_render(v: *mut LibVgmstream) -> c_int;
    fn libvgmstream_seek(v: *mut LibVgmstream, sample: i64);
}

/// A seekable rodio `Source` that streams one vgmstream subsong on demand. vgmstream seeks natively,
/// so there is no cache: it renders a batch at a time and a seek is a direct library call. Output is
/// always stereo f32 (mono duplicated, multichannel downmixed to two).
pub struct VgmSource {
    v: *mut LibVgmstream, // live decoder; freed on Drop
    sf: *mut LibStreamfile, // backing file; closed on Drop, separate from v
    src_channels: u16,    // channels vgmstream produces (1 or 2 after the downmix cap)
    rate: u32,
    total: Option<Duration>,
    buf: Vec<f32>, // current render batch, interleaved (src_channels)
    pos: usize,
    done: bool,
    dup: Option<f32>, // for a mono source: the held sample to emit as the second channel
}

// The handle and file are only ever touched from the single thread that owns the source (the audio
// thread, after the source is moved there); each vgmstream context is independent, so this is safe.
unsafe impl Send for VgmSource {}

impl Drop for VgmSource {
    fn drop(&mut self) {
        unsafe {
            libvgmstream_free(self.v);
            libstreamfile_close(self.sf);
        }
    }
}

impl VgmSource {
    /// Ensure `buf` has an unread sample, rendering more batches as needed. Returns false when the
    /// stream has truly ended.
    fn fill(&mut self) -> bool {
        // Guard against a decoder that reports 0 samples without finishing (shouldn't happen, but a
        // runaway loop would hang the audio thread).
        let mut guard = 0;
        while self.pos >= self.buf.len() {
            if self.done {
                return false;
            }
            let r = unsafe { libvgmstream_render(self.v) };
            if r < 0 {
                self.done = true;
                return false;
            }
            let dec = unsafe { &*(*self.v).decoder };
            let n = dec.buf_samples.max(0) as usize;
            if n > 0 {
                let total = n * self.src_channels as usize;
                self.buf.clear();
                self.buf
                    .extend_from_slice(unsafe { std::slice::from_raw_parts(dec.buf as *const f32, total) });
                self.pos = 0;
            }
            if dec.done {
                self.done = true;
            }
            guard += 1;
            if guard > 4096 {
                self.done = true;
                return false;
            }
        }
        true
    }
}

impl Iterator for VgmSource {
    type Item = f32;
    fn next(&mut self) -> Option<f32> {
        if let Some(r) = self.dup.take() {
            return Some(r); // second channel of a mono frame
        }
        if self.pos >= self.buf.len() && !self.fill() {
            return None;
        }
        let s = self.buf[self.pos];
        self.pos += 1;
        if self.src_channels == 1 {
            self.dup = Some(s); // emit the same sample again as the right channel
        }
        Some(s)
    }
}

impl Source for VgmSource {
    fn current_frame_len(&self) -> Option<usize> {
        None
    }
    fn channels(&self) -> u16 {
        OUT_CHANNELS
    }
    fn sample_rate(&self) -> u32 {
        self.rate
    }
    fn total_duration(&self) -> Option<Duration> {
        self.total
    }
    fn try_seek(&mut self, pos: Duration) -> Result<(), SeekError> {
        let target = (pos.as_secs_f64() * self.rate as f64).round().max(0.0) as i64;
        unsafe { libvgmstream_seek(self.v, target) };
        self.buf.clear();
        self.pos = 0;
        self.dup = None;
        self.done = false; // a backward seek re-enables rendering past a previous end
        Ok(())
    }
}

/// Open a vgmstream file (default subsong) as a lazy, seekable stereo source. Errors cleanly when the
/// file is not something vgmstream's core build can decode, so the skip-unplayable path takes over.
pub fn open(path: &Path) -> Result<VgmSource> {
    let cpath = CString::new(path.as_os_str().to_string_lossy().as_bytes())
        .map_err(|_| anyhow!("path has an interior NUL: {}", path.display()))?;
    let sf = unsafe { libstreamfile_open_from_stdio(cpath.as_ptr()) };
    if sf.is_null() {
        return Err(anyhow!("cannot open {}", path.display()));
    }
    let mut cfg = VgmConfig::player_defaults();
    let v = unsafe { libvgmstream_create(sf, 0, &mut cfg) };
    if v.is_null() {
        unsafe { libstreamfile_close(sf) };
        return Err(anyhow!("libvgmstream could not decode {}", path.display()));
    }
    let fmt = unsafe { &*(*v).format };
    let src_channels = fmt.channels.clamp(1, OUT_CHANNELS as c_int) as u16;
    let rate = fmt.sample_rate.max(1) as u32;
    let samples = if fmt.play_samples > 0 {
        fmt.play_samples
    } else {
        fmt.stream_samples
    };
    let total = (samples > 0).then(|| Duration::from_secs_f64(samples as f64 / rate as f64));

    Ok(VgmSource {
        v,
        sf,
        src_channels,
        rate,
        total,
        buf: Vec::new(),
        pos: 0,
        done: false,
        dup: None,
    })
}

/// Length of a vgmstream file for the playlist (opens it to read the decoded length), or `None` when
/// the file isn't a vgmstream format or can't be decoded.
pub fn duration(path: &Path) -> Option<Duration> {
    if !is_vgmstream(path) {
        return None;
    }
    open(path).ok().and_then(|s| s.total_duration())
}

/// Sample rate, channels, and bitrate (kbps) for the playlist display, read from the file's own
/// stream info (vgmstream carries no lofty-readable tags). `None` when the file isn't a vgmstream
/// format or can't be decoded. Mirrors `ffdec::media_info`.
pub fn media_info(path: &Path) -> Option<(u32, u8, u32)> {
    if !is_vgmstream(path) {
        return None;
    }
    let cpath = CString::new(path.as_os_str().to_string_lossy().as_bytes()).ok()?;
    let sf = unsafe { libstreamfile_open_from_stdio(cpath.as_ptr()) };
    if sf.is_null() {
        return None;
    }
    let mut cfg = VgmConfig::player_defaults();
    let v = unsafe { libvgmstream_create(sf, 0, &mut cfg) };
    if v.is_null() {
        unsafe { libstreamfile_close(sf) };
        return None;
    }
    let fmt = unsafe { &*(*v).format };
    let rate = fmt.sample_rate.max(0) as u32;
    let ch = fmt.channels.clamp(0, u8::MAX as c_int) as u8;
    let kbps = (fmt.stream_bitrate.max(0) / 1000) as u32;
    unsafe {
        libvgmstream_free(v);
        libstreamfile_close(sf);
    }
    (rate > 0).then_some((rate, ch, kbps))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_vgmstream_matches_the_allowlist() {
        for e in ["genh", "GENH", "dsp", "hca", "brstm", "vag", "at9", "fsb"] {
            assert!(is_vgmstream(&std::path::PathBuf::from(format!("x.{e}"))), ".{e}");
        }
        // generic container names must never route here (the pure-Rust / ffmpeg paths own them)
        for e in ["ogg", "wav", "flac", "mp3", "aac", "opus", "adx", "at3", "oma"] {
            assert!(!is_vgmstream(&std::path::PathBuf::from(format!("x.{e}"))), ".{e}");
        }
        assert!(!is_vgmstream(std::path::Path::new("noext")));
    }

    #[test]
    fn open_errors_cleanly_on_non_vgmstream() {
        let p = std::env::temp_dir().join("linamp_not_a_vgmstream.genh");
        std::fs::write(&p, b"this is definitely not a vgmstream file").unwrap();
        assert!(open(&p).is_err());
        let _ = std::fs::remove_file(&p);
    }

    /// Render every vgmstream sample present and confirm it produces non-silent stereo audio.
    /// libvgmstream is the only decoder, so this is a decode/render smoke test. (qa-music is
    /// gitignored, so it skips on CI.)
    #[test]
    fn renders_real_vgmstream_files() {
        let dir = std::path::Path::new("qa-music/format-samples");
        if !dir.exists() {
            return;
        }
        let mut checked = 0;
        for entry in std::fs::read_dir(dir).unwrap().flatten() {
            let path = entry.path();
            if !is_vgmstream(&path) {
                continue;
            }
            let src = open(&path).unwrap_or_else(|e| panic!("open {}: {e}", path.display()));
            assert_eq!(src.channels(), 2, "{}: stereo out", path.display());
            assert!(src.sample_rate() >= 8000, "{}: plausible rate", path.display());
            let got: Vec<f32> = src.take(44_100 * 2 * 10).collect();
            assert!(got.len() > 44_100, "{}: too little audio", path.display());
            let peak = got.iter().fold(0.0f32, |m, &s| m.max(s.abs()));
            assert!(peak > 0.003, "{}: near-silence (peak {peak})", path.display());
            checked += 1;
        }
        assert!(checked > 0, "no vgmstream samples found to render");
    }

    /// Streaming seek: a forward seek lands near the target with audio, a backward seek replays
    /// earlier audio. vgmstream seeks natively, so both are direct library calls. Local sample only.
    #[test]
    fn seeking_forward_then_back_produces_audio() {
        let p = std::path::Path::new("qa-music/format-samples/genh-sample.genh");
        if !p.exists() {
            return;
        }
        let mut src = open(p).expect("load genh");
        assert!(src.total_duration().unwrap() > Duration::from_secs(20));
        src.try_seek(Duration::from_secs(10)).unwrap();
        let fwd: Vec<f32> = (&mut src).take(44_100).collect();
        assert!(fwd.len() > 1000, "no audio after forward seek");
        assert!(fwd.iter().any(|&s| s.abs() > 0.003), "silence after forward seek");
        src.try_seek(Duration::from_secs(1)).unwrap();
        let back: Vec<f32> = (&mut src).take(44_100).collect();
        assert!(back.iter().any(|&s| s.abs() > 0.003), "silence after backward seek");
    }
}
