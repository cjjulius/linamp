//! Stereo balance (left/right pan) as a rodio `Source`: panning attenuates the
//! opposite channel, leaving the panned-to side at full gain. The UI writes via
//! a [`BalanceShared`] handle; the audio thread reads it (atomic) and applies
//! per-channel gain live.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;

use rodio::Source;

/// Left/right gains for a balance value in `-1.0..=1.0` (−1 = full left,
/// 0 = centre, +1 = full right).
pub fn pan_gains(b: f32) -> (f32, f32) {
    let b = b.clamp(-1.0, 1.0);
    let left = 1.0 - b.max(0.0);
    let right = 1.0 + b.min(0.0);
    (left, right)
}

/// Thread-shared balance value (an `f32` stored as bits).
pub struct BalanceShared {
    bits: AtomicU32,
}

impl BalanceShared {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            bits: AtomicU32::new(0f32.to_bits()),
        })
    }

    pub fn set(&self, b: f32) {
        self.bits.store(b.clamp(-1.0, 1.0).to_bits(), Ordering::Relaxed);
    }

    pub fn get(&self) -> f32 {
        f32::from_bits(self.bits.load(Ordering::Relaxed))
    }
}

/// A rodio `Source` that applies stereo balance. Mono (and anything that isn't
/// 2-channel) passes through untouched.
pub struct BalanceSource<S> {
    inner: S,
    shared: Arc<BalanceShared>,
    channels: u16,
    ch: u16,
}

/// Wrap `inner` so its stereo image is panned per `shared`.
pub fn balance<S>(inner: S, shared: Arc<BalanceShared>) -> BalanceSource<S>
where
    S: Source<Item = f32> + Send + 'static,
{
    let channels = inner.channels().max(1);
    BalanceSource {
        inner,
        shared,
        channels,
        ch: 0,
    }
}

impl<S> Iterator for BalanceSource<S>
where
    S: Source<Item = f32>,
{
    type Item = f32;

    fn next(&mut self) -> Option<f32> {
        let s = self.inner.next()?;
        if self.channels != 2 {
            return Some(s);
        }
        let (l, r) = pan_gains(self.shared.get());
        let g = if self.ch == 0 { l } else { r };
        self.ch = (self.ch + 1) % self.channels;
        Some(s * g)
    }
}

impl<S> Source for BalanceSource<S>
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
        self.ch = 0;
        self.inner.try_seek(pos)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn centre_is_unity() {
        let (l, r) = pan_gains(0.0);
        assert_eq!((l, r), (1.0, 1.0));
    }

    #[test]
    fn full_left_mutes_right() {
        let (l, r) = pan_gains(-1.0);
        assert_eq!(l, 1.0);
        assert_eq!(r, 0.0);
    }

    #[test]
    fn full_right_mutes_left() {
        let (l, r) = pan_gains(1.0);
        assert_eq!(l, 0.0);
        assert_eq!(r, 1.0);
    }

    #[test]
    fn partial_pan_attenuates_one_side() {
        let (l, r) = pan_gains(0.5);
        assert_eq!(l, 0.5);
        assert_eq!(r, 1.0);
        assert!(pan_gains(5.0).0 == 0.0); // clamps
    }

    #[test]
    fn shared_roundtrip() {
        let s = BalanceShared::new();
        assert_eq!(s.get(), 0.0);
        s.set(-0.3);
        assert!((s.get() + 0.3).abs() < 1e-6);
        s.set(2.0);
        assert_eq!(s.get(), 1.0);
    }
}
