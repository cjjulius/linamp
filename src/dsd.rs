//! DSD (.dsf/.dff) playback.
//!
//! DSD (Direct Stream Digital) is 1-bit sigma-delta audio at very high rates (DSD64 =
//! 2.8224 MHz, DSD128/256 above). symphonia has no DSD support, so, like the other
//! side-decoders, this is a special case: `dsd-reader` (pure Rust, MIT/Apache) parses
//! the DSF/DFF container and yields the planar 1-bit stream; this module converts it to
//! normal PCM and wraps the result as a seekable rodio `Source`.
//!
//! DSD->PCM is a decimation problem: the 1-bit stream carries the audio in the low band
//! with the quantiser noise shaped up high, so playback is a low-pass + downsample. We
//! do it in two stages, both permissive and self-contained: a windowed-sinc decimating
//! FIR brings the DSD rate down to ~8x the output (removing most of the HF noise), then
//! `rubato` resamples that to 44.1 kHz with a sharp anti-alias filter (removing the
//! rest). Output is interleaved f32 stereo (mono is duplicated up by the caller path via
//! the channel count).
//!
//! Uncompressed DSD only, mono and stereo. DST-compressed DFF is refused cleanly (the
//! only DST decoder, cladst, is GPL, outside our permissive bar), as is anything with
//! more than two channels.

use std::path::Path;
use std::time::Duration;

use anyhow::{anyhow, Result};
use rodio::source::SeekError;
use rodio::Source;

const OUT_RATE: u32 = 44_100;

/// True when `path` is a DSD file routed through this module.
pub fn is_dsd(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| e.eq_ignore_ascii_case("dsf") || e.eq_ignore_ascii_case("dff"))
        .unwrap_or(false)
}

/// A seekable rodio `Source` over fully-decoded DSD (interleaved f32 at 44.1 kHz).
pub struct DsdSource {
    samples: Vec<f32>,
    channels: u16,
    pos: usize,
    total: Option<Duration>,
}

impl Iterator for DsdSource {
    type Item = f32;
    fn next(&mut self) -> Option<f32> {
        let s = self.samples.get(self.pos).copied();
        if s.is_some() {
            self.pos += 1;
        }
        s
    }
}

impl Source for DsdSource {
    fn current_frame_len(&self) -> Option<usize> {
        None
    }
    fn channels(&self) -> u16 {
        self.channels
    }
    fn sample_rate(&self) -> u32 {
        OUT_RATE
    }
    fn total_duration(&self) -> Option<Duration> {
        self.total
    }
    fn try_seek(&mut self, pos: Duration) -> Result<(), SeekError> {
        let frame = (pos.as_secs_f64() * OUT_RATE as f64).round() as usize;
        self.pos = frame
            .saturating_mul(self.channels as usize)
            .min(self.samples.len());
        Ok(())
    }
}

/// A streaming windowed-sinc low-pass FIR that decimates by `factor`. Fed one input
/// sample at a time; emits one output every `factor` inputs.
struct FirDecimator {
    taps: Vec<f32>,
    hist: Vec<f32>, // ring buffer, newest at `head`
    head: usize,
    since: usize, // inputs since the last emitted output
    factor: usize,
}

impl FirDecimator {
    /// `factor` decimation, cutoff at `cutoff_hz` relative to the input rate `in_rate`.
    fn new(factor: usize, cutoff_hz: f64, in_rate: f64, taps: usize) -> Self {
        let fc = cutoff_hz / in_rate; // normalised cutoff (0..0.5)
        let m = (taps - 1) as f64;
        let mut h = vec![0.0f32; taps];
        let mut sum = 0.0f64;
        for (n, slot) in h.iter_mut().enumerate() {
            let k = n as f64 - m / 2.0;
            let sinc = if k.abs() < 1e-9 {
                2.0 * fc
            } else {
                (2.0 * std::f64::consts::PI * fc * k).sin() / (std::f64::consts::PI * k)
            };
            // Blackman-Harris window (deep ~-92 dB stopband, to reject the DSD HF noise).
            let t = 2.0 * std::f64::consts::PI * n as f64 / m;
            let w = 0.35875 - 0.48829 * t.cos() + 0.14128 * (2.0 * t).cos()
                - 0.01168 * (3.0 * t).cos();
            let v = sinc * w;
            *slot = v as f32;
            sum += v;
        }
        // Normalise to unity DC gain.
        for v in &mut h {
            *v = (*v as f64 / sum) as f32;
        }
        FirDecimator {
            taps: h,
            hist: vec![0.0; taps],
            head: 0,
            since: 0,
            factor,
        }
    }

