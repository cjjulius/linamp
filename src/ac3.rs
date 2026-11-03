//! AC-3 / E-AC-3 (Dolby Digital) playback via the pure-Rust filmcraft-ac3 crate (#161).
//!
//! symphonia has no AC-3 decoder; the usual free ones (liba52, ffmpeg) are GPL, which
//! is why this was deferred under LinAmp's Apache-2.0 license. filmcraft-ac3 is a
//! clean-room, MIT/Apache pure-Rust decoder, so AC-3 joins MIDI/tracker/APE as a
//! special-case source: a lazy, seekable rodio `Source` that decodes raw `.ac3`
//! syncframes and downmixes to stereo. Vetted against ffmpeg (RMS ~1e-5) before use.

use std::path::Path;
use std::time::Duration;

use anyhow::{anyhow, Result};
use filmcraft_ac3::{parse_header, Decoded, Decoder};
use rodio::source::SeekError;
use rodio::Source;

/// Raw Dolby Digital extensions handled here (AC-3 and E-AC-3).
pub const AC3_EXTS: &[&str] = &["ac3", "eac3", "ec3"];

/// True when `path` is a raw Dolby Digital file routed through this module.
pub fn is_ac3(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| AC3_EXTS.contains(&e.to_ascii_lowercase().as_str()))
        .unwrap_or(false)
}

/// A lazy, seekable rodio `Source` that decodes a raw AC-3 stream to stereo f32.
pub struct Ac3Source {
    dec: Decoder,
    data: Vec<u8>,
    /// Byte offset + cumulative output-sample (per channel) start of each frame, for seeking.
    frames: Vec<(usize, u64)>,
    sample_rate: u32,
    /// Index of the next frame to decode when the buffer drains.
    next_frame: usize,
    /// Current frame downmixed to interleaved stereo f32, and our read position.
    buf: Vec<f32>,
    pos: usize,
    total: Option<Duration>,
}

impl Ac3Source {
    /// Decode frame `idx` into `self.buf` as interleaved stereo f32.
    fn load_frame(&mut self, idx: usize) -> Result<()> {
        let (off, _) = self.frames[idx];
        let hdr = parse_header(&self.data[off..]).map_err(|e| anyhow!("AC-3 header: {e:?}"))?;
        let end = (off + hdr.frame_bytes).min(self.data.len());
        let decoded = self
            .dec
            .decode(&self.data[off..end])
            .map_err(|e| anyhow!("AC-3 frame {idx}: {e:?}"))?;
        downmix_stereo(&decoded, &mut self.buf);
        self.pos = 0;
        Ok(())
    }

    fn fill(&mut self) -> bool {
        while self.pos >= self.buf.len() {
            if self.next_frame >= self.frames.len() {
                return false;
            }
            let idx = self.next_frame;
            self.next_frame += 1;
            if self.load_frame(idx).is_err() {
                // A non-primary E-AC-3 syncframe decodes to nothing; skip it. Any real
                // error leaves buf empty and the loop advances to the next frame.
                continue;
            }
        }
        true
    }
}

