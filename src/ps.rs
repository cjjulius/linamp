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

use anyhow::{bail, Result};
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
    if let Some(codec) = codec {
        if !es.is_empty() {
            return Ok((codec, es));
        }
    }
    // The `mpeg-ps` crate parses only MPEG-2 packs, so an MPEG-1 System Stream
    // (ffmpeg `-f mpeg`, VCD-era `.mpg`) yields nothing above. Fall back to a scanner
    // that also understands MPEG-1 pack and PES headers.
    if let Some(found) = scan_ps_audio(data) {
        return Ok(found);
    }
    bail!("no supported audio (MPEG audio / AC-3) in program stream")
}

/// Fallback program-stream audio extractor that understands both MPEG-1 and MPEG-2
/// pack/PES headers (the `mpeg-ps` crate handles only MPEG-2). It walks start codes,
/// steps over each pack/system/padding header by its real length, and concatenates
/// the first audio stream's PES payloads: MPEG audio (stream ids 0xC0..=0xDF) or
/// AC-3 in private_stream_1 (0xBD, the 4-byte DVD substream header stripped).
/// `None` when no audio stream is present.
fn scan_ps_audio(data: &[u8]) -> Option<(Codec, Vec<u8>)> {
    let n = data.len();
    let mut pos = 0usize;
    let mut codec: Option<Codec> = None;
    let mut es: Vec<u8> = Vec::new();
    let mut lock: Option<u8> = None; // chosen audio stream id
    let mut sub_lock: Option<u8> = None; // chosen private_stream_1 substream id
    while pos + 4 <= n {
        if !(data[pos] == 0 && data[pos + 1] == 0 && data[pos + 2] == 1) {
            pos += 1; // resync to the next start code
            continue;
        }
        let id = data[pos + 3];
        match id {
            0xBA => {
                // Pack header. MPEG-2 (next byte top bits 0b01): 14 bytes + stuffing.
                // MPEG-1 (top bits 0b0010): a fixed 12 bytes.
                if pos + 5 > n {
                    break;
                }
                if (data[pos + 4] & 0xC0) == 0x40 {
                    if pos + 14 > n {
                        break;
                    }
                    pos += 14 + (data[pos + 13] & 0x07) as usize;
                } else {
                    pos += 12;
                }
            }
            0xB9 => break, // program end
            // Headers / padding that carry a 2-byte length: skip the whole block.
            0xBB | 0xBC | 0xBE | 0xBF | 0xF0..=0xFF => {
                if pos + 6 > n {
                    break;
                }
                let len = ((data[pos + 4] as usize) << 8) | data[pos + 5] as usize;
                pos += 6 + len;
            }
            // PES packets (audio, private, video) carry a 2-byte length.
            0xBD | 0xC0..=0xEF => {
                if pos + 6 > n {
                    break;
                }
                let pes_len = ((data[pos + 4] as usize) << 8) | data[pos + 5] as usize;
                if pes_len == 0 {
                    pos += 6; // unbounded (seen on video); step past the prefix
                    continue;
                }
                let start = pos + 6;
                let end = (start + pes_len).min(n);
                if (0xC0..=0xDF).contains(&id) {
                    if let Some(payload) = pes_payload(&data[start..end]) {
                        if lock.is_none() {
                            lock = Some(id);
                            codec = Some(Codec::Mpa);
                        }
                        if lock == Some(id) {
                            es.extend_from_slice(payload);
                        }
                    }
                } else if id == 0xBD {
                    if let Some(payload) = pes_payload(&data[start..end]) {
                        if payload.len() >= 4 {
                            let sub = payload[0];
                            if (0x80..=0x87).contains(&sub) {
                                if lock.is_none() {
                                    lock = Some(0xBD);
                                    sub_lock = Some(sub);
                                    codec = Some(Codec::Ac3);
                                }
                                if lock == Some(0xBD) && sub_lock == Some(sub) {
                                    es.extend_from_slice(&payload[4..]);
                                }
                            }
                        }
                    }
                }
                pos = end;
            }
            _ => pos += 4, // unknown start code; step past it
        }
    }
    let codec = codec?;
    if es.is_empty() {
        return None;
    }
    Some((codec, es))
}

/// Return the elementary-stream payload of one PES packet body (the bytes after the
/// 6-byte start-code + length prefix), skipping the PES header in either its MPEG-2
/// form ('10' marker, flags, `PES_header_data_length`) or its MPEG-1 form (0xFF
/// stuffing, optional STD buffer, then the PTS/DTS field). `None` if it runs off the
/// end.
fn pes_payload(body: &[u8]) -> Option<&[u8]> {
    let n = body.len();
    if n == 0 {
        return None;
    }
    let mut i;
    if (body[0] & 0xC0) == 0x80 {
        // MPEG-2: byte0/byte1 flags, byte2 = header data length.
        if n < 3 {
            return None;
        }
        i = 3 + body[2] as usize;
    } else {
        // MPEG-1: up to 16 stuffing bytes.
        i = 0;
        let mut stuffed = 0;
        while i < n && body[i] == 0xFF && stuffed < 16 {
            i += 1;
            stuffed += 1;
        }
        if i >= n {
            return None;
        }
        // Optional STD buffer scale/size (top bits 0b01): 2 bytes.
        if (body[i] & 0xC0) == 0x40 {
            i += 2;
            if i >= n {
                return None;
            }
        }
        // PTS/DTS field: PTS only (0x2x) is 5 bytes, PTS+DTS (0x3x) is 10, none (0x0F) is 1.
        if (body[i] & 0xF0) == 0x20 {
            i += 5;
        } else if (body[i] & 0xF0) == 0x30 {
            i += 10;
        } else {
            i += 1;
        }
    }
    if i > n {
        return None;
    }
    Some(&body[i..])
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

    /// An MPEG-1 System Stream (ffmpeg `-f mpeg`, as VCD-era `.mpg` files are) is not
    /// parsed by the `mpeg-ps` crate; the MPEG-1 fallback scanner must extract its MP2
    /// audio. Make the sample with:
    ///   ffmpeg -i src.wav -f lavfi -i testsrc=d=75 -map 0:a -map 1:v \
    ///     -c:a mp2 -c:v mpeg1video -f mpeg video-mp2-mpeg1.mpg
    /// (qa-music is gitignored, so this skips on CI.)
    #[test]
    fn decodes_audio_from_mpeg1_program_stream() {
        let Some(p) = sample("video-mp2-mpeg1.mpg") else { return };
        let src = open_ps(&p).unwrap_or_else(|e| panic!("mpeg-1 ps: {e}"));
        let (sr, ch) = (src.sample_rate(), src.channels());
        assert!(sr >= 8000, "rate {sr}");
        assert!(ch == 1 || ch == 2, "channels {ch}");
        let got: Vec<f32> = src.collect();
        let secs = got.len() as f64 / (sr as f64 * ch as f64);
        assert!((70.0..=80.0).contains(&secs), "mpeg-1 decoded {secs:.1}s, expected ~75s");
        let energy = (got.iter().map(|s| (*s as f64).powi(2)).sum::<f64>() / got.len() as f64).sqrt();
        assert!(energy > 0.01, "mpeg-1 RMS {energy:.4} too low (silence?)");
    }

    #[test]
    fn duration_is_plausible() {
        let Some(p) = sample("video-mp2.mpg") else { return };
        let d = duration(&p).expect("ps duration");
        assert!(d.as_secs() > 1 && d.as_secs() < 600, "duration {d:?}");
    }
}
