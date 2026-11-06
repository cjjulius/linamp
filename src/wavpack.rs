//! WavPack (.wv) playback via the pure-Rust `wavicle` crate.
//!
//! symphonia 0.6's `wavpack` feature is a non-functional stub (see
//! `decode::tests`), so, like MIDI, trackers, APE and AC-3, WavPack is a special
//! case: `wavicle` (pure Rust, MIT/Apache-2.0, `#![forbid(unsafe_code)]`, verified
//! against the reference `wvunpack`/`wavpack`) decodes the whole stream to
//! interleaved samples, wrapped here as a seekable rodio `Source`.
//!
//! Scope (vetted empirically, not just from the crate's docs): lossless 16- and
//! 24-bit integer, mono/stereo, any sample rate. Float, 8-bit, hybrid/lossy,
//! correction files and more than two channels are refused cleanly, so the
//! skip-unplayable guard takes over rather than playing noise. The float guard is
//! ours: `wavicle` 0.1 decodes a float stream to noise while returning Ok, so we
//! reject it up front. The decode is whole-file (the crate exposes no frame API),
//! so seeking is just an index into the decoded buffer. Vetted bit-exact against
//! the source PCM across these variants before use (see the tests).

use std::fs;
use std::path::Path;
use std::time::Duration;

use anyhow::{anyhow, Result};
use rodio::source::SeekError;
use rodio::Source;

/// True when `path` is a WavPack file routed through this module.
pub fn is_wavpack(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| e.eq_ignore_ascii_case("wv"))
        .unwrap_or(false)
}

/// A seekable rodio `Source` over a fully-decoded WavPack stream (interleaved
/// f32 in [-1, 1]).
pub struct WavpackSource {
    samples: Vec<f32>,
    channels: u16,
    sample_rate: u32,
    pos: usize,
    total: Option<Duration>,
}

impl Iterator for WavpackSource {
    type Item = f32;
    fn next(&mut self) -> Option<f32> {
        let s = self.samples.get(self.pos).copied();
        if s.is_some() {
            self.pos += 1;
        }
        s
    }
}

impl Source for WavpackSource {
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

/// How many leading bytes of `bytes` are WavPack audio blocks. Real `.wv` files
/// almost always carry a trailing APEv2 tag (and sometimes an ID3v1 trailer),
/// which `wavicle` 0.1 treats as a hard error ("BadMagic") rather than stopping
/// at the audio, so trim to the audio-block run before handing it the stream.
/// Walks block headers (each `ck_size + 8` bytes) and stops at the first that
/// doesn't parse as a WavPack block.
fn audio_extent(bytes: &[u8]) -> usize {
    use wavicle::block::BlockHeader;
    let mut off = 0usize;
    while off + 32 <= bytes.len() {
        match BlockHeader::parse(&bytes[off..]) {
            Ok(h) => {
                let len = h.block_len();
                if len < 32 || off + len > bytes.len() {
                    break;
                }
                off += len;
            }
            Err(_) => break,
        }
    }
    off
}

/// Decode a WavPack file fully and wrap it as a seekable source. Errors cleanly on
/// an unreadable file or a variant `wavicle` 0.1 doesn't implement (24/32-bit,
/// float, >2 channels), so the skip-unplayable path takes over.
pub fn open_wavpack(path: &Path) -> Result<WavpackSource> {
    let bytes = fs::read(path)?;
    let audio = &bytes[..audio_extent(&bytes)];
    // Refuse float WavPack up front: wavicle 0.1 decodes a float stream to noise
    // while still returning Ok, so without this guard a float .wv would play
    // garbage instead of being skipped. (16- and 24-bit integer decode correctly,
    // vetted bit-exact.)
    if wavicle::block::StreamInfo::scan(audio)
        .map(|i| i.is_float)
        .unwrap_or(false)
    {
        return Err(anyhow!("floating-point WavPack is not supported"));
    }
    let decoded = wavicle::decode::decode_stream(audio)
        .map_err(|e| anyhow!("WavPack decode {}: {e:?}", path.display()))?;
    if decoded.channels == 0 || decoded.sample_rate == 0 {
        return Err(anyhow!("WavPack has no audio"));
    }
    if decoded.is_float {
        return Err(anyhow!("floating-point WavPack is not supported"));
    }
    if decoded.channels > 2 {
        return Err(anyhow!("WavPack with more than 2 channels is not supported"));
    }
    // wavicle hands back interleaved integer samples in an i32, at the stream's
    // native bit depth; scale to f32 in [-1, 1] by that depth (16 and 24-bit both
    // decode correctly, so the shift must track bits_per_sample, not a constant).
    let scale = (1i64 << (decoded.bits_per_sample.max(1) - 1)) as f32;
    let samples: Vec<f32> = decoded.samples.iter().map(|&s| s as f32 / scale).collect();
    let frames = samples.len() / decoded.channels as usize;
    let total = Some(Duration::from_secs_f64(
        frames as f64 / decoded.sample_rate as f64,
    ));
    Ok(WavpackSource {
        channels: decoded.channels as u16,
        sample_rate: decoded.sample_rate,
        samples,
        pos: 0,
        total,
    })
}

/// The length of a WavPack file, so the playlist can show a time when lofty
/// can't. Reads just the block headers (no full decode). `None` when it isn't a
/// WavPack file, can't be read, or the stream doesn't carry a total sample count.
pub fn duration(path: &Path) -> Option<Duration> {
    if !is_wavpack(path) {
        return None;
    }
    let bytes = fs::read(path).ok()?;
    let audio = &bytes[..audio_extent(&bytes)];
    let info = wavicle::block::StreamInfo::scan(audio).ok()?;
    let total = info.total_samples?;
    if info.sample_rate == 0 {
        return None;
    }
    Some(Duration::from_secs_f64(total as f64 / info.sample_rate as f64))
}


#[cfg(test)]
mod tests {
    use super::*;

