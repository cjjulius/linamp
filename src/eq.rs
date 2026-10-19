//! Ten-band graphic equalizer DSP.
//!
//! A chain of RBJ-cookbook peaking biquad filters (one per band) is applied to
//! the f32 sample stream as a rodio [`Source`], inserted ahead of the
//! visualization tap so the analyzer shows post-EQ audio — just like Winamp.
//! The UI thread pushes settings through an [`EqShared`] handle; the audio
//! thread notices the version bump and recomputes coefficients without locking
//! on the hot path.

use std::f32::consts::PI;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rodio::Source;

/// Band centre frequencies (Hz), matching the classic Winamp 10-band EQ.
pub const BAND_FREQS: [f32; 10] = [
    60.0, 170.0, 310.0, 600.0, 1000.0, 3000.0, 6000.0, 12000.0, 14000.0, 16000.0,
];

/// Fixed filter Q. A moderate value keeps adjacent bands from fighting while
/// still giving each slider an audible, roughly one-third-octave reach.
const Q: f32 = 1.0;

/// Built-in equalizer presets: `(name, preamp dB, 10 band gains in dB)` at
/// [`BAND_FREQS`]. Band gains stay within the ±12 dB slider range.
pub const PRESETS: &[(&str, f32, [f32; 10])] = &[
    ("Flat", 0.0, [0.0; 10]),
    ("Rock", 0.0, [8.0, 5.0, -4.0, -7.0, -3.0, 2.0, 6.0, 9.0, 10.0, 10.0]),
    ("Pop", 0.0, [-2.0, 3.0, 6.0, 7.0, 4.0, 0.0, -2.0, -3.0, -3.0, -3.0]),
    ("Jazz", 0.0, [5.0, 4.0, 2.0, 3.0, -1.0, -1.0, 0.0, 2.0, 4.0, 5.0]),
    ("Classical", 0.0, [6.0, 5.0, 4.0, 2.0, -2.0, -2.0, 0.0, 3.0, 5.0, 6.0]),
    ("Dance", 0.0, [9.0, 7.0, 3.0, 0.0, 0.0, -4.0, -6.0, -6.0, 0.0, 0.0]),
    ("Bass Boost", 0.0, [10.0, 9.0, 8.0, 5.0, 2.0, -1.0, -3.0, -5.0, -6.0, -6.0]),
    ("Treble Boost", 0.0, [-7.0, -6.0, -5.0, -2.0, 0.0, 4.0, 7.0, 9.0, 10.0, 10.0]),
    ("Vocal", 0.0, [-3.0, -2.0, 0.0, 3.0, 5.0, 5.0, 4.0, 2.0, 0.0, -2.0]),
    ("Soft", 0.0, [4.0, 2.0, 1.0, 0.0, -1.0, -2.0, 0.0, 2.0, 4.0, 6.0]),
];

/// Auto-gain preamp (dB) that offsets a preset's largest boost so it is less
/// likely to clip: the negative of the loudest positive band, else 0.
pub fn auto_preamp(bands: &[f32; 10]) -> f32 {
    -bands.iter().cloned().fold(0.0_f32, f32::max)
}

/// A snapshot of the equalizer controls.
#[derive(Clone, Copy)]
pub struct EqParams {
    pub enabled: bool,
    pub preamp_db: f32,
    pub bands: [f32; 10],
}

impl Default for EqParams {
    fn default() -> Self {
        Self {
            enabled: false,
            preamp_db: 0.0,
            bands: [0.0; 10],
        }
    }
}

/// Thread-shared EQ settings. The UI thread calls [`EqShared::set`]; the audio
/// thread polls [`EqShared::version`] (a cheap atomic load) each sample and only
/// takes the lock when something actually changed.
pub struct EqShared {
    params: Mutex<EqParams>,
    version: AtomicU64,
}

impl EqShared {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            params: Mutex::new(EqParams::default()),
            version: AtomicU64::new(0),
        })
    }

    /// Replace the live EQ settings (called from the UI thread).
    pub fn set(&self, enabled: bool, preamp_db: f32, bands: [f32; 10]) {
        if let Ok(mut p) = self.params.lock() {
            *p = EqParams {
                enabled,
                preamp_db,
                bands,
            };
        }
        self.version.fetch_add(1, Ordering::Release);
    }

    fn version(&self) -> u64 {
        self.version.load(Ordering::Acquire)
    }

    fn snapshot(&self) -> EqParams {
        self.params.lock().map(|p| *p).unwrap_or_default()
    }
}

