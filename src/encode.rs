//! Rip / convert: encode decoded audio to a file (#183).
//!
//! The decode side (`src/decode.rs`) yields interleaved f32 PCM; this module
//! encodes that to a chosen output format and copies the source tags over. One arm
//! per format behind a small registry (`OutputFormat`), so later formats drop in by
//! adding a variant and an encode arm.
//!
//! FLAC was the intended lossless target but is deferred: the pure-Rust `flacenc`
//! produces output symphonia 0.6 cannot decode (its partial final frame), and the
//! libFLAC bindings need a system library absent on Windows. MP3 (libmp3lame) stands
//! in as the compressed target meanwhile; FLAC is tracked as a follow-up.

use std::path::Path;

use anyhow::{bail, Context, Result};
use rodio::Source;

/// An output format the converter can write. `ALL` is the registry the UI lists and
/// the dispatch matches, so adding a format is one variant plus its arm here.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum OutputFormat {
    /// MP3 via libmp3lame (lossy, widely compatible).
    Mp3,
    /// 16-bit PCM WAV (`hound`, lossless, uncompressed).
    Wav,
}

impl OutputFormat {
    /// Every format the converter offers, in menu order.
    pub const ALL: &'static [OutputFormat] = &[OutputFormat::Mp3, OutputFormat::Wav];

    /// The file extension (no dot).
    pub fn ext(self) -> &'static str {
        match self {
            OutputFormat::Mp3 => "mp3",
            OutputFormat::Wav => "wav",
        }
    }
}

/// Convert one file: decode `src`, encode it to `dst` in `format`, then copy the
/// source tags over (best-effort). `dst`'s parent directory must already exist.
pub fn convert_file(src: &Path, dst: &Path, format: OutputFormat) -> Result<()> {
    let (samples, rate, channels) = decode_pcm(src)?;
    match format {
        OutputFormat::Mp3 => encode_mp3(dst, &samples, rate, channels)?,
        OutputFormat::Wav => encode_wav(dst, &samples, rate, channels)?,
    }
    // Carry the metadata across; a tag failure shouldn't fail the whole convert.
    if let Err(e) = copy_tags(src, dst) {
        eprintln!("convert: tags not copied to {}: {e}", dst.display());
    }
    Ok(())
}

/// Decode `src` fully to interleaved f32 PCM, returning `(samples, rate, channels)`.
fn decode_pcm(src: &Path) -> Result<(Vec<f32>, u32, u16)> {
    let source = crate::decode::open_file(src)?;
    let rate = source.sample_rate();
    let channels = source.channels();
    let samples: Vec<f32> = source.collect();
    if samples.is_empty() {
        bail!("no audio decoded from {}", src.display());
    }
    Ok((samples, rate, channels))
}

/// Interleaved f32 (-1.0..=1.0) to 16-bit PCM, clamped.
fn f32_to_i16(s: f32) -> i16 {
    (s.clamp(-1.0, 1.0) * i16::MAX as f32).round() as i16
}

