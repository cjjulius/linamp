//! Monkey's Audio (.ape) playback via the pure-Rust ape-decoder crate (#182).
//!
//! symphonia ships no APE decoder and there is no other viable one, so, like MIDI
//! (crate::midi) and tracker modules (crate::tracker), APE is a special case: this
//! wraps ape-decoder as a lazy, seekable rodio `Source` that decodes one frame at a
//! time (no up-front decode of the whole file) and seeks by sample. The crate's PCM
//! was vetted bit-exact against ffmpeg across every compression level, at 16-bit and
//! 24-bit, stereo and mono, and 8-bit (unsigned), before use.

use std::fs::File;
use std::io::BufReader;
use std::path::Path;
use std::time::Duration;

use anyhow::{anyhow, Result};
use ape_decoder::ApeDecoder;
use rodio::source::SeekError;
use rodio::Source;

/// True when `path` is a Monkey's Audio file routed through this module.
pub fn is_ape(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| e.eq_ignore_ascii_case("ape"))
        .unwrap_or(false)
}

/// A lazy, seekable rodio `Source` that decodes an APE file one frame at a time.
pub struct ApeSource {
    dec: ApeDecoder<BufReader<File>>,
    channels: u16,
    sample_rate: u32,
    bits: u16,
    /// Whether 8-bit samples are signed (vs the usual unsigned WAV convention).
    signed8: bool,
    /// Bytes per interleaved sample group (channels * bytes_per_sample).
    block_align: usize,
    total_frames: u32,
    /// Next frame to decode once the current buffer is drained.
    next_frame: u32,
    /// The current frame decoded to interleaved f32 (reused across frames), and our
    /// read position in it.
    buf: Vec<f32>,
    pos: usize,
    total: Option<Duration>,
}

impl ApeSource {
    /// Decode frame `idx`, dropping `skip` leading sample-groups (from a seek that
    /// lands mid-frame), into `self.buf` as interleaved f32 in [-1, 1].
    fn load_frame(&mut self, idx: u32, skip: u32) -> Result<()> {
        let bytes = self
            .dec
            .decode_frame(idx)
            .map_err(|e| anyhow!("APE frame {idx}: {e:?}"))?;
        let skip_bytes = (skip as usize).saturating_mul(self.block_align);
        let pcm = bytes.get(skip_bytes..).unwrap_or(&[]);
        decode_pcm_into(&mut self.buf, pcm, self.bits, self.signed8);
        self.pos = 0;
        Ok(())
    }

    /// Ensure `buf[pos]` is readable, decoding the next frame as needed. Returns
    /// false at end of stream or on a decode error (playback then stops cleanly).
    fn fill(&mut self) -> bool {
        while self.pos >= self.buf.len() {
            if self.next_frame >= self.total_frames {
                return false;
            }
            let idx = self.next_frame;
            self.next_frame += 1;
            if self.load_frame(idx, 0).is_err() {
                return false;
            }
        }
        true
    }
}

impl Iterator for ApeSource {
    type Item = f32;
    fn next(&mut self) -> Option<f32> {
        if !self.fill() {
            return None;
        }
        let s = self.buf[self.pos];
        self.pos += 1;
        Some(s)
    }
}

impl Source for ApeSource {
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
        let target = (pos.as_secs_f64() * self.sample_rate as f64).round() as u64;
        let sr = self
            .dec
            .seek(target)
            .map_err(|_| SeekError::NotSupported { underlying_source: "ape" })?;
        self.load_frame(sr.frame_index, sr.skip_samples)
            .map_err(|_| SeekError::NotSupported { underlying_source: "ape" })?;
        self.next_frame = sr.frame_index + 1;
        Ok(())
    }
}

/// Decode interleaved little-endian PCM `bytes` of `bits` depth into `out` (cleared
/// first) as interleaved f32 in [-1, 1]. 8-bit is unsigned unless `signed8`.
fn decode_pcm_into(out: &mut Vec<f32>, bytes: &[u8], bits: u16, signed8: bool) {
    out.clear();
    match bits {
        8 if signed8 => out.extend(bytes.iter().map(|&b| b as i8 as f32 / 128.0)),
        8 => out.extend(bytes.iter().map(|&b| (b as f32 - 128.0) / 128.0)),
        24 => out.extend(bytes.chunks_exact(3).map(|c| {
            // Sign-extend the 24-bit little-endian sample into an i32.
            let v = ((c[0] as i32) | ((c[1] as i32) << 8) | ((c[2] as i32) << 16)) << 8;
            (v >> 8) as f32 / 8_388_608.0
        })),
        32 => out.extend(
            bytes
                .chunks_exact(4)
                .map(|c| i32::from_le_bytes([c[0], c[1], c[2], c[3]]) as f32 / 2_147_483_648.0),
        ),
        // 16-bit (the common case) and anything else.
        _ => out.extend(
            bytes
                .chunks_exact(2)
                .map(|c| i16::from_le_bytes([c[0], c[1]]) as f32 / 32_768.0),
        ),
    }
}

