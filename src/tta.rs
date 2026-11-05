//! True Audio (.tta) playback via the pure-Rust `oxideav-tta` crate.
//!
//! symphonia has no TTA decoder, so, like APE and WavPack, TTA is a special-case
//! side-decoder: `oxideav-tta` (pure Rust, MIT) decodes the whole stream to
//! interleaved samples, wrapped here as a seekable rodio `Source`. The crate exposes
//! no frame API, so decode is whole-file and seeking is an index into the buffer.
//!
//! Lossless 16-bit (and deeper) mono/stereo; more than two channels is refused
//! cleanly so the skip-unplayable guard takes over. Vetted bit-exact against the
//! source PCM, and comfortably real-time (hundreds of times faster), before use.

use std::fs;
use std::path::Path;
use std::time::Duration;

use anyhow::{anyhow, Result};
use rodio::source::SeekError;
use rodio::Source;

/// True when `path` is a True Audio file routed through this module.
pub fn is_tta(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| e.eq_ignore_ascii_case("tta"))
        .unwrap_or(false)
}

/// A seekable rodio `Source` over a fully-decoded TTA stream (interleaved f32).
pub struct TtaSource {
    samples: Vec<f32>,
    channels: u16,
    sample_rate: u32,
    pos: usize,
    total: Option<Duration>,
}

impl Iterator for TtaSource {
    type Item = f32;
    fn next(&mut self) -> Option<f32> {
        let s = self.samples.get(self.pos).copied();
        if s.is_some() {
            self.pos += 1;
        }
        s
    }
}

impl Source for TtaSource {
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

/// Decode a TTA file fully and wrap it as a seekable source. Errors cleanly on an
/// unreadable file, a CRC failure, a password-protected stream, or more than two
/// channels, so the skip-unplayable path takes over.
pub fn open_tta(path: &Path) -> Result<TtaSource> {
    let bytes = fs::read(path)?;
    let (info, raw) = oxideav_tta::decode(&bytes)
        .map_err(|e| anyhow!("TTA decode {}: {e:?}", path.display()))?;
    if info.channels == 0 || info.sample_rate == 0 {
        return Err(anyhow!("TTA has no audio"));
    }
    if info.channels > 2 {
        return Err(anyhow!("TTA with more than 2 channels is not supported"));
    }
    // oxideav-tta hands back interleaved integer samples at the stream's native bit
    // depth; scale to f32 in [-1, 1] by that depth (16 and 24-bit both decode fine).
    let scale = (1i64 << (info.bits_per_sample.max(1) - 1)) as f32;
    let samples: Vec<f32> = raw.iter().map(|&s| s as f32 / scale).collect();
    let frames = samples.len() / info.channels as usize;
    let total = Some(Duration::from_secs_f64(frames as f64 / info.sample_rate as f64));
    Ok(TtaSource {
        channels: info.channels,
        sample_rate: info.sample_rate,
        samples,
        pos: 0,
        total,
    })
}

/// The length of a TTA file, so the playlist can show a time, from the fixed 22-byte
/// TTA1 header (no full decode; the crate's own header parser isn't public).
/// Layout: "TTA1", format(2), channels(2), bits(2), sample_rate(4 LE at 10),
/// total_samples(4 LE at 14), crc(4). `None` when it isn't a TTA or is too short.
pub fn duration(path: &Path) -> Option<Duration> {
    if !is_tta(path) {
        return None;
    }
    let mut buf = [0u8; 18];
    {
        use std::io::Read;
        let mut f = fs::File::open(path).ok()?;
        f.read_exact(&mut buf).ok()?;
    }
    if &buf[0..4] != b"TTA1" {
        return None;
    }
    let sample_rate = u32::from_le_bytes([buf[10], buf[11], buf[12], buf[13]]);
    let total_samples = u32::from_le_bytes([buf[14], buf[15], buf[16], buf[17]]);
    if sample_rate == 0 {
        return None;
    }
    Some(Duration::from_secs_f64(
        total_samples as f64 / sample_rate as f64,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Option<std::path::PathBuf> {
        let p = std::path::PathBuf::from("qa-music/format-samples/hypno-this.tta");
        p.exists().then_some(p)
    }

    /// TTA is lossless, so its decode must match the source WAV sample-for-sample.
    /// (qa-music is gitignored, so this skips on CI.)
    #[test]
    fn decodes_bit_exact_against_the_source_wav() {
        let Some(tta) = sample() else { return };
        let mut r = hound::WavReader::open("qa-music/format-samples/hypno-this.wav").unwrap();
        let spec = r.spec();
        let want: Vec<i16> = r.samples::<i16>().map(|x| x.unwrap()).collect();
        let src = open_tta(&tta).unwrap();
        assert_eq!(src.channels(), spec.channels);
        assert_eq!(src.sample_rate(), spec.sample_rate);
        let got: Vec<f32> = src.collect();
        assert_eq!(got.len(), want.len(), "sample count");
        for (i, (&g, &w)) in got.iter().zip(want.iter()).enumerate() {
            assert_eq!((g * 32_768.0).round() as i32, w as i32, "sample {i}");
        }
    }

    #[test]
    fn seek_lands_on_the_right_samples() {
        let Some(tta) = sample() else { return };
        let mut r = hound::WavReader::open("qa-music/format-samples/hypno-this.wav").unwrap();
        let want: Vec<i16> = r.samples::<i16>().map(|x| x.unwrap()).collect();
        let mut src = open_tta(&tta).unwrap();
        let (sr, ch) = (src.sample_rate(), src.channels() as usize);
        let secs = src.total_duration().unwrap().as_secs_f64() * 0.4;
        src.try_seek(Duration::from_secs_f64(secs)).unwrap();
        let got: Vec<f32> = src.collect();
        let start = (secs * sr as f64).round() as usize * ch;
        for (&g, &w) in got.iter().zip(want[start.min(want.len())..].iter()) {
            assert_eq!((g * 32_768.0).round() as i32, w as i32);
        }
    }

    #[test]
    fn duration_reads_without_full_decode() {
        let Some(tta) = sample() else { return };
        let d = duration(&tta).expect("tta duration");
        assert!(d.as_secs() > 1 && d.as_secs() < 600, "duration {d:?}");
    }
}