impl Iterator for Ac3Source {
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

impl Source for Ac3Source {
    fn current_frame_len(&self) -> Option<usize> {
        None
    }
    fn channels(&self) -> u16 {
        2
    }
    fn sample_rate(&self) -> u32 {
        self.sample_rate
    }
    fn total_duration(&self) -> Option<Duration> {
        self.total
    }
    fn try_seek(&mut self, pos: Duration) -> Result<(), SeekError> {
        let target = (pos.as_secs_f64() * self.sample_rate as f64) as u64;
        // Find the frame whose output range contains the target sample.
        let idx = match self.frames.binary_search_by(|&(_, s)| s.cmp(&target)) {
            Ok(i) => i,
            Err(i) => i.saturating_sub(1),
        };
        self.dec.reset();
        self.load_frame(idx)
            .map_err(|_| SeekError::NotSupported { underlying_source: "ac3" })?;
        self.next_frame = idx + 1;
        // Drop the samples before the target within this frame (2 per interleaved group).
        let skip = (target.saturating_sub(self.frames[idx].1)) as usize * 2;
        self.pos = skip.min(self.buf.len());
        Ok(())
    }
}

/// Downmix a decoded AC-3 frame (planar, WAV order L,R,C,LFE,Ls,Rs) to interleaved
/// stereo f32 in `out`. Uses the standard ITU -3 dB centre/surround coefficients;
/// LFE is dropped. Mono is duplicated; dual-mono and stereo pass straight through.
fn downmix_stereo(d: &Decoded, out: &mut Vec<f32>) {
    out.clear();
    let ch = &d.channels;
    if ch.is_empty() {
        return;
    }
    let n = ch[0].len();
    // Map the present channels (WAV order) to L/R/C/Ls/Rs by the coding mode, so the
    // downmix picks the right planes regardless of how many are present.
    let (li, ri, ci, lsi, rsi) = roles(d.header.acmod, d.header.lfeon, ch.len());
    const K: f32 = 0.707_106_77; // -3 dB
    // Bounds-checked: a malformed frame could report an acmod implying more channels
    // than it actually decoded, so missing planes read as silence rather than panic.
    let get = |opt: Option<usize>, i: usize| {
        opt.and_then(|c| ch.get(c)).and_then(|v| v.get(i)).copied().unwrap_or(0.0)
    };
    out.reserve(n * 2);
    for i in 0..n {
        let c = get(ci, i) * K;
        let mut lo = get(li, i) + c + get(lsi, i) * K;
        let mut ro = get(ri, i) + c + get(rsi, i) * K;
        lo = lo.clamp(-1.0, 1.0);
        ro = ro.clamp(-1.0, 1.0);
        out.push(lo);
        out.push(ro);
    }
}

/// Indices of (L, R, C, Ls, Rs) within a decoded frame's WAV-ordered channel vec,
/// given `acmod`, `lfeon` and the channel count. LFE occupies the slot after C.
fn roles(acmod: u8, lfeon: bool, nch: usize) -> (Option<usize>, Option<usize>, Option<usize>, Option<usize>, Option<usize>) {
    // Mono (1/0): the single plane is centre; feed it to both.
    if acmod == 1 {
        return (Some(0), Some(0), None, None, None);
    }
    // Build the WAV-order role layout (excluding LFE), then account for the LFE slot.
    // WAV order is L, R, C, LFE, Ls, Rs; only the modes' channels are present.
    let has_c = matches!(acmod, 3 | 5 | 7);
    // L, R are present for every non-mono mode (dual-mono 0 maps ch0/ch1 to L/R).
    let (l, r) = (Some(0), Some(1));
    let (mut c, mut ls, mut rs) = (None, None, None);
    let mut i = 2usize;
    if has_c {
        c = Some(i);
        i += 1;
    }
    if lfeon {
        i += 1; // LFE occupies this slot but is dropped from the downmix.
    }
    match acmod {
        4 | 5 => {
            // Single (mono) surround: send it to both Ls and Rs at -3 dB.
            if i < nch {
                ls = Some(i);
                rs = Some(i);
            }
        }
        6 | 7 => {
            if i < nch {
                ls = Some(i);
            }
            if i + 1 < nch {
                rs = Some(i + 1);
            }
        }
        _ => {}
    }
    (l, r, c, ls, rs)
}

/// Open a raw AC-3 file as a lazy, seekable stereo source. Scans syncframes once for
/// the seek index and duration, and validates the first frame so an unsupported or
/// corrupt stream errors cleanly (caught by the skip-unplayable guard).
pub fn open_ac3(path: &Path) -> Result<Ac3Source> {
    let data = std::fs::read(path)?;
    // Scan syncframes: record each frame's byte offset and cumulative sample start.
    let mut frames = Vec::new();
    let mut pos = 0usize;
    let mut sample_rate = 0u32;
    let mut total_samples = 0u64;
    while pos + 8 <= data.len() {
        let hdr = match parse_header(&data[pos..]) {
            Ok(h) => h,
            Err(_) => break,
        };
        if hdr.frame_bytes == 0 || pos + hdr.frame_bytes > data.len() {
            break;
        }
        if sample_rate == 0 {
            sample_rate = hdr.sample_rate;
        }
        // Only primary syncframes contribute output samples to the stream clock.
        frames.push((pos, total_samples));
        if hdr.is_primary() {
            total_samples += hdr.samples() as u64;
        }
        pos += hdr.frame_bytes;
    }
    if frames.is_empty() || sample_rate == 0 {
        return Err(anyhow!("no AC-3 syncframes in {}", path.display()));
    }
    // Validate the first frame actually decodes (catches unsupported E-AC-3 features).
    let mut dec = Decoder::new();
    let (off, _) = frames[0];
    let hdr = parse_header(&data[off..]).map_err(|e| anyhow!("AC-3 header: {e:?}"))?;
    let end = (off + hdr.frame_bytes).min(data.len());
    dec.decode(&data[off..end]).map_err(|e| anyhow!("AC-3: {e:?}"))?;
    dec.reset();
    let total = Some(Duration::from_secs_f64(total_samples as f64 / sample_rate as f64));
    Ok(Ac3Source {
        dec,
        data,
        frames,
        sample_rate,
        next_frame: 0,
        buf: Vec::new(),
        pos: 0,
        total,
    })
}

/// The length of a raw AC-3 file, so the playlist can show a time when lofty can't.
/// `None` when it isn't an AC-3 or can't be opened.
pub fn duration(path: &Path) -> Option<Duration> {
    if !is_ac3(path) {
        return None;
    }
    open_ac3(path).ok().and_then(|s| s.total_duration())
}

#[cfg(test)]
mod tests {
    use super::*;

