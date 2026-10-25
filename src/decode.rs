//! Local-file decoding for formats rodio 0.20's `Decoder` can't open directly.
//!
//! rodio's public `Decoder` API can't pass a format hint to symphonia, so it
//! returns `UnrecognizedFormat` on files it can't content-probe (MPEG audio Layer
//! I/II — `.mp1`/`.mp2`/`.mpa`), and it outright *panics* on the ISO-MP4 family
//! (`.m4a`/`.mp4`/`.m4b`) — an `unreachable!` on a seek. symphonia itself handles
//! all of these; it just needs an extension hint. We decode such a file here with
//! a hint and hand back a lazy, seekable rodio `Source` (#157/#164/#163).

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

/// Extensions routed through this path instead of rodio's `Decoder`: MPEG audio
/// Layer I/II (rodio 0.20 can't pass a hint, #157) and the ISO-MP4 family (rodio
/// 0.20 *panics* opening these — an `unreachable!` on a seek — #164). Both decode
/// cleanly via symphonia with an extension hint.
pub const HINTED_EXTS: &[&str] = &["mp1", "mp2", "mpa", "m4a", "mp4", "m4b"];

/// True when `path` should be decoded here rather than by rodio's `Decoder`.
pub fn needs_hinted_decode(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| HINTED_EXTS.contains(&e.to_ascii_lowercase().as_str()))
        .unwrap_or(false)
}

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
/// hint (which rodio 0.20's `Decoder` can't pass). Used for [`HINTED_EXTS`]. The
/// first audio packet is decoded here so the real sample rate / channel count are
/// known before playback, and so an unplayable file fails now with an error rather
/// than later.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hinted_exts_cover_mpeg_layer12_and_iso_mp4() {
        for p in ["song.mp2", "SONG.MP1", "clip.mpa", "track.m4a", "movie.mp4", "book.m4b"] {
            assert!(needs_hinted_decode(Path::new(p)), "{p} should be hinted");
        }
        // Everyday formats go through rodio's Decoder, not this path.
        assert!(!needs_hinted_decode(Path::new("song.mp3")));
        assert!(!needs_hinted_decode(Path::new("song.flac")));
        assert!(!needs_hinted_decode(Path::new("noext")));
    }

    #[test]
    fn open_file_errors_instead_of_panicking_on_a_non_audio_file() {
        // A missing file surfaces an error rather than panicking.
        assert!(open_file(Path::new("definitely-not-here.mp2")).is_err());
    }

    #[test]
    fn open_file_decodes_and_seeks_a_real_track() {
        // Uses the committed MP3 fixture to exercise the whole path: open_file works
        // for any symphonia format via the hint, so this covers decode, the Source
        // impl and try_seek without needing an .mp2/.m4a sample (none is on CI).
        let p = Path::new("music/Aria Nocturne - Crystal Harbor.mp3");
        if !p.exists() {
            return; // fixture absent (e.g. a partial checkout): skip, don't fail
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
}
