//! Musepack (.mpc) playback via the pure-Rust `oxideav-musepack` crate.
//!
//! symphonia has no Musepack decoder, so, like APE, WavPack and TTA, Musepack is a
//! special-case side-decoder: `oxideav-musepack` (pure Rust, MIT) decodes the whole
//! stream to interleaved samples, wrapped here as a seekable rodio `Source`. One
//! call, `decode_mpc_stream_tagged`, handles both stream generations (SV7 `MP+` and
//! SV8 `MPCK`) and skips a leading ID3v2 tag block; seeking is an index into the
//! decoded buffer.
//!
//! Mono and stereo; more than two channels (the rare stream-level-3 case) is refused
//! cleanly so the skip-unplayable guard takes over. Vetted before use: real-music
//! SV8 decode within 1 LSB of the ffmpeg `mpc8` oracle at ~31x real-time, and the
//! crate's own real mppenc SV7/SV8 fixtures within 1 LSB.

use std::fs;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{anyhow, Result};
use rodio::source::SeekError;
use rodio::Source;

/// True when `path` is a Musepack file routed through this module. Covers the
/// modern `.mpc` and the legacy SV7 extensions `.mp+` / `.mpp`.
pub fn is_mpc(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| {
            e.eq_ignore_ascii_case("mpc")
                || e.eq_ignore_ascii_case("mp+")
                || e.eq_ignore_ascii_case("mpp")
        })
        .unwrap_or(false)
}

/// The slot a background decode thread fills once the whole-file decode finishes.
struct DecodeSlot {
    /// Interleaved f32 samples, moved in when the decode completes (empty on a
    /// decode failure, which ends the source so the next track plays).
    samples: Mutex<Vec<f32>>,
}

/// A seekable rodio `Source` over a Musepack stream. The decode can run eagerly
/// ([`open_mpc`], used by the shared `open_source` dispatch and the viz test) or on
/// a background thread ([`open_mpc_streaming`], used for playback) so a multi-second
/// whole-file decode never blocks the UI. Until the samples are ready the source
/// yields silence, then plays the decoded audio from the start, the same ready-flag
/// idea the CD source uses (#160).
pub struct MpcSource {
    channels: u16,
    sample_rate: u32,
    total: Option<Duration>,
    pos: usize,
    /// The decoded samples once available, taken out of `slot` on the first poll
    /// after `ready`. `None` while a background decode is still running.
    decoded: Option<Vec<f32>>,
    slot: Arc<DecodeSlot>,
    /// Set by the decode thread when `slot` is filled. Shared with the player via
    /// [`MpcSource::ready_flag`] so a crossfade's fade-in can wait for real audio.
    ready: Arc<AtomicBool>,
}

impl MpcSource {
    /// Move the decoded buffer out of the shared slot once the decode thread has
    /// signalled ready. True when samples are available to play.
    fn ensure_decoded(&mut self) -> bool {
        if self.decoded.is_some() {
            return true;
        }
        if self.ready.load(Ordering::Acquire) {
            let s = std::mem::take(&mut *self.slot.samples.lock().unwrap_or_else(|e| e.into_inner()));
            self.decoded = Some(s);
            true
        } else {
            false
        }
    }

    /// Readiness flag shared with the background decode thread, so the player can
    /// hold a crossfade's fade-in until real audio flows (mirrors the CD source).
    /// Already set for the eager [`open_mpc`].
    pub fn ready_flag(&self) -> Arc<AtomicBool> {
        self.ready.clone()
    }
}

impl Iterator for MpcSource {
    type Item = f32;
    fn next(&mut self) -> Option<f32> {
        if !self.ensure_decoded() {
            // Background decode still running: yield silence so the sink stays
            // alive and the UI never blocks; real audio starts once it is ready.
            return Some(0.0);
        }
        let d = self.decoded.as_ref().unwrap();
        let s = d.get(self.pos).copied();
        if s.is_some() {
            self.pos += 1;
        }
        s
    }
}

