//! MPEG Program Stream (.mpg/.mpeg/.vob) audio extraction via the pure-Rust
//! `mpeg-ps`.
//!
//! symphonia can't demux a Program Stream, so, like TS/AVI/FLV, this pulls the first
//! audio stream's PES payloads out and decodes them: MPEG audio (MP1/MP2) in stream
//! ids 0xC0..=0xDF reuses the shared symphonia path; AC-3, carried in
//! private_stream_1 (0xBD) behind a 4-byte DVD substream header, goes to
//! `crate::ac3`. DTS and LPCM substreams are recognised but not yet routed.
//!
//! The whole file is read into memory (the demuxer is slice-based), so a very large
//! VOB is buffered whole; fine for clips, a known limit for feature-length discs.

use std::io::Cursor;
use std::path::Path;
use std::time::Duration;

use anyhow::{anyhow, bail, Result};
use rodio::Source;
use symphonia::core::formats::probe::Hint;
use symphonia::core::io::MediaSourceStream;

use crate::ac3::open_ac3_bytes;
use crate::decode::{open_stream, ContainerAudio};

/// Program-stream container extensions handled here.
pub const PS_EXTS: &[&str] = &["mpg", "mpeg", "vob", "m2p"];

/// True when `path` is a Program Stream routed through this module.
pub fn is_ps(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| PS_EXTS.iter().any(|x| e.eq_ignore_ascii_case(x)))
        .unwrap_or(false)
}

enum Codec {
    /// MPEG-1/2 audio (MP1/MP2), decoded by symphonia.
    Mpa,
    /// Dolby Digital, decoded by crate::ac3.
    Ac3,
}

/// Extract the first audio stream's elementary stream from a program stream. Locks
/// onto the first audio stream it sees (by stream id, and by substream id for the
/// private_stream_1 AC-3 case) and concatenates its payloads.
fn extract_audio_es(data: &[u8]) -> Result<(Codec, Vec<u8>)> {
    let scan = mpeg_ps::program_stream::scan_packs(data);
    let mut codec: Option<Codec> = None;
    let mut es: Vec<u8> = Vec::new();
    let mut lock: Option<u8> = None; // chosen stream id
    let mut sub_lock: Option<u8> = None; // chosen private_stream_1 substream id
    for pack in &scan.packs {
        for pes in &pack.pes_packets {
            let sid = pes.stream_id.0;
            if (0xC0..=0xDF).contains(&sid) {
                // MPEG audio stream.
                if lock.is_none() {
                    lock = Some(sid);
                    codec = Some(Codec::Mpa);
                }
                if lock == Some(sid) {
                    es.extend_from_slice(pes.payload);
                }
            } else if sid == 0xBD {
                // private_stream_1 carries AC-3/DTS/LPCM; payload[0] is the substream
                // id, then a 3-byte DVD header (frame count + first-access-unit offset).
                let payload = pes.payload;
                if payload.len() < 4 {
                    continue;
                }
                let sub = payload[0];
                if (0x80..=0x87).contains(&sub) {
                    // AC-3.
                    if lock.is_none() {
                        lock = Some(0xBD);
                        sub_lock = Some(sub);
                        codec = Some(Codec::Ac3);
                    }
                    if lock == Some(0xBD) && sub_lock == Some(sub) {
                        es.extend_from_slice(&payload[4..]);
                    }
                }
                // DTS (0x88..=0x8F) and LPCM (0xA0..=0xA7) are not yet routed.
            }
        }
    }
    let codec = codec
        .ok_or_else(|| anyhow!("no supported audio (MPEG audio / AC-3) in program stream"))?;
    if es.is_empty() {
        bail!("no audio data in program stream");
    }
    Ok((codec, es))
}

/// Open a program stream's audio as a seekable source.
pub fn open_ps(path: &Path) -> Result<ContainerAudio> {
    let data = std::fs::read(path)?;
    let (codec, es) = extract_audio_es(&data)?;
    match codec {
        Codec::Ac3 => Ok(ContainerAudio::Ac3(Box::new(open_ac3_bytes(
            es,
            "program-stream ac-3",
        )?))),
        Codec::Mpa => {
            let mut hint = Hint::new();
            hint.with_extension("mp2");
            let mss = MediaSourceStream::new(Box::new(Cursor::new(es)), Default::default());
            Ok(ContainerAudio::Symphonia(open_stream(
                mss,
                hint,
                "program-stream audio",
            )?))
        }
    }
}

/// The length of a program stream's audio, so the playlist can show a time. `None`
/// when it isn't a PS or the audio can't be opened.
pub fn duration(path: &Path) -> Option<Duration> {
    if !is_ps(path) {
        return None;
    }
    open_ps(path).ok().and_then(|s| s.total_duration())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(name: &str) -> Option<std::path::PathBuf> {
        let p = std::path::PathBuf::from(format!("qa-music/format-samples/{name}"));
        p.exists().then_some(p)
    }

    /// MP2 (in a .mpg) and AC-3 (in a .vob's private_stream_1) must decode the full
    /// ~75s clip with real energy. The .vob exercises the 4-byte DVD substream strip.
    /// (qa-music is gitignored, so this skips on CI.)
    #[test]
    fn decodes_audio_from_program_stream() {
        for name in ["video-mp2.mpg", "video-ac3.vob"] {
            let Some(p) = sample(name) else { continue };
            let src = open_ps(&p).unwrap_or_else(|e| panic!("{name}: {e}"));
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
        let Some(p) = sample("video-mp2.mpg") else { return };
        let d = duration(&p).expect("ps duration");
        assert!(d.as_secs() > 1 && d.as_secs() < 600, "duration {d:?}");
    }
}
