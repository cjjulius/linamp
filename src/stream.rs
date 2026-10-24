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

use std::sync::mpsc::{sync_channel, Receiver, SyncSender};
use std::time::Duration;

use anyhow::{anyhow, Result};
use rodio::Source;

/// ~1 second of stereo audio buffered between the decode thread and the audio
/// output, giving the network/decoder slack without unbounded memory growth.
const CHANNEL_SAMPLES: usize = 48_000 * 2;

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
/// or an error if the connection or probe failed.
pub fn open(url: &str) -> Result<StreamSource> {
    let url = url.to_string();
    // Carries the probed format info (or an early error) back to this call.
    let (info_tx, info_rx) = sync_channel::<Result<(u32, u16), String>>(1);
    let (sample_tx, sample_rx) = sync_channel::<f32>(CHANNEL_SAMPLES);

    std::thread::Builder::new()
        .name("linamp-stream".into())
        .spawn(move || decode_loop(&url, info_tx, sample_tx))
        .map_err(|e| anyhow!("couldn't start stream thread: {e}"))?;

    match info_rx.recv() {
        Ok(Ok((sample_rate, channels))) => Ok(StreamSource {
            rx: sample_rx,
            sample_rate,
            channels,
        }),
        Ok(Err(e)) => Err(anyhow!(e)),
        Err(_) => Err(anyhow!("stream closed before it started")),
    }
}

/// Fetch, probe and decode the stream, reporting the format once known and then
/// streaming samples until the receiver goes away or the stream ends.
fn decode_loop(
    url: &str,
    info_tx: SyncSender<Result<(u32, u16), String>>,
    sample_tx: SyncSender<f32>,
) {
    use symphonia::core::audio::SampleBuffer;
    use symphonia::core::codecs::DecoderOptions;
    use symphonia::core::formats::FormatOptions;
    use symphonia::core::io::{MediaSourceStream, ReadOnlySource};
    use symphonia::core::meta::MetadataOptions;
    use symphonia::core::probe::Hint;

    // Connect (blocking) on this background thread so the UI never stalls.
    let resp = match ureq::get(url).call() {
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

    let reader = resp.into_reader();
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
    use super::mime_to_ext;

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
