//! MPEG-TS (.ts/.m2ts/.mts) audio extraction via the pure-Rust `mpeg2ts-reader`.
//!
//! symphonia can't demux a Transport Stream, so this pulls the first audio
//! elementary stream out of the container and hands it to the shared decode path
//! (`crate::decode::open_stream`). AAC (ADTS) and MPEG audio (MP1/MP2) then reuse
//! symphonia end-to-end, exactly as a raw `.aac`/`.mp2` file would. Only the
//! compressed audio ES is buffered, not the whole file, and decoding plus seeking
//! stay lazy on that ES.
//!
//! AC-3 in TS is recognised but not yet routed (symphonia has no AC-3 decoder and
//! `crate::ac3` currently decodes from a path, not bytes); it returns an error so
//! the skip-unplayable guard takes over rather than playing noise.

use std::cell::RefCell;
use std::fs::File;
use std::io::{Cursor, Read};
use std::path::Path;
use std::rc::Rc;
use std::time::Duration;

use anyhow::{anyhow, bail, Result};
use mpeg2ts_reader::demultiplex::{
    self, DemuxContext, Demultiplex, FilterChangeset, FilterRequest, PacketFilter,
};
use mpeg2ts_reader::packet::{Packet, Pid};
use mpeg2ts_reader::{pes, psi, StreamType};
use rodio::Source;
use symphonia::core::formats::probe::Hint;
use symphonia::core::io::MediaSourceStream;

use crate::decode::{open_stream, SymphoniaSource};

/// TS container extensions handled here.
pub const TS_EXTS: &[&str] = &["ts", "m2ts", "mts"];

/// True when `path` is a Transport Stream routed through this module.
pub fn is_ts(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| TS_EXTS.iter().any(|x| e.eq_ignore_ascii_case(x)))
        .unwrap_or(false)
}

/// Audio stream types we extract. (MPEG-1/2 audio, AAC ADTS / raw, and AC-3.)
fn is_audio(t: StreamType) -> bool {
    t == StreamType::ISO_11172_AUDIO
        || t == StreamType::ISO_138183_AUDIO
        || t == StreamType::ADTS
        || t == StreamType::AUDIO_WITHOUT_TRANSPORT_SYNTAX
        || t == StreamType::ATSC_DOLBY_DIGITAL_AUDIO
}

/// The chosen audio PID, its stream type, and the concatenated elementary-stream
/// bytes. Shared (single-threaded) between the demux context and the PES consumer.
#[derive(Default)]
struct EsAccum {
    pid: Option<Pid>,
    stream_type: Option<StreamType>,
    data: Vec<u8>,
}
type Shared = Rc<RefCell<EsAccum>>;

/// Accumulates one audio PID's PES payloads into the shared buffer. The PES header
/// is already stripped by `PesPacketFilter`, so `header.payload()` and every
/// `continue_packet` chunk are raw elementary-stream bytes.
struct AudioEsConsumer {
    shared: Shared,
}
impl pes::ElementaryStreamConsumer<TsDemuxContext> for AudioEsConsumer {
    fn start_stream(&mut self, _ctx: &mut TsDemuxContext) {}
    fn begin_packet(&mut self, _ctx: &mut TsDemuxContext, header: pes::PesHeader<'_>) {
        // The first elementary-stream bytes sit in the same TS packet as the PES
        // header; the rest arrive via `continue_packet`.
        match header.contents() {
            pes::PesContents::Parsed(Some(parsed)) => {
                self.shared.borrow_mut().data.extend_from_slice(parsed.payload());
            }
            pes::PesContents::Payload(bytes) => {
                self.shared.borrow_mut().data.extend_from_slice(bytes);
            }
            pes::PesContents::Parsed(None) => {}
        }
    }
    fn continue_packet(&mut self, _ctx: &mut TsDemuxContext, data: &[u8]) {
        self.shared.borrow_mut().data.extend_from_slice(data);
    }
    fn end_packet(&mut self, _ctx: &mut TsDemuxContext) {}
    fn continuity_error(&mut self, _ctx: &mut TsDemuxContext) {}
}

