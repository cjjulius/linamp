//! AVI (.avi) audio extraction via the pure-Rust `filmcraft-avi` (same author as our
//! `filmcraft-ac3` decoder).
//!
//! symphonia can't demux AVI, so, like MPEG-TS, this pulls the first audio stream's
//! chunks out of the RIFF container and decodes them. MP3 / MP2 reuse the shared
//! symphonia path (`crate::decode::open_stream`); raw PCM is wrapped in a WAV header
//! and decoded the same way; AC-3 is fed to `crate::ac3`. The audio format comes from
//! the stream's `WAVEFORMATEX` tag.
//!
//! The compressed audio is buffered (the demuxer reads the file with random access,
//! so the whole AVI is not loaded), then decoding and seeking stay lazy on that ES.
//! AAC-in-AVI (raw, config in the stream header) is refused for now.

use std::cell::RefCell;
use std::fs::File;
use std::io::{Cursor, Read, Seek, SeekFrom};
use std::path::Path;
use std::time::Duration;

use anyhow::{anyhow, bail, Result};
use filmcraft_avi::{ByteSource, StreamKind, WaveFormat};
use rodio::Source;
use symphonia::core::formats::probe::Hint;
use symphonia::core::io::MediaSourceStream;

use crate::ac3::open_ac3_bytes;
use crate::decode::{open_stream, ContainerAudio};

/// True when `path` is an AVI file routed through this module.
pub fn is_avi(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| e.eq_ignore_ascii_case("avi"))
        .unwrap_or(false)
}

/// A `ByteSource` over a file by random `read_at`, so `filmcraft-avi` can parse a
/// large AVI without loading the whole thing into memory.
struct FileSource {
    file: RefCell<File>,
    len: u64,
}
impl FileSource {
    fn open(path: &Path) -> std::io::Result<Self> {
        let file = File::open(path)?;
        let len = file.metadata()?.len();
        Ok(Self {
            file: RefCell::new(file),
            len,
        })
    }
}
impl ByteSource for FileSource {
    fn len(&self) -> u64 {
        self.len
    }
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> std::io::Result<()> {
        let mut f = self.file.borrow_mut();
        f.seek(SeekFrom::Start(offset))?;
        f.read_exact(buf)
    }
}

/// Wrap raw interleaved PCM in a 44-byte WAV header so symphonia can decode it. The
/// format tag (1 = integer, 3 = float), channel count, rate and bit depth come from
/// the AVI audio stream's `WAVEFORMATEX`.
fn wrap_wav(wf: &WaveFormat, pcm: &[u8]) -> Vec<u8> {
    let data_len = pcm.len() as u32;
    let bytes_per_sample = (wf.bits_per_sample as u32).div_ceil(8);
    let block_align = wf.channels as u32 * bytes_per_sample;
    let byte_rate = wf.sample_rate * block_align;
    let mut v = Vec::with_capacity(44 + pcm.len());
    v.extend_from_slice(b"RIFF");
    v.extend_from_slice(&(36 + data_len).to_le_bytes());
    v.extend_from_slice(b"WAVE");
    v.extend_from_slice(b"fmt ");
    v.extend_from_slice(&16u32.to_le_bytes());
    v.extend_from_slice(&wf.format_tag.to_le_bytes());
    v.extend_from_slice(&wf.channels.to_le_bytes());
    v.extend_from_slice(&wf.sample_rate.to_le_bytes());
    v.extend_from_slice(&byte_rate.to_le_bytes());
    v.extend_from_slice(&(block_align as u16).to_le_bytes());
    v.extend_from_slice(&wf.bits_per_sample.to_le_bytes());
    v.extend_from_slice(b"data");
    v.extend_from_slice(&data_len.to_le_bytes());
    v.extend_from_slice(pcm);
    v
}

/// Open an AVI's audio as a seekable source: find the first audio stream, read its
/// chunks into the elementary stream, and decode by format tag. Errors cleanly on an
/// unreadable file or an audio format we don't handle (skip-unplayable then wins).
pub fn open_avi(path: &Path) -> Result<ContainerAudio> {
    let src = FileSource::open(path)?;
    let avi = filmcraft_avi::open(&src).map_err(|e| anyhow!("AVI parse {}: {e:?}", path.display()))?;
    // The first audio stream wins.
    let (idx, stream) = avi
        .streams
        .iter()
        .enumerate()
        .find(|(_, s)| s.kind == StreamKind::Audio && s.audio.is_some())
        .ok_or_else(|| anyhow!("no audio stream in {}", path.display()))?;
    let wf = stream.audio.as_ref().unwrap().clone();
    // Concatenate the stream's chunks into the elementary stream.
    let mut es = Vec::new();
    for i in 0..stream.chunks.len() {
        let chunk = avi
            .read_chunk(&src, idx, i)
            .map_err(|e| anyhow!("AVI chunk {i} in {}: {e:?}", path.display()))?;
        es.extend_from_slice(&chunk);
    }
    if es.is_empty() {
        bail!("no audio data in {}", path.display());
    }
    // Route by WAVEFORMATEX format tag.
    match wf.format_tag {
        0x0001 | 0x0003 => {
            // Integer / IEEE-float PCM: wrap in WAV and let symphonia decode it.
            let wav = wrap_wav(&wf, &es);
            let mut hint = Hint::new();
            hint.with_extension("wav");
            let mss = MediaSourceStream::new(Box::new(Cursor::new(wav)), Default::default());
            Ok(ContainerAudio::Symphonia(open_stream(mss, hint, "avi pcm")?))
        }
        0x0050 | 0x0055 => {
            // MPEG audio (MP2) / MP3.
            let mut hint = Hint::new();
            hint.with_extension(if wf.format_tag == 0x0055 { "mp3" } else { "mp2" });
            let mss = MediaSourceStream::new(Box::new(Cursor::new(es)), Default::default());
            Ok(ContainerAudio::Symphonia(open_stream(mss, hint, "avi audio")?))
        }
        0x2000 => Ok(ContainerAudio::Ac3(Box::new(open_ac3_bytes(es, "avi ac-3")?))),
        other => bail!("unsupported AVI audio format tag 0x{other:04x}"),
    }
}

/// The length of an AVI file's audio, so the playlist can show a time. `None` when it
/// isn't an AVI or the audio can't be opened.
pub fn duration(path: &Path) -> Option<Duration> {
    if !is_avi(path) {
        return None;
    }
    open_avi(path).ok().and_then(|s| s.total_duration())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(name: &str) -> Option<std::path::PathBuf> {
        let p = std::path::PathBuf::from(format!("qa-music/format-samples/{name}"));
        p.exists().then_some(p)
    }

    /// MP3, PCM and AC-3 audio pulled from an AVI must decode the full ~75s clip with
    /// real energy. The AVI files are made from the CC0 clip (qa-music is gitignored,
    /// so this skips on CI).
    #[test]
    fn decodes_audio_from_avi() {
        for name in ["video-mp3.avi", "video-pcm.avi", "video-ac3.avi"] {
            let Some(p) = sample(name) else { continue };
            let src = open_avi(&p).unwrap_or_else(|e| panic!("{name}: {e}"));
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
        let Some(p) = sample("video-mp3.avi") else { return };
        let d = duration(&p).expect("avi duration");
        assert!(d.as_secs() > 1 && d.as_secs() < 600, "duration {d:?}");
    }
}