fn encode_wav(dst: &Path, samples: &[f32], rate: u32, channels: u16) -> Result<()> {
    let spec = hound::WavSpec {
        channels,
        sample_rate: rate,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut w = hound::WavWriter::create(dst, spec)
        .with_context(|| format!("create {}", dst.display()))?;
    for &s in samples {
        w.write_sample(f32_to_i16(s))?;
    }
    w.finalize()?;
    Ok(())
}

fn encode_mp3(dst: &Path, samples: &[f32], rate: u32, channels: u16) -> Result<()> {
    use mp3lame_encoder::{Bitrate, Builder, DualPcm, FlushNoGap, MonoPcm, Quality};
    if channels == 0 || channels > 2 {
        bail!("MP3 export supports mono or stereo, not {channels} channels");
    }
    let mut builder = Builder::new().ok_or_else(|| anyhow::anyhow!("LAME init failed"))?;
    let m = |r: std::result::Result<(), mp3lame_encoder::BuildError>, what: &str| {
        r.map_err(|e| anyhow::anyhow!("LAME {what}: {e:?}"))
    };
    m(builder.set_num_channels(channels as u8), "channels")?;
    m(builder.set_sample_rate(rate), "sample rate")?;
    m(builder.set_brate(Bitrate::Kbps192), "bitrate")?;
    m(builder.set_quality(Quality::Good), "quality")?;
    let mut enc = builder
        .build()
        .map_err(|e| anyhow::anyhow!("LAME build: {e:?}"))?;

    let pcm: Vec<i16> = samples.iter().map(|&s| f32_to_i16(s)).collect();
    let per_channel = if channels == 1 { pcm.len() } else { pcm.len() / 2 };
    // encode_to_vec / flush_to_vec write into the Vec's spare capacity without
    // reserving it themselves, so reserve the LAME-recommended worst case first,
    // or libmp3lame writes out of bounds.
    let mut out: Vec<u8> = Vec::with_capacity(mp3lame_encoder::max_required_buffer_size(per_channel));
    if channels == 1 {
        enc.encode_to_vec(MonoPcm(&pcm), &mut out)
            .map_err(|e| anyhow::anyhow!("LAME encode: {e:?}"))?;
    } else {
        // libmp3lame takes planar channels; de-interleave stereo.
        let left: Vec<i16> = pcm.iter().step_by(2).copied().collect();
        let right: Vec<i16> = pcm.iter().skip(1).step_by(2).copied().collect();
        enc.encode_to_vec(DualPcm { left: &left, right: &right }, &mut out)
            .map_err(|e| anyhow::anyhow!("LAME encode: {e:?}"))?;
    }
    // Flush needs its own headroom (LAME emits up to a few KiB at the end).
    out.reserve(mp3lame_encoder::max_required_buffer_size(0));
    enc.flush_to_vec::<FlushNoGap>(&mut out)
        .map_err(|e| anyhow::anyhow!("LAME flush: {e:?}"))?;
    std::fs::write(dst, &out).with_context(|| format!("write {}", dst.display()))?;
    Ok(())
}

/// Copy the source file's primary tag onto the destination (lofty maps it to the
/// destination format's tag type: ID3 for MP3, RIFF/ID3 for WAV).
fn copy_tags(src: &Path, dst: &Path) -> Result<()> {
    use lofty::config::WriteOptions;
    use lofty::file::TaggedFileExt;
    use lofty::tag::TagExt;
    let tagged = lofty::read_from_path(src)?;
    if let Some(tag) = tagged.primary_tag() {
        tag.clone().save_to_path(dst, WriteOptions::default())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a short stereo 44.1 kHz sine as a source WAV on disk.
    fn write_source_wav(path: &Path) {
        let rate = 44_100u32;
        let spec = hound::WavSpec {
            channels: 2,
            sample_rate: rate,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut w = hound::WavWriter::create(path, spec).unwrap();
        for i in 0..rate {
            let v = (std::f32::consts::TAU * 440.0 * i as f32 / rate as f32).sin();
            w.write_sample(f32_to_i16(v)).unwrap();
            w.write_sample(f32_to_i16(v)).unwrap();
        }
        w.finalize().unwrap();
    }

    #[test]
    fn registry_covers_the_encode_arms() {
        // `ALL` drives the UI and dispatch; every advertised format needs an ext.
        for f in OutputFormat::ALL {
            assert!(!f.ext().is_empty());
        }
    }

    #[test]
    fn convert_wav_to_mp3_and_wav_roundtrips() {
        // Encode a known source to MP3 and WAV, then decode each back through the
        // normal decode path to prove the output is valid and plays in LinAmp.
        let dir = std::env::temp_dir();
        let src = dir.join("linamp_encode_src.wav");
        write_source_wav(&src);

        for (format, out) in [
            (OutputFormat::Mp3, dir.join("linamp_encode_out.mp3")),
            (OutputFormat::Wav, dir.join("linamp_encode_out.wav")),
        ] {
            convert_file(&src, &out, format).unwrap_or_else(|e| panic!("{format:?}: {e}"));
            assert!(out.exists() && std::fs::metadata(&out).unwrap().len() > 0);
            let mut back = crate::decode::open_file(&out)
                .unwrap_or_else(|e| panic!("{format:?} output did not decode: {e}"));
            assert_eq!(back.sample_rate(), 44_100, "{format:?} rate");
            assert_eq!(back.channels(), 2, "{format:?} channels");
            assert!((&mut back).take(4096).count() > 0, "{format:?} yields audio");
            let _ = std::fs::remove_file(&out);
        }
        let _ = std::fs::remove_file(&src);
    }
}