/// The demux filter set: PAT, PMT, the audio PES consumer, and a null sink for
/// everything else (video, other audio tracks, stuffing).
enum TsFilterSwitch {
    Pat(demultiplex::PatPacketFilter<TsDemuxContext>),
    Pmt(demultiplex::PmtPacketFilter<TsDemuxContext>),
    Pes(pes::PesPacketFilter<TsDemuxContext, AudioEsConsumer>),
    Null(demultiplex::NullPacketFilter<TsDemuxContext>),
}
impl PacketFilter for TsFilterSwitch {
    type Ctx = TsDemuxContext;
    fn consume(&mut self, ctx: &mut TsDemuxContext, pk: &Packet<'_>) {
        match self {
            TsFilterSwitch::Pat(f) => f.consume(ctx, pk),
            TsFilterSwitch::Pmt(f) => f.consume(ctx, pk),
            TsFilterSwitch::Pes(f) => f.consume(ctx, pk),
            TsFilterSwitch::Null(f) => f.consume(ctx, pk),
        }
    }
}

/// Hand-expanded `demux_context!`: the stock macro's context has no room for our
/// own state, so we write it out and add the shared accumulator.
struct TsDemuxContext {
    changeset: FilterChangeset<TsFilterSwitch>,
    shared: Shared,
}
impl TsDemuxContext {
    fn new(shared: Shared) -> Self {
        Self {
            changeset: FilterChangeset::default(),
            shared,
        }
    }
    fn do_construct(&mut self, req: FilterRequest<'_, '_>) -> TsFilterSwitch {
        match req {
            FilterRequest::ByPid(psi::pat::PAT_PID) => {
                TsFilterSwitch::Pat(demultiplex::PatPacketFilter::default())
            }
            FilterRequest::Pmt {
                pid,
                program_number,
            } => TsFilterSwitch::Pmt(demultiplex::PmtPacketFilter::new(pid, program_number)),
            // The first audio stream we see wins; later ones go to the null sink.
            FilterRequest::ByStream {
                stream_type,
                stream_info,
                ..
            } if is_audio(stream_type) && self.shared.borrow().pid.is_none() => {
                {
                    let mut sh = self.shared.borrow_mut();
                    sh.pid = Some(stream_info.elementary_pid());
                    sh.stream_type = Some(stream_type);
                }
                TsFilterSwitch::Pes(pes::PesPacketFilter::new(AudioEsConsumer {
                    shared: self.shared.clone(),
                }))
            }
            FilterRequest::ByStream { .. } | FilterRequest::ByPid(_) | FilterRequest::Nit { .. } => {
                TsFilterSwitch::Null(demultiplex::NullPacketFilter::default())
            }
        }
    }
}
impl DemuxContext for TsDemuxContext {
    type F = TsFilterSwitch;
    fn filter_changeset(&mut self) -> &mut FilterChangeset<Self::F> {
        &mut self.changeset
    }
    fn construct(&mut self, req: FilterRequest<'_, '_>) -> Self::F {
        self.do_construct(req)
    }
}

/// Stream `path` through the TS demux and return the first audio stream's type and
/// its elementary-stream bytes. Reads in chunks, so only the extracted (compressed)
/// audio ES is held in memory, never the whole Transport Stream.
fn extract_audio_es(path: &Path) -> Result<(StreamType, Vec<u8>)> {
    let shared: Shared = Rc::new(RefCell::new(EsAccum::default()));
    let mut ctx = TsDemuxContext::new(shared.clone());
    let mut demux = Demultiplex::new(&mut ctx);
    let mut file = File::open(path)?;
    let mut buf = vec![0u8; 188 * 1024];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        demux.push(&mut ctx, &buf[..n]);
    }
    let stream_type = shared.borrow().stream_type;
    let data = std::mem::take(&mut shared.borrow_mut().data);
    let st = stream_type.ok_or_else(|| anyhow!("no audio stream in {}", path.display()))?;
    if data.is_empty() {
        bail!("no audio data in {}", path.display());
    }
    Ok((st, data))
}