    // A local raw .ac3 sample (qa-music is gitignored, so these skip on CI). The
    // decoder itself was vetted against ffmpeg (RMS ~1e-5) in the spike; these guard
    // the framing, downmix-to-stereo and seek wrapper against regressions.
    fn sample() -> Option<std::path::PathBuf> {
        let p = std::path::Path::new("qa-music/format-samples/sample.ac3");
        p.exists().then(|| p.to_path_buf())
    }

    // All local .ac3 samples present (stereo plus, when present, a 5.1 file that
    // exercises the downmix path). qa-music is gitignored, so this skips on CI.
    fn samples() -> Vec<std::path::PathBuf> {
        ["sample.ac3", "song-stereo.ac3", "song-5.1.ac3"]
            .iter()
            .map(|n| std::path::Path::new("qa-music/format-samples").join(n))
            .filter(|p| p.exists())
            .collect()
    }

    #[test]
    fn decodes_to_stereo_in_range() {
        let ps = samples();
        if ps.is_empty() {
            return;
        }
        for p in ps {
            let src = open_ac3(&p).unwrap();
            assert_eq!(src.channels(), 2, "{}", p.display());
            assert!(matches!(src.sample_rate(), 32000 | 44100 | 48000));
            let total = src.total.unwrap().as_secs_f64();
            let rate = src.sample_rate();
            let got: Vec<f32> = src.collect();
            assert!(!got.is_empty(), "AC-3 decoded no samples for {}", p.display());
            assert!(
                got.iter().all(|s| s.is_finite() && s.abs() <= 1.0),
                "samples out of range for {}",
                p.display()
            );
            // Interleaved stereo, so the frame count lines up with the reported duration.
            let secs = got.len() as f64 / 2.0 / rate as f64;
            assert!((secs - total).abs() < 0.1, "{}: decoded {secs:.3}s vs {total:.3}s", p.display());
        }
    }

    #[test]
    fn seek_lands_near_the_target() {
        let Some(p) = sample() else { return };
        let full = open_ac3(&p).unwrap().count();
        let mut src = open_ac3(&p).unwrap();
        let dur = src.total_duration().unwrap();
        let rate = src.sample_rate();
        let secs = dur.as_secs_f64() * 0.4;
        src.try_seek(Duration::from_secs_f64(secs)).unwrap();
        let tail = src.count();
        let expected_tail = full - (secs * rate as f64) as usize * 2;
        // One AC-3 frame (1536 samples/ch, 2 ch) of slack for frame-boundary rounding.
        assert!(
            (tail as i64 - expected_tail as i64).abs() <= 1536 * 2,
            "seek tail {tail} vs expected ~{expected_tail}"
        );
    }
}
