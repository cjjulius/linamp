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
#[derive(Clone, Copy, PartialEq, Eq, Debug, serde::Serialize, serde::Deserialize)]
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

    /// A human label for the format picker in Preferences.
    pub fn display(self) -> &'static str {
        match self {
            OutputFormat::Mp3 => "MP3",
            OutputFormat::Vorbis => "Ogg Vorbis",
            OutputFormat::Opus => "Opus",
            OutputFormat::Wav => "WAV",
            OutputFormat::Aiff => "AIFF",
        }
    }

    /// True for the lossy formats, where a bitrate setting applies. WAV and AIFF are
    /// uncompressed 16-bit PCM, so bitrate does not apply to them.
    pub fn is_lossy(self) -> bool {
        matches!(self, OutputFormat::Mp3 | OutputFormat::Vorbis | OutputFormat::Opus)
    }

    /// Bitrate presets (kbps) the Bitrate dropdown offers for this format, most
    /// common first in ascending order. Empty for the lossless formats (WAV/AIFF),
    /// where the dropdown is disabled. These are curated common values, not every
    /// value the codec accepts; more can be added later.
    pub fn bitrate_presets(self) -> &'static [u32] {
        match self {
            OutputFormat::Mp3 => &[64, 96, 128, 160, 192, 256, 320],
            OutputFormat::Vorbis => &[64, 96, 128, 160, 192, 256, 320],
            // Opus is efficient at low bitrates and tops out well below MP3's 320.
            OutputFormat::Opus => &[48, 64, 96, 128, 160, 192, 256],
            OutputFormat::Wav | OutputFormat::Aiff => &[],
        }
    }

    /// Sample-rate presets the Sample dropdown offers for this format, in addition to
    /// the "same as source" choice the caller prepends. Opus encodes only at 48 kHz,
    /// so it offers that one rate; the others offer the common PCM rates.
    pub fn sample_rate_presets(self) -> &'static [u32] {
        match self {
            OutputFormat::Opus => &[48_000],
            _ => &[22_050, 32_000, 44_100, 48_000],
        }
    }

    /// Clamp a bitrate to one this format offers (the nearest preset), so a value
    /// carried over from another format stays valid. Lossless formats return 0.
    pub fn clamp_bitrate(self, kbps: u32) -> u32 {
        let presets = self.bitrate_presets();
        presets
            .iter()
            .copied()
            .min_by_key(|&p| p.abs_diff(kbps))
            .unwrap_or(0)
    }
}

/// Default bitrate (kbps) for a new install.
pub const DEFAULT_BITRATE_KBPS: u32 = 192;

/// The global conversion settings, chosen once in Preferences and applied to every
/// queued conversion. Per-track overrides may come later; for now this one setting
/// governs all conversions.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ConvertSettings {
    /// Output format.
    pub format: OutputFormat,
    /// Target bitrate in kbps, used only for the lossy formats.
    pub bitrate_kbps: u32,
    /// Target sample rate, or `None` to keep the source rate. Ignored for Opus.
    pub sample_rate: Option<u32>,
}

impl Default for ConvertSettings {
    fn default() -> Self {
        Self {
            format: OutputFormat::Mp3,
            bitrate_kbps: DEFAULT_BITRATE_KBPS,
            sample_rate: None,
        }
    }
}

/// How a conversion ended: it ran to completion, or it was cancelled partway (the
/// caller removes the partial output).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ConvertOutcome {
    Completed,
    Cancelled,
}

/// Convert one file with the default settings (source rate, 192 kbps), no progress
/// reporting and no cancellation. The simple entry point the round-trip tests use;
/// the queue uses [`convert_file_cb`]. Kept as the minimal public API even though the
/// app now goes through the queue.
#[allow(dead_code)]
pub fn convert_file(src: &Path, dst: &Path, format: OutputFormat) -> Result<()> {
    let settings = ConvertSettings { format, ..Default::default() };
    convert_file_cb(src, dst, settings, &mut |_| {}, &|| false).map(|_| ())
}

/// Fraction of a conversion's progress attributed to the decode phase; the rest to
/// encoding. Decode is a single pass up front, so the bar crawls through this band
/// then fills the remainder while encoding.
const DECODE_WEIGHT: f32 = 0.3;