/// Normalized biquad coefficients (the `a0` term divided out).
#[derive(Clone, Copy, Default)]
pub struct Biquad {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
}

/// Direct-form-I filter memory for one biquad on one channel.
#[derive(Clone, Copy, Default)]
struct BiquadState {
    x1: f32,
    x2: f32,
    y1: f32,
    y2: f32,
}

impl Biquad {
    /// Apply one sample, advancing `s`.
    #[inline]
    fn process(&self, s: &mut BiquadState, x: f32) -> f32 {
        let y = self.b0 * x + self.b1 * s.x1 + self.b2 * s.x2 - self.a1 * s.y1 - self.a2 * s.y2;
        s.x2 = s.x1;
        s.x1 = x;
        s.y2 = s.y1;
        s.y1 = y;
        y
    }
}

/// RBJ-cookbook peaking EQ coefficients. `gain_db == 0` yields a transparent
/// (identity) filter, so untouched bands cost almost nothing audibly.
pub fn peaking(f0: f32, q: f32, gain_db: f32, fs: f32) -> Biquad {
    let a = 10f32.powf(gain_db / 40.0);
    let w0 = 2.0 * PI * (f0 / fs);
    let cos_w0 = w0.cos();
    let alpha = w0.sin() / (2.0 * q);

    let b0 = 1.0 + alpha * a;
    let b1 = -2.0 * cos_w0;
    let b2 = 1.0 - alpha * a;
    let a0 = 1.0 + alpha / a;
    let a1 = -2.0 * cos_w0;
    let a2 = 1.0 - alpha / a;

    Biquad {
        b0: b0 / a0,
        b1: b1 / a0,
        b2: b2 / a0,
        a1: a1 / a0,
        a2: a2 / a0,
    }
}

/// A rodio `Source` that applies the preamp and 10 peaking filters per channel.
/// Passes audio through unchanged when the EQ is disabled.
pub struct EqSource<S> {
    inner: S,
    shared: Arc<EqShared>,
    channels: usize,
    sample_rate: f32,
    ch: usize,
    local_ver: u64,
    enabled: bool,
    preamp: f32,
    coeffs: [Biquad; 10],
    state: Vec<[BiquadState; 10]>,
}

/// Wrap `inner` so its samples are shaped by the EQ described by `shared`.
pub fn eq<S>(inner: S, shared: Arc<EqShared>) -> EqSource<S>
where
    S: Source<Item = f32> + Send + 'static,
{
    let channels = inner.channels().max(1) as usize;
    let sample_rate = inner.sample_rate().max(1) as f32;
    let mut src = EqSource {
        inner,
        shared,
        channels,
        sample_rate,
        ch: 0,
        local_ver: 0,
        enabled: false,
        preamp: 1.0,
        coeffs: [Biquad::default(); 10],
        state: vec![[BiquadState::default(); 10]; channels],
    };
    src.reload();
    src
}

impl<S> EqSource<S> {
    /// Pull the latest settings and recompute coefficients.
    fn reload(&mut self) {
        // Read the version *before* the snapshot so we never record a version
        // newer than the data we actually loaded (at worst we reload once more).
        let ver = self.shared.version();
        let p = self.shared.snapshot();
        self.enabled = p.enabled;
        self.preamp = 10f32.powf(p.preamp_db / 20.0);
        for i in 0..10 {
            self.coeffs[i] = peaking(BAND_FREQS[i], Q, p.bands[i], self.sample_rate);
        }
        self.local_ver = ver;
    }
}

impl<S> Iterator for EqSource<S>
where
    S: Source<Item = f32>,
{
    type Item = f32;

    fn next(&mut self) -> Option<f32> {
        let x = self.inner.next()?;
        if self.shared.version() != self.local_ver {
            self.reload();
        }

        let ch = self.ch;
        self.ch += 1;
        if self.ch >= self.channels {
            self.ch = 0;
        }

        if !self.enabled {
            return Some(x);
        }

        let coeffs = self.coeffs;
        let st = &mut self.state[ch];
        let mut v = x * self.preamp;
        for i in 0..10 {
            v = coeffs[i].process(&mut st[i], v);
        }
        Some(v)
    }
}