    /// Push one input sample; returns Some(output) on a decimation boundary.
    #[inline]
    fn push(&mut self, x: f32) -> Option<f32> {
        let n = self.hist.len();
        self.head = (self.head + 1) % n;
        self.hist[self.head] = x;
        self.since += 1;
        if self.since < self.factor {
            return None;
        }
        self.since = 0;
        // Dot product of taps with history, taps[0] against the newest sample.
        let mut acc = 0.0f32;
        let mut idx = self.head;
        for &t in &self.taps {
            acc += t * self.hist[idx];
            idx = if idx == 0 { n - 1 } else { idx - 1 };
        }
        Some(acc)
    }
}

/// Decode a DSD file fully and wrap it as a seekable source. Errors cleanly (unreadable
/// file, DST-compressed, unknown rate, more than two channels) so the skip-unplayable
/// path takes over.
pub fn open_dsd(path: &Path) -> Result<DsdSource> {
    let reader = dsd_reader::DsdReader::from_container(path.to_path_buf())
        .map_err(|e| anyhow!("DSD open {}: {e}", path.display()))?;
    let channels = reader.channels_num();
    if channels == 0 {
        return Err(anyhow!("DSD has no audio"));
    }
    if channels > 2 {
        return Err(anyhow!("DSD with more than 2 channels is not supported"));
    }
    // dsd_rate() is a multiplier over DSD64 (1 = 2.8224 MHz, 2 = DSD128, ...); the real
    // bit rate in Hz is that times DSD_64_RATE.
    let mult = reader.dsd_rate();
    if mult <= 0 {
        return Err(anyhow!("DSD has an unknown rate"));
    }
    let dsd_rate = mult as f64 * dsd_reader::DSD_64_RATE as f64;

    // DSD rates are 44100 * 2^n (DSD64 = 64x), so the decimation to 44.1 kHz is an exact
    // integer. Do it in two streaming stages per channel: stage A decimates most of the
    // way with a short FIR (anti-alias for its own decimation), then stage B does the
    // sharp ~20 kHz cut at the low intermediate rate, where a deep stopband is cheap. The
    // sharp filter has to live at the low rate: at the DSD rate a 20 kHz cut would need
    // thousands of taps. Chaining them streaming keeps only the final PCM in memory.
    let total_ratio = (dsd_rate / OUT_RATE as f64).round() as usize;
    if total_ratio < 8 {
        return Err(anyhow!("unexpected DSD rate {dsd_rate} vs {OUT_RATE}"));
    }
    let factor_b = 8usize;
    let factor_a = (total_ratio / factor_b).max(1);
    let inter_rate = dsd_rate / factor_a as f64;
    let taps_a = (16 * factor_a).clamp(64, 512);
    let mut stage_a: Vec<FirDecimator> = (0..channels)
        .map(|_| FirDecimator::new(factor_a, 0.45 * inter_rate, dsd_rate, taps_a))
        .collect();
    let mut stage_b: Vec<FirDecimator> = (0..channels)
        .map(|_| FirDecimator::new(factor_b, 20_000.0, inter_rate, 1024))
        .collect();
    let mut pcm_planar: Vec<Vec<f32>> = vec![Vec::new(); channels];

    let iter = reader
        .dsd_iter()
        .map_err(|e| anyhow!("DSD iter {}: {e}", path.display()))?;
    for (read_size, chan_bufs) in iter {
        if chan_bufs.len() < channels {
            return Err(anyhow!("DSD stream channel mismatch"));
        }
        for ch in 0..channels {
            let bytes = &chan_bufs[ch];
            let valid = read_size.min(bytes.len());
            let (a, b) = (&mut stage_a[ch], &mut stage_b[ch]);
            let out = &mut pcm_planar[ch];
            for &byte in &bytes[..valid] {
                // dsd_iter yields LSB-first bytes: bit 0 is the earliest sample.
                for bit in 0..8 {
                    let s = if (byte >> bit) & 1 == 1 { 1.0f32 } else { -1.0f32 };
                    if let Some(y) = a.push(s) {
                        if let Some(z) = b.push(y) {
                            out.push(z);
                        }
                    }
                }
            }
        }
    }
    if pcm_planar.iter().all(|c| c.is_empty()) {
        return Err(anyhow!("decoded no audio from {}", path.display()));
    }

    // Interleave.
    let frames = pcm_planar.iter().map(|c| c.len()).min().unwrap_or(0);
    let mut samples = Vec::with_capacity(frames * channels);
    for i in 0..frames {
        for ch in pcm_planar.iter() {
            samples.push(ch[i]);
        }
    }
    let total = (frames > 0).then(|| Duration::from_secs_f64(frames as f64 / OUT_RATE as f64));
    Ok(DsdSource {
        samples,
        channels: channels as u16,
        pos: 0,
        total,
    })
}

