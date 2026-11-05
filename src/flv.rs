//! FLV (.flv) audio extraction via the pure-Rust `scuffle-flv`.
//!
//! symphonia can't demux Flash Video, so, like MPEG-TS and AVI, this pulls the
//! audio tags out of the container and decodes them through the shared path. MP3
//! reuses symphonia directly; AAC arrives as a sequence-header (AudioSpecificConfig)
//! then raw access units, so each AU is wrapped in a 7-byte ADTS header built from
//! that config, turning it into an ADTS stream symphonia reads. Only the compressed
//! audio is buffered; decoding and seeking stay lazy on it.
//!
//! Legacy FLV audio tags only (the common case); enhanced (E-RTMP) tags and audio
//! formats other than MP3/AAC are skipped.

use std::io::Cursor;
use std::path::Path;
use std::time::Duration;

use anyhow::{anyhow, bail, Result};
use bytes::Bytes;
use rodio::Source;
use scuffle_flv::audio::body::legacy::aac::AacAudioData;
use scuffle_flv::audio::body::legacy::LegacyAudioTagBody;
use scuffle_flv::audio::body::AudioTagBody;
use scuffle_flv::audio::header::legacy::SoundFormat;
use scuffle_flv::audio::header::AudioTagHeader;
use scuffle_flv::file::FlvFile;
use scuffle_flv::tag::FlvTagData;
use symphonia::core::formats::probe::Hint;
use symphonia::core::io::MediaSourceStream;

use crate::decode::{open_stream, ContainerAudio};

/// True when `path` is an FLV file routed through this module.
pub fn is_flv(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| e.eq_ignore_ascii_case("flv"))
        .unwrap_or(false)
}

/// Build a 7-byte ADTS header (no CRC) for one AAC access unit of `au_len` bytes,
/// from the stream's AudioSpecificConfig. `None` for a config this doesn't cover
/// (object types outside 1..=4, i.e. not plain AAC main/LC/SSR/LTP).
fn adts_header(asc: &[u8], au_len: usize) -> Option<[u8; 7]> {
    if asc.len() < 2 {
        return None;
    }
    let object_type = asc[0] >> 3; // 5 bits
    let freq_idx = ((asc[0] & 0b111) << 1) | (asc[1] >> 7); // 4 bits
    let chan = (asc[1] >> 3) & 0b1111; // 4 bits
    if !(1..=4).contains(&object_type) {
        return None;
    }
    let profile = object_type - 1; // ADTS profile = MPEG-4 object type - 1 (2 bits)
    let frame_len = (au_len + 7) as u32;
    if frame_len > 0x1FFF {
        return None;
    }
    Some([
        0xFF,
        0xF1, // syncword + MPEG-4, layer 0, protection absent
        (profile << 6) | (freq_idx << 2) | ((chan >> 2) & 1),
        ((chan & 3) << 6) | ((frame_len >> 11) as u8 & 0b11),
        ((frame_len >> 3) & 0xFF) as u8,
        (((frame_len & 7) as u8) << 5) | 0x1F,
        0xFC,
    ])
}

#[derive(PartialEq)]
enum Codec {
    Mp3,
    Aac,
}

/// Open an FLV's audio as a seekable source: demux the audio tags, assemble an
/// elementary stream (MP3 frames, or ADTS-wrapped AAC), and decode it through the
/// shared symphonia path.
pub fn open_flv(path: &Path) -> Result<ContainerAudio> {
    let data = Bytes::from(std::fs::read(path)?);
    let flv = FlvFile::demux(&mut Cursor::new(data))
        .map_err(|e| anyhow!("FLV parse {}: {e:?}", path.display()))?;

    let mut codec: Option<Codec> = None;
    let mut es: Vec<u8> = Vec::new();
    let mut asc: Option<Vec<u8>> = None;
    for tag in flv.tags {
        let FlvTagData::Audio(audio) = tag.data else {
            continue;
        };
        // Legacy tags only; the format is in the header.
        let AudioTagHeader::Legacy(header) = &audio.header else {
            continue;
        };
        match (header.sound_format, &audio.body) {
            (SoundFormat::Mp3, AudioTagBody::Legacy(LegacyAudioTagBody::Other { sound_data })) => {
                codec = Some(Codec::Mp3);
                es.extend_from_slice(sound_data);
            }
            (SoundFormat::Aac, AudioTagBody::Legacy(LegacyAudioTagBody::Aac(aac))) => match aac {
                AacAudioData::SequenceHeader(cfg) => asc = Some(cfg.to_vec()),
                AacAudioData::Raw(au) => {
                    if let Some(cfg) = &asc {
                        if let Some(hdr) = adts_header(cfg, au.len()) {
                            codec = Some(Codec::Aac);
                            es.extend_from_slice(&hdr);
                            es.extend_from_slice(au);
                        }
                    }
                }
                _ => {}
            },
            _ => {}
        }
    }

    let codec = codec.ok_or_else(|| anyhow!("no supported audio (MP3/AAC) in {}", path.display()))?;
    if es.is_empty() {
        bail!("no audio data in {}", path.display());
    }
    let mut hint = Hint::new();
    hint.with_extension(if codec == Codec::Mp3 { "mp3" } else { "aac" });
    let mss = MediaSourceStream::new(Box::new(Cursor::new(es)), Default::default());
    Ok(ContainerAudio::Symphonia(open_stream(mss, hint, "flv audio")?))
}

/// The length of an FLV file's audio, so the playlist can show a time. `None` when
/// it isn't an FLV or the audio can't be opened.
pub fn duration(path: &Path) -> Option<Duration> {
    if !is_flv(path) {
        return None;
    }
    open_flv(path).ok().and_then(|s| s.total_duration())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(name: &str) -> Option<std::path::PathBuf> {
        let p = std::path::PathBuf::from(format!("qa-music/format-samples/{name}"));
        p.exists().then_some(p)
    }

    /// MP3 and AAC audio pulled from an FLV must decode the full ~75s clip with real
    /// energy. The AAC path exercises the ASC->ADTS wrapping. (qa-music is gitignored,
    /// so this skips on CI.)
    #[test]
    fn decodes_audio_from_flv() {
        for name in ["video-mp3.flv", "video-aac.flv"] {
            let Some(p) = sample(name) else { continue };
            let src = open_flv(&p).unwrap_or_else(|e| panic!("{name}: {e}"));
            let (sr, ch) = (src.sample_rate(), src.channels());
            assert!(sr >= 8000, "{name} rate {sr}");
            assert!(ch == 1 || ch == 2, "{name} channels {ch}");
            let got: Vec<f32> = src.collect();
            let secs = got.len() as f64 / (sr as f64 * ch as f64);
            assert!((70.0..=80.0).contains(&secs), "{name} decoded {secs:.1}s, expected ~75s");
            let energy = (got.iter().map(|s| (*s as f64).powi(2)).sum::<f64>() / got.len() as f64).sqrt();
            assert!(energy > 0.01, "{name} RMS {energy:.4} too low (silence?)");
        }
    }

    #[test]
    fn duration_is_plausible() {
        let Some(p) = sample("video-mp3.flv") else { return };
        let d = duration(&p).expect("flv duration");
        assert!(d.as_secs() > 1 && d.as_secs() < 600, "duration {d:?}");
    }
}