impl<S> Source for EqSource<S>
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
        // Clear filter memory so the jump doesn't ring from stale history.
        for ch in self.state.iter_mut() {
            *ch = [BiquadState::default(); 10];
        }
        self.ch = 0;
        self.inner.try_seek(pos)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RMS of a sine after passing through `bq` at frequency `f`, measured over
    /// the settled tail.
    fn rms_through(bq: Biquad, f: f32, fs: f32) -> f32 {
        let mut st = BiquadState::default();
        let n = 8192;
        let mut acc = 0.0f64;
        for i in 0..n {
            let x = (2.0 * PI * f * i as f32 / fs).sin();
            let y = bq.process(&mut st, x);
            if i >= n / 2 {
                acc += (y as f64) * (y as f64);
            }
        }
        (acc / (n / 2) as f64).sqrt() as f32
    }

    #[test]
    fn zero_gain_is_transparent() {
        let bq = peaking(1000.0, Q, 0.0, 44_100.0);
        // At 0 dB the numerator and denominator coincide, so b == a.
        assert!((bq.b0 - 1.0).abs() < 1e-5);
        assert!((bq.b1 - bq.a1).abs() < 1e-5);
        assert!((bq.b2 - bq.a2).abs() < 1e-5);
        // A sine should pass at ~unity RMS (a pure sine's RMS is 1/sqrt(2)).
        let r = rms_through(bq, 1000.0, 44_100.0);
        assert!((r - 0.70710677).abs() < 1e-3, "rms={r}");
    }

    #[test]
    fn boost_raises_center_frequency() {
        let bq = peaking(1000.0, Q, 12.0, 44_100.0);
        let base = 0.70710677; // RMS of a unit sine
        let r = rms_through(bq, 1000.0, 44_100.0);
        let ratio = r / base;
        // +12 dB is ~3.98x amplitude; allow for finite Q / settling.
        assert!(ratio > 3.0 && ratio < 4.5, "ratio={ratio}");
    }

    #[test]
    fn cut_lowers_center_frequency() {
        let bq = peaking(1000.0, Q, -12.0, 44_100.0);
        let base = 0.70710677;
        let ratio = rms_through(bq, 1000.0, 44_100.0) / base;
        assert!(ratio > 0.2 && ratio < 0.35, "ratio={ratio}");
    }

    #[test]
    fn distant_band_barely_moves() {
        // A 60 Hz band boost should leave a 10 kHz tone almost untouched.
        let bq = peaking(60.0, Q, 12.0, 44_100.0);
        let ratio = rms_through(bq, 10_000.0, 44_100.0) / 0.70710677;
        assert!((ratio - 1.0).abs() < 0.05, "ratio={ratio}");
    }

    #[test]
    fn presets_are_well_formed() {
        assert!(PRESETS.iter().any(|(n, _, _)| *n == "Flat"));
        for (name, preamp, bands) in PRESETS {
            assert!(preamp.abs() <= 12.0, "{name} preamp");
            assert!(bands.iter().all(|b| b.abs() <= 12.0), "{name} bands");
        }
        let flat = PRESETS.iter().find(|(n, _, _)| *n == "Flat").unwrap();
        assert_eq!(flat.2, [0.0; 10]);
    }

    #[test]
    fn auto_preamp_offsets_largest_boost() {
        assert_eq!(auto_preamp(&[0.0; 10]), 0.0);
        let mut b = [0.0; 10];
        b[3] = 8.0;
        assert_eq!(auto_preamp(&b), -8.0);
        assert_eq!(auto_preamp(&[-4.0; 10]), 0.0); // no boost → no cut
    }

    #[test]
    fn shared_set_bumps_version_and_snapshot() {
        let s = EqShared::new();
        let v0 = s.version();
        s.set(true, 6.0, [1.0; 10]);
        assert!(s.version() > v0);
        let p = s.snapshot();
        assert!(p.enabled);
        assert_eq!(p.preamp_db, 6.0);
        assert_eq!(p.bands[0], 1.0);
    }
}
