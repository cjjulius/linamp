use std::f32::consts::PI;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rodio::Source;
use rustfft::{num_complex::Complex, Fft};

/// Number of mono samples retained for visualization (~0.2s at 44.1kHz).
const CAP: usize = 8192;

/// Shared, thread-safe window of recent audio samples. The rodio playback
/// thread writes into it via [`TapSource`]; the UI thread reads snapshots.
pub struct VizShared {
    inner: Mutex<Ring>,
    sample_rate: AtomicU32,
}

struct Ring {
    data: Vec<f32>,
    pos: usize,
}

impl VizShared {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            inner: Mutex::new(Ring {
                data: vec![0.0; CAP],
                pos: 0,
            }),
            sample_rate: AtomicU32::new(44_100),
        })
    }

    fn push_batch(&self, samples: &[f32]) {
        if let Ok(mut r) = self.inner.lock() {
            for &s in samples {
                let i = r.pos;
                r.data[i] = s;
                r.pos = (i + 1) % CAP;
            }
        }
    }

    /// Zero the buffer (called when a new track starts).
    pub fn clear(&self) {
        if let Ok(mut r) = self.inner.lock() {
            r.data.iter_mut().for_each(|x| *x = 0.0);
            r.pos = 0;
        }
    }

    /// Copy the most recent `n` samples into `out`, oldest-first.
    pub fn snapshot(&self, out: &mut Vec<f32>, n: usize) {
        let n = n.min(CAP);
        out.clear();
        out.reserve(n);
        if let Ok(r) = self.inner.lock() {
            for k in 0..n {
                let idx = (r.pos + CAP - n + k) % CAP;
                out.push(r.data[idx]);
            }
        } else {
            out.extend(std::iter::repeat(0.0).take(n));
        }
    }

    pub fn set_rate(&self, sr: u32) {
        self.sample_rate.store(sr.max(1), Ordering::Relaxed);
    }

    pub fn rate(&self) -> u32 {
        self.sample_rate.load(Ordering::Relaxed)
    }
}

/// A rodio `Source` that passes audio through unchanged while mixing each frame
/// to mono and feeding it, in small batches, into a [`VizShared`] ring.
pub struct TapSource<S> {
    inner: S,
    shared: Arc<VizShared>,
    channels: u16,
    ch_idx: u16,
    frame_acc: f32,
    batch: Vec<f32>,
}

/// Wrap `inner` so its samples are tapped into `shared`.
pub fn tap<S>(inner: S, shared: Arc<VizShared>) -> TapSource<S>
where
    S: Source<Item = f32> + Send + 'static,
{
    let channels = inner.channels().max(1);
    TapSource {
        inner,
        shared,
        channels,
        ch_idx: 0,
        frame_acc: 0.0,
        batch: Vec::with_capacity(1024),
    }
}

impl<S> Iterator for TapSource<S>
where
    S: Source<Item = f32>,
{
    type Item = f32;

    fn next(&mut self) -> Option<f32> {
        let s = self.inner.next()?;
        self.frame_acc += s;
        self.ch_idx += 1;
        if self.ch_idx >= self.channels {
            self.batch.push(self.frame_acc / self.channels as f32);
            self.frame_acc = 0.0;
            self.ch_idx = 0;
            if self.batch.len() >= 512 {
                self.shared.push_batch(&self.batch);
                self.batch.clear();
            }
        }
        Some(s)
    }
}

impl<S> Source for TapSource<S>
where
    S: Source<Item = f32>,
{
    fn current_frame_len(&self) -> Option<usize> {
        self.inner.current_frame_len()
    }
    fn channels(&self) -> u16 {
        self.inner.channels()
    }
    fn sample_rate(&self) -> u32 {
        self.inner.sample_rate()
    }
    fn total_duration(&self) -> Option<Duration> {
        self.inner.total_duration()
    }
    fn try_seek(&mut self, pos: Duration) -> Result<(), rodio::source::SeekError> {
        // Forward seeks to the decoder; drop any buffered viz samples.
        self.batch.clear();
        self.shared.clear();
        self.inner.try_seek(pos)
    }
}

/// Reduce raw samples to `bands` normalized (0..1) log-spaced spectrum bars.
pub fn compute_bars(
    samples: &[f32],
    fft: &dyn Fft<f32>,
    fft_size: usize,
    bands: usize,
    sample_rate: u32,
) -> Vec<f32> {
    if samples.len() < fft_size {
        return vec![0.0; bands];
    }
    let slice = &samples[samples.len() - fft_size..];

    // Hann window into complex buffer.
    let mut buf: Vec<Complex<f32>> = slice
        .iter()
        .enumerate()
        .map(|(i, &s)| {
            let w = 0.5 - 0.5 * (2.0 * PI * i as f32 / (fft_size as f32 - 1.0)).cos();
            Complex {
                re: s * w,
                im: 0.0,
            }
        })
        .collect();
    fft.process(&mut buf);

    let half = fft_size / 2;
    // Normalize: fft_size/16 keeps typical music's peaks near the top (full
    // fft_size is too quiet; raw bins pin the bars at 1.0).
    let norm = fft_size as f32 / 16.0;
    let mags: Vec<f32> = buf[..half].iter().map(|c| c.norm() / norm).collect();

    let nyquist = sample_rate as f32 / 2.0;
    let fmin = 40.0_f32;
    let fmax = nyquist.min(16_000.0).max(fmin + 1.0);

    let mut out = Vec::with_capacity(bands);
    for b in 0..bands {
        let f0 = fmin * (fmax / fmin).powf(b as f32 / bands as f32);
        let f1 = fmin * (fmax / fmin).powf((b + 1) as f32 / bands as f32);
        let i0 = ((f0 / nyquist) * half as f32) as usize;
        let i1 = (((f1 / nyquist) * half as f32) as usize).max(i0 + 1).min(half);
        let (mut sum, mut cnt) = (0.0_f32, 0usize);
        for &m in &mags[i0..i1] {
            sum += m;
            cnt += 1;
        }
        let avg = if cnt > 0 { sum / cnt as f32 } else { 0.0 };
        // Map roughly -60..0 dBFS to 0..1.
        let db = 20.0 * (avg + 1e-6).log10();
        out.push(((db + 60.0) / 60.0).clamp(0.0, 1.0));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ring_snapshot_len() {
        let v = VizShared::new();
        v.push_batch(&[0.1, 0.2, 0.3]);
        let mut out = Vec::new();
        v.snapshot(&mut out, 4);
        assert_eq!(out.len(), 4);
    }

    #[test]
    fn bars_on_silence_are_zero() {
        use rustfft::FftPlanner;
        let fft = FftPlanner::<f32>::new().plan_fft_forward(1024);
        let bars = compute_bars(&vec![0.0f32; 1024], &*fft, 1024, 10, 44100);
        assert_eq!(bars.len(), 10);
        assert!(bars.iter().all(|b| *b <= 0.01));
    }
}
