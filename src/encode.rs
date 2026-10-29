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
    /// Opus in Ogg via libopus (lossy, open, best quality at low bitrates). Always
    /// encoded at 48 kHz (Opus's native rate), resampling other rates with rubato.
    Opus,
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
        OutputFormat::Opus,
        OutputFormat::Wav,
        OutputFormat::Aiff,
    ];

    /// The file extension (no dot).
    pub fn ext(self) -> &'static str {
        match self {
            OutputFormat::Mp3 => "mp3",
            OutputFormat::Vorbis => "ogg",
            OutputFormat::Opus => "opus",
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
        OutputFormat::Opus => encode_opus(dst, &samples, rate, channels)?,
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

    // Feed libvorbis in modest per-channel blocks (it is built to be driven
    // incrementally; the docs call ~1024 samples a reasonable block size), rather
    // than handing it the whole track at once. De-interleave each block into one
    // buffer per channel, reusing the buffers across blocks to bound memory.
    const BLOCK_FRAMES: usize = 4096;
    let n = channels as usize;
    let mut planar: Vec<Vec<f32>> = vec![Vec::with_capacity(BLOCK_FRAMES); n];
    for block in samples.chunks(BLOCK_FRAMES * n) {
        for p in planar.iter_mut() {
            p.clear();
        }
        for frame in block.chunks_exact(n) {
            for (c, &s) in frame.iter().enumerate() {
                planar[c].push(s);
            }
        }
        encoder.encode_audio_block(&planar)?;
    }
    encoder.finish()?;
    Ok(())
}

/// Resample planar f32 (one buffer per channel) from `from_rate` to 48 kHz with a sinc
/// resampler, returning planar 48 kHz. Opus only accepts 8/12/16/24/48 kHz, so other
/// rates are brought to 48 kHz first. The resampler's leading delay is dropped and the
/// result trimmed to the ideal output length so it stays aligned and correctly sized.
fn resample_to_48k(planar: &[Vec<f32>], from_rate: u32) -> Result<Vec<Vec<f32>>> {
    use rubato::{
        Resampler, SincFixedIn, SincInterpolationParameters, SincInterpolationType, WindowFunction,
    };
    let channels = planar.len();
    let params = SincInterpolationParameters {
        sinc_len: 256,
        f_cutoff: 0.95,
        interpolation: SincInterpolationType::Linear,
        oversampling_factor: 256,
        window: WindowFunction::BlackmanHarris2,
    };
    const CHUNK: usize = 4096;
    let ratio = 48_000.0 / from_rate as f64;
    let mut rs = SincFixedIn::<f32>::new(ratio, 2.0, params, CHUNK, channels)
        .map_err(|e| anyhow::anyhow!("resampler init: {e}"))?;
    let delay = rs.output_delay();
    let total = planar.first().map(|c| c.len()).unwrap_or(0);
    let expected = (total as f64 * ratio).round() as usize;
    let need = delay + expected;
    let mut out: Vec<Vec<f32>> = vec![Vec::new(); channels];
    // Feed fixed CHUNK frames per call (what SincFixedIn requires), padding past the end
    // with zeros, until enough output is produced to cover the delay plus the real length.
    let mut pos = 0usize;
    let cap = total + 10 * CHUNK; // guard against a non-terminating feed
    while out.first().map(|c| c.len()).unwrap_or(0) < need && pos < cap {
        let chunk: Vec<Vec<f32>> = planar
            .iter()
            .map(|ch| {
                let mut c = vec![0.0f32; CHUNK];
                for (i, slot) in c.iter_mut().enumerate() {
                    if let Some(&s) = ch.get(pos + i) {
                        *slot = s;
                    }
                }
                c
            })
            .collect();
        let got = rs
            .process(&chunk, None)
            .map_err(|e| anyhow::anyhow!("resample: {e}"))?;
        for (ci, g) in got.into_iter().enumerate() {
            out[ci].extend(g);
        }
        pos += CHUNK;
    }
    for ch in &mut out {
        if delay < ch.len() {
            ch.drain(0..delay);
        }
        ch.truncate(expected);
    }
    Ok(out)
}

