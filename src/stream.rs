//! Streaming playback of an HTTP(S) "location" (internet radio or a remote
//! file), for the classic Winamp "Open URL" (Ctrl+L) feature.
//!
//! rodio's `Decoder` needs a `Read + Seek` source, which a live stream isn't, so
//! we decode with symphonia directly: a background thread fetches the URL
//! ([`ureq`]), wraps it as a non-seekable `ReadOnlySource`, probes the format,
//! and decodes forward-only, pushing interleaved f32 samples through a bounded
//! channel. [`StreamSource`] is a rodio `Source` that plays those samples, so
//! the existing EQ / visualization / balance pipeline applies unchanged.
//!
//! Dropping the `StreamSource` (sink stopped) closes the receiver, which makes
//! the decode thread's `send` fail and the thread exit — so it is self-cleaning.

use std::io::Read;
use std::sync::mpsc::{sync_channel, Receiver, SyncSender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{anyhow, Result};
use rodio::Source;

/// ~1 second of stereo audio buffered between the decode thread and the audio
/// output, giving the network/decoder slack without unbounded memory growth.
const CHANNEL_SAMPLES: usize = 48_000 * 2;

/// The current ICY stream title (SHOUTcast/Icecast "now playing"), shared from
/// the decode thread to the UI. `None` when the stream carries no metadata.
pub type SharedTitle = Arc<Mutex<Option<String>>>;

/// A rodio source fed by the background decode thread.
pub struct StreamSource {
    rx: Receiver<f32>,
    sample_rate: u32,
    channels: u16,
}

impl Iterator for StreamSource {
    type Item = f32;

    fn next(&mut self) -> Option<f32> {
        // Blocks until the next sample arrives; returns None (ending the source)
        // once the decode thread drops the sender (stream ended or errored).
        self.rx.recv().ok()
    }
}

impl Source for StreamSource {
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
        None // a live stream has no known length
    }
}

/// Open `url` and start decoding it on a background thread. Returns a rodio
/// source once the format has been probed (so sample rate / channels are known),
/// plus a shared handle to the live ICY stream title; or an error if the
/// connection or probe failed.
pub fn open(url: &str) -> Result<(StreamSource, SharedTitle)> {
    let url = url.to_string();
    // Carries the probed format info (or an early error) back to this call.
    let (info_tx, info_rx) = sync_channel::<Result<(u32, u16), String>>(1);
    let (sample_tx, sample_rx) = sync_channel::<f32>(CHANNEL_SAMPLES);
    let title: SharedTitle = Arc::new(Mutex::new(None));
    let title_thread = title.clone();

    std::thread::Builder::new()
        .name("linamp-stream".into())
        .spawn(move || decode_loop(&url, info_tx, sample_tx, title_thread))
        .map_err(|e| anyhow!("couldn't start stream thread: {e}"))?;

    match info_rx.recv() {
        Ok(Ok((sample_rate, channels))) => Ok((
            StreamSource {
                rx: sample_rx,
                sample_rate,
                channels,
            },
            title,
        )),
        Ok(Err(e)) => Err(anyhow!(e)),
        Err(_) => Err(anyhow!("stream closed before it started")),
    }
}

/// A `Read` adapter that strips ICY (SHOUTcast/Icecast) metadata blocks from an
/// audio stream, exposing only the audio bytes and publishing the latest
/// `StreamTitle` to a shared handle. Metadata arrives every `metaint` audio
/// bytes as a length byte (×16) followed by that many bytes of text.
struct IcyReader<R> {
    inner: R,
    metaint: usize,
    until_meta: usize,
    title: SharedTitle,
}

impl<R: Read> IcyReader<R> {
    fn new(inner: R, metaint: usize, title: SharedTitle) -> Self {
        Self {
            inner,
            metaint,
            until_meta: metaint,
            title,
        }
    }
}

