//! The single local-file decode path: symphonia with an extension hint.
//!
//! This replaces rodio 0.20's `Decoder` for local files (#163). rodio's `Decoder`
//! can't pass a format hint (so it fails on MPEG Layer I/II, #157) and *panics*
//! opening the ISO-MP4 family (an `unreachable!` on a seek, #164). Decoding here
//! with symphonia and a hint handles every format and returns a lazy, seekable
//! rodio `Source`, so one code path covers all local playback.

use std::fs::File;
use std::path::Path;
use std::time::Duration;

use anyhow::{anyhow, bail, Result};
use opus::{Channels as OpusChannels, Decoder as OpusDecoder};
use rodio::source::SeekError;
use rodio::Source;
use symphonia::core::codecs::audio::well_known::CODEC_ID_OPUS;
use symphonia::core::codecs::audio::{AudioCodecParameters, AudioDecoder, AudioDecoderOptions};
use symphonia::core::codecs::CodecParameters;
use symphonia::core::errors::Error as SymError;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, FormatReader, SeekMode, SeekTo, TrackType};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::units::{Time, Timestamp};

/// Largest Opus frame: 120 ms at 48 kHz, per channel. libopus decodes any Opus
/// packet into at most this many samples per channel, so a reused scratch buffer of
/// this size (times the channel count) never overflows.
const OPUS_MAX_FRAME: usize = 5760;

/// The per-packet decoder behind a [`SymphoniaSource`]: symphonia for the formats it
/// supports, or libopus for Opus. symphonia 0.6 demuxes Opus in Ogg/MP4/WebM but
/// ships no Opus decoder, so we pull its demuxed packets and decode them with
/// libopus (#181). The container demux and packet pump are shared either way.
enum PacketDecoder {
    Symphonia(Box<dyn AudioDecoder>),
    /// libopus always decodes to 48 kHz; `channels` is 1 or 2. `scratch` is a reused
    /// decode buffer sized for the largest Opus frame.
    Opus {
        dec: OpusDecoder,
        channels: usize,
        scratch: Vec<f32>,
    },
}

/// A lazy, seekable rodio `Source` backed by a symphonia demuxer and either
/// symphonia's or libopus's decoder: packets are decoded on demand (not the whole
/// file up front), so a long track isn't held in memory fully decoded, and
/// `try_seek` maps to symphonia's seek (#163).
pub struct SymphoniaSource {
    format: Box<dyn FormatReader>,
    decoder: PacketDecoder,
    track_id: u32,
    sample_rate: u32,
    channels: u16,
    total_duration: Option<Duration>,
    /// The current decoded packet's interleaved f32 samples, and our read cursor.
    samples: Vec<f32>,
    pos: usize,
    /// Interleaved samples still to drop from the start of the stream (Opus encoder
    /// delay / pre-skip from the OpusHead); 0 for every other codec.
    preskip: usize,
}

impl SymphoniaSource {
    /// Decode forward until a packet for our track yields audio, replacing
    /// `samples`. Returns false at end of stream / on a fatal error.
    fn fill(&mut self) -> bool {
        loop {
            let packet = match self.format.next_packet() {
                Ok(Some(p)) => p,
                Ok(None) => return false, // end of stream
                Err(_) => return false,
            };
            if packet.track_id != self.track_id {
                continue;
            }
            let filled = match &mut self.decoder {
                PacketDecoder::Symphonia(d) => match d.decode(&packet) {
                    Ok(decoded) => {
                        // Resizes `samples` to the packet's interleaved length and fills it.
                        decoded.copy_to_vec_interleaved(&mut self.samples);
                        true
                    }
                    Err(SymError::DecodeError(_)) => false, // skip a bad frame
                    Err(_) => return false,                 // fatal
                },
                PacketDecoder::Opus { dec, channels, scratch } => {
                    match dec.decode_float(&packet.data, scratch, false) {
                        Ok(frames) => {
                            self.samples.clear();
                            self.samples.extend_from_slice(&scratch[..frames * *channels]);
                            true
                        }
                        Err(_) => false, // skip a bad Opus packet
                    }
                }
            };
            if filled {
                self.drop_preskip();
                self.pos = 0;
                if !self.samples.is_empty() {
                    return true;
                }
            }
        }
    }

