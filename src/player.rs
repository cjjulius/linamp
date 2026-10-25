use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use rodio::{OutputStream, OutputStreamHandle, Sink, Source};

use crate::balance::{self, BalanceShared};
use crate::eq::{self, EqShared};
use crate::viz::{self, VizShared};

/// High-level playback state the UI cares about.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PlayState {
    Stopped,
    Playing,
    Paused,
}

/// Wraps the rodio output stream + sink and exposes a simple transport API.
///
/// `OutputStream` is not `Send`, which is fine: the whole player lives on the
/// UI thread and egui drives it from `update()`.
pub struct Player {
    _stream: OutputStream,
    handle: OutputStreamHandle,
    sink: Option<Sink>,
    state: PlayState,
    volume: f32,
    /// Extra output gain multiplier for fades (1.0 = normal).
    fade: f32,
    current_path: Option<PathBuf>,
    /// The streaming URL currently playing (classic "Open location"), if any.
    current_url: Option<String>,
    /// Live ICY stream title for the current stream (updated by its decode thread).
    stream_title: Option<crate::stream::SharedTitle>,
    viz: Arc<VizShared>,
    eq: Arc<EqShared>,
    balance: Arc<BalanceShared>,
    device_name: Option<String>,
}

impl Player {
    pub fn new() -> Result<Self> {
        let (stream, handle) = OutputStream::try_default()?;
        Ok(Self {
            _stream: stream,
            handle,
            sink: None,
            state: PlayState::Stopped,
            volume: 0.8,
            fade: 1.0,
            current_path: None,
            current_url: None,
            stream_title: None,
            viz: VizShared::new(),
            eq: EqShared::new(),
            balance: BalanceShared::new(),
            device_name: Self::default_device_name(),
        })
    }

    fn default_device_name() -> Option<String> {
        use rodio::cpal::traits::{DeviceTrait, HostTrait};
        rodio::cpal::default_host()
            .default_output_device()
            .and_then(|d| d.name().ok())
    }

    /// If the system default output device changed (example: headphones plugged in),
    /// rebuild the output stream on the new device and resume playback in place.
    pub fn poll_device(&mut self) {
        let current = Self::default_device_name();
        if current.is_none() || current == self.device_name {
            return;
        }
        let path = self.current_path.clone();
        let pos = self.position();
        let was = self.state;
        if let Ok((stream, handle)) = OutputStream::try_default() {
            self._stream = stream;
            self.handle = handle;
            self.device_name = current;
            self.sink = None;
            self.state = PlayState::Stopped;
            // A stream can't be resumed in place on a device change; it simply
            // stops (re-open it with Ctrl+L). Only local files resume.
            self.current_url = None;
            self.stream_title = None;
            if matches!(was, PlayState::Playing | PlayState::Paused) {
                if let Some(p) = path {
                    if self.play_file(&p).is_ok() {
                        self.seek(pos);
                        if was == PlayState::Paused {
                            self.pause();
                        }
                    }
                }
            }
        }
    }

    /// Shared handle to the live-sample buffer used for visualization.
    pub fn viz(&self) -> Arc<VizShared> {
        self.viz.clone()
    }

    /// Shared handle to the live equalizer settings.
    pub fn eq(&self) -> Arc<EqShared> {
        self.eq.clone()
    }

    /// Set the stereo balance (−1 = left, 0 = centre, +1 = right).
    pub fn set_balance(&self, b: f32) {
        self.balance.set(b);
    }

    pub fn state(&self) -> PlayState {
        self.state
    }

    /// The URL currently streaming (classic "Open location"), if any.
    pub fn current_url(&self) -> Option<&str> {
        self.current_url.as_deref()
    }

    /// The live ICY stream title ("now playing") for the current stream, if the
    /// stream reports one.
    pub fn stream_title(&self) -> Option<String> {
        self.stream_title
            .as_ref()
            .and_then(|t| t.lock().ok().and_then(|g| g.clone()))
    }

    pub fn volume(&self) -> f32 {
        self.volume
    }

    pub fn set_volume(&mut self, v: f32) {
        self.volume = v.clamp(0.0, 1.0);
        self.apply_gain();
    }

    /// Set the fade multiplier (1.0 = normal) applied on top of the volume.
    pub fn set_fade(&mut self, f: f32) {
        self.fade = f.clamp(0.0, 1.0);
        self.apply_gain();
    }

    fn apply_gain(&self) {
        if let Some(sink) = &self.sink {
            // Preamp now lives in the EQ DSP; the sink carries volume × fade.
            sink.set_volume(self.volume * self.fade);
        }
    }

