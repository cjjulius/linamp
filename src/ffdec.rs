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
/// extensions), Dolby TrueHD / MLP (the MLP-based lossless codec used on Blu-ray,
/// raw `.thd`/`.mlp` streams), and RealAudio / RealMedia (`.ra`/`.rm`: the RealMedia
/// demuxer plus the reverse-engineered RealAudio decoders: ra_144/ra_288, Cook,
/// SIPR, ATRAC3 and RealAudio Lossless), TAK (`.tak`, Tom's lossless Audio
/// Kompressor), and Sony ATRAC (`.oma`/`.omg`/`.aa3` OpenMG, `.at3` RIFF, `.aea`
/// MiniDisc: ATRAC1/ATRAC3/ATRAC3+). None has a permissive pure-Rust decoder mature
/// enough for real-time playback, so FFmpeg demuxes and decodes them through the one
/// generic path here (multichannel is downmixed to stereo; a `.rm` that also carries
/// video plays its best audio stream). TAK is lossless and at most 24-bit, which f32
/// represents exactly, so routing it through the f32 path preserves its samples.
///
/// Also a set of older/retro formats FFmpeg already decodes natively, routed here for the
/// same reason (no mature pure-Rust decoder): Shorten (`.shn`, lossless), CRI ADX (`.adx`,
/// game audio), Amiga 8SVX (`.8svx`), Creative Voice (`.voc`), Sun/NeXT (`.au`) and Sony
/// Wave64 (`.w64`).
///
/// `.ram` is deliberately excluded: it is a plain-text metafile holding a stream URL,
/// not an audio container, so it would belong to playlist handling, not the decoder.
/// `.at9` (ATRAC9) is held back until there is a sample to verify it against.
pub fn is_ffmpeg_audio(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| {
            e.eq_ignore_ascii_case("wma")
                || e.eq_ignore_ascii_case("dts")
                || e.eq_ignore_ascii_case("thd")
                || e.eq_ignore_ascii_case("mlp")
                || e.eq_ignore_ascii_case("ra")
                || e.eq_ignore_ascii_case("rm")
                || e.eq_ignore_ascii_case("tak")
                || e.eq_ignore_ascii_case("oma")
                || e.eq_ignore_ascii_case("omg")
                || e.eq_ignore_ascii_case("aa3")
                || e.eq_ignore_ascii_case("at3")
                || e.eq_ignore_ascii_case("aea")
                || e.eq_ignore_ascii_case("shn")
                || e.eq_ignore_ascii_case("8svx")
                || e.eq_ignore_ascii_case("adx")
                || e.eq_ignore_ascii_case("voc")
                || e.eq_ignore_ascii_case("au")
                || e.eq_ignore_ascii_case("w64")
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

// `ictx.packets()` only ends on EOF and silently retries every other read error, so a
// truncated or corrupt input whose av_read_frame keeps returning a non-EOF error (e.g. an
// I/O error) spins it forever at 100% CPU and hangs the player. We read packets manually and
// bail after this many consecutive read errors, so a bad file ends instead of looping, while
// a handful of transient glitches are still tolerated.
const MAX_CONSECUTIVE_READ_ERRORS: u32 = 1024;

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
            .as_chunks::<4>()
            .0
            .iter()
            .map(|&b| f32::from_ne_bytes(b)),
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

    // Read packets manually rather than via `ictx.packets()` (see MAX_CONSECUTIVE_READ_ERRORS):
    // stop on EOF, and bail after a run of read errors so a bad file ends instead of looping.
    let mut read_errors: u32 = 0;
    let mut packet = ffmpeg::codec::packet::Packet::empty();
    loop {
        match packet.read(&mut ictx) {
            Ok(()) => {
                read_errors = 0;
                if packet.stream() == stream_index {
                    decoder
                        .send_packet(&packet)
                        .map_err(|e| anyhow!("ffmpeg send packet: {e}"))?;
                    drain(&mut decoder, &mut resampler, &mut in_layout, &mut samples)?;
                }
            }
            Err(ffmpeg::Error::Eof) => break,
            Err(_) => {
                read_errors += 1;
                if read_errors >= MAX_CONSECUTIVE_READ_ERRORS {
                    break;
                }
            }
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

/// True if FFmpeg can decode at least one audio frame from `path`. Cheap: it stops at the
/// first decoded frame instead of decoding the whole file. Some streams open and demux fine
/// but decode nothing - an old TAK v1 file, for instance, which our FFmpeg can read but not
/// decode. Those should be kept out of the playlist rather than added and then skipped at
/// playback, so the playlist uses this at add time to gate FFmpeg-routed files. `false` if
/// the file will not open, has no audio stream, or yields no frame.
pub fn probe_decodable(path: &Path) -> bool {
    if ffmpeg::init().is_err() {
        return false;
    }
    let Ok(mut ictx) = ffmpeg::format::input(&path) else {
        return false;
    };
    let Some(stream) = ictx.streams().best(ffmpeg::media::Type::Audio) else {
        return false;
    };
    let stream_index = stream.index();
    let decoder = ffmpeg::codec::context::Context::from_parameters(stream.parameters())
        .ok()
        .and_then(|c| c.decoder().audio().ok());
    let Some(mut decoder) = decoder else {
        return false;
    };
    let mut frame = AudioFrame::empty();
    let mut read_errors: u32 = 0;
    let mut packet = ffmpeg::codec::packet::Packet::empty();
    loop {
        match packet.read(&mut ictx) {
            Ok(()) => {
                read_errors = 0;
                if packet.stream() == stream_index
                    && decoder.send_packet(&packet).is_ok()
                    && decoder.receive_frame(&mut frame).is_ok()
                {
                    return true;
                }
            }
            Err(ffmpeg::Error::Eof) => break,
            Err(_) => {
                read_errors += 1;
                if read_errors >= MAX_CONSECUTIVE_READ_ERRORS {
                    break;
                }
            }
        }
    }
    // A decoder may hold the first frame until it is flushed at end of stream.
    decoder.send_eof().is_ok() && decoder.receive_frame(&mut frame).is_ok()
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
    if units > 0 {
        // units are in `time_base` ticks; seconds = units * (num / den).
        let secs = units as f64 * time_base.numerator() as f64 / time_base.denominator() as f64;
        if secs > 0.0 {
            return Some(Duration::from_secs_f64(secs));
        }
    }
    // Last resort: Shorten (`.shn`) stores no length in the container, the stream,
    // or any packet (FFmpeg reports N/A for all three), so neither path above finds
    // one and the duration would otherwise appear only once playback starts and
    // backfills it. Decode the stream and count the samples. This is a full decode,
    // but it runs only for the few files the cheap paths miss, and those formats are
    // small and fast to decode (a several-MB `.shn` decodes in well under a tenth of
    // a second), so the playlist scan stays responsive.
    duration_by_decode(path)
}

/// Read the best audio stream's sample rate, channel count, and bitrate (kbps) for the playlist
/// display, without decoding. Unlike `duration`, this works for any file FFmpeg can open (not only
/// the `is_ffmpeg_audio` set), so it also fills in the video/AV containers and the formats whose
/// own decoders expose no metadata. Bitrate prefers the audio stream's own figure, then the
/// container's overall rate, then a file-size / duration estimate, so even a lossless stream that
/// declares no bitrate still shows an accurate number. `None` if there is no readable audio stream.
pub fn media_info(path: &Path) -> Option<(u32, u8, u32)> {
    ffmpeg::init().ok()?;
    let ictx = ffmpeg::format::input(&path).ok()?;
    let micros = ictx.duration();
    let fmt_bits = ictx.bit_rate(); // AVFormatContext.bit_rate (bits/s), whole file
    let stream = ictx.streams().best(ffmpeg::media::Type::Audio)?;
    let decoder = ffmpeg::codec::context::Context::from_parameters(stream.parameters())
        .ok()?
        .decoder()
        .audio()
        .ok()?;
    let rate = decoder.rate();
    if rate == 0 {
        return None;
    }
    let channels = decoder.channels() as u8;
    // Bitrate in bits/s: the codec's own figure, else the container's overall rate, else estimate
    // from the file size and duration (covers lossless streams that report no bitrate).
    let mut bits = decoder.bit_rate() as u64;
    if bits == 0 {
        bits = fmt_bits.max(0) as u64;
    }
    if bits == 0 && micros > 0 {
        if let Ok(meta) = std::fs::metadata(path) {
            bits = meta.len().saturating_mul(8).saturating_mul(1_000_000) / micros as u64;
        }
    }
    Some((rate, channels, (bits / 1000) as u32))
}

/// Decode a whole audio stream and count its output samples to get an exact length,
/// for the formats that expose no duration any cheaper way (see `duration`). It only
/// counts each decoded frame's own sample count, so there is no resampling cost, and
/// it reuses the same guarded manual packet loop as `open` so a corrupt file ends
/// instead of looping.
fn duration_by_decode(path: &Path) -> Option<Duration> {
    let mut ictx = ffmpeg::format::input(&path).ok()?;
    let stream = ictx.streams().best(ffmpeg::media::Type::Audio)?;
    let stream_index = stream.index();
    let mut decoder = ffmpeg::codec::context::Context::from_parameters(stream.parameters())
        .ok()?
        .decoder()
        .audio()
        .ok()?;
    let rate = decoder.rate();
    if rate == 0 {
        return None;
    }

    let mut frame = AudioFrame::empty();
    let mut total_samples: u64 = 0;
    let mut read_errors: u32 = 0;
    let mut packet = ffmpeg::codec::packet::Packet::empty();
    loop {
        match packet.read(&mut ictx) {
            Ok(()) => {
                read_errors = 0;
                if packet.stream() == stream_index && decoder.send_packet(&packet).is_ok() {
                    while decoder.receive_frame(&mut frame).is_ok() {
                        total_samples += frame.samples() as u64;
                    }
                }
            }
            Err(ffmpeg::Error::Eof) => break,
            Err(_) => {
                read_errors += 1;
                if read_errors >= MAX_CONSECUTIVE_READ_ERRORS {
                    break;
                }
            }
        }
    }
    if decoder.send_eof().is_ok() {
        while decoder.receive_frame(&mut frame).is_ok() {
            total_samples += frame.samples() as u64;
        }
    }

    (total_samples > 0).then(|| Duration::from_secs_f64(total_samples as f64 / rate as f64))
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

    /// Decode hypno-this.thd (Dolby TrueHD, MLP-based lossless) and check it against an
    /// ffmpeg-CLI decode of the same file (the oracle: `ffmpeg -i hypno-this.thd -f f32le
    /// -ac 2 -ar 44100`). Both use ffmpeg's truehd decoder, so the match is near-exact; a
    /// small lag search absorbs any frame-alignment offset. (qa-music gitignored.)
    #[test]
    fn decodes_truehd_close_to_the_ffmpeg_oracle() {
        let thd = std::path::PathBuf::from("qa-music/format-samples/hypno-this.thd");
        let oracle_path =
            std::path::PathBuf::from("qa-music/format-samples/oracles/hypno-truehd-oracle.f32");
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

    /// Decode hypno-this.ra (RealAudio, via the RealMedia demuxer) and check it against an
    /// ffmpeg-CLI decode of the same file (the oracle: `ffmpeg -i hypno-this.ra -f f32le
    /// -ac 2 -ar 8000`). The sample is RealAudio 1.0 (ra_144, 8 kHz mono, upmixed to
    /// stereo); both paths run ffmpeg's ra_144 decoder, so the match is near-exact. A small
    /// lag search absorbs any frame-alignment offset. (qa-music gitignored, so this skips
    /// on CI.) RealAudio in the wild is usually Cook; a Cook `.ra`/`.rm` with its own oracle
    /// drops into this same shape when present (see the README for making one).
    #[test]
    fn decodes_realaudio_close_to_the_ffmpeg_oracle() {
        let ra = std::path::PathBuf::from("qa-music/format-samples/hypno-this.ra");
        let oracle_path =
            std::path::PathBuf::from("qa-music/format-samples/oracles/hypno-ra-oracle.f32");
        if !ra.exists() || !oracle_path.exists() {
            return;
        }
        let raw = std::fs::read(&oracle_path).unwrap();
        let oracle: Vec<f32> = raw
            .chunks_exact(4)
            .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
            .collect();
        let src = open(&ra).unwrap();
        assert_eq!(src.channels(), 2, "stereo output");
        let got: Vec<f32> = src.collect();
        assert!(got.len() > 8_000 * 2, "too little audio");
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
        assert!(best > 0.999, "RealAudio decode correlation vs oracle too low: {best:.4}");
    }

    /// Decode cook-sample.rm (RealAudio Cook / G2, the codec RealAudio files use in
    /// practice) and check it against an ffmpeg-CLI decode (`ffmpeg -i cook-sample.rm
    /// -f f32le -ac 2 -ar 44100`). Cook is decode-only (no encoder), so this uses a real
    /// RealMedia sample rather than a minted one; both paths run ffmpeg's cook decoder, so
    /// the match is near-exact, with a small lag search for frame alignment. (qa-music
    /// gitignored; see the README for fetching the sample.)
    #[test]
    fn decodes_cook_close_to_the_ffmpeg_oracle() {
        let rm = std::path::PathBuf::from("qa-music/format-samples/cook-sample.rm");
        let oracle_path =
            std::path::PathBuf::from("qa-music/format-samples/oracles/cook-oracle.f32");
        if !rm.exists() || !oracle_path.exists() {
            return;
        }
        let raw = std::fs::read(&oracle_path).unwrap();
        let oracle: Vec<f32> = raw
            .chunks_exact(4)
            .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
            .collect();
        let src = open(&rm).unwrap();
        assert_eq!(src.channels(), 2, "stereo output");
        let got: Vec<f32> = src.collect();
        assert!(got.len() > 44_100, "too little audio");
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
        assert!(best > 0.999, "Cook decode correlation vs oracle too low: {best:.4}");
    }

    /// Decode tak-sample.tak (TAK, Tom's lossless Audio Kompressor) and check it against an
    /// ffmpeg-CLI decode of the same file (the oracle: `ffmpeg -i tak-sample.tak -t 30
    /// -f f32le -ac 2 -ar 44100`, the first 30 s only to keep the test quick). TAK has no
    /// encoder outside the Windows reference tool, so this is a procured sample; both paths
    /// run ffmpeg's tak decoder, so the match is near-exact (a small lag search absorbs any
    /// frame-alignment offset). Our decode is the whole file; the correlation only compares
    /// the overlapping prefix. (qa-music gitignored; see the README for fetching the sample.)
    #[test]
    fn decodes_tak_close_to_the_ffmpeg_oracle() {
        let tak = std::path::PathBuf::from("qa-music/format-samples/tak-sample.tak");
        let oracle_path =
            std::path::PathBuf::from("qa-music/format-samples/oracles/tak-oracle.f32");
        if !tak.exists() || !oracle_path.exists() {
            return;
        }
        let raw = std::fs::read(&oracle_path).unwrap();
        let oracle: Vec<f32> = raw
            .chunks_exact(4)
            .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
            .collect();
        let src = open(&tak).unwrap();
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
        assert!(best > 0.999, "TAK decode correlation vs oracle too low: {best:.4}");
    }

    /// Shared helper for the lossy-codec oracle tests: decode `sample` through our FFmpeg
    /// path and return the best correlation of its left channel against `oracle` (an
    /// ffmpeg-CLI f32le/stereo/`rate` decode of the same file), over a small lag search.
    /// Returns None when the sample or oracle is absent (qa-music is gitignored).
    #[cfg(feature = "ffmpeg")]
    fn oracle_correlation(sample: &str, oracle: &str) -> Option<f64> {
        let s = std::path::PathBuf::from(sample);
        let o = std::path::PathBuf::from(oracle);
        if !s.exists() || !o.exists() {
            return None;
        }
        let raw = std::fs::read(&o).unwrap();
        let oracle: Vec<f32> = raw
            .chunks_exact(4)
            .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
            .collect();
        let src = open(&s).unwrap();
        assert_eq!(src.channels(), 2, "stereo output");
        let got: Vec<f32> = src.collect();
        assert!(got.len() > 44_100, "too little audio from {sample}");
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
        Some(best)
    }

    /// Decode atrac3plus-sample.oma (Sony ATRAC3+, the OpenMG/Walkman container) against an
    /// ffmpeg-CLI oracle (`ffmpeg -i atrac3plus-sample.oma -t 20 -f f32le -ac 2 -ar 44100`).
    /// ATRAC has no encoder here, so this is a procured sample; both paths run ffmpeg's
    /// atrac3p decoder, so the match is near-exact. (qa-music gitignored; see the README.)
    #[test]
    fn decodes_atrac3plus_close_to_the_ffmpeg_oracle() {
        let Some(best) = oracle_correlation(
            "qa-music/format-samples/atrac3plus-sample.oma",
            "qa-music/format-samples/oracles/atrac3plus-oracle.f32",
        ) else {
            return;
        };
        assert!(best > 0.999, "ATRAC3+ decode correlation vs oracle too low: {best:.4}");
    }

    /// Decode atrac1-sample.aea (Sony ATRAC1, the MiniDisc codec, a different decoder from
    /// ATRAC3+) against an ffmpeg-CLI oracle (first 20 s). Both paths run ffmpeg's atrac1
    /// decoder, so the match is near-exact. (qa-music gitignored; see the README.)
    #[test]
    fn decodes_atrac1_close_to_the_ffmpeg_oracle() {
        let Some(best) = oracle_correlation(
            "qa-music/format-samples/atrac1-sample.aea",
            "qa-music/format-samples/oracles/atrac1-oracle.f32",
        ) else {
            return;
        };
        assert!(best > 0.999, "ATRAC1 decode correlation vs oracle too low: {best:.4}");
    }

    /// Tier 1 retro/older formats FFmpeg decodes natively, each against an ffmpeg-CLI oracle
    /// (`ffmpeg -i <sample> -t 20 -f f32le -ac 2 -ar <rate> <oracle>`). Shorten is lossless;
    /// ADX is ADPCM; the rest are PCM in older containers. Both paths use the same FFmpeg
    /// decoder, so each matches near-exactly. (qa-music gitignored; see the README.)
    #[test]
    fn decodes_shorten_close_to_the_ffmpeg_oracle() {
        if let Some(best) = oracle_correlation(
            "qa-music/format-samples/shorten-sample.shn",
            "qa-music/format-samples/oracles/shorten-oracle.f32",
        ) {
            assert!(best > 0.999, "Shorten decode correlation vs oracle too low: {best:.4}");
        }
    }

    #[test]
    fn decodes_8svx_close_to_the_ffmpeg_oracle() {
        if let Some(best) = oracle_correlation(
            "qa-music/format-samples/amiga-sample.8svx",
            "qa-music/format-samples/oracles/amiga-oracle.f32",
        ) {
            assert!(best > 0.999, "8SVX decode correlation vs oracle too low: {best:.4}");
        }
    }

    #[test]
    fn decodes_adx_close_to_the_ffmpeg_oracle() {
        if let Some(best) = oracle_correlation(
            "qa-music/format-samples/adx-sample.adx",
            "qa-music/format-samples/oracles/adx-oracle.f32",
        ) {
            assert!(best > 0.999, "ADX decode correlation vs oracle too low: {best:.4}");
        }
    }

    #[test]
    fn decodes_voc_close_to_the_ffmpeg_oracle() {
        if let Some(best) = oracle_correlation(
            "qa-music/format-samples/voc-sample.voc",
            "qa-music/format-samples/oracles/voc-oracle.f32",
        ) {
            assert!(best > 0.999, "VOC decode correlation vs oracle too low: {best:.4}");
        }
    }

    #[test]
    fn decodes_au_close_to_the_ffmpeg_oracle() {
        if let Some(best) = oracle_correlation(
            "qa-music/format-samples/au-sample.au",
            "qa-music/format-samples/oracles/au-oracle.f32",
        ) {
            assert!(best > 0.999, "AU decode correlation vs oracle too low: {best:.4}");
        }
    }

    #[test]
    fn decodes_wave64_close_to_the_ffmpeg_oracle() {
        if let Some(best) = oracle_correlation(
            "qa-music/format-samples/w64-sample.w64",
            "qa-music/format-samples/oracles/w64-oracle.f32",
        ) {
            assert!(best > 0.999, "Wave64 decode correlation vs oracle too low: {best:.4}");
        }
    }

    /// probe_decodable keeps files that open but decode nothing out of the playlist. Our
    /// bundled FFmpeg demuxes a TAK v1 stream (tak-v1.tak) but decodes zero frames from it,
    /// while a TAK 2.x stream (tak-sample.tak) decodes normally, so the probe must reject the
    /// first and accept the second. (qa-music gitignored, so this skips on CI.)
    #[test]
    fn probe_decodable_rejects_undecodable_and_accepts_good() {
        let good = std::path::PathBuf::from("qa-music/format-samples/tak-sample.tak");
        if good.exists() {
            assert!(probe_decodable(&good), "a decodable TAK 2.x stream must probe true");
        }
        let bad = std::path::PathBuf::from("qa-music/format-samples/tak-v1.tak");
        if bad.exists() {
            assert!(!probe_decodable(&bad), "an undecodable TAK v1 stream must probe false");
        }
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

    /// Shorten (`.shn`) reports no duration in the container, the stream, or any
    /// packet, so duration() must fall back to a decode-and-count pass; otherwise the
    /// playlist shows no length until the track is played. The counted length must
    /// also match what a full decode via open() reports. Local sample only.
    #[test]
    fn duration_of_shorten_via_decode() {
        let p = std::path::PathBuf::from("qa-music/format-samples/shorten-sample.shn");
        if !p.exists() {
            return;
        }
        let d = duration(&p).expect("shorten duration should be Some").as_secs_f64();
        assert!(d > 1.0 && d < 3600.0, "shorten duration {d}");
        // The decode-count must agree with the length open() derives from the full
        // decoded sample buffer (allow a frame's worth of slack).
        let decoded = open(&p).unwrap().total_duration().unwrap().as_secs_f64();
        assert!((d - decoded).abs() < 0.05, "counted {d} vs decoded {decoded}");
    }
}
