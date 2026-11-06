//! Optional FFmpeg-backed decoding for codecs with no permissive pure-Rust
//! decoder, chiefly Windows Media Audio (.wma).
//!
//! This module only exists when the crate is built with `--features ffmpeg`. It
//! links the system FFmpeg libraries (LGPL), so the default build stays pure Rust
//! and ships nothing under FFmpeg's licence; a packager who wants WMA opts in and
//! takes on bundling libav*. See the `ffmpeg` feature note in Cargo.toml.
//!
//! Like APE/WavPack/TTA this is a whole-file side-decoder: FFmpeg demuxes the
//! container and decodes the audio stream, swresample converts it to interleaved
//! f32, and the result is wrapped as a seekable rodio `Source`. Output is always
//! stereo so any WMA (mono, stereo, or multichannel WMA Pro) plays through the
//! normal stereo path; seeking is an index into the decoded buffer, exact like the
//! other side-decoders.

use std::path::Path;
use std::time::Duration;

use anyhow::{anyhow, Result};
use ffmpeg_next as ffmpeg;
use ffmpeg::format::sample::{Sample, Type as SampleType};
use ffmpeg::util::channel_layout::ChannelLayout;
use ffmpeg::util::frame::audio::Audio as AudioFrame;
use rodio::source::SeekError;
use rodio::Source;

/// Extensions routed through FFmpeg when the feature is on: WMA (ASF container,
/// wmav1/wmav2/WMA Pro/WMA Lossless), DTS (DCA Coherent Acoustics, incl. the HD
/// extensions) and Dolby TrueHD / MLP (the MLP-based lossless codec used on Blu-ray,
/// raw `.thd`/`.mlp` streams). None has a permissive pure-Rust decoder mature enough for
/// real-time playback, so FFmpeg demuxes and decodes them through the one generic path
/// here (multichannel is downmixed to stereo).
pub fn is_ffmpeg_audio(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| {
            e.eq_ignore_ascii_case("wma")
                || e.eq_ignore_ascii_case("dts")
                || e.eq_ignore_ascii_case("thd")
                || e.eq_ignore_ascii_case("mlp")
        })
        .unwrap_or(false)
}

/// A seekable rodio `Source` over a fully-decoded FFmpeg stream (interleaved f32,
/// stereo).
pub struct FfmpegSource {
    samples: Vec<f32>,
    sample_rate: u32,
    pos: usize,
    total: Option<Duration>,
}

const OUT_CHANNELS: u16 = 2;

impl Iterator for FfmpegSource {
    type Item = f32;
    fn next(&mut self) -> Option<f32> {
        let s = self.samples.get(self.pos).copied();
        if s.is_some() {
            self.pos += 1;
        }
        s
    }
}

impl Source for FfmpegSource {
    fn current_frame_len(&self) -> Option<usize> {
        None
    }
    fn channels(&self) -> u16 {
        OUT_CHANNELS
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
            .saturating_mul(OUT_CHANNELS as usize)
            .min(self.samples.len());
        Ok(())
    }
}

/// Pull every interleaved-stereo-f32 sample out of a decoded/resampled frame.
/// swresample hands back a packed frame, so plane 0 holds all channels
/// interleaved; `samples()` is the valid per-channel count (the buffer may be
/// padded past it).
fn append_frame(out: &mut Vec<f32>, frame: &AudioFrame) {
    let valid = frame.samples() * OUT_CHANNELS as usize;
    let bytes = &frame.data(0)[..valid * 4];
    out.extend(
        bytes
            .chunks_exact(4)
            .map(|b| f32::from_ne_bytes([b[0], b[1], b[2], b[3]])),
    );
}