impl<R: Read> Read for IcyReader<R> {
    fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
        if self.until_meta == 0 {
            let mut len_byte = [0u8; 1];
            // A clean EOF right on a metadata boundary ends the stream.
            if self.inner.read(&mut len_byte)? == 0 {
                return Ok(0);
            }
            let len = len_byte[0] as usize * 16;
            if len > 0 {
                let mut meta = vec![0u8; len];
                self.inner.read_exact(&mut meta)?;
                if let Some(t) = parse_stream_title(&meta) {
                    if let Ok(mut guard) = self.title.lock() {
                        *guard = Some(t);
                    }
                }
            }
            self.until_meta = self.metaint;
        }
        let want = out.len().min(self.until_meta);
        let n = self.inner.read(&mut out[..want])?;
        self.until_meta -= n;
        Ok(n)
    }
}

/// Extract the song title from an ICY metadata block (`StreamTitle='...';`).
fn parse_stream_title(meta: &[u8]) -> Option<String> {
    let s = String::from_utf8_lossy(meta);
    let start = s.find("StreamTitle='")? + "StreamTitle='".len();
    let rest = &s[start..];
    let end = rest.find("';")?;
    let title = rest[..end].trim().trim_matches('\0').trim();
    if title.is_empty() {
        None
    } else {
        Some(title.to_string())
    }
}

/// Fetch, probe and decode the stream, reporting the format once known and then
/// streaming samples until the receiver goes away or the stream ends.
fn decode_loop(
    url: &str,
    info_tx: SyncSender<Result<(u32, u16), String>>,
    sample_tx: SyncSender<f32>,
    title: SharedTitle,
) {
    use symphonia::core::audio::SampleBuffer;
    use symphonia::core::codecs::DecoderOptions;
    use symphonia::core::formats::FormatOptions;
    use symphonia::core::io::{MediaSourceStream, ReadOnlySource};
    use symphonia::core::meta::MetadataOptions;
    use symphonia::core::probe::Hint;

    // Connect (blocking) on this background thread so the UI never stalls. Ask
    // for ICY metadata so radio streams can report their current song.
    let resp = match ureq::get(url).set("Icy-MetaData", "1").call() {
        Ok(r) => r,
        Err(e) => {
            let _ = info_tx.send(Err(format!("couldn't open {url}: {e}")));
            return;
        }
    };
    // A content-type hint helps symphonia pick the right demuxer for streams
    // whose URL has no file extension (e.g. "audio/mpeg").
    let mut hint = Hint::new();
    if let Some(ct) = resp.header("Content-Type") {
        if let Some(ext) = mime_to_ext(ct) {
            hint.with_extension(ext);
        }
    }

    // If the server interleaves ICY metadata, strip it out before decoding.
    let metaint = resp
        .header("icy-metaint")
        .and_then(|v| v.parse::<usize>().ok())
        .filter(|&n| n > 0);
    let reader: Box<dyn Read + Send + Sync> = match metaint {
        Some(mi) => Box::new(IcyReader::new(resp.into_reader(), mi, title)),
        None => resp.into_reader(),
    };
    let mss = MediaSourceStream::new(Box::new(ReadOnlySource::new(reader)), Default::default());

    let probed = match symphonia::default::get_probe().format(
        &hint,
        mss,
        &FormatOptions::default(),
        &MetadataOptions::default(),
    ) {
        Ok(p) => p,
        Err(e) => {
            let _ = info_tx.send(Err(format!("unsupported stream: {e}")));
            return;
        }
    };
    let mut format = probed.format;
    let track = match format.default_track() {
        Some(t) => t.clone(),
        None => {
            let _ = info_tx.send(Err("stream has no audio track".into()));
            return;
        }
    };
    let track_id = track.id;
    let mut decoder =
        match symphonia::default::get_codecs().make(&track.codec_params, &DecoderOptions::default())
        {
            Ok(d) => d,
            Err(e) => {
                let _ = info_tx.send(Err(format!("no decoder for stream: {e}")));
                return;
            }
        };

    // Sample rate / channels are taken from the first decoded packet's spec and
    // reported to the caller before any samples flow.
    let mut announced = false;
    let mut sbuf: Option<SampleBuffer<f32>> = None;

    loop {
        let packet = match format.next_packet() {
            Ok(p) => p,
            Err(_) => break, // end of stream or read error
        };
        if packet.track_id() != track_id {
            continue;
        }
        let decoded = match decoder.decode(&packet) {
            Ok(d) => d,
            // A single bad packet in a live stream shouldn't kill playback.
            Err(symphonia::core::errors::Error::DecodeError(_)) => continue,
            Err(_) => break,
        };

        let spec = *decoded.spec();
        if sbuf.is_none() {
            let sample_rate = spec.rate;
            let channels = spec.channels.count() as u16;
            if info_tx.send(Ok((sample_rate, channels))).is_err() {
                return; // caller gave up already
            }
            announced = true;
            sbuf = Some(SampleBuffer::<f32>::new(decoded.capacity() as u64, spec));
        }
        let sbuf = sbuf.as_mut().unwrap();
        sbuf.copy_interleaved_ref(decoded);
        for &s in sbuf.samples() {
            // Blocks when the buffer is full (backpressure); fails once the
            // audio side drops the receiver, which is our cue to stop.
            if sample_tx.send(s).is_err() {
                return;
            }
        }
    }

    // Stream ended before (or without) producing audio: make sure the caller
    // isn't left waiting on the handshake.
    if !announced {
        let _ = info_tx.send(Err("stream ended before any audio".into()));
    }
}