/// Convert one file: decode `src`, (optionally) resample, encode it to `dst` per
/// `settings`, then copy the source tags over (best-effort). `progress` is called
/// with a fraction in 0.0..=1.0 as the work advances, and `cancel` is polled
/// periodically; when it returns true the conversion stops and the partial output is
/// removed, returning [`ConvertOutcome::Cancelled`]. `dst`'s parent directory must
/// already exist.
pub fn convert_file_cb(
    src: &Path,
    dst: &Path,
    settings: ConvertSettings,
    progress: &mut dyn FnMut(f32),
    cancel: &dyn Fn() -> bool,
) -> Result<ConvertOutcome> {
    let Some((mut samples, mut rate, channels)) =
        decode_pcm(src, &mut |f| progress(f * DECODE_WEIGHT), cancel)?
    else {
        return Ok(ConvertOutcome::Cancelled);
    };
    // Resample to the requested rate (Opus resamples to 48 kHz itself, so skip it
    // there and let encode_opus handle the rate).
    if settings.format != OutputFormat::Opus {
        if let Some(target) = settings.sample_rate {
            if target != rate && channels > 0 {
                samples = resample_interleaved(&samples, channels, rate, target)?;
                rate = target;
            }
        }
    }
    let br = settings.bitrate_kbps;
    let enc = &mut |f: f32| progress(DECODE_WEIGHT + f * (1.0 - DECODE_WEIGHT));
    let completed = match settings.format {
        OutputFormat::Mp3 => encode_mp3(dst, &samples, rate, channels, br, enc, cancel)?,
        OutputFormat::Vorbis => encode_vorbis(dst, &samples, rate, channels, br, enc, cancel)?,
        OutputFormat::Opus => encode_opus(dst, &samples, rate, channels, br, enc, cancel)?,
        OutputFormat::Wav => encode_wav(dst, &samples, rate, channels, enc, cancel)?,
        OutputFormat::Aiff => encode_aiff(dst, &samples, rate, channels, enc, cancel)?,
    };
    if !completed {
        let _ = std::fs::remove_file(dst);
        return Ok(ConvertOutcome::Cancelled);
    }
    // Carry the metadata across; a tag failure shouldn't fail the whole convert.
    if let Err(e) = copy_tags(src, dst) {
        eprintln!("convert: tags not copied to {}: {e}", dst.display());
    }
    progress(1.0);
    Ok(ConvertOutcome::Completed)
}

/// Decode `src` fully to interleaved f32 PCM, returning `(samples, rate, channels)`,
/// or `None` if `cancel` fired during the decode. `progress` reports a fraction in
/// 0.0..=1.0 of the decode when the source's duration is known.
fn decode_pcm(
    src: &Path,
    progress: &mut dyn FnMut(f32),
    cancel: &dyn Fn() -> bool,
) -> Result<Option<(Vec<f32>, u32, u16)>> {
    let mut source = crate::decode::open_file(src)?;
    let rate = source.sample_rate();
    let channels = source.channels();
    // Estimate the total sample count from the reported duration, so decode progress
    // can advance; many streams report no duration, in which case progress stays at 0
    // through the decode and the bar simply starts moving when encoding begins.
    let total = source
        .total_duration()
        .map(|d| (d.as_secs_f64() * rate as f64 * channels.max(1) as f64) as usize)
        .filter(|&t| t > 0);
    let mut samples: Vec<f32> = Vec::new();
    // Pull in chunks so cancellation and progress are responsive without the overhead
    // of a per-sample check.
    const STEP: usize = 1 << 16;
    loop {
        let before = samples.len();
        samples.extend((&mut source).take(STEP));
        if samples.len() == before {
            break; // source exhausted
        }
        if cancel() {
            return Ok(None);
        }
        if let Some(total) = total {
            progress((samples.len() as f32 / total as f32).min(1.0));
        }
    }
    if samples.is_empty() {
        bail!("no audio decoded from {}", src.display());
    }
    Ok(Some((samples, rate, channels)))
}