    /// Shape `source` through the EQ, the analyzer tap and the stereo balance, then
    /// start it on a fresh sink. Shared by file and stream playback so both apply
    /// the same pipeline (and so a new decode path only has to produce a `Source`).
    fn append_source<S>(&mut self, source: S) -> Result<()>
    where
        S: Source<Item = f32> + Send + 'static,
    {
        self.viz.clear();
        self.viz.set_rate(source.sample_rate());
        // Shape with the EQ, then tap for visualization so the analyzer reflects
        // what you hear, and apply stereo balance last.
        let equalized = eq::eq(source, self.eq.clone());
        let tapped = viz::tap(equalized, self.viz.clone());
        let balanced = balance::balance(tapped, self.balance.clone());

        let sink = Sink::try_new(&self.handle)?;
        sink.append(balanced);
        self.sink = Some(sink);
        self.apply_gain();
        Ok(())
    }

    /// Load and immediately start playing a file.
    pub fn play_file(&mut self, path: &Path) -> Result<()> {
        // MIDI is synthesized (rustysynth + a bundled SoundFont), not decoded (#159);
        // everything else goes through the single symphonia decode path (#163). Both
        // produce a rodio `Source` fed into the shared EQ / analyzer / balance chain.
        let is_midi = path
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| matches!(e.to_ascii_lowercase().as_str(), "mid" | "midi"))
            .unwrap_or(false);
        if is_midi {
            let sf = crate::midi::soundfont_path().ok_or_else(|| {
                anyhow::anyhow!("MIDI playback needs a SoundFont (.sf2) in the soundfont/ folder")
            })?;
            let source = crate::midi::open_midi(path, &sf)?;
            self.append_source(source)?;
        } else if crate::tracker::is_tracker(path) {
            // Tracker modules (.mod/.xm/.s3m/.it) are rendered by xmrs (#158).
            let source = crate::tracker::open_tracker(path)?;
            self.append_source(source)?;
        } else {
            let source = crate::decode::open_file(path)?;
            self.append_source(source)?;
        }
        self.current_path = Some(path.to_path_buf());
        self.current_url = None;
        self.stream_title = None;
        self.state = PlayState::Playing;
        Ok(())
    }

    /// Start playing an already-opened stream source (see [`crate::stream`]). The
    /// connection and decode happen on a background thread; the UI opens the
    /// stream without blocking and calls this once the format is known.
    pub fn play_stream(
        &mut self,
        source: crate::stream::StreamSource,
        title: crate::stream::SharedTitle,
        url: String,
    ) -> Result<()> {
        self.stream_title = Some(title);
        self.append_source(source)?;
        self.current_path = None;
        self.current_url = Some(url);
        self.state = PlayState::Playing;
        Ok(())
    }

    pub fn pause(&mut self) {
        if let Some(sink) = &self.sink {
            if self.state == PlayState::Playing {
                sink.pause();
                self.state = PlayState::Paused;
            }
        }
    }

    pub fn resume(&mut self) {
        if let Some(sink) = &self.sink {
            if self.state == PlayState::Paused {
                sink.play();
                self.state = PlayState::Playing;
            }
        }
    }

    pub fn toggle_pause(&mut self) {
        match self.state {
            PlayState::Playing => self.pause(),
            PlayState::Paused => self.resume(),
            PlayState::Stopped => {}
        }
    }

    pub fn stop(&mut self) {
        if let Some(sink) = self.sink.take() {
            sink.stop();
        }
        self.state = PlayState::Stopped;
        self.current_url = None;
        self.stream_title = None;
        self.fade = 1.0; // reset so the next track isn't faded
        self.viz.clear(); // silence the analyzer so its bars fall to zero
    }

    /// Current playback position within the track.
    pub fn position(&self) -> Duration {
        self.sink
            .as_ref()
            .map(|s| s.get_pos())
            .unwrap_or_default()
    }

    /// Seek to an absolute position. Not all formats support seeking.
    pub fn seek(&mut self, pos: Duration) {
        if let Some(sink) = &self.sink {
            let _ = sink.try_seek(pos);
        }
    }

    /// True once the current track has played out (sink drained) while we still
    /// think we're playing. Used by the UI to auto-advance.
    pub fn finished(&self) -> bool {
        matches!(self.state, PlayState::Playing | PlayState::Paused)
            && self.sink.as_ref().map(|s| s.empty()).unwrap_or(false)
    }
}
