//! Speex (.spx) playback via libspeex (the `speex-safe` wrapper over vendored C).
//!
//! Speex is a deprecated CELP speech codec, almost always carried in Ogg. symphonia has
//! no Speex decoder, and the pure-Rust `oxideav-speex` decodes incorrectly (vetted: NB
//! ~0.66, WB ~0.27 correlation against the reference), so this uses libspeex, the
//! reference decoder, built from vendored C source like our libopus. The `ogg` crate
//! demuxes the container and each audio packet is handed to libspeex, mirroring how Opus
//! is decoded.
//!
//! Narrowband (8 kHz), wideband (16 kHz) and ultra-wideband (32 kHz); mono and stereo.
//! Speex stereo is intensity-coded (mono frames plus in-band balance); for now the mono
//! frames are played on both channels (dual-mono), which is clean but not the original
//! stereo image. Output is interleaved f32 at the stream's native rate.

use std::path::Path;
use std::time::Duration;

use anyhow::{anyhow, Result};
use rodio::source::SeekError;
use rodio::Source;

/// True when `path` is a Speex file routed through this module.
pub fn is_speex(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| e.eq_ignore_ascii_case("spx"))
        .unwrap_or(false)
}

/// A seekable rodio `Source` over a fully-decoded Speex stream (interleaved f32).
pub struct SpeexSource {
    samples: Vec<f32>,
    channels: u16,
    sample_rate: u32,
    pos: usize,
    total: Option<Duration>,
}

impl Iterator for SpeexSource {
    type Item = f32;
    fn next(&mut self) -> Option<f32> {
        let s = self.samples.get(self.pos).copied();
        if s.is_some() {
            self.pos += 1;
        }
        s
    }
}

impl Source for SpeexSource {
    fn current_frame_len(&self) -> Option<usize> {
        None
    }
    fn channels(&self) -> u16 {
        self.channels
    }
    fn sample_rate(&self) -> u32 {
        self.sample_rate
    }
    fn total_duration(&self) -> Option<Duration> {
        self.total
    }
    fn try_seek(&mut self, pos: Duration) -> Result<(), SeekError> {
        let frame = (pos.as_secs_f64() * self.sample_rate as f64).round() as usize;
        self.pos = frame
            .saturating_mul(self.channels as usize)
            .min(self.samples.len());
        Ok(())
    }
}

/// The fields we need from the 80-byte Ogg/Speex header packet (fixed little-endian
/// layout, magic "Speex   " at offset 0).
struct Header {
    rate: u32,
    mode: u32,
    channels: u32,
    frames_per_packet: u32,
}

fn parse_header(buf: &[u8]) -> Result<Header> {
    if buf.len() < 68 || &buf[0..8] != b"Speex   " {
        return Err(anyhow!("not a Speex header"));
    }
    let u32le = |o: usize| u32::from_le_bytes([buf[o], buf[o + 1], buf[o + 2], buf[o + 3]]);
    Ok(Header {
        rate: u32le(36),
        mode: u32le(40),
        channels: u32le(48),
        frames_per_packet: u32le(64).max(1),
    })
}

/// Decode a Speex file fully and wrap it as a seekable source. Errors cleanly (unreadable
/// file, not Ogg/Speex, unknown mode, more than two channels) so the skip-unplayable path
/// takes over.
pub fn open_speex(path: &Path) -> Result<SpeexSource> {
    use speex_safe::{DynamicDecoder, ModeId, SpeexBits};

    let file = std::fs::File::open(path)?;
    let mut rd = ogg::PacketReader::new(std::io::BufReader::new(file));

    // Packet 0 is the Speex header, packet 1 the Vorbis-comment block, 2.. are audio.
    let hdr_pkt = rd
        .read_packet_expected()
        .map_err(|e| anyhow!("Speex {}: no header packet: {e}", path.display()))?;
    let header = parse_header(&hdr_pkt.data)?;
    if header.channels == 0 {
        return Err(anyhow!("Speex has no audio"));
    }
    if header.channels > 2 {
        return Err(anyhow!("Speex with more than 2 channels is not supported"));
    }
    if header.rate == 0 {
        return Err(anyhow!("Speex has an unknown rate"));
    }
    let mode = match header.mode {
        0 => ModeId::NarrowBand,
        1 => ModeId::WideBand,
        2 => ModeId::UltraWideBand,
        m => return Err(anyhow!("unsupported Speex mode {m}")),
    };
    let mut decoder = DynamicDecoder::new(mode);
    // Skip the comment packet.
    let _ = rd.read_packet();

    let mut mono: Vec<i16> = Vec::new();
    while let Ok(Some(packet)) = rd.read_packet() {
        if packet.data.is_empty() {
            continue;
        }
        let mut data = packet.data.clone();
        let mut bits = SpeexBits::new();
        bits.read_from(&mut data);
        // A packet can carry several frames; decode until the bits run out.
        for _ in 0..header.frames_per_packet {
            match decoder.decode_int_to_owned(&mut bits) {
                Ok(frame) => mono.extend_from_slice(&frame),
                Err(_) => break,
            }
        }
    }
    if mono.is_empty() {
        return Err(anyhow!("decoded no audio from {}", path.display()));
    }

    // To f32, expanding to the native channel count (stereo = dual-mono for now).
    let channels = header.channels as usize;
    let mut samples = Vec::with_capacity(mono.len() * channels);
    for &s in &mono {
        let v = s as f32 / 32_768.0;
        for _ in 0..channels {
            samples.push(v);
        }
    }
    let frames = mono.len();
    let total = (frames > 0).then(|| Duration::from_secs_f64(frames as f64 / header.rate as f64));
    Ok(SpeexSource {
        samples,
        channels: header.channels as u16,
        sample_rate: header.rate,
        pos: 0,
        total,
    })
}