    /// Drop any remaining Opus pre-skip from the front of the freshly filled
    /// `samples` (a no-op once consumed, and for non-Opus where `preskip` is 0).
    fn drop_preskip(&mut self) {
        if self.preskip > 0 {
            let drop = self.preskip.min(self.samples.len());
            self.samples.drain(..drop);
            self.preskip -= drop;
        }
    }
}

impl Iterator for SymphoniaSource {
    type Item = f32;

    fn next(&mut self) -> Option<f32> {
        if self.pos >= self.samples.len() && !self.fill() {
            return None;
        }
        let s = self.samples[self.pos];
        self.pos += 1;
        Some(s)
    }
}

impl Source for SymphoniaSource {
    fn current_frame_len(&self) -> Option<usize> {
        // Samples left in the current packet (all at one rate/channel layout);
        // None once drained so rodio re-queries after the next decode.
        let rem = self.samples.len() - self.pos;
        (rem > 0).then_some(rem)
    }
    fn channels(&self) -> u16 {
        self.channels
    }
    fn sample_rate(&self) -> u32 {
        self.sample_rate
    }
    fn total_duration(&self) -> Option<Duration> {
        self.total_duration
    }
    fn try_seek(&mut self, pos: Duration) -> Result<(), SeekError> {
        // symphonia 0.6's Time is (seconds, nanos); build it from the Duration
        // (try_new only fails on nanos >= 1e9, which a Duration never is).
        let time = Time::try_new(pos.as_secs() as i64, pos.subsec_nanos()).unwrap_or(Time::ZERO);
        let to = SeekTo::Time {
            time,
            track_id: Some(self.track_id),
        };
        self.format.seek(SeekMode::Accurate, to).map_err(|e| {
            SeekError::Other(Box::new(std::io::Error::other(e.to_string())))
        })?;
        // Drop decoder state built up before the seek, and the buffered packet.
        match &mut self.decoder {
            PacketDecoder::Symphonia(d) => d.reset(),
            PacketDecoder::Opus { dec, .. } => {
                let _ = dec.reset_state();
            }
        }
        self.samples.clear();
        self.pos = 0;
        Ok(())
    }
}

