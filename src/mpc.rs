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

/// A seekable rodio `Source` over a fully-decoded Musepack stream (interleaved f32).
pub struct MpcSource {
    samples: Vec<f32>,
    channels: u16,
    sample_rate: u32,
    pos: usize,
    total: Option<Duration>,
}

impl Iterator for MpcSource {
    type Item = f32;
    fn next(&mut self) -> Option<f32> {
        let s = self.samples.get(self.pos).copied();
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
        let frame = (pos.as_secs_f64() * self.sample_rate as f64).round() as usize;
        self.pos = frame
            .saturating_mul(self.channels as usize)
            .min(self.samples.len());
        Ok(())
    }
}

/// Decode a Musepack file fully and wrap it as a seekable source. Errors cleanly on
/// an unreadable file, a non-Musepack buffer, a decode failure, an unknown sample
/// rate, or more than two channels, so the skip-unplayable path takes over.
pub fn open_mpc(path: &Path) -> Result<MpcSource> {
    let bytes = fs::read(path)?;
    let stream = oxideav_musepack::mpc_decode::decode_mpc_stream_tagged(&bytes)
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
    let frames = samples.len() / channels as usize;
    let total = (sample_rate > 0 && frames > 0)
        .then(|| Duration::from_secs_f64(frames as f64 / sample_rate as f64));
    Ok(MpcSource {
        channels: channels as u16,
        sample_rate,
        samples,
        pos: 0,
        total,
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
        let oracle_path = std::path::PathBuf::from("qa-music/format-samples/hypno-mpc-oracle.s16");
        if !oracle_path.exists() {
            return;
        }
        let raw = std::fs::read(&oracle_path).unwrap();
        let oracle: Vec<i16> = raw
            .chunks_exact(2)
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
        let oracle_path = std::path::PathBuf::from("qa-music/format-samples/hypno-mpc-oracle.s16");
        if !oracle_path.exists() {
            return;
        }
        let raw = std::fs::read(&oracle_path).unwrap();
        let oracle: Vec<i16> = raw
            .chunks_exact(2)
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
}
