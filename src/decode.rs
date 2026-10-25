//! Local-file decoding for formats rodio 0.20's `Decoder` can't open directly.
//!
//! rodio's public `Decoder` API can't pass a format hint to symphonia, so it
//! returns `UnrecognizedFormat` on files whose container it can't content-probe
//! (notably MPEG audio Layer I/II — `.mp1`/`.mp2`/`.mpa`). symphonia itself can
//! decode those (the `mp3` bundle ships the Layer I/II decoder); it just needs an
//! extension hint. We decode such a file here with a hint and hand back an
//! in-memory, seekable rodio `Source` (#157).

use std::fs::File;
use std::path::Path;

use anyhow::{bail, Result};
use rodio::buffer::SamplesBuffer;

/// Extensions routed through this path instead of rodio's `Decoder`.
pub const HINTED_EXTS: &[&str] = &["mp1", "mp2", "mpa"];

/// True when `path` should be decoded here rather than by rodio's `Decoder`.
pub fn needs_hinted_decode(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| HINTED_EXTS.contains(&e.to_ascii_lowercase().as_str()))
        .unwrap_or(false)
}

/// Eagerly decode a whole local audio file via symphonia (with an extension hint)
/// into an in-memory `SamplesBuffer`, which is a seekable rodio `Source`. Used for
/// the extensions in [`HINTED_EXTS`]. The file is decoded up front, so this is for
/// the occasional legacy clip, not the hot path for everyday formats.
pub fn decode_file(path: &Path) -> Result<SamplesBuffer<f32>> {
    use symphonia::core::audio::SampleBuffer;
    use symphonia::core::codecs::DecoderOptions;
    use symphonia::core::errors::Error as SymError;
    use symphonia::core::formats::FormatOptions;
    use symphonia::core::io::MediaSourceStream;
    use symphonia::core::meta::MetadataOptions;
    use symphonia::core::probe::Hint;

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
        .ok_or_else(|| anyhow::anyhow!("file has no audio track"))?
        .clone();
    let track_id = track.id;
    let mut decoder =
        symphonia::default::get_codecs().make(&track.codec_params, &DecoderOptions::default())?;

    let mut samples: Vec<f32> = Vec::new();
    let mut rate = 0u32;
    let mut channels = 0u16;
    let mut sbuf: Option<SampleBuffer<f32>> = None;

    loop {
        let packet = match format.next_packet() {
            Ok(p) => p,
            Err(_) => break, // end of stream
        };
        if packet.track_id() != track_id {
            continue;
        }
        let decoded = match decoder.decode(&packet) {
            Ok(d) => d,
            Err(SymError::DecodeError(_)) => continue, // skip a bad frame
            Err(_) => break,
        };
        let spec = *decoded.spec();
        if sbuf.is_none() {
            rate = spec.rate;
            channels = spec.channels.count() as u16;
            sbuf = Some(SampleBuffer::<f32>::new(decoded.capacity() as u64, spec));
        }
        let sb = sbuf.as_mut().unwrap();
        sb.copy_interleaved_ref(decoded);
        samples.extend_from_slice(sb.samples());
    }

    if samples.is_empty() || channels == 0 || rate == 0 {
        bail!("no audio decoded from {}", path.display());
    }
    Ok(SamplesBuffer::new(channels, rate, samples))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn hinted_exts_are_the_mpeg_layer12_set() {
        assert!(needs_hinted_decode(Path::new("song.mp2")));
        assert!(needs_hinted_decode(Path::new("SONG.MP1"))); // case-insensitive
        assert!(needs_hinted_decode(Path::new("clip.mpa")));
        // Everyday formats go through rodio's Decoder, not this path.
        assert!(!needs_hinted_decode(Path::new("song.mp3")));
        assert!(!needs_hinted_decode(Path::new("song.flac")));
        assert!(!needs_hinted_decode(Path::new("noext")));
    }

    #[test]
    fn decode_file_errors_instead_of_panicking_on_a_non_audio_file() {
        // A missing file surfaces an error rather than panicking.
        let missing = PathBuf::from("definitely-not-here.mp2");
        assert!(decode_file(&missing).is_err());
    }
}
