use std::fs::File;
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use rodio::{Decoder, OutputStream, OutputStreamHandle, Sink, Source};

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
    current_path: Option<PathBuf>,
    viz: Arc<VizShared>,
    eq: Arc<EqShared>,
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
            current_path: None,
            viz: VizShared::new(),
            eq: EqShared::new(),
            device_name: Self::default_device_name(),
        })
    }

    fn default_device_name() -> Option<String> {
        use rodio::cpal::traits::{DeviceTrait, HostTrait};
        rodio::cpal::default_host()
            .default_output_device()
            .and_then(|d| d.name().ok())
    }

    /// If the system default output device changed (e.g. headphones plugged in),
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
            if let Some(p) = path {
                if matches!(was, PlayState::Playing | PlayState::Paused) {
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

    pub fn state(&self) -> PlayState {
        self.state
    }

    pub fn volume(&self) -> f32 {
        self.volume
    }

    pub fn set_volume(&mut self, v: f32) {
        self.volume = v.clamp(0.0, 1.0);
        self.apply_gain();
    }

    fn apply_gain(&self) {
        if let Some(sink) = &self.sink {
            // Preamp now lives in the EQ DSP; the sink carries plain volume.
            sink.set_volume(self.volume);
        }
    }

    /// Load and immediately start playing a file.
    pub fn play_file(&mut self, path: &Path) -> Result<()> {
        let file = File::open(path)?;
        let source = Decoder::new(BufReader::new(file))?.convert_samples::<f32>();

        self.viz.clear();
        self.viz.set_rate(source.sample_rate());
        // Shape the audio with the EQ, then tap for visualization so the
        // analyzer reflects what you actually hear.
        let equalized = eq::eq(source, self.eq.clone());
        let tapped = viz::tap(equalized, self.viz.clone());

        let sink = Sink::try_new(&self.handle)?;
        sink.append(tapped);
        self.sink = Some(sink);
        self.current_path = Some(path.to_path_buf());
        self.state = PlayState::Playing;
        self.apply_gain();
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