/// Resample interleaved f32 PCM from `from` to `to` Hz, preserving channel count.
/// De-interleaves, resamples each channel with the shared sinc resampler, then
/// re-interleaves.
fn resample_interleaved(samples: &[f32], channels: u16, from: u32, to: u32) -> Result<Vec<f32>> {
    let n = channels as usize;
    if n == 0 {
        return Ok(samples.to_vec());
    }
    let mut planar: Vec<Vec<f32>> = vec![Vec::with_capacity(samples.len() / n); n];
    for frame in samples.chunks_exact(n) {
        for (c, &s) in frame.iter().enumerate() {
            planar[c].push(s);
        }
    }
    let planar = resample_planar(&planar, from, to)?;
    let frames = planar.first().map(|c| c.len()).unwrap_or(0);
    let mut out = Vec::with_capacity(frames * n);
    for i in 0..frames {
        for ch in &planar {
            out.push(ch.get(i).copied().unwrap_or(0.0));
        }
    }
    Ok(out)
}

/// Interleaved f32 (-1.0..=1.0) to 16-bit PCM, clamped.
fn f32_to_i16(s: f32) -> i16 {
    (s.clamp(-1.0, 1.0) * i16::MAX as f32).round() as i16
}

/// How often (in samples) the encode arms poll `cancel` and report `progress`. Large
/// enough that the per-sample hot loop stays cheap, small enough that a cancel takes
/// effect promptly.
const ENCODE_POLL: usize = 1 << 13;