impl Source for MpcSource {
    fn current_frame_len(&self) -> Option<usize> {
        None
    }
    fn channels(&self) -> u16 {
        self.channels
    }
    fn sample_rate(&self) -> u32 {
        self.sample_rate
    }
    fn total_duration(&self) -> Option<Duration> {
        self.total
    }
    fn try_seek(&mut self, pos: Duration) -> Result<(), SeekError> {
        // Can't seek into audio that isn't decoded yet; ignore during the brief
        // pre-roll (a seek then is effectively a no-op from the start).
        if !self.ensure_decoded() {
            return Ok(());
        }
        let frame = (pos.as_secs_f64() * self.sample_rate as f64).round() as usize;
        self.pos = frame
            .saturating_mul(self.channels as usize)
            .min(self.decoded.as_ref().map(|d| d.len()).unwrap_or(0));
        Ok(())
    }
}

/// Decode Musepack `bytes` fully into (channels, sample rate, interleaved f32
/// samples). Errors on a non-Musepack buffer, a decode failure, an unknown sample
/// rate, or more than two channels.
fn decode_bytes(bytes: &[u8], path: &Path) -> Result<(u16, u32, Vec<f32>)> {
    let stream = oxideav_musepack::mpc_decode::decode_mpc_stream_tagged(bytes)
        .map_err(|e| anyhow!("Musepack decode {}: {e:?}", path.display()))?;
    let channels = stream.channels();
    if channels == 0 {
        return Err(anyhow!("Musepack has no audio"));
    }
    if channels > 2 {
        return Err(anyhow!("Musepack with more than 2 channels is not supported"));
    }
    let sample_rate = stream
        .sample_rate_hz()
        .ok_or_else(|| anyhow!("Musepack has an unknown sample rate"))?;
    // The crate hands back interleaved samples in the signed-16-bit domain (f64);
    // scale to f32 in [-1, 1) by the full-scale 16-bit magnitude.
    let samples: Vec<f32> = stream.pcm().iter().map(|&v| (v / 32_768.0) as f32).collect();
    Ok((channels as u16, sample_rate, samples))
}

/// Read Musepack channels / sample rate / duration header-only (no audio decode),
/// so [`open_mpc_streaming`] can report the format before the decode finishes.
/// lofty reads the SV7/SH properties header-only for both stream generations.
fn mpc_header(path: &Path) -> Option<(u16, u32, Option<Duration>)> {
    use lofty::file::AudioFile;
    let tagged = lofty::read_from_path(path).ok()?;
    let p = tagged.properties();
    let sr = p.sample_rate()?;
    let ch = u16::from(p.channels()?);
    let d = p.duration();
    Some((ch, sr, (!d.is_zero()).then_some(d)))
}

/// Decode a Musepack file fully and wrap it as a seekable source, ready to play
/// immediately. Used by the shared `open_source` dispatch and the viz test. Errors
/// cleanly on an unreadable file, a non-Musepack buffer, a decode failure, an
/// unknown sample rate, or more than two channels, so skip-unplayable takes over.
pub fn open_mpc(path: &Path) -> Result<MpcSource> {
    let bytes = fs::read(path)?;
    let (channels, sample_rate, samples) = decode_bytes(&bytes, path)?;
    let frames = samples.len() / channels as usize;
    let total = (sample_rate > 0 && frames > 0)
        .then(|| Duration::from_secs_f64(frames as f64 / sample_rate as f64));
    Ok(MpcSource {
        channels,
        sample_rate,
        total,
        pos: 0,
        decoded: Some(samples),
        slot: Arc::new(DecodeSlot { samples: Mutex::new(Vec::new()) }),
        ready: Arc::new(AtomicBool::new(true)),
    })
}