/// The length of a Speex file, for the playlist, from the Ogg granule positions (no
/// decode). `None` when it isn't a Speex file or the length can't be determined.
pub fn duration(path: &Path) -> Option<Duration> {
    if !is_speex(path) {
        return None;
    }
    let file = std::fs::File::open(path).ok()?;
    let mut rd = ogg::PacketReader::new(std::io::BufReader::new(file));
    let hdr = rd.read_packet_expected().ok()?;
    let header = parse_header(&hdr.data).ok()?;
    if header.rate == 0 {
        return None;
    }
    // The last page's granule position is the total decoded sample count per channel.
    let mut last_granule = 0u64;
    while let Ok(Some(p)) = rd.read_packet() {
        last_granule = last_granule.max(p.absgp_page());
    }
    if last_granule == 0 {
        return None;
    }
    Some(Duration::from_secs_f64(last_granule as f64 / header.rate as f64))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn corr(a: &[f32], b: &[f32]) -> f64 {
        let n = a.len().min(b.len());
        if n < 2000 {
            return 0.0;
        }
        let (a, b) = (&a[..n], &b[..n]);
        let ma = a.iter().map(|&x| x as f64).sum::<f64>() / n as f64;
        let mb = b.iter().map(|&x| x as f64).sum::<f64>() / n as f64;
        let (mut num, mut da, mut db) = (0.0, 0.0, 0.0);
        for i in 0..n {
            let (xa, xb) = (a[i] as f64 - ma, b[i] as f64 - mb);
            num += xa * xb;
            da += xa * xa;
            db += xb * xb;
        }
        if da <= 0.0 || db <= 0.0 {
            return 0.0;
        }
        num / (da.sqrt() * db.sqrt())
    }

    fn best_corr(ours: &[f32], oracle: &[f32]) -> f64 {
        let mut best = 0.0;
        for lag in 0..400usize {
            if lag >= ours.len() {
                break;
            }
            let c = corr(&ours[lag..], oracle);
            if c > best {
                best = c;
            }
        }
        best
    }

    fn oracle_mono(path: &str) -> Vec<f32> {
        let raw = std::fs::read(path).unwrap();
        raw.chunks_exact(2)
            .map(|b| i16::from_le_bytes([b[0], b[1]]) as f32 / 32_768.0)
            .collect()
    }

    /// Mono NB / WB must decode to within a tight correlation of the libspeex oracle
    /// (ffmpeg -c:a libspeex), since both are libspeex. (qa-music is gitignored; skips CI.)
    #[test]
    fn decodes_nb_wb_against_the_libspeex_oracle() {
        for (spx, oracle) in [
            ("qa-music/format-samples/speex-nb.spx", "qa-music/format-samples/speex-nb-oracle.s16"),
            ("qa-music/format-samples/speex-wb.spx", "qa-music/format-samples/speex-wb-oracle.s16"),
        ] {
            if !std::path::Path::new(spx).exists() || !std::path::Path::new(oracle).exists() {
                continue;
            }
            let src = open_speex(std::path::Path::new(spx)).unwrap();
            assert_eq!(src.channels(), 1, "{spx} should be mono");
            let got: Vec<f32> = src.collect();
            let want = oracle_mono(oracle);
            let c = best_corr(&got, &want);
            assert!(c > 0.98, "{spx}: correlation vs libspeex oracle too low: {c:.3}");
        }
    }

    /// Stereo UWB decodes and recovers the content: compare the mono-summed output to the
    /// mono-summed oracle (our dual-mono won't match the oracle's stereo image exactly,
    /// but the content must correlate).
    #[test]
    fn decodes_stereo_uwb_content() {
        let spx = "qa-music/format-samples/hypno-this.spx";
        let oracle = "qa-music/format-samples/speex-uwb-oracle.s16";
        if !std::path::Path::new(spx).exists() || !std::path::Path::new(oracle).exists() {
            return;
        }
        let src = open_speex(std::path::Path::new(spx)).unwrap();
        assert_eq!(src.channels(), 2);
        assert_eq!(src.sample_rate(), 32_000);
        let got: Vec<f32> = src.collect();
        // Mono-sum both (step by 2 channels).
        let ours: Vec<f32> = got.chunks_exact(2).map(|c| c[0] + c[1]).collect();
        let raw = std::fs::read(oracle).unwrap();
        let want: Vec<f32> = raw
            .chunks_exact(4)
            .map(|c| {
                let l = i16::from_le_bytes([c[0], c[1]]) as f32 / 32_768.0;
                let r = i16::from_le_bytes([c[2], c[3]]) as f32 / 32_768.0;
                l + r
            })
            .collect();
        let c = best_corr(&ours, &want);
        assert!(c > 0.95, "UWB content correlation too low: {c:.3}");
    }

    #[test]
    fn duration_reads_without_full_decode() {
        let spx = std::path::Path::new("qa-music/format-samples/speex-wb.spx");
        if !spx.exists() {
            return;
        }
        let d = duration(spx).expect("speex duration");
        assert!(d.as_secs() > 1 && d.as_secs() < 600, "duration {d:?}");
    }
}
