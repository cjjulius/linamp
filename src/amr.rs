//! AMR (.amr / .awb) playback via the pure-Rust `rvoip-codec-core` decoder.
//!
//! AMR (Adaptive Multi-Rate) is a 3GPP CELP speech codec: AMR-NB (narrowband, 8 kHz)
//! and AMR-WB (wideband, 16 kHz), the format of old mobile voice memos and MMS. symphonia
//! has no AMR decoder. Unlike the Speex case, the pure-Rust option here is correct:
//! `rvoip-codec-core` is tested against the 3GPP conformance sequences and vetted at
//! 0.9999 correlation against the ffmpeg oracle, so no C library is needed.
//!
//! The `.amr` storage container is parsed here directly (it is trivial): a magic header
//! (`#!AMR\n` for NB, `#!AMR-WB\n` for WB) followed by one frame per 20 ms, each a 1-byte
//! table-of-contents octet (whose bits 3..6 are the mode) plus a mode-sized payload.
//! Single-channel only (the common case); output is interleaved f32 at the native rate.

use std::path::Path;
use std::time::Duration;

use anyhow::{anyhow, Result};
use rodio::source::SeekError;
use rodio::Source;

/// AMR-NB storage-format speech payload sizes (bytes, excluding the 1-byte TOC) by mode.
/// Modes 0..=7 are speech; 8 is SID (comfort noise), the rest carry no speech.
const NB_SIZES: [usize; 16] = [12, 13, 15, 17, 19, 20, 26, 31, 5, 0, 0, 0, 0, 0, 0, 0];
/// AMR-WB storage-format speech payload sizes by mode (0..=8 speech, 9 SID).
const WB_SIZES: [usize; 16] = [17, 23, 32, 36, 40, 46, 50, 58, 60, 5, 0, 0, 0, 0, 0, 0];

/// True when `path` is an AMR file routed through this module.
pub fn is_amr(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| e.eq_ignore_ascii_case("amr") || e.eq_ignore_ascii_case("awb"))
        .unwrap_or(false)
}

/// A seekable rodio `Source` over a fully-decoded AMR stream (mono f32).
pub struct AmrSource {
    samples: Vec<f32>,
    sample_rate: u32,
    pos: usize,
    total: Option<Duration>,
}

impl Iterator for AmrSource {
    type Item = f32;
    fn next(&mut self) -> Option<f32> {
        let s = self.samples.get(self.pos).copied();
        if s.is_some() {
            self.pos += 1;
        }
        s
    }
}

impl Source for AmrSource {
    fn current_frame_len(&self) -> Option<usize> {
        None
    }
    fn channels(&self) -> u16 {
        1
    }
    fn sample_rate(&self) -> u32 {
        self.sample_rate
    }
    fn total_duration(&self) -> Option<Duration> {
        self.total
    }
    fn try_seek(&mut self, pos: Duration) -> Result<(), SeekError> {
        let frame = (pos.as_secs_f64() * self.sample_rate as f64).round() as usize;
        self.pos = frame.min(self.samples.len());
        Ok(())
    }
}

/// Which AMR variant a buffer holds, from its magic header, with the byte offset where
/// the frames start, the per-frame sample count, the output rate, and the mode→size table.
struct Variant {
    codec: codec_core::types::CodecType,
    data_offset: usize,
    frame_samples: usize,
    rate: u32,
    sizes: [usize; 16],
}

fn detect(buf: &[u8]) -> Result<Variant> {
    if buf.starts_with(b"#!AMR-WB\n") {
        Ok(Variant {
            codec: codec_core::types::CodecType::AmrWb,
            data_offset: 9,
            frame_samples: 320,
            rate: 16_000,
            sizes: WB_SIZES,
        })
    } else if buf.starts_with(b"#!AMR\n") {
        Ok(Variant {
            codec: codec_core::types::CodecType::AmrNb,
            data_offset: 6,
            frame_samples: 160,
            rate: 8_000,
            sizes: NB_SIZES,
        })
    } else if buf.starts_with(b"#!AMR_MC") || buf.starts_with(b"#!AMR-WB_MC") {
        Err(anyhow!("multi-channel AMR is not supported"))
    } else {
        Err(anyhow!("not a recognized AMR storage file"))
    }
}