/// Open a Musepack file for playback, decoding the whole stream on a background
/// thread so the multi-second decode never blocks the UI thread (the freeze this
/// fixes). The source reports its format up front from a header-only read and yields
/// silence until the decode lands, then plays from the start. Falls back to the
/// eager [`open_mpc`] when the header can't be read, so correctness is never traded
/// for the async path; a `channels`/`sample rate` the header can't give, or a
/// channel count over two, is refused up front so skip-unplayable takes over.
pub fn open_mpc_streaming(path: &Path) -> Result<MpcSource> {
    let Some((channels, sample_rate, total)) = mpc_header(path) else {
        // Unreadable header (rare): decode eagerly so the file still plays correctly.
        return open_mpc(path);
    };
    if channels == 0 || channels > 2 {
        return Err(anyhow!("Musepack with {channels} channels is not supported"));
    }
    let bytes = fs::read(path)?;
    let slot = Arc::new(DecodeSlot { samples: Mutex::new(Vec::new()) });
    let ready = Arc::new(AtomicBool::new(false));
    {
        let slot = slot.clone();
        let ready = ready.clone();
        let path = path.to_path_buf();
        std::thread::spawn(move || {
            // On a decode failure, leave the slot empty: the source then ends at
            // once and the player advances, as the eager open's error would.
            let samples = decode_bytes(&bytes, &path).map(|(_, _, s)| s).unwrap_or_default();
            *slot.samples.lock().unwrap_or_else(|e| e.into_inner()) = samples;
            ready.store(true, Ordering::Release);
        });
    }
    Ok(MpcSource {
        channels,
        sample_rate,
        total,
        pos: 0,
        decoded: None,
        slot,
        ready,
    })
}