/// Decode a file fully through FFmpeg and wrap it as a seekable stereo source.
/// Errors cleanly (unreadable file, no audio stream, unsupported codec) so the
/// skip-unplayable path takes over.
pub fn open(path: &Path) -> Result<FfmpegSource> {
    ffmpeg::init().map_err(|e| anyhow!("ffmpeg init: {e}"))?;

    let mut ictx = ffmpeg::format::input(&path)
        .map_err(|e| anyhow!("ffmpeg open {}: {e}", path.display()))?;
    let stream = ictx
        .streams()
        .best(ffmpeg::media::Type::Audio)
        .ok_or_else(|| anyhow!("no audio stream in {}", path.display()))?;
    let stream_index = stream.index();

    let mut decoder = ffmpeg::codec::context::Context::from_parameters(stream.parameters())
        .map_err(|e| anyhow!("ffmpeg codec params: {e}"))?
        .decoder()
        .audio()
        .map_err(|e| anyhow!("ffmpeg audio decoder: {e}"))?;

    let rate = decoder.rate();
    if rate == 0 {
        return Err(anyhow!("audio stream has no sample rate"));
    }

    let mut samples: Vec<f32> = Vec::new();
    let mut decoded = AudioFrame::empty();
    let mut resampled = AudioFrame::empty();
    // Built lazily from the FIRST decoded frame's real format/layout/rate:
    // pre-decode the decoder can misreport these (WMA frames come back planar
    // float), and swresample rejects a frame that doesn't match how it was set up.
    let mut resampler: Option<ffmpeg::software::resampling::context::Context> = None;
    // The stable input layout captured from the first frame. Later frames can come
    // back with their layout left unset (bitmask 0); swresample's run() rejects a
    // frame whose layout differs from setup, so we stamp this onto every frame.
    let mut in_layout = ChannelLayout::STEREO;

    let mut drain = |decoder: &mut ffmpeg::decoder::Audio,
                     resampler: &mut Option<ffmpeg::software::resampling::context::Context>,
                     in_layout: &mut ChannelLayout,
                     samples: &mut Vec<f32>|
     -> Result<()> {
        while decoder.receive_frame(&mut decoded).is_ok() {
            if resampler.is_none() {
                *in_layout = if decoded.channel_layout().is_empty() {
                    ChannelLayout::default(decoded.channels() as i32)
                } else {
                    decoded.channel_layout()
                };
                // Convert whatever the codec produces to packed (interleaved) f32
                // stereo at the native rate: upmixes mono and downmixes
                // multichannel, so every WMA plays through the stereo path.
                *resampler = Some(
                    ffmpeg::software::resampling::context::Context::get(
                        decoded.format(),
                        *in_layout,
                        decoded.rate(),
                        Sample::F32(SampleType::Packed),
                        ChannelLayout::STEREO,
                        decoded.rate(),
                    )
                    .map_err(|e| anyhow!("ffmpeg resampler: {e}"))?,
                );
            }
            // Stamp the stable layout so a frame with an unset layout still matches.
            decoded.set_channel_layout(*in_layout);
            let r = resampler.as_mut().unwrap();
            r.run(&decoded, &mut resampled)
                .map_err(|e| anyhow!("ffmpeg resample: {e}"))?;
            append_frame(samples, &resampled);
        }
        Ok(())
    };

    for (stream, packet) in ictx.packets() {
        if stream.index() == stream_index {
            decoder
                .send_packet(&packet)
                .map_err(|e| anyhow!("ffmpeg send packet: {e}"))?;
            drain(&mut decoder, &mut resampler, &mut in_layout, &mut samples)?;
        }
    }
    decoder
        .send_eof()
        .map_err(|e| anyhow!("ffmpeg eof: {e}"))?;
    drain(&mut decoder, &mut resampler, &mut in_layout, &mut samples)?;

    // Flush any samples the resampler is still buffering (downmix carries none at
    // equal rates, but flush unconditionally so nothing is dropped).
    if let Some(mut r) = resampler {
        while r.flush(&mut resampled).is_ok() && resampled.samples() > 0 {
            append_frame(&mut samples, &resampled);
        }
    }

    if samples.is_empty() {
        return Err(anyhow!("decoded no audio from {}", path.display()));
    }

    let frames = samples.len() / OUT_CHANNELS as usize;
    let total = Some(Duration::from_secs_f64(frames as f64 / rate as f64));
    Ok(FfmpegSource {
        samples,
        sample_rate: rate,
        pos: 0,
        total,
    })
}