/// The length of a DSD file, for the playlist, from the container header (no decode).
/// `None` when it isn't a DSD file or the length can't be determined.
pub fn duration(path: &Path) -> Option<Duration> {
    if !is_dsd(path) {
        return None;
    }
    let reader = dsd_reader::DsdReader::from_container(path.to_path_buf()).ok()?;
    let mult = reader.dsd_rate();
    let channels = reader.channels_num();
    if mult <= 0 || channels == 0 {
        return None;
    }
    let rate_hz = mult as u64 * dsd_reader::DSD_64_RATE as u64;
    // audio_length is total bytes across all channels; 8 DSD samples per byte.
    let bytes_per_ch = reader.audio_length() / channels as u64;
    let samples_per_ch = bytes_per_ch.saturating_mul(8);
    if samples_per_ch == 0 || rate_hz == 0 {
        return None;
    }
    Some(Duration::from_secs_f64(samples_per_ch as f64 / rate_hz as f64))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Option<std::path::PathBuf> {
        let p = std::path::PathBuf::from("qa-music/format-samples/hypno-dsd64.dsf");
        p.exists().then_some(p)
    }

    fn corr(a: &[f32], b: &[f32]) -> f64 {
        let n = a.len().min(b.len());
        if n < 1000 {
            return 0.0;
        }
        let (a, b) = (&a[..n], &b[..n]);
        let ma = a.iter().map(|&x| x as f64).sum::<f64>() / n as f64;
        let mb = b.iter().map(|&x| x as f64).sum::<f64>() / n as f64;
        let mut num = 0.0;
        let mut da = 0.0;
        let mut db = 0.0;
        for i in 0..n {
            let xa = a[i] as f64 - ma;
            let xb = b[i] as f64 - mb;
            num += xa * xb;
            da += xa * xa;
            db += xb * xb;
        }
        if da <= 0.0 || db <= 0.0 {
            return 0.0;
        }
        num / (da.sqrt() * db.sqrt())
    }

    /// The decoded DSD must recover the music: compared to the original PCM the DSF was
    /// made from, the left channel correlates near 1.0 (DSD->PCM is filtered, so this is
    /// a correlation check, not a sample-exact one). (qa-music is gitignored; skips on CI.)
    #[test]
    fn decodes_and_recovers_the_source_audio() {
        let Some(dsf) = sample() else { return };
        let src = open_dsd(&dsf).unwrap();
        assert_eq!(src.channels(), 2);
        assert_eq!(src.sample_rate(), 44_100);
        let got: Vec<f32> = src.collect();
        assert!(got.len() > 44_100 * 2, "too little audio: {}", got.len());
        let left: Vec<f32> = got.iter().step_by(2).copied().collect();

        let mut r = hound::WavReader::open("qa-music/format-samples/hypno-this.wav").unwrap();
        let orig: Vec<f32> = r
            .samples::<i16>()
            .step_by(2)
            .map(|s| s.unwrap() as f32 / 32768.0)
            .collect();

        // Search a small lag for the best correlation (filter group delay).
        let mut best = 0.0f64;
        for lag in 0..300 {
            if lag >= left.len() {
                break;
            }
            let c = corr(&left[lag..], &orig);
            if c > best {
                best = c;
            }
        }
        assert!(best > 0.98, "DSD decode correlation with source too low: {best:.3}");
    }

    #[test]
    fn seek_moves_the_position() {
        let Some(dsf) = sample() else { return };
        let mut src = open_dsd(&dsf).unwrap();
        let total = src.total_duration().unwrap();
        src.try_seek(total.mul_f64(0.5)).unwrap();
        let after: Vec<f32> = src.collect();
        assert!(!after.is_empty(), "nothing after seek");
    }

    #[test]
    fn duration_reads_without_full_decode() {
        let Some(dsf) = sample() else { return };
        let d = duration(&dsf).expect("dsd duration");
        assert!(d.as_secs() >= 1 && d.as_secs() < 600, "duration {d:?}");
    }
}