/// The length of a Musepack file, for the playlist, without decoding the audio.
/// SV7 is read straight from its fixed header; anything else (SV8, or a tagged
/// stream) falls back to lofty, which reads Musepack properties header-only.
/// `None` when it isn't a Musepack file or the length can't be determined.
pub fn duration(path: &Path) -> Option<Duration> {
    if !is_mpc(path) {
        return None;
    }
    // SV7: the fixed §1 header carries the sample total and rate directly, so this
    // is a cheap parse independent of lofty.
    if let Ok(bytes) = fs::read(path) {
        if let Ok(h) = oxideav_musepack::sv7_header::Sv7HeaderFields::parse(&bytes) {
            if let Some(sr) = h.sample_rate_hz() {
                let total = h.effective_total_samples();
                if sr > 0 && total > 0 {
                    return Some(Duration::from_secs_f64(total as f64 / f64::from(sr)));
                }
            }
        }
    }
    // SV8 (and ID3v2-prefixed streams): lofty reads the SH properties header-only.
    use lofty::file::AudioFile;
    let tagged = lofty::read_from_path(path).ok()?;
    let d = tagged.properties().duration();
    (!d.is_zero()).then_some(d)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Option<std::path::PathBuf> {
        let p = std::path::PathBuf::from("qa-music/format-samples/hypno-this.mpc");
        p.exists().then_some(p)
    }

    /// Decode hypno-this.mpc and check it against an ffmpeg-CLI decode of the same
    /// file (the oracle: `ffmpeg -i hypno-this.mpc -f s16le -ac 2 -ar 44100`).
    /// Musepack is lossy, but both paths decode the same bitstream, so the output
    /// must match the oracle to within 1 LSB. The oracle emits the synthesis-priming
    /// lead the crate trims, so the comparison drops the first 481 frames of the
    /// oracle. (qa-music is gitignored, so this skips on CI.)
    #[test]
    fn decodes_within_1_lsb_of_the_ffmpeg_oracle() {
        let Some(mpc) = sample() else { return };
        let oracle_path = std::path::PathBuf::from("qa-music/format-samples/oracles/hypno-mpc-oracle.s16");
        if !oracle_path.exists() {
            return;
        }
        let raw = std::fs::read(&oracle_path).unwrap();
        let oracle: Vec<i16> = raw
            .as_chunks::<2>().0.iter()
            .map(|b| i16::from_le_bytes([b[0], b[1]]))
            .collect();

        let src = open_mpc(&mpc).unwrap();
        assert_eq!(src.channels(), 2, "stereo");
        assert_eq!(src.sample_rate(), 44_100);
        let got: Vec<i16> = src
            .map(|v| (v * 32_768.0).round().clamp(-32_768.0, 32_767.0) as i16)
            .collect();

        // Drop the oracle's 481-sample (per channel) synthesis-priming lead.
        const LEAD_FRAMES: usize = 481;
        let off = LEAD_FRAMES * 2;
        let n = got.len().min(oracle.len().saturating_sub(off));
        assert!(n > 44_100 * 2, "too few comparable samples: {n}");
        let mut max_d = 0i32;
        for i in 0..n {
            let d = (i32::from(got[i]) - i32::from(oracle[off + i])).abs();
            if d > max_d {
                max_d = d;
            }
        }
        assert!(max_d <= 1, "max sample diff vs ffmpeg oracle too high: {max_d}");
    }

    /// Seeking must land on the right samples: decode, seek to 40%, and confirm the
    /// first samples after the seek match the oracle at that offset (within 1 LSB).
    #[test]
    fn seek_lands_on_the_right_samples() {
        let Some(mpc) = sample() else { return };
        let oracle_path = std::path::PathBuf::from("qa-music/format-samples/oracles/hypno-mpc-oracle.s16");
        if !oracle_path.exists() {
            return;
        }
        let raw = std::fs::read(&oracle_path).unwrap();
        let oracle: Vec<i16> = raw
            .as_chunks::<2>().0.iter()
            .map(|b| i16::from_le_bytes([b[0], b[1]]))
            .collect();

        let mut src = open_mpc(&mpc).unwrap();
        let sr = src.sample_rate();
        let secs = src.total_duration().unwrap().as_secs_f64() * 0.4;
        src.try_seek(Duration::from_secs_f64(secs)).unwrap();
        let got: Vec<i16> = src
            .take(8192)
            .map(|v| (v * 32_768.0).round().clamp(-32_768.0, 32_767.0) as i16)
            .collect();

        const LEAD_FRAMES: usize = 481;
        let start = ((secs * f64::from(sr)).round() as usize + LEAD_FRAMES) * 2;
        let mut max_d = 0i32;
        let mut n = 0usize;
        for (g, o) in got.iter().zip(oracle[start.min(oracle.len())..].iter()) {
            let d = (i32::from(*g) - i32::from(*o)).abs();
            if d > max_d {
                max_d = d;
            }
            n += 1;
        }
        assert!(n > 1000, "too few post-seek samples: {n}");
        assert!(max_d <= 1, "post-seek diff vs oracle too high: {max_d}");
    }

    #[test]
    fn duration_reads_without_full_decode() {
        let Some(mpc) = sample() else { return };
        let d = duration(&mpc).expect("mpc duration");
        assert!(d.as_secs() > 1 && d.as_secs() < 600, "duration {d:?}");
    }

    /// The background-decode open reports the format up front from the header and,
    /// once the decode thread finishes, plays the exact same samples as the eager
    /// open, from the start (no silence consumed before `ready`).
    #[test]
    fn streaming_open_matches_the_eager_decode() {
        let Some(mpc) = sample() else { return };
        let eager: Vec<f32> = open_mpc(&mpc).unwrap().collect();

        let src = open_mpc_streaming(&mpc).unwrap();
        // Format is known before the decode lands (header-only read).
        assert_eq!(src.channels(), 2, "stereo");
        assert_eq!(src.sample_rate(), 44_100);
        assert!(src.total_duration().is_some(), "duration from header");

        // Wait for the background decode (bounded), before pulling any samples so no
        // pre-roll silence is consumed.
        let ready = src.ready_flag();
        let start = std::time::Instant::now();
        while !ready.load(Ordering::Acquire) {
            assert!(start.elapsed() < Duration::from_secs(30), "decode never finished");
            std::thread::sleep(Duration::from_millis(20));
        }
        let got: Vec<f32> = src.collect();
        assert_eq!(got.len(), eager.len(), "same sample count as the eager decode");
        assert!(got == eager, "streamed samples match the eager decode");
    }
}