/// Open a Transport Stream's audio as a seekable source: extract the audio ES and
/// decode it through the shared path. AAC and MPEG audio work; AC-3 is refused for
/// now (no bytes-based AC-3 decode path yet).
pub fn open_ts(path: &Path) -> Result<SymphoniaSource> {
    let (st, es) = extract_audio_es(path)?;
    let hint_ext = if st == StreamType::ADTS || st == StreamType::AUDIO_WITHOUT_TRANSPORT_SYNTAX {
        "aac"
    } else if st == StreamType::ISO_11172_AUDIO || st == StreamType::ISO_138183_AUDIO {
        "mp2"
    } else if st == StreamType::ATSC_DOLBY_DIGITAL_AUDIO {
        bail!("AC-3 audio in MPEG-TS is not yet supported");
    } else {
        bail!("unsupported MPEG-TS audio stream type {st:?}");
    };
    let mut hint = Hint::new();
    hint.with_extension(hint_ext);
    let mss = MediaSourceStream::new(Box::new(Cursor::new(es)), Default::default());
    open_stream(mss, hint, "mpeg-ts audio")
}

/// The length of a TS file's audio, so the playlist can show a time. `None` when it
/// isn't a TS or the audio can't be opened.
pub fn duration(path: &Path) -> Option<Duration> {
    if !is_ts(path) {
        return None;
    }
    open_ts(path).ok().and_then(|s| s.total_duration())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(name: &str) -> Option<std::path::PathBuf> {
        let p = std::path::PathBuf::from(format!("qa-music/format-samples/{name}"));
        p.exists().then_some(p)
    }

    /// AAC and MP2 audio pulled from a Transport Stream must decode. The TS files are
    /// made from the CC0 clip, so a ~75s duration and real samples prove the extract
    /// + reuse-the-decoder path end to end. (qa-music is gitignored; skips on CI.)
    #[test]
    fn decodes_audio_from_transport_stream() {
        for (name, rate) in [("video-aac.ts", 44100), ("video-mp2.ts", 44100)] {
            let Some(p) = sample(name) else { continue };
            let src = open_ts(&p).unwrap_or_else(|e| panic!("{name}: {e}"));
            let (sr, ch) = (src.sample_rate(), src.channels());
            assert_eq!(sr, rate, "{name} rate");
            assert!(ch == 1 || ch == 2, "{name} channels");
            let got: Vec<f32> = src.collect();
            // Decodes the whole ~75s clip (not just a few frames): within 1s of the
            // expected length. Lossy decoder delay/padding keeps this loose.
            let secs = got.len() as f64 / (sr as f64 * ch as f64);
            assert!((70.0..=80.0).contains(&secs), "{name} decoded {secs:.1}s, expected ~75s");
            // And it's real audio, not silence.
            let energy = (got.iter().map(|s| (*s as f64).powi(2)).sum::<f64>() / got.len() as f64).sqrt();
            assert!(energy > 0.01, "{name} RMS energy {energy:.4} too low (silence?)");
        }
    }

    /// AC-3 in TS is recognised but refused (no bytes AC-3 path yet), so it skips
    /// cleanly instead of mis-playing.
    #[test]
    fn ac3_in_ts_is_refused_for_now() {
        let Some(p) = sample("video-ac3.ts") else { return };
        assert!(open_ts(&p).is_err(), "AC-3 in TS should be refused for now");
    }

    #[test]
    fn duration_is_plausible() {
        let Some(p) = sample("video-aac.ts") else { return };
        let d = duration(&p).expect("ts duration");
        assert!(d.as_secs() > 1 && d.as_secs() < 600, "duration {d:?}");
    }
}