/// Map a Content-Type to a container/codec extension symphonia recognises.
fn mime_to_ext(content_type: &str) -> Option<&'static str> {
    let ct = content_type.split(';').next().unwrap_or("").trim().to_ascii_lowercase();
    match ct.as_str() {
        "audio/mpeg" | "audio/mp3" => Some("mp3"),
        "audio/aac" | "audio/aacp" => Some("aac"),
        "audio/mp4" | "audio/m4a" | "audio/x-m4a" => Some("m4a"),
        "audio/ogg" | "application/ogg" => Some("ogg"),
        "audio/flac" | "audio/x-flac" => Some("flac"),
        "audio/wav" | "audio/x-wav" | "audio/wave" => Some("wav"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Cursor, Read};

    #[test]
    fn parse_stream_title_extracts_song() {
        let meta = b"StreamTitle='Artist - Song';StreamUrl='http://x';\0\0\0";
        assert_eq!(parse_stream_title(meta).as_deref(), Some("Artist - Song"));
        // Empty title yields None.
        assert_eq!(parse_stream_title(b"StreamTitle='';\0"), None);
        // No StreamTitle field yields None.
        assert_eq!(parse_stream_title(b"StreamUrl='x';"), None);
    }

    #[test]
    fn icy_reader_strips_metadata_and_captures_title() {
        // metaint = 4: 4 audio bytes, then a 1-byte length (×16) + that many meta
        // bytes, then audio again.
        let title_text = b"StreamTitle='Now Playing';";
        let blocks = (title_text.len() + 15) / 16; // length byte is in 16-byte units
        let mut meta = vec![0u8; blocks * 16];
        meta[..title_text.len()].copy_from_slice(title_text);
        let mut raw = Vec::new();
        raw.extend_from_slice(b"AAAA"); // audio
        raw.push(blocks as u8); // meta length byte
        raw.extend_from_slice(&meta); // meta block
        raw.extend_from_slice(b"BBBB"); // audio

        let title: SharedTitle = Arc::new(Mutex::new(None));
        let mut r = IcyReader::new(Cursor::new(raw), 4, title.clone());
        let mut audio = Vec::new();
        r.read_to_end(&mut audio).unwrap();
        // Only the audio bytes come through; the metadata block is stripped.
        assert_eq!(audio, b"AAAABBBB");
        assert_eq!(title.lock().unwrap().as_deref(), Some("Now Playing"));
    }

    #[test]
    fn mime_maps_common_stream_types() {
        assert_eq!(mime_to_ext("audio/mpeg"), Some("mp3"));
        // Parameters and case are ignored.
        assert_eq!(mime_to_ext("audio/MPEG; charset=utf-8"), Some("mp3"));
        assert_eq!(mime_to_ext("application/ogg"), Some("ogg"));
        assert_eq!(mime_to_ext("audio/x-flac"), Some("flac"));
        assert_eq!(mime_to_ext("text/html"), None);
    }
}