/// Open a local audio file as a lazy, seekable symphonia source, with an extension
/// hint (which rodio 0.20's `Decoder` can't pass). The single decode path for every
/// local file (#163). The first audio packet is decoded here so the real sample
/// rate / channel count are known before playback, and so an unplayable file fails
/// now with an error rather than later.
pub fn open_file(path: &Path) -> Result<SymphoniaSource> {
    let file = File::open(path)?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());

    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }

    let mut format = symphonia::default::get_probe().probe(
        &hint,
        mss,
        FormatOptions::default(),
        MetadataOptions::default(),
    )?;
    let track = format
        .default_track(TrackType::Audio)
        .ok_or_else(|| anyhow!("file has no audio track"))?
        .clone();
    let track_id = track.id;
    let audio_params = match track.codec_params {
        Some(CodecParameters::Audio(p)) => p,
        _ => bail!("no audio codec parameters in {}", path.display()),
    };
    // Opus: symphonia 0.6 demuxes the container but ships no Opus decoder, so decode
    // its packets with libopus; every other codec goes through symphonia (#181).
    let is_opus = audio_params.codec == CODEC_ID_OPUS;
    let opus_channels = audio_params.channels.as_ref().map(|c| c.count()).unwrap_or(2);
    let mut decoder = if is_opus {
        if opus_channels == 0 || opus_channels > 2 {
            bail!("unsupported Opus channel count ({opus_channels}) in {}", path.display());
        }
        let ch = if opus_channels == 1 { OpusChannels::Mono } else { OpusChannels::Stereo };
        let dec = OpusDecoder::new(48_000, ch)
            .map_err(|e| anyhow!("opus decoder init failed for {}: {e}", path.display()))?;
        PacketDecoder::Opus {
            dec,
            channels: opus_channels,
            scratch: vec![0.0; OPUS_MAX_FRAME * opus_channels],
        }
    } else {
        PacketDecoder::Symphonia(
            symphonia::default::get_codecs()
                .make_audio_decoder(&audio_params, &AudioDecoderOptions::default())?,
        )
    };

    // The container's length, when it reports one (frames x time-base).
    let total_duration = container_duration(track.time_base, track.num_frames);

    // Opus carries encoder delay (pre-skip, at 48 kHz) to drop from the start; count
    // it in interleaved samples. Zero for every other codec.
    let mut preskip = if is_opus { opus_preskip(&audio_params) * opus_channels } else { 0 };

    // Decode the first audio packet to learn the spec (symphonia) and prime the
    // buffer. Opus is always 48 kHz with the channel count from the OpusHead.
    let (sample_rate, channels, mut samples) = loop {
        let packet = match format.next_packet() {
            Ok(Some(p)) => p,
            Ok(None) => bail!("no audio in {}", path.display()),
            Err(e) => return Err(anyhow!("no audio in {}: {e}", path.display())),
        };
        if packet.track_id != track_id {
            continue;
        }
        match &mut decoder {
            PacketDecoder::Symphonia(d) => match d.decode(&packet) {
                Ok(decoded) => {
                    let spec = decoded.spec();
                    let rate = spec.rate();
                    let ch = spec.channels().count() as u16;
                    let mut v: Vec<f32> = Vec::new();
                    decoded.copy_to_vec_interleaved(&mut v);
                    break (rate, ch, v);
                }
                Err(SymError::DecodeError(_)) => continue,
                Err(e) => bail!("decode error in {}: {e}", path.display()),
            },
            PacketDecoder::Opus { dec, channels, scratch } => {
                match dec.decode_float(&packet.data, scratch, false) {
                    Ok(frames) => {
                        break (48_000, *channels as u16, scratch[..frames * *channels].to_vec())
                    }
                    Err(_) => continue, // skip a bad opening packet
                }
            }
        }
    };
    if sample_rate == 0 || channels == 0 {
        bail!("no audio decoded from {}", path.display());
    }
    // Drop pre-skip from the primed samples; any remainder is dropped as fill() runs.
    if preskip > 0 {
        let drop = preskip.min(samples.len());
        samples.drain(..drop);
        preskip -= drop;
    }

    Ok(SymphoniaSource {
        format,
        decoder,
        track_id,
        sample_rate,
        channels,
        total_duration,
        samples,
        pos: 0,
        preskip,
    })
}

/// Opus encoder delay (pre-skip) in samples at 48 kHz, read from the OpusHead in the
/// track's extra data. Returns 0 when it is absent or too short to parse. OpusHead:
/// "OpusHead"(8) + version(1) + channels(1) + pre_skip(2, little-endian) + ...
fn opus_preskip(params: &AudioCodecParameters) -> usize {
    match &params.extra_data {
        Some(d) if d.len() >= 12 && &d[..8] == b"OpusHead" => {
            u16::from_le_bytes([d[10], d[11]]) as usize
        }
        _ => 0,
    }
}

/// The track length from the container alone (frames x time-base), without
/// decoding. A fast fallback for the playlist when lofty can't read a format's
/// length (example: raw `.aac` or `.mpa`, which lofty rejects). Returns `None` when
/// the container reports no frame count.
pub fn probe_duration(path: &Path) -> Option<Duration> {
    let file = File::open(path).ok()?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());
    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }
    let format = symphonia::default::get_probe()
        .probe(
            &hint,
            mss,
            FormatOptions::default(),
            MetadataOptions::default(),
        )
        .ok()?;
    let track = format.default_track(TrackType::Audio)?;
    container_duration(track.time_base, track.num_frames)
}