/// Decode an AMR file fully and wrap it as a seekable source. Errors cleanly (unreadable
/// file, unknown magic, multi-channel) so the skip-unplayable path takes over.
pub fn open_amr(path: &Path) -> Result<AmrSource> {
    let buf = std::fs::read(path)?;
    let v = detect(&buf)?;
    let mut codec = codec_core::CodecFactory::create(codec_core::CodecConfig::new(v.codec))
        .map_err(|e| anyhow!("AMR init {}: {e}", path.display()))?;

    let mut pcm: Vec<i16> = Vec::new();
    let mut i = v.data_offset;
    while i < buf.len() {
        let toc = buf[i];
        i += 1;
        let mode = ((toc >> 3) & 0x0F) as usize;
        let size = v.sizes[mode];
        let is_speech = size > 0 && mode < 9;
        if is_speech {
            if i + size > buf.len() {
                break; // truncated final frame
            }
            match codec.decode(&buf[i..i + size]) {
                Ok(frame) => pcm.extend_from_slice(&frame),
                // A bad frame becomes silence rather than aborting the whole file.
                Err(_) => pcm.resize(pcm.len() + v.frame_samples, 0),
            }
            i += size;
        } else {
            // SID or no-data/lost frame: keep the timeline aligned with silence.
            pcm.resize(pcm.len() + v.frame_samples, 0);
            i += size;
        }
    }
    if pcm.is_empty() {
        return Err(anyhow!("decoded no audio from {}", path.display()));
    }

    let samples: Vec<f32> = pcm.iter().map(|&s| s as f32 / 32_768.0).collect();
    let total = Some(Duration::from_secs_f64(samples.len() as f64 / v.rate as f64));
    Ok(AmrSource {
        samples,
        sample_rate: v.rate,
        pos: 0,
        total,
    })
}

/// The length of an AMR file, for the playlist, from the frame structure (no decode).
/// `None` when it isn't an AMR file. Each frame is a fixed 20 ms regardless of mode.
pub fn duration(path: &Path) -> Option<Duration> {
    if !is_amr(path) {
        return None;
    }
    let buf = std::fs::read(path).ok()?;
    let v = detect(&buf).ok()?;
    let mut i = v.data_offset;
    let mut frames = 0u64;
    while i < buf.len() {
        let mode = ((buf[i] >> 3) & 0x0F) as usize;
        i += 1 + v.sizes[mode];
        frames += 1;
    }
    if frames == 0 {
        return None;
    }
    Some(Duration::from_secs_f64(
        (frames * v.frame_samples as u64) as f64 / v.rate as f64,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// AMR-NB must decode to within a tight correlation of the ffmpeg oracle
    /// (ffmpeg -i sample.amr). (qa-music is gitignored, so this skips on CI.)
    #[test]
    fn decodes_amr_nb_against_the_ffmpeg_oracle() {
        let amr = std::path::Path::new("qa-music/format-samples/sample.amr");
        let oracle = std::path::Path::new("qa-music/format-samples/oracles/amr-nb-oracle.s16");
        if !amr.exists() || !oracle.exists() {
            return;
        }
        let src = open_amr(amr).unwrap();
        assert_eq!(src.channels(), 1);
        assert_eq!(src.sample_rate(), 8_000);
        let got: Vec<f32> = src.collect();
        let raw = std::fs::read(oracle).unwrap();
        let want: Vec<f32> = raw
            .chunks_exact(2)
            .map(|b| i16::from_le_bytes([b[0], b[1]]) as f32 / 32_768.0)
            .collect();
        let n = got.len().min(want.len());
        assert!(n > 8_000, "too little audio: {n}");
        let (got, want) = (&got[..n], &want[..n]);
        let (ma, mb) = (
            got.iter().map(|&x| x as f64).sum::<f64>() / n as f64,
            want.iter().map(|&x| x as f64).sum::<f64>() / n as f64,
        );
        let (mut num, mut da, mut db) = (0.0, 0.0, 0.0);
        for i in 0..n {
            let (xa, xb) = (got[i] as f64 - ma, want[i] as f64 - mb);
            num += xa * xb;
            da += xa * xa;
            db += xb * xb;
        }
        let corr = num / (da.sqrt() * db.sqrt());
        assert!(corr > 0.99, "AMR-NB correlation vs oracle too low: {corr:.4}");
    }

    #[test]
    fn duration_reads_without_full_decode() {
        let amr = std::path::Path::new("qa-music/format-samples/sample.amr");
        if !amr.exists() {
            return;
        }
        let d = duration(amr).expect("amr duration");
        assert!(d.as_secs() > 1 && d.as_secs() < 600, "duration {d:?}");
    }
}