/// Opus in Ogg via libopus. Opus runs at 48 kHz, so resample other rates first, then
/// encode 20 ms frames and mux them into an Ogg stream with the required OpusHead (its
/// pre-skip is the encoder lookahead our decoder drops) and OpusTags pages. symphonia's
/// ogg reader plus our libopus decode path read the result (checked by the round-trip test).
fn encode_opus(dst: &Path, samples: &[f32], rate: u32, channels: u16) -> Result<()> {
    use ogg::{PacketWriteEndInfo, PacketWriter};
    use opus::{Application, Channels, Encoder};
    if channels == 0 || channels > 2 {
        bail!("Opus export supports mono or stereo, not {channels} channels");
    }
    let n = channels as usize;

    // De-interleave, then bring to 48 kHz (Opus's only encode rate) if needed.
    let mut planar: Vec<Vec<f32>> = vec![Vec::with_capacity(samples.len() / n); n];
    for frame in samples.chunks_exact(n) {
        for (c, &s) in frame.iter().enumerate() {
            planar[c].push(s);
        }
    }
    let planar = if rate == 48_000 {
        planar
    } else {
        resample_to_48k(&planar, rate)?
    };

    // Re-interleave at 48 kHz for the encoder (encode_float takes interleaved input).
    let frames = planar.first().map(|c| c.len()).unwrap_or(0);
    let mut inter: Vec<f32> = Vec::with_capacity(frames * n);
    for i in 0..frames {
        for ch in &planar {
            inter.push(ch.get(i).copied().unwrap_or(0.0));
        }
    }

    let ch_enum = if n == 1 { Channels::Mono } else { Channels::Stereo };
    let mut enc = Encoder::new(48_000, ch_enum, Application::Audio)
        .map_err(|e| anyhow::anyhow!("opus init: {e}"))?;
    // Pre-skip = the encoder's lookahead (at 48 kHz); the decoder drops this many leading
    // samples. ~312 is the usual value if the query ever fails.
    let pre_skip = enc.get_lookahead().unwrap_or(312).max(0) as u64;

    let file = std::fs::File::create(dst).with_context(|| format!("create {}", dst.display()))?;
    let mut pw = PacketWriter::new(file);
    let serial: u32 = 0x4c69_6e41; // any fixed serial is fine for a single stream.

    // OpusHead: magic, version, channels, pre-skip (u16 LE), input rate (u32 LE),
    // output gain (i16 LE), channel-mapping family (0 for mono/stereo). Its own page.
    let mut head = Vec::with_capacity(19);
    head.extend_from_slice(b"OpusHead");
    head.push(1);
    head.push(n as u8);
    head.extend_from_slice(&(pre_skip as u16).to_le_bytes());
    head.extend_from_slice(&rate.to_le_bytes());
    head.extend_from_slice(&0i16.to_le_bytes());
    head.push(0);
    pw.write_packet(head, serial, PacketWriteEndInfo::EndPage, 0)
        .context("write OpusHead")?;

    // OpusTags: magic, vendor string, comment count (0). Tags are copied afterwards by
    // copy_tags (lofty maps them to the Vorbis comments Opus uses). Its own page.
    let mut tags = Vec::new();
    tags.extend_from_slice(b"OpusTags");
    let vendor = b"LinAmp";
    tags.extend_from_slice(&(vendor.len() as u32).to_le_bytes());
    tags.extend_from_slice(vendor);
    tags.extend_from_slice(&0u32.to_le_bytes());
    pw.write_packet(tags, serial, PacketWriteEndInfo::EndPage, 0)
        .context("write OpusTags")?;

    // Audio: 20 ms frames (960 samples/channel at 48 kHz). Granule accumulates the 48 kHz
    // sample count; the decoder plays granule - pre_skip samples. Pad the final frame.
    const FRAME: usize = 960;
    let per_frame = FRAME * n;
    let mut packet = vec![0u8; 4000];
    let mut granule: u64 = 0;
    let frame_count = inter.len().div_ceil(per_frame).max(1);
    for fi in 0..frame_count {
        let start = fi * per_frame;
        let mut pcm = vec![0.0f32; per_frame];
        if start < inter.len() {
            let end = (start + per_frame).min(inter.len());
            pcm[..end - start].copy_from_slice(&inter[start..end]);
        }
        let len = enc
            .encode_float(&pcm, &mut packet)
            .map_err(|e| anyhow::anyhow!("opus encode: {e}"))?;
        granule += FRAME as u64;
        let info = if fi + 1 == frame_count {
            PacketWriteEndInfo::EndStream
        } else {
            PacketWriteEndInfo::NormalPacket
        };
        pw.write_packet(packet[..len].to_vec(), serial, info, granule)
            .context("write opus packet")?;
    }
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

/// Copy the source file's primary tag onto the destination, re-mapped to the tag
/// type the destination format actually supports. Writing the source's own tag
/// type blindly fails for formats that reject it: an MP3's ID3v2 tag cannot go
/// into an Ogg (Vorbis comments only), so without the re-map an OGG convert loses
/// every tag (lofty errors with "format does not support it" and the caller
/// swallows it). lofty's `ItemKey` is format-neutral, so copying the items into a
/// fresh tag of the destination's native type maps them to that format's own
/// frames/comments. AIFF/WAV/MP3 keep working since their native type is ID3.
fn copy_tags(src: &Path, dst: &Path) -> Result<()> {
    use lofty::config::WriteOptions;
    use lofty::file::TaggedFileExt;
    use lofty::tag::{Tag, TagExt};
    let src_tagged = lofty::read_from_path(src)?;
    let Some(src_tag) = src_tagged.primary_tag() else {
        return Ok(());
    };
    let dst_type = lofty::read_from_path(dst)?.primary_tag_type();
    let mut out = Tag::new(dst_type);
    for item in src_tag.items() {
        out.insert(item.clone());
    }
    for pic in src_tag.pictures() {
        out.push_picture(pic.clone());
    }
    out.save_to_path(dst, WriteOptions::default())?;
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
            (OutputFormat::Opus, dir.join("linamp_encode_out.opus")),
            (OutputFormat::Wav, dir.join("linamp_encode_out.wav")),
            (OutputFormat::Aiff, dir.join("linamp_encode_out.aiff")),
        ] {
            convert_file(&src, &out, format).unwrap_or_else(|e| panic!("{format:?}: {e}"));
            assert!(out.exists() && std::fs::metadata(&out).unwrap().len() > 0);
            let mut back = crate::decode::open_file(&out)
                .unwrap_or_else(|e| panic!("{format:?} output did not decode: {e}"));
            // Opus only runs at 48 kHz, so its output is resampled from the 44.1 kHz
            // source; every other format keeps the source rate.
            let expect_rate = if format == OutputFormat::Opus { 48_000 } else { 44_100 };
            assert_eq!(back.sample_rate(), expect_rate, "{format:?} rate");
            assert_eq!(back.channels(), 2, "{format:?} channels");
            assert!((&mut back).take(4096).count() > 0, "{format:?} yields audio");
            let _ = std::fs::remove_file(&out);
        }
        let _ = std::fs::remove_file(&src);
    }

    #[test]
    fn convert_carries_tags_across_formats() {
        // A source tagged with ID3 (as an MP3 or WAV is) must keep its tags when
        // converted to a format whose native tag type differs. Regression for OGG
        // converts silently dropping every tag because copy_tags wrote the source's
        // ID3v2 straight onto the Ogg, which holds only Vorbis comments.
        use lofty::config::WriteOptions;
        use lofty::file::TaggedFileExt;
        use lofty::tag::{Accessor, Tag, TagExt, TagType};
        let dir = std::env::temp_dir();
        let src = dir.join("linamp_tagcopy_src.wav");
        write_source_wav(&src);
        {
            let mut t = Tag::new(TagType::Id3v2);
            t.set_title("Aria Nocturne".to_string());
            t.set_artist("Gundream".to_string());
            t.set_album("Crystal Harbor".to_string());
            t.save_to_path(&src, WriteOptions::default()).unwrap();
        }
        for (format, out) in [
            (OutputFormat::Vorbis, dir.join("linamp_tagcopy_out.ogg")),
            (OutputFormat::Aiff, dir.join("linamp_tagcopy_out.aiff")),
        ] {
            convert_file(&src, &out, format).unwrap_or_else(|e| panic!("{format:?}: {e}"));
            let tagged = lofty::read_from_path(&out).unwrap();
            let tag = tagged
                .primary_tag()
                .unwrap_or_else(|| panic!("{format:?}: no tag written"));
            assert_eq!(tag.title().as_deref(), Some("Aria Nocturne"), "{format:?} title");
            assert_eq!(tag.artist().as_deref(), Some("Gundream"), "{format:?} artist");
            assert_eq!(tag.album().as_deref(), Some("Crystal Harbor"), "{format:?} album");
            let _ = std::fs::remove_file(&out);
        }
        let _ = std::fs::remove_file(&src);
    }
}