/// Open an APE file as a lazy, seekable source. Errors cleanly on an unreadable
/// file or an exotic variant we don't handle (caught by the skip-unplayable guard).
pub fn open_ape(path: &Path) -> Result<ApeSource> {
    let file = File::open(path)?;
    let dec = ApeDecoder::new(BufReader::new(file))
        .map_err(|e| anyhow!("couldn't open {}: {e:?}", path.display()))?;
    let info = dec.info().clone();
    // pcm_to_f32 reads little-endian integer samples; refuse the rare exotic variants
    // rather than play byte-swapped or misread noise (the skip-unplayable path then
    // takes over). Real-world APE is little-endian integer.
    if info.is_floating_point {
        return Err(anyhow!("floating-point APE is not supported"));
    }
    if info.is_big_endian {
        return Err(anyhow!("big-endian APE is not supported"));
    }
    if info.channels == 0 || info.sample_rate == 0 {
        return Err(anyhow!("APE has no audio"));
    }
    let total = Some(Duration::from_secs_f64(
        info.total_samples as f64 / info.sample_rate as f64,
    ));
    Ok(ApeSource {
        channels: info.channels,
        sample_rate: info.sample_rate,
        bits: info.bits_per_sample,
        signed8: info.is_signed_8bit,
        block_align: info.block_align as usize,
        total_frames: info.total_frames,
        next_frame: 0,
        buf: Vec::new(),
        pos: 0,
        total,
        dec,
    })
}

/// The length of an APE file, so the playlist can show a time when lofty can't.
/// `None` when it isn't an APE or can't be opened.
pub fn duration(path: &Path) -> Option<Duration> {
    if !is_ape(path) {
        return None;
    }
    open_ape(path).ok().and_then(|s| s.total_duration())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Local .ape samples (qa-music is gitignored, so these skip on CI). `sample.ape`
    /// is 16-bit; `sample-24.ape`, when present, exercises the 24-bit path. The crate's
    /// own decode_all is the oracle: it was vetted bit-exact against ffmpeg, so matching
    /// it proves the streaming wrapper (framing, seeking, buffer reuse) is faithful.
    fn samples() -> Vec<std::path::PathBuf> {
        ["qa-music/format-samples/sample.ape", "qa-music/format-samples/sample-24.ape"]
            .iter()
            .map(std::path::PathBuf::from)
            .filter(|p| p.exists())
            .collect()
    }

    fn reference(p: &Path) -> (ape_decoder::ApeInfo, Vec<f32>) {
        let mut dec = ApeDecoder::new(BufReader::new(File::open(p).unwrap())).unwrap();
        let info = dec.info().clone();
        let mut f = Vec::new();
        decode_pcm_into(&mut f, &dec.decode_all().unwrap(), info.bits_per_sample, info.is_signed_8bit);
        (info, f)
    }

    #[test]
    fn streamed_source_matches_decode_all() {
        let ps = samples();
        if ps.is_empty() {
            return;
        }
        for p in ps {
            let (info, want) = reference(&p);
            let src = open_ape(&p).unwrap();
            assert_eq!(src.channels(), info.channels);
            assert_eq!(src.sample_rate(), info.sample_rate);
            let got: Vec<f32> = src.collect();
            assert_eq!(got.len(), want.len(), "sample count for {}", p.display());
            assert!(got == want, "streamed samples must match decode_all for {}", p.display());
        }
    }

    #[test]
    fn seek_lands_on_the_right_samples() {
        let ps = samples();
        if ps.is_empty() {
            return;
        }
        for p in ps {
            let (info, want) = reference(&p);
            let mut src = open_ape(&p).unwrap();
            // Seek to a fraction of the file's own length, so this holds whatever the
            // fixture's duration is (the fixture names encode bit depth, not seconds).
            let secs = src.total_duration().unwrap().as_secs_f64() * 0.4;
            src.try_seek(Duration::from_secs_f64(secs)).unwrap();
            let got: Vec<f32> = src.collect();
            let start =
                (secs * info.sample_rate as f64).round() as usize * info.channels as usize;
            let want = &want[start.min(want.len())..];
            assert_eq!(got.len(), want.len(), "seeked tail length for {}", p.display());
            assert!(got == want, "seeked samples must match the tail for {}", p.display());
        }
    }
}
