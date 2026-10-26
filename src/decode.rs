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
use rodio::source::SeekError;
use rodio::Source;
use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::{Decoder, DecoderOptions};
use symphonia::core::errors::Error as SymError;
use symphonia::core::formats::{FormatOptions, FormatReader, SeekMode, SeekTo};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;
use symphonia::core::units::Time;

/// A lazy, seekable rodio `Source` backed by a symphonia decoder: packets are
/// decoded on demand (not the whole file up front), so a long track isn't held in
/// memory fully decoded, and `try_seek` maps to symphonia's seek (#163).
pub struct SymphoniaSource {
    format: Box<dyn FormatReader>,
    decoder: Box<dyn Decoder>,
    track_id: u32,
    sample_rate: u32,
    channels: u16,
    total_duration: Option<Duration>,
    /// The current decoded packet's interleaved f32 samples, and our read cursor.
    samples: Vec<f32>,
    pos: usize,
}

impl SymphoniaSource {
    /// Decode forward until a packet for our track yields audio, replacing
    /// `samples`. Returns false at end of stream / on a fatal error.
    fn fill(&mut self) -> bool {
        loop {
            let packet = match self.format.next_packet() {
                Ok(p) => p,
                Err(_) => return false,
            };
            if packet.track_id() != self.track_id {
                continue;
            }
            match self.decoder.decode(&packet) {
                Ok(decoded) => {
                    let spec = *decoded.spec();
                    let mut sb = SampleBuffer::<f32>::new(decoded.capacity() as u64, spec);
                    sb.copy_interleaved_ref(decoded);
                    self.samples.clear();
                    self.samples.extend_from_slice(sb.samples());
                    self.pos = 0;
                    if !self.samples.is_empty() {
                        return true;
                    }
                }
                Err(SymError::DecodeError(_)) => continue, // skip a bad frame
                Err(_) => return false,
            }
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
        let to = SeekTo::Time {
            time: Time::from(pos.as_secs_f64()),
            track_id: Some(self.track_id),
        };
        self.format.seek(SeekMode::Accurate, to).map_err(|e| {
            SeekError::Other(Box::new(std::io::Error::new(
                std::io::ErrorKind::Other,
                e.to_string(),
            )))
        })?;
        // Drop decoder state built up before the seek, and the buffered packet.
        self.decoder.reset();
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

    let probed = symphonia::default::get_probe().format(
        &hint,
        mss,
        &FormatOptions::default(),
        &MetadataOptions::default(),
    )?;
    let mut format = probed.format;
    let track = format
        .default_track()
        .ok_or_else(|| anyhow!("file has no audio track"))?
        .clone();
    let track_id = track.id;
    let mut decoder =
        symphonia::default::get_codecs().make(&track.codec_params, &DecoderOptions::default())?;

    // The container's length, when it reports one (frames x time-base).
    let total_duration = match (track.codec_params.time_base, track.codec_params.n_frames) {
        (Some(tb), Some(n)) => {
            let t = tb.calc_time(n);
            Some(Duration::new(t.seconds, (t.frac * 1e9) as u32))
        }
        _ => None,
    };

    // Decode the first audio packet to learn the real spec and prime the buffer.
    let (sample_rate, channels, samples) = loop {
        let packet = format
            .next_packet()
            .map_err(|e| anyhow!("no audio in {}: {e}", path.display()))?;
        if packet.track_id() != track_id {
            continue;
        }
        match decoder.decode(&packet) {
            Ok(decoded) => {
                let spec = *decoded.spec();
                let mut sb = SampleBuffer::<f32>::new(decoded.capacity() as u64, spec);
                sb.copy_interleaved_ref(decoded);
                break (spec.rate, spec.channels.count() as u16, sb.samples().to_vec());
            }
            Err(SymError::DecodeError(_)) => continue,
            Err(e) => bail!("decode error in {}: {e}", path.display()),
        }
    };
    if sample_rate == 0 || channels == 0 {
        bail!("no audio decoded from {}", path.display());
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
    })
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
    let probed = symphonia::default::get_probe()
        .format(
            &hint,
            mss,
            &FormatOptions::default(),
            &MetadataOptions::default(),
        )
        .ok()?;
    let track = probed.format.default_track()?;
    match (track.codec_params.time_base, track.codec_params.n_frames) {
        (Some(tb), Some(n)) => {
            let t = tb.calc_time(n);
            Some(Duration::new(t.seconds, (t.frac * 1e9) as u32))
        }
        _ => None,
    }
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
        // Guard the SUPPORTED_EXTS list against advertising formats symphonia can't
        // decode. The decoders we rely on must be registered, and Opus / WavPack
        // (no decoder in symphonia 0.5) must stay off the supported list, else a
        // file is accepted into the playlist then fails to play.
        use symphonia::core::codecs::{
            CODEC_TYPE_AAC, CODEC_TYPE_ALAC, CODEC_TYPE_FLAC, CODEC_TYPE_MP3,
            CODEC_TYPE_OPUS, CODEC_TYPE_VORBIS, CODEC_TYPE_WAVPACK,
        };
        let codecs = symphonia::default::get_codecs();
        for c in [
            CODEC_TYPE_MP3, CODEC_TYPE_AAC, CODEC_TYPE_VORBIS, CODEC_TYPE_FLAC, CODEC_TYPE_ALAC,
        ] {
            assert!(codecs.get_codec(c).is_some(), "expected a decoder for {c:?}");
        }
        for c in [CODEC_TYPE_OPUS, CODEC_TYPE_WAVPACK] {
            assert!(codecs.get_codec(c).is_none(), "unexpected decoder for {c:?}");
        }
        for ext in ["opus", "wv"] {
            assert!(
                !crate::playlist::SUPPORTED_EXTS.contains(&ext),
                "{ext} can't be decoded, so it must not be advertised as supported"
            );
        }
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