/// A track length from its container time-base and frame count, if both are known.
/// symphonia 0.6's `calc_time` takes a `Timestamp` and returns an `Option<Time>`,
/// which is (seconds, nanos); convert that to a `std::time::Duration`.
fn container_duration(
    time_base: Option<symphonia::core::units::TimeBase>,
    num_frames: Option<u64>,
) -> Option<Duration> {
    let (tb, n) = (time_base?, num_frames?);
    let t = tb.calc_time(Timestamp::try_from(n).ok()?)?;
    Some(Duration::from_nanos(t.as_nanos().max(0) as u64))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_file_decodes_and_seeks_an_mp2() {
        // MPEG Layer II needs symphonia's "mp2" feature (we also enable mp1); with
        // only "mp3" it failed to decode with "unsupported codec" (#157). Uses the
        // local qa sample when present (qa-music is gitignored, so this skips on CI).
        let p = Path::new("qa-music/Generated_tests/sample_scale.mp2");
        if !p.exists() {
            return;
        }
        let mut src = open_file(p).expect("MP2 (Layer II) should decode");
        assert!(src.sample_rate() >= 8000 && src.channels() >= 1);
        assert!((&mut src).take(4096).count() > 0, "should yield audio");
        // Seeking works (the user's request): jump a second in and keep decoding.
        src.try_seek(Duration::from_secs(1)).expect("MP2 should seek");
        assert!(src.next().is_some(), "should still decode after a seek");
    }

    #[test]
    fn supported_exts_match_the_symphonia_registry() {
        // Guard the SUPPORTED_EXTS list against advertising formats we can't decode.
        // The symphonia-registry decoders we rely on must be registered. Opus and
        // WavPack have no decoder in symphonia 0.6 (their features are non-functional
        // stubs), but Opus IS supported because crate::decode decodes it with libopus
        // instead (#181); WavPack has no viable decoder, so it stays off the list.
        use symphonia::core::codecs::audio::well_known::{
            CODEC_ID_AAC, CODEC_ID_ALAC, CODEC_ID_FLAC, CODEC_ID_MP3, CODEC_ID_OPUS,
            CODEC_ID_VORBIS, CODEC_ID_WAVPACK,
        };
        let codecs = symphonia::default::get_codecs();
        for c in [
            CODEC_ID_MP3, CODEC_ID_AAC, CODEC_ID_VORBIS, CODEC_ID_FLAC, CODEC_ID_ALAC,
        ] {
            assert!(codecs.get_audio_decoder(c).is_some(), "expected a decoder for {c:?}");
        }
        // symphonia itself still has no Opus/WavPack decoder; Opus is handled by libopus.
        for c in [CODEC_ID_OPUS, CODEC_ID_WAVPACK] {
            assert!(codecs.get_audio_decoder(c).is_none(), "unexpected decoder for {c:?}");
        }
        assert!(
            crate::playlist::SUPPORTED_EXTS.contains(&"opus"),
            "opus is decoded via libopus, so it must be advertised as supported"
        );
        assert!(
            !crate::playlist::SUPPORTED_EXTS.contains(&"wv"),
            "wavpack can't be decoded, so it must not be advertised as supported"
        );
    }

    #[test]
    fn open_file_decodes_and_seeks_an_opus() {
        // Opus is demuxed by symphonia and decoded by libopus (#181). Uses the local
        // qa sample when present (qa-music is gitignored, so this skips on CI).
        let p = Path::new("qa-music/format-samples/hypno-this.opus");
        if !p.exists() {
            return;
        }
        let mut src = open_file(p).expect("Opus should decode");
        assert_eq!(src.sample_rate(), 48_000, "libopus decodes to 48 kHz");
        assert!(src.channels() == 1 || src.channels() == 2);
        assert!((&mut src).take(8192).count() > 0, "should yield audio");
        src.try_seek(Duration::from_secs(1)).expect("Opus should seek");
        assert!(src.next().is_some(), "should still decode after a seek");
    }

    #[test]
    fn open_file_errors_instead_of_panicking_on_a_non_audio_file() {
        // A missing file surfaces an error rather than panicking.
        assert!(open_file(Path::new("definitely-not-here.mp2")).is_err());
    }

    #[test]
    fn open_file_decodes_a_wav() {
        // Now that WAV is a symphonia format too, the single path decodes it. Write
        // a tiny mono 16-bit PCM WAV (4 samples at 8 kHz) and decode it back.
        let samples: [i16; 4] = [0, 8000, -8000, 16000];
        let mut wav = Vec::new();
        let data_len = (samples.len() * 2) as u32;
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&(36 + data_len).to_le_bytes());
        wav.extend_from_slice(b"WAVE");
        wav.extend_from_slice(b"fmt ");
        wav.extend_from_slice(&16u32.to_le_bytes()); // fmt chunk size
        wav.extend_from_slice(&1u16.to_le_bytes()); // PCM
        wav.extend_from_slice(&1u16.to_le_bytes()); // mono
        wav.extend_from_slice(&8000u32.to_le_bytes()); // sample rate
        wav.extend_from_slice(&16000u32.to_le_bytes()); // byte rate (8000*1*2)
        wav.extend_from_slice(&2u16.to_le_bytes()); // block align
        wav.extend_from_slice(&16u16.to_le_bytes()); // bits per sample
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&data_len.to_le_bytes());
        for s in samples {
            wav.extend_from_slice(&s.to_le_bytes());
        }
        let p = std::env::temp_dir().join("linamp_decode_test.wav");
        std::fs::write(&p, &wav).unwrap();

        let mut src = open_file(&p).expect("should decode the WAV");
        assert_eq!(src.sample_rate(), 8000);
        assert_eq!(src.channels(), 1);
        assert_eq!((&mut src).count(), samples.len());
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn open_file_decodes_and_seeks_a_real_track() {
        // Uses the committed MP3 fixture to exercise the whole path: open_file works
        // for any symphonia format via the hint, so this covers decode, the Source
        // impl and try_seek without needing an .mp2/.m4a sample (none is on CI).
        let p = Path::new("music/Aria Nocturne - Crystal Harbor.mp3");
        if !p.exists() {
            return; // fixture absent (example: a partial checkout): skip, don't fail
        }
        let mut src = open_file(p).expect("should decode the fixture");
        assert!(src.sample_rate() >= 8000);
        assert!(src.channels() >= 1);
        assert!(src.total_duration().is_some(), "MP3 reports a length");
        // Yields real decoded samples on demand.
        let first: Vec<f32> = (&mut src).take(4096).collect();
        assert_eq!(first.len(), 4096);
        // Seeking resets cleanly and keeps producing audio (no panic).
        src.try_seek(Duration::from_secs(1)).expect("seek should succeed");
        assert!(src.next().is_some(), "should still decode after a seek");
    }

    #[test]
    fn probe_duration_reads_lengths_lofty_cannot() {
        // Raw .aac and .mpa are the cases lofty can't measure; the symphonia probe
        // fills them in. qa-music is local/gitignored, so skip on a bare checkout.
        for (f, secs) in [("qa-music/scale.aac", 59.7), ("qa-music/scale.mpa", 60.0)] {
            let p = Path::new(f);
            if !p.exists() {
                continue;
            }
            let d = probe_duration(p).unwrap_or_else(|| panic!("{f}: no duration"));
            assert!(
                (d.as_secs_f64() - secs).abs() < 1.0,
                "{f}: expected ~{secs}s, got {d:?}"
            );
        }
    }
}
