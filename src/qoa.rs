//! QOA (Quite OK Audio, `.qoa`) via the pure-Rust `qoaudio` crate (MIT/Apache-2.0), behind the `qoa`
//! feature. QOA is a simple, fixed-ratio lossy format (2023); the decoder is pure Rust, so unlike the
//! native side-decoders there is no FFI and no bundled library. `qoaudio` exposes the stream as an
//! iterator of samples (interleaved by channel); `qoaudio` only decodes forward, so seeking re-creates
//! the decoder from the in-memory bytes and renders forward (seek-by-reset), like the chip decoders.
//! Output is the file's own sample rate and channel count (QOA is 1-8 channels).

use std::io::Cursor;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, Result};
use qoaudio::{ProcessingMode, QoaDecoder, QoaItem};
use rodio::source::SeekError;
use rodio::Source;

const BATCH: usize = 4096;

/// Extensions routed here.
pub const QOA_EXTS: &[&str] = &["qoa"];

pub fn is_qoa(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| QOA_EXTS.iter().any(|x| e.eq_ignore_ascii_case(x)))
        .unwrap_or(false)
}

type Decoder = QoaDecoder<Cursor<Arc<[u8]>>>;

/// A seekable rodio `Source` streaming a QOA file as f32, at the file's own rate and channel count.
pub struct QoaSource {
    bytes: Arc<[u8]>,
    dec: Decoder,
    channels: u16,
    sample_rate: u32,
    total: Option<Duration>,
    cap_samples: u64, // total interleaved i16 count (samples-per-channel * channels), 0 = unknown
    pos_samples: u64, // interleaved samples emitted so far
    buf: Vec<f32>,
    idx: usize,
    ended: bool,
}

/// (channels, sample_rate, total interleaved sample count) from a fixed-mode QOA header.
fn fixed_header(dec: &Decoder) -> Result<(u16, u32, u64)> {
    match dec.mode() {
        ProcessingMode::FixedSamples { channels, sample_rate, samples } => {
            Ok((*channels as u16, *sample_rate, *samples as u64 * *channels as u64))
        }
        // Streaming QOA (no total, header per frame) is not produced by .qoa files; skip it.
        ProcessingMode::Streaming => Err(anyhow!("streaming QOA is not supported")),
    }
}

impl QoaSource {
    fn fill(&mut self) -> bool {
        if self.ended {
            return false;
        }
        self.buf.clear();
        while self.buf.len() < BATCH {
            match self.dec.next() {
                Some(Ok(QoaItem::Sample(s))) => self.buf.push(s as f32 / 32768.0),
                Some(Ok(QoaItem::FrameHeader(_))) => {}
                Some(Err(_)) | None => {
                    self.ended = true;
                    break;
                }
            }
        }
        self.pos_samples += self.buf.len() as u64;
        self.idx = 0;
        !self.buf.is_empty()
    }
}

impl Iterator for QoaSource {
    type Item = f32;
    fn next(&mut self) -> Option<f32> {
        if self.idx >= self.buf.len() && !self.fill() {
            return None;
        }
        let s = self.buf[self.idx];
        self.idx += 1;
        Some(s)
    }
}

impl Source for QoaSource {
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
        let ch = self.channels.max(1) as u64;
        let target = (pos.as_secs_f64() * self.sample_rate as f64).round() as u64 * ch;
        // qoaudio only decodes forward; re-create the decoder from the bytes and render forward.
        self.dec = QoaDecoder::new(Cursor::new(self.bytes.clone()))
            .map_err(|_| SeekError::NotSupported { underlying_source: "qoa" })?;
        self.pos_samples = 0;
        self.ended = false;
        while self.pos_samples < target {
            match self.dec.next() {
                Some(Ok(QoaItem::Sample(_))) => self.pos_samples += 1,
                Some(Ok(QoaItem::FrameHeader(_))) => {}
                Some(Err(_)) | None => {
                    self.ended = true;
                    break;
                }
            }
        }
        self.buf.clear();
        self.idx = 0;
        if self.cap_samples > 0 && self.pos_samples >= self.cap_samples {
            self.ended = true;
        }
        Ok(())
    }
}

/// Load a QOA file as a lazy, seekable source. Errors cleanly on a non-QOA (or streaming-QOA) file so
/// the skip-unplayable path takes over.
pub fn open(path: &Path) -> Result<QoaSource> {
    let bytes: Arc<[u8]> = std::fs::read(path)?.into();
    let dec = QoaDecoder::new(Cursor::new(bytes.clone()))
        .map_err(|e| anyhow!("not a QOA file ({}): {e:?}", path.display()))?;
    let (channels, sample_rate, cap_samples) = fixed_header(&dec)?;
    let total = dec.total_duration();
    Ok(QoaSource {
        bytes,
        dec,
        channels,
        sample_rate,
        total,
        cap_samples,
        pos_samples: 0,
        buf: Vec::new(),
        idx: 0,
        ended: false,
    })
}

/// Length of a QOA file for the playlist, read from its header, or `None` without the `qoa` feature.
pub fn duration(path: &Path) -> Option<Duration> {
    if !is_qoa(path) {
        return None;
    }
    let bytes: Arc<[u8]> = std::fs::read(path).ok()?.into();
    let dec = QoaDecoder::new(Cursor::new(bytes)).ok()?;
    dec.total_duration()
}

/// The QOA file's own (sample_rate, channels) from its header, for the playlist readout.
pub fn media_info(path: &Path) -> Option<(u32, u8)> {
    if !is_qoa(path) {
        return None;
    }
    let bytes: Arc<[u8]> = std::fs::read(path).ok()?.into();
    let dec = QoaDecoder::new(Cursor::new(bytes)).ok()?;
    match dec.mode() {
        ProcessingMode::FixedSamples { channels, sample_rate, .. } => Some((*sample_rate, *channels)),
        ProcessingMode::Streaming => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_qoa_matches_the_extension() {
        for e in ["qoa", "QOA"] {
            assert!(is_qoa(&std::path::PathBuf::from(format!("x.{e}"))), ".{e}");
        }
        assert!(!is_qoa(std::path::Path::new("x.mp3")));
        assert!(!is_qoa(std::path::Path::new("noext")));
    }

    /// Decode every QOA sample present and confirm non-silent audio. (qa-music is gitignored, skips on
    /// CI.)
    #[test]
    fn decodes_real_qoa_files() {
        let dir = std::path::Path::new("qa-music/format-samples");
        if !dir.exists() {
            return;
        }
        let mut checked = 0;
        for entry in std::fs::read_dir(dir).unwrap().flatten() {
            let path = entry.path();
            if !is_qoa(&path) {
                continue;
            }
            let src = open(&path).unwrap_or_else(|e| panic!("open {}: {e}", path.display()));
            let ch = src.channels();
            assert!((1..=8).contains(&ch), "{}: channels {ch}", path.display());
            let sr = src.sample_rate();
            let got: Vec<f32> = src.take(sr as usize * ch as usize * 5).collect();
            assert!(got.len() > sr as usize, "{}: too little audio", path.display());
            let peak = got.iter().fold(0.0f32, |m, &s| m.max(s.abs()));
            assert!(peak > 0.003, "{}: near-silence (peak {peak})", path.display());
            checked += 1;
        }
        assert!(checked > 0, "no QOA samples found to decode");
    }
}
