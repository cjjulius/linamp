//! Rip / convert: encode decoded audio to a file (#183).
//!
//! The decode side (`src/decode.rs`) yields interleaved f32 PCM; this module
//! encodes that to a chosen output format and copies the source tags over. One arm
//! per format behind a small registry (`OutputFormat`), so later formats drop in by
//! adding a variant and an encode arm.
//!
//! Formats: MP3 (libmp3lame, lossy), Ogg Vorbis (libvorbis, lossy), WAV and AIFF
//! (both 16-bit PCM, lossless/uncompressed). Each output round-trips back through
//! `decode::open_file`, so everything the converter writes also plays in LinAmp.
//!
//! FLAC was the intended lossless-compressed target but is deferred: the pure-Rust
//! `flacenc` produces output symphonia 0.6 cannot decode (its partial final frame),
//! and the libFLAC bindings need a system library absent on Windows. It is tracked as
//! a follow-up (#190). Opus encoding (Ogg muxing + resampling) is tracked separately.

use std::path::Path;

use anyhow::{bail, Context, Result};
use rodio::Source;

/// An output format the converter can write. `ALL` is the registry the UI lists and
/// the dispatch matches, so adding a format is one variant plus its arm here.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum OutputFormat {
    /// MP3 via libmp3lame (lossy, widely compatible).
    Mp3,
    /// Ogg Vorbis via libvorbis (lossy, open, smaller than MP3 at equal quality).
    Vorbis,
    /// 16-bit PCM WAV (`hound`, lossless, uncompressed).
    Wav,
    /// 16-bit PCM AIFF (lossless, uncompressed; the Apple-world counterpart of WAV).
    Aiff,
}

impl OutputFormat {
    /// Every format the converter offers, in menu order.
    pub const ALL: &'static [OutputFormat] = &[
        OutputFormat::Mp3,
        OutputFormat::Vorbis,
        OutputFormat::Wav,
        OutputFormat::Aiff,
    ];

    /// The file extension (no dot).
    pub fn ext(self) -> &'static str {
        match self {
            OutputFormat::Mp3 => "mp3",
            OutputFormat::Vorbis => "ogg",
            OutputFormat::Wav => "wav",
            OutputFormat::Aiff => "aiff",
        }
    }
}