fn encode_wav(
    dst: &Path,
    samples: &[f32],
    rate: u32,
    channels: u16,
    progress: &mut dyn FnMut(f32),
    cancel: &dyn Fn() -> bool,
) -> Result<bool> {
    let spec = hound::WavSpec {
        channels,
        sample_rate: rate,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut w = hound::WavWriter::create(dst, spec)
        .with_context(|| format!("create {}", dst.display()))?;
    let total = samples.len().max(1);
    for (i, &s) in samples.iter().enumerate() {
        if i % ENCODE_POLL == 0 {
            if cancel() {
                drop(w); // leave the partial file for the caller to remove
                return Ok(false);
            }
            progress(i as f32 / total as f32);
        }
        w.write_sample(f32_to_i16(s))?;
    }
    w.finalize()?;
    Ok(true)
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
fn encode_aiff(
    dst: &Path,
    samples: &[f32],
    rate: u32,
    channels: u16,
    progress: &mut dyn FnMut(f32),
    cancel: &dyn Fn() -> bool,
) -> Result<bool> {
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
    let total = samples.len().max(1);
    for (i, &s) in samples.iter().enumerate() {
        if i % ENCODE_POLL == 0 {
            if cancel() {
                return Ok(false); // nothing written to disk yet
            }
            progress(i as f32 / total as f32);
        }
        buf.extend_from_slice(&f32_to_i16(s).to_be_bytes());
    }
    std::fs::write(dst, &buf).with_context(|| format!("write {}", dst.display()))?;
    Ok(true)
}

/// Ogg Vorbis via libvorbis (`vorbis_rs`). The crate muxes the Ogg container itself;
/// it takes planar (per-channel) f32 blocks, so de-interleave first. symphonia's
/// `ogg`+`vorbis` readers decode the result (checked by the round-trip test).
fn encode_vorbis(
    dst: &Path,
    samples: &[f32],
    rate: u32,
    channels: u16,
    bitrate_kbps: u32,
    progress: &mut dyn FnMut(f32),
    cancel: &dyn Fn() -> bool,
) -> Result<bool> {
    use std::num::{NonZeroU32, NonZeroU8};
    use vorbis_rs::{VorbisBitrateManagementStrategy, VorbisEncoderBuilder};
    if channels == 0 {
        bail!("Vorbis export needs at least one channel");
    }
    let rate = NonZeroU32::new(rate).ok_or_else(|| anyhow::anyhow!("zero sample rate"))?;
    let ch = NonZeroU8::new(channels as u8).ok_or_else(|| anyhow::anyhow!("too many channels"))?;
    let file = std::fs::File::create(dst).with_context(|| format!("create {}", dst.display()))?;
    let mut builder = VorbisEncoderBuilder::new(rate, ch, file)?;
    // Pure VBR selected by a target bitrate (libvorbis takes bit/s).
    if let Some(target) = NonZeroU32::new(bitrate_kbps.saturating_mul(1000)) {
        builder.bitrate_management_strategy(VorbisBitrateManagementStrategy::Vbr {
            target_bitrate: target,
        });
    }
    let mut encoder = builder.build()?;

    // Feed libvorbis in modest per-channel blocks (it is built to be driven
    // incrementally; the docs call ~1024 samples a reasonable block size), rather
    // than handing it the whole track at once. De-interleave each block into one
    // buffer per channel, reusing the buffers across blocks to bound memory.
    const BLOCK_FRAMES: usize = 4096;
    let n = channels as usize;
    let total_frames = (samples.len() / n).max(1);
    let mut done_frames = 0usize;
    let mut planar: Vec<Vec<f32>> = vec![Vec::with_capacity(BLOCK_FRAMES); n];
    for block in samples.chunks(BLOCK_FRAMES * n) {
        if cancel() {
            drop(encoder); // leave the partial file for the caller to remove
            return Ok(false);
        }
        for p in planar.iter_mut() {
            p.clear();
        }
        for frame in block.chunks_exact(n) {
            for (c, &s) in frame.iter().enumerate() {
                planar[c].push(s);
            }
        }
        encoder.encode_audio_block(&planar)?;
        done_frames += block.len() / n;
        progress(done_frames as f32 / total_frames as f32);
    }
    encoder.finish()?;
    Ok(true)
}

/// Resample planar f32 (one buffer per channel) from `from_rate` to `to_rate` with a
/// sinc resampler, returning the planar result. Used both to bring a track to Opus's
/// 48 kHz and to honour the Preferences sample-rate setting for the other formats. The
/// resampler's leading delay is dropped and the result trimmed to the ideal output
/// length so it stays aligned and correctly sized. A no-op when the rates match.
fn resample_planar(planar: &[Vec<f32>], from_rate: u32, to_rate: u32) -> Result<Vec<Vec<f32>>> {
    use rubato::{
        Resampler, SincFixedIn, SincInterpolationParameters, SincInterpolationType, WindowFunction,
    };
    if from_rate == to_rate || from_rate == 0 {
        return Ok(planar.to_vec());
    }
    let channels = planar.len();
    let params = SincInterpolationParameters {
        sinc_len: 256,
        f_cutoff: 0.95,
        interpolation: SincInterpolationType::Linear,
        oversampling_factor: 256,
        window: WindowFunction::BlackmanHarris2,
    };
    const CHUNK: usize = 4096;
    let ratio = to_rate as f64 / from_rate as f64;
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
fn encode_opus(
    dst: &Path,
    samples: &[f32],
    rate: u32,
    channels: u16,
    bitrate_kbps: u32,
    progress: &mut dyn FnMut(f32),
    cancel: &dyn Fn() -> bool,
) -> Result<bool> {
    use ogg::{PacketWriteEndInfo, PacketWriter};
    use opus::{Application, Bitrate, Channels, Encoder};
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
    let planar = resample_planar(&planar, rate, 48_000)?;

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
    // Target bitrate (libopus takes bit/s); left at the encoder default if zero.
    if bitrate_kbps > 0 {
        enc.set_bitrate(Bitrate::Bits(bitrate_kbps.saturating_mul(1000) as i32))
            .map_err(|e| anyhow::anyhow!("opus bitrate: {e}"))?;
    }
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
        if fi % 64 == 0 {
            if cancel() {
                drop(pw); // leave the partial file for the caller to remove
                return Ok(false);
            }
            progress(fi as f32 / frame_count as f32);
        }
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
    Ok(true)
}

/// Map a kbps value to the nearest LAME CBR preset. The Preferences dropdown already
/// restricts to this format's presets, but snap anyway so a stray value stays valid.
fn mp3_bitrate(kbps: u32) -> mp3lame_encoder::Bitrate {
    use mp3lame_encoder::Bitrate::*;
    let supported = [8u32, 16, 24, 32, 40, 48, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320];
    let nearest = supported
        .iter()
        .copied()
        .min_by_key(|&b| b.abs_diff(kbps))
        .unwrap_or(192);
    match nearest {
        8 => Kbps8,
        16 => Kbps16,
        24 => Kbps24,
        32 => Kbps32,
        40 => Kbps40,
        48 => Kbps48,
        64 => Kbps64,
        80 => Kbps80,
        96 => Kbps96,
        112 => Kbps112,
        128 => Kbps128,
        160 => Kbps160,
        192 => Kbps192,
        224 => Kbps224,
        256 => Kbps256,
        _ => Kbps320,
    }
}

fn encode_mp3(
    dst: &Path,
    samples: &[f32],
    rate: u32,
    channels: u16,
    bitrate_kbps: u32,
    progress: &mut dyn FnMut(f32),
    cancel: &dyn Fn() -> bool,
) -> Result<bool> {
    use mp3lame_encoder::{Builder, DualPcm, FlushNoGap, MonoPcm, Quality};
    if channels == 0 || channels > 2 {
        bail!("MP3 export supports mono or stereo, not {channels} channels");
    }
    let mut builder = Builder::new().ok_or_else(|| anyhow::anyhow!("LAME init failed"))?;
    let m = |r: std::result::Result<(), mp3lame_encoder::BuildError>, what: &str| {
        r.map_err(|e| anyhow::anyhow!("LAME {what}: {e:?}"))
    };
    m(builder.set_num_channels(channels as u8), "channels")?;
    m(builder.set_sample_rate(rate), "sample rate")?;
    m(builder.set_brate(mp3_bitrate(bitrate_kbps)), "bitrate")?;
    m(builder.set_quality(Quality::Good), "quality")?;
    let mut enc = builder
        .build()
        .map_err(|e| anyhow::anyhow!("LAME build: {e:?}"))?;

    let pcm: Vec<i16> = samples.iter().map(|&s| f32_to_i16(s)).collect();
    let mut out: Vec<u8> = Vec::new();
    // Feed LAME in blocks (per-channel frames) so the convert reports progress and can
    // be cancelled; encode_to_vec / flush_to_vec write into the Vec's spare capacity
    // without reserving it, so reserve the LAME-recommended worst case before each call
    // or libmp3lame writes out of bounds. On cancel nothing has been written to disk
    // yet (the file is created only by the final write), so just return.
    const BLK: usize = 1 << 16; // per-channel frames per block
    if channels == 1 {
        let total = pcm.len().max(1);
        let mut i = 0;
        while i < pcm.len() {
            if cancel() {
                return Ok(false);
            }
            let end = (i + BLK).min(pcm.len());
            out.reserve(mp3lame_encoder::max_required_buffer_size(end - i));
            enc.encode_to_vec(MonoPcm(&pcm[i..end]), &mut out)
                .map_err(|e| anyhow::anyhow!("LAME encode: {e:?}"))?;
            i = end;
            progress(i as f32 / total as f32);
        }
    } else {
        // libmp3lame takes planar channels; de-interleave stereo.
        let left: Vec<i16> = pcm.iter().step_by(2).copied().collect();
        let right: Vec<i16> = pcm.iter().skip(1).step_by(2).copied().collect();
        let total = left.len().max(1);
        let mut i = 0;
        while i < left.len() {
            if cancel() {
                return Ok(false);
            }
            let end = (i + BLK).min(left.len());
            out.reserve(mp3lame_encoder::max_required_buffer_size(end - i));
            enc.encode_to_vec(DualPcm { left: &left[i..end], right: &right[i..end] }, &mut out)
                .map_err(|e| anyhow::anyhow!("LAME encode: {e:?}"))?;
            i = end;
            progress(i as f32 / total as f32);
        }
    }
    // Flush needs its own headroom (LAME emits up to a few KiB at the end).
    out.reserve(mp3lame_encoder::max_required_buffer_size(0));
    enc.flush_to_vec::<FlushNoGap>(&mut out)
        .map_err(|e| anyhow::anyhow!("LAME flush: {e:?}"))?;
    std::fs::write(dst, &out).with_context(|| format!("write {}", dst.display()))?;
    Ok(true)
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

    #[test]
    fn convert_cb_reports_monotonic_progress_to_one() {
        // The queue relies on convert_file_cb reporting progress that never goes
        // backwards and reaches 1.0 on success.
        let dir = std::env::temp_dir();
        let src = dir.join("linamp_cb_src.wav");
        write_source_wav(&src);
        let out = dir.join("linamp_cb_out.wav");
        let (mut last, mut monotonic, mut max) = (-1.0f32, true, 0.0f32);
        let settings = ConvertSettings { format: OutputFormat::Wav, ..Default::default() };
        let outcome = convert_file_cb(
            &src,
            &out,
            settings,
            &mut |f| {
                if f + 1e-6 < last {
                    monotonic = false;
                }
                last = f;
                max = max.max(f);
            },
            &|| false,
        )
        .unwrap();
        assert_eq!(outcome, ConvertOutcome::Completed);
        assert!(monotonic, "progress went backwards");
        assert!((max - 1.0).abs() < 1e-6, "progress reached {max}, expected 1.0");
        assert!(out.exists());
        let _ = std::fs::remove_file(&src);
        let _ = std::fs::remove_file(&out);
    }

    #[test]
    fn convert_cb_cancel_leaves_no_output() {
        // Cancelling aborts the convert and removes any partial output (the queue's
        // right-click cancel cleans up leftover files).
        let dir = std::env::temp_dir();
        let src = dir.join("linamp_cancel_src.wav");
        write_source_wav(&src);
        let out = dir.join("linamp_cancel_out.wav");
        let _ = std::fs::remove_file(&out);
        let settings = ConvertSettings { format: OutputFormat::Wav, ..Default::default() };
        let outcome =
            convert_file_cb(&src, &out, settings, &mut |_| {}, &|| true).unwrap();
        assert_eq!(outcome, ConvertOutcome::Cancelled);
        assert!(!out.exists(), "cancel left a partial file behind");
        let _ = std::fs::remove_file(&src);
    }

    #[test]
    fn convert_honours_target_sample_rate() {
        // A non-Opus convert with a target sample rate resamples to it; the output
        // decodes back at that rate.
        let dir = std::env::temp_dir();
        let src = dir.join("linamp_rate_src.wav"); // written at 44_100
        write_source_wav(&src);
        let out = dir.join("linamp_rate_out.wav");
        let settings = ConvertSettings {
            format: OutputFormat::Wav,
            bitrate_kbps: 192,
            sample_rate: Some(22_050),
        };
        convert_file_cb(&src, &out, settings, &mut |_| {}, &|| false).unwrap();
        let back = crate::decode::open_file(&out).unwrap();
        assert_eq!(back.sample_rate(), 22_050, "target sample rate not applied");
        let _ = std::fs::remove_file(&src);
        let _ = std::fs::remove_file(&out);
    }

    #[test]
    fn convert_at_a_chosen_bitrate_still_decodes() {
        // A lossy convert at a non-default bitrate produces a valid file. Guards the
        // bitrate plumbing for each lossy format.
        let dir = std::env::temp_dir();
        let src = dir.join("linamp_br_src.wav");
        write_source_wav(&src);
        for (format, kbps, ext) in [
            (OutputFormat::Mp3, 128, "mp3"),
            (OutputFormat::Vorbis, 96, "ogg"),
            (OutputFormat::Opus, 64, "opus"),
        ] {
            let out = dir.join(format!("linamp_br_out.{ext}"));
            let settings = ConvertSettings { format, bitrate_kbps: kbps, sample_rate: None };
            convert_file_cb(&src, &out, settings, &mut |_| {}, &|| false)
                .unwrap_or_else(|e| panic!("{format:?}@{kbps}: {e}"));
            let mut back = crate::decode::open_file(&out)
                .unwrap_or_else(|e| panic!("{format:?}@{kbps} did not decode: {e}"));
            assert!((&mut back).take(2048).count() > 0, "{format:?}@{kbps} yields audio");
            let _ = std::fs::remove_file(&out);
        }
        let _ = std::fs::remove_file(&src);
    }

    #[test]
    fn bitrate_presets_and_clamp_are_consistent() {
        // Lossy formats offer presets; lossless offer none. clamp snaps to the nearest
        // preset so a value carried over from another format stays valid.
        assert!(OutputFormat::Wav.bitrate_presets().is_empty());
        assert!(OutputFormat::Aiff.bitrate_presets().is_empty());
        for f in [OutputFormat::Mp3, OutputFormat::Vorbis, OutputFormat::Opus] {
            assert!(!f.bitrate_presets().is_empty(), "{f:?} has presets");
            // Clamp returns a value that is actually offered.
            let c = f.clamp_bitrate(1_000_000);
            assert!(f.bitrate_presets().contains(&c), "{f:?} clamp off-list");
            assert_eq!(c, *f.bitrate_presets().last().unwrap(), "clamp high -> max");
            assert_eq!(f.clamp_bitrate(0), f.bitrate_presets()[0], "clamp low -> min");
        }
    }

    #[test]
    fn output_format_serde_roundtrips() {
        // Prefs persist OutputFormat; it must survive a JSON round-trip.
        for &f in OutputFormat::ALL {
            let s = serde_json::to_string(&f).unwrap();
            let back: OutputFormat = serde_json::from_str(&s).unwrap();
            assert_eq!(back, f);
        }
    }
}