    const DIR: &str = "qa-music/format-samples/";

    fn path(name: &str) -> Option<std::path::PathBuf> {
        let p = std::path::PathBuf::from(format!("{DIR}{name}"));
        p.exists().then_some(p)
    }

    /// A WAV, decoded to normalized f32, as the bit-exact oracle. (The .wv variants
    /// are lossless transcodes of the same clip, so a correct decode matches the WAV
    /// sample-for-sample at 16-bit, and within rounding for 24-bit upconverts.)
    fn wav_f32(name: &str) -> (u16, u32, Vec<f32>) {
        let mut r = hound::WavReader::open(format!("{DIR}{name}")).unwrap();
        let spec = r.spec();
        let s = r.samples::<i16>().map(|x| x.unwrap() as f32 / 32_768.0).collect();
        (spec.channels, spec.sample_rate, s)
    }

    fn rms(a: &[f32], b: &[f32]) -> f64 {
        let n = a.len().min(b.len());
        let acc: f64 = (0..n).map(|i| ((a[i] - b[i]) as f64).powi(2)).sum();
        (acc / n.max(1) as f64).sqrt()
    }

    /// 16-bit stereo (the common case), including the trailing APEv2 tag that every
    /// real tagger writes and wavicle itself can't skip: must decode bit-exact.
    #[test]
    fn decodes_16bit_stereo_bit_exact() {
        let Some(wv) = path("hypno-this.wv") else { return };
        let (ch, sr, want) = wav_f32("hypno-this.wav");
        let src = open_wavpack(&wv).unwrap();
        assert_eq!(src.channels(), ch);
        assert_eq!(src.sample_rate(), sr);
        let got: Vec<f32> = src.collect();
        assert_eq!(got.len(), want.len());
        assert_eq!(rms(&got, &want), 0.0, "16-bit stereo must be bit-exact");
    }

    /// Mono path.
    #[test]
    fn decodes_mono_bit_exact() {
        let Some(wv) = path("hypno-mono.wv") else { return };
        let (ch, _, want) = wav_f32("hypno-mono.wav");
        let src = open_wavpack(&wv).unwrap();
        assert_eq!(src.channels(), ch, "mono");
        let got: Vec<f32> = src.collect();
        assert_eq!(got.len(), want.len());
        assert_eq!(rms(&got, &want), 0.0, "mono must be bit-exact");
    }

    /// A non-44.1k rate is carried through.
    #[test]
    fn decodes_48k_stereo() {
        let Some(wv) = path("hypno-48k.wv") else { return };
        let (_, sr, want) = wav_f32("hypno-48k.wav");
        let src = open_wavpack(&wv).unwrap();
        assert_eq!(src.sample_rate(), sr);
        assert_eq!(sr, 48_000);
        let got: Vec<f32> = src.collect();
        assert_eq!(rms(&got, &want), 0.0);
    }

    /// 24-bit integer: wavicle's own docs say "not implemented", but it actually
    /// decodes 24-bit correctly, so we accept it. This is an upconvert of the 16-bit
    /// clip, so it must match the 16-bit source within rounding.
    #[test]
    fn decodes_24bit_integer() {
        let Some(wv) = path("hypno-32bit.wv") else { return };
        let (_, _, want) = wav_f32("hypno-this.wav");
        let src = open_wavpack(&wv).unwrap();
        let got: Vec<f32> = src.collect();
        assert_eq!(got.len(), want.len());
        assert!(rms(&got, &want) < 1e-4, "24-bit upconvert must match the 16-bit source");
    }

    /// An untagged file (no APEv2/ID3 trailer) decodes the same as the tagged one,
    /// so the audio-extent trim doesn't cut audio when there is no trailer.
    #[test]
    fn decodes_untagged() {
        let Some(wv) = path("hypno-notag.wv") else { return };
        let (_, _, want) = wav_f32("hypno-this.wav");
        let got: Vec<f32> = open_wavpack(&wv).unwrap().collect();
        assert_eq!(got.len(), want.len());
        assert_eq!(rms(&got, &want), 0.0);
    }

    /// Variants wavicle 0.1 can't decode correctly must be refused (returning an
    /// error, so the skip-unplayable guard takes over) rather than play noise:
    /// 8-bit, float, and more than two channels.
    #[test]
    fn refuses_unsupported_variants() {
        for name in ["hypno-8bit.wv", "hypno-float.wv", "hypno-5_1.wv"] {
            let Some(p) = path(name) else { continue };
            assert!(
                open_wavpack(&p).is_err(),
                "{name} is unsupported and must be refused, not mis-played"
            );
        }
    }

    #[test]
    fn seek_lands_on_the_right_samples() {
        let Some(wv) = path("hypno-this.wv") else { return };
        let (_, sr, want) = wav_f32("hypno-this.wav");
        let mut src = open_wavpack(&wv).unwrap();
        let ch = src.channels() as usize;
        let secs = src.total_duration().unwrap().as_secs_f64() * 0.4;
        src.try_seek(Duration::from_secs_f64(secs)).unwrap();
        let got: Vec<f32> = src.collect();
        let start = (secs * sr as f64).round() as usize * ch;
        assert_eq!(rms(&got, &want[start.min(want.len())..]), 0.0);
    }

    #[test]
    fn duration_reads_without_full_decode() {
        let Some(wv) = path("hypno-this.wv") else { return };
        let d = duration(&wv).expect("wavpack duration");
        assert!(d.as_secs() > 1 && d.as_secs() < 600, "duration {d:?}");
    }
}