/// The length of a file FFmpeg can read, for the playlist, without a full audio
/// decode. `None` when FFmpeg can't open it or can't determine a length.
pub fn duration(path: &Path) -> Option<Duration> {
    if !is_ffmpeg_audio(path) {
        return None;
    }
    ffmpeg::init().ok()?;
    let mut ictx = ffmpeg::format::input(&path).ok()?;
    // Containered / bitrate-estimable inputs (WMA, DTS) report it directly:
    // ictx.duration() is in AV_TIME_BASE units (microseconds).
    let micros = ictx.duration();
    if micros > 0 {
        return Some(Duration::from_micros(micros as u64));
    }
    // Raw elementary streams (Dolby TrueHD / MLP, .thd/.mlp) carry no container
    // duration, and FFmpeg can't estimate one from a lossless VBR bitrate. Sum the
    // audio packets' own durations instead: this only demuxes (no decode), and each
    // packet declares its sample count in the stream time base, so the total is
    // exact and still cheap enough for the playlist scan. Without this the length
    // only appears once playback starts (it is then backfilled from the source).
    let (index, time_base) = {
        let stream = ictx.streams().best(ffmpeg::media::Type::Audio)?;
        (stream.index(), stream.time_base())
    };
    let mut units: i64 = 0;
    for (stream, packet) in ictx.packets() {
        if stream.index() == index {
            let d = packet.duration();
            if d > 0 {
                units += d;
            }
        }
    }
    if units <= 0 {
        return None;
    }
    // units are in `time_base` ticks; seconds = units * (num / den).
    let secs = units as f64 * time_base.numerator() as f64 / time_base.denominator() as f64;
    (secs > 0.0).then(|| Duration::from_secs_f64(secs))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Option<std::path::PathBuf> {
        let p = std::path::PathBuf::from("qa-music/format-samples/hypno-this.wma");
        p.exists().then_some(p)
    }

    /// Decode hypno-this.wma and check it against an ffmpeg-CLI decode of the same
    /// file (the oracle: `ffmpeg -i hypno-this.wma -f f32le -ac 2 -ar 44100`). WMA
    /// is lossy, but both paths run the same libavcodec decoder, so the output must
    /// match the oracle very tightly. (qa-music is gitignored, so this skips on CI.)
    #[test]
    fn decodes_close_to_the_ffmpeg_oracle() {
        let Some(wma) = sample() else { return };
        let oracle_path = std::path::PathBuf::from("qa-music/format-samples/oracles/hypno-wma-oracle.f32");
        if !oracle_path.exists() {
            return;
        }
        let raw = std::fs::read(&oracle_path).unwrap();
        let oracle: Vec<f32> = raw
            .chunks_exact(4)
            .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
            .collect();

        let src = open(&wma).unwrap();
        assert_eq!(src.channels(), 2, "stereo output");
        assert_eq!(src.sample_rate(), 44_100);
        let got: Vec<f32> = src.collect();

        // Sample counts should match to within a frame (encoder delay handling is
        // identical, so in practice this is exact).
        let n = got.len().min(oracle.len());
        let diff = (got.len() as i64 - oracle.len() as i64).abs();
        assert!(diff <= 8, "sample-count drift {diff} (got {}, oracle {})", got.len(), oracle.len());

        let mut sq = 0.0f64;
        for i in 0..n {
            let d = (got[i] - oracle[i]) as f64;
            sq += d * d;
        }
        let rms = (sq / n as f64).sqrt();
        assert!(rms < 1e-4, "RMS vs ffmpeg oracle too high: {rms}");
    }

    /// Decode hypno-this.dts (DTS/DCA) and check it against an ffmpeg-CLI decode of the
    /// same file (`ffmpeg -i hypno-this.dts -f f32le -ac 2 -ar 48000`... stored at the
    /// file's native 44.1k). Both use ffmpeg's dca decoder, so the match is near-exact;
    /// a small lag search absorbs any frame-alignment offset. (qa-music gitignored.)
    #[test]
    fn decodes_dts_close_to_the_ffmpeg_oracle() {
        let dts = std::path::PathBuf::from("qa-music/format-samples/hypno-this.dts");
        let oracle_path = std::path::PathBuf::from("qa-music/format-samples/oracles/hypno-dts-oracle.f32");
        if !dts.exists() || !oracle_path.exists() {
            return;
        }
        let raw = std::fs::read(&oracle_path).unwrap();
        let oracle: Vec<f32> = raw
            .chunks_exact(4)
            .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
            .collect();
        let src = open(&dts).unwrap();
        assert_eq!(src.channels(), 2, "stereo output");
        let got: Vec<f32> = src.collect();
        assert!(got.len() > 44_100 * 2, "too little audio");
        // Left channels (step by 2).
        let gl: Vec<f32> = got.iter().step_by(2).copied().collect();
        let ol: Vec<f32> = oracle.iter().step_by(2).copied().collect();
        let corr = |a: &[f32], b: &[f32]| -> f64 {
            let n = a.len().min(b.len());
            if n < 2000 {
                return 0.0;
            }
            let (a, b) = (&a[..n], &b[..n]);
            let (ma, mb) = (
                a.iter().map(|&x| x as f64).sum::<f64>() / n as f64,
                b.iter().map(|&x| x as f64).sum::<f64>() / n as f64,
            );
            let (mut num, mut da, mut db) = (0.0, 0.0, 0.0);
            for i in 0..n {
                let (xa, xb) = (a[i] as f64 - ma, b[i] as f64 - mb);
                num += xa * xb;
                da += xa * xa;
                db += xb * xb;
            }
            if da <= 0.0 || db <= 0.0 { 0.0 } else { num / (da.sqrt() * db.sqrt()) }
        };
        let mut best = 0.0;
        for lag in 0..600usize {
            if lag >= gl.len() {
                break;
            }
            let c = corr(&gl[lag..], &ol);
            if c > best {
                best = c;
            }
        }
        assert!(best > 0.999, "DTS decode correlation vs oracle too low: {best:.4}");
    }

    /// Decode truehd-test.thd (Dolby TrueHD, MLP-based lossless) and check it against an
    /// ffmpeg-CLI decode of the same file (the oracle: `ffmpeg -i truehd-test.thd -f f32le
    /// -ac 2 -ar 44100`). Both use ffmpeg's truehd decoder, so the match is near-exact; a
    /// small lag search absorbs any frame-alignment offset. (qa-music gitignored.)
    #[test]
    fn decodes_truehd_close_to_the_ffmpeg_oracle() {
        let thd = std::path::PathBuf::from("qa-music/format-samples/truehd-test.thd");
        let oracle_path = std::path::PathBuf::from("qa-music/format-samples/oracles/truehd-oracle.f32");
        if !thd.exists() || !oracle_path.exists() {
            return;
        }
        let raw = std::fs::read(&oracle_path).unwrap();
        let oracle: Vec<f32> = raw
            .chunks_exact(4)
            .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
            .collect();
        let src = open(&thd).unwrap();
        assert_eq!(src.channels(), 2, "stereo output");
        let got: Vec<f32> = src.collect();
        assert!(got.len() > 44_100 * 2, "too little audio");
        let gl: Vec<f32> = got.iter().step_by(2).copied().collect();
        let ol: Vec<f32> = oracle.iter().step_by(2).copied().collect();
        let corr = |a: &[f32], b: &[f32]| -> f64 {
            let n = a.len().min(b.len());
            if n < 2000 {
                return 0.0;
            }
            let (a, b) = (&a[..n], &b[..n]);
            let (ma, mb) = (
                a.iter().map(|&x| x as f64).sum::<f64>() / n as f64,
                b.iter().map(|&x| x as f64).sum::<f64>() / n as f64,
            );
            let (mut num, mut da, mut db) = (0.0, 0.0, 0.0);
            for i in 0..n {
                let (xa, xb) = (a[i] as f64 - ma, b[i] as f64 - mb);
                num += xa * xb;
                da += xa * xa;
                db += xb * xb;
            }
            if da <= 0.0 || db <= 0.0 { 0.0 } else { num / (da.sqrt() * db.sqrt()) }
        };
        let mut best = 0.0;
        for lag in 0..600usize {
            if lag >= gl.len() {
                break;
            }
            let c = corr(&gl[lag..], &ol);
            if c > best {
                best = c;
            }
        }
        assert!(best > 0.999, "TrueHD decode correlation vs oracle too low: {best:.4}");
    }

    /// Seeking must land on the right samples: decode, seek to 40%, and confirm the
    /// first samples after the seek match the oracle at that offset.
    #[test]
    fn seek_lands_on_the_right_samples() {
        let Some(wma) = sample() else { return };
        let oracle_path = std::path::PathBuf::from("qa-music/format-samples/oracles/hypno-wma-oracle.f32");
        if !oracle_path.exists() {
            return;
        }
        let raw = std::fs::read(&oracle_path).unwrap();
        let oracle: Vec<f32> = raw
            .chunks_exact(4)
            .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
            .collect();

        let mut src = open(&wma).unwrap();
        let sr = src.sample_rate();
        let secs = src.total_duration().unwrap().as_secs_f64() * 0.4;
        src.try_seek(Duration::from_secs_f64(secs)).unwrap();
        let got: Vec<f32> = src.take(4096).collect();
        let start = (secs * sr as f64).round() as usize * 2;

        let mut sq = 0.0f64;
        let mut n = 0usize;
        for (g, o) in got.iter().zip(oracle[start.min(oracle.len())..].iter()) {
            let d = (*g - *o) as f64;
            sq += d * d;
            n += 1;
        }
        let rms = (sq / n.max(1) as f64).sqrt();
        assert!(rms < 1e-4, "post-seek RMS too high: {rms}");
    }

    /// Decode must run comfortably faster than real time (release only; the debug
    /// build is far slower and not representative).
    #[test]
    #[ignore = "timing; run explicitly in release with --ignored"]
    fn decode_is_real_time() {
        let Some(wma) = sample() else { return };
        let t = std::time::Instant::now();
        let src = open(&wma).unwrap();
        let dur = src.total_duration().unwrap().as_secs_f64();
        let _: Vec<f32> = src.collect();
        let elapsed = t.elapsed().as_secs_f64();
        let rtf = dur / elapsed;
        assert!(rtf > 5.0, "decode only {rtf:.1}x real time");
    }

    #[test]
    fn duration_reads_without_full_decode() {
        let Some(wma) = sample() else { return };
        let d = duration(&wma).expect("wma duration");
        assert!(d.as_secs() > 1 && d.as_secs() < 600, "duration {d:?}");
    }

    /// Raw TrueHD/MLP streams carry no container duration, so duration() must sum
    /// the packets' own durations to report a length before playback (otherwise the
    /// playlist shows no time until the track is played). Local sample only.
    #[test]
    fn duration_of_raw_truehd_without_container() {
        for name in ["hypno-this.thd", "hypno-this.mlp"] {
            let p = std::path::PathBuf::from(format!("qa-music/format-samples/{name}"));
            if !p.exists() {
                continue;
            }
            let d = duration(&p).unwrap_or_else(|| panic!("{name} duration should be Some"));
            assert!(d.as_secs() > 1 && d.as_secs() < 600, "{name} duration {d:?}");
        }
    }
}