/// Convert one file: decode `src`, encode it to `dst` in `format`, then copy the
/// source tags over (best-effort). `dst`'s parent directory must already exist.
pub fn convert_file(src: &Path, dst: &Path, format: OutputFormat) -> Result<()> {
    let (samples, rate, channels) = decode_pcm(src)?;
    match format {
        OutputFormat::Mp3 => encode_mp3(dst, &samples, rate, channels)?,
        OutputFormat::Vorbis => encode_vorbis(dst, &samples, rate, channels)?,
        OutputFormat::Wav => encode_wav(dst, &samples, rate, channels)?,
        OutputFormat::Aiff => encode_aiff(dst, &samples, rate, channels)?,
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

/// Encode an 80-bit IEEE 754 extended-precision float (big-endian), the odd type
/// AIFF's COMM chunk stores its sample rate in. Ported faithfully from Apple's
/// canonical `ConvertToIeeeExtended`; the sample rate is always finite and positive,
/// so the infinity/NaN and sign branches never fire in practice but are kept for
/// fidelity. `encode_aiff`'s round-trip test proves the result reads back exactly.
fn sample_rate_to_extended(rate: f64) -> [u8; 10] {
    // frexp: split `x` into mantissa `m` (0.5 <= |m| < 1) and exponent `e`, x = m*2^e.
    fn frexp(x: f64) -> (f64, i32) {
        if x == 0.0 || !x.is_finite() {
            return (x, 0);
        }
        let bits = x.to_bits();
        let raw = ((bits >> 52) & 0x7ff) as i32;
        if raw == 0 {
            // Subnormal: scale up into the normal range, then correct the exponent.
            let (m, e) = frexp(x * 2f64.powi(64));
            return (m, e - 64);
        }
        let e = raw - 1022;
        let mantissa = f64::from_bits((bits & !(0x7ffu64 << 52)) | (1022u64 << 52));
        (mantissa, e)
    }
    let ldexp = |x: f64, n: i32| x * 2f64.powi(n);

    let mut bytes = [0u8; 10];
    if rate == 0.0 {
        return bytes;
    }
    let (sign, num) = if rate < 0.0 { (0x8000u32, -rate) } else { (0u32, rate) };
    let (mut f_mant, mut expon) = frexp(num);
    let (hi_mant, lo_mant);
    if expon > 16384 || f_mant >= 1.0 || f_mant.is_nan() {
        // Infinity or NaN (the original's `!(f_mant < 1.0)`, spelled out).
        expon = (sign | 0x7FFF) as i32;
        hi_mant = 0u32;
        lo_mant = 0u32;
    } else {
        expon += 16382;
        if expon < 0 {
            f_mant = ldexp(f_mant, expon);
            expon = 0;
        }
        expon |= sign as i32;
        f_mant = ldexp(f_mant, 32);
        let f_s = f_mant.floor();
        hi_mant = f_s as u32;
        f_mant = ldexp(f_mant - f_s, 32);
        lo_mant = f_mant.floor() as u32;
    }
    bytes[0] = (expon >> 8) as u8;
    bytes[1] = expon as u8;
    bytes[2..6].copy_from_slice(&hi_mant.to_be_bytes());
    bytes[6..10].copy_from_slice(&lo_mant.to_be_bytes());
    bytes
}

/// 16-bit big-endian PCM AIFF. Hand-written (AIFF is a simple IFF container, like
/// WAV with big-endian samples and an 80-bit float sample rate); symphonia's `aiff`
/// reader decodes the result, which the round-trip test checks.
fn encode_aiff(dst: &Path, samples: &[f32], rate: u32, channels: u16) -> Result<()> {
    if channels == 0 {
        bail!("AIFF export needs at least one channel");
    }
    let frames = (samples.len() / channels as usize) as u32;
    let data_len = samples.len() * 2; // 16-bit, always even
    let mut buf: Vec<u8> = Vec::with_capacity(data_len + 64);

    // COMM chunk body (18 bytes): channels, frame count, bits, 80-bit sample rate.
    let mut comm = Vec::with_capacity(18);
    comm.extend_from_slice(&(channels as i16).to_be_bytes());
    comm.extend_from_slice(&frames.to_be_bytes());
    comm.extend_from_slice(&16i16.to_be_bytes());
    comm.extend_from_slice(&sample_rate_to_extended(rate as f64));

    // SSND chunk body: offset, block size, then the sample data.
    let ssnd_len = 8 + data_len;

    // FORM size covers "AIFF" + the two sub-chunks (each with its 8-byte header).
    let form_size = 4 + (8 + comm.len()) + (8 + ssnd_len);
    buf.extend_from_slice(b"FORM");
    buf.extend_from_slice(&(form_size as u32).to_be_bytes());
    buf.extend_from_slice(b"AIFF");

    buf.extend_from_slice(b"COMM");
    buf.extend_from_slice(&(comm.len() as u32).to_be_bytes());
    buf.extend_from_slice(&comm);

    buf.extend_from_slice(b"SSND");
    buf.extend_from_slice(&(ssnd_len as u32).to_be_bytes());
    buf.extend_from_slice(&0u32.to_be_bytes()); // offset
    buf.extend_from_slice(&0u32.to_be_bytes()); // block size
    for &s in samples {
        buf.extend_from_slice(&f32_to_i16(s).to_be_bytes());
    }
    std::fs::write(dst, &buf).with_context(|| format!("write {}", dst.display()))?;
    Ok(())
}

/// Ogg Vorbis via libvorbis (`vorbis_rs`). The crate muxes the Ogg container itself;
/// it takes planar (per-channel) f32 blocks, so de-interleave first. symphonia's
/// `ogg`+`vorbis` readers decode the result (checked by the round-trip test).
fn encode_vorbis(dst: &Path, samples: &[f32], rate: u32, channels: u16) -> Result<()> {
    use std::num::{NonZeroU32, NonZeroU8};
    use vorbis_rs::VorbisEncoderBuilder;
    if channels == 0 {
        bail!("Vorbis export needs at least one channel");
    }
    let rate = NonZeroU32::new(rate).ok_or_else(|| anyhow::anyhow!("zero sample rate"))?;
    let ch = NonZeroU8::new(channels as u8).ok_or_else(|| anyhow::anyhow!("too many channels"))?;
    let file = std::fs::File::create(dst).with_context(|| format!("create {}", dst.display()))?;
    let mut builder = VorbisEncoderBuilder::new(rate, ch, file)?;
    let mut encoder = builder.build()?;

    // De-interleave into one buffer per channel (what encode_audio_block wants).
    let n = channels as usize;
    let mut planar: Vec<Vec<f32>> = vec![Vec::with_capacity(samples.len() / n); n];
    for frame in samples.chunks_exact(n) {
        for (c, &s) in frame.iter().enumerate() {
            planar[c].push(s);
        }
    }
    encoder.encode_audio_block(&planar)?;
    encoder.finish()?;
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
    fn convert_roundtrips_through_every_format() {
        // Encode a known source to MP3 and WAV, then decode each back through the
        // normal decode path to prove the output is valid and plays in LinAmp.
        let dir = std::env::temp_dir();
        let src = dir.join("linamp_encode_src.wav");
        write_source_wav(&src);

        for (format, out) in [
            (OutputFormat::Mp3, dir.join("linamp_encode_out.mp3")),
            (OutputFormat::Vorbis, dir.join("linamp_encode_out.ogg")),
            (OutputFormat::Wav, dir.join("linamp_encode_out.wav")),
            (OutputFormat::Aiff, dir.join("linamp_encode_out.aiff")),
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
