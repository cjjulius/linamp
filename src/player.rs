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
    /// Crossfade length in seconds; 0 disables it (then track changes cut over as
    /// before). Set from Preferences.
    crossfade_secs: f32,
    /// The outgoing track during a crossfade, still playing on its own sink while it
    /// fades out. Dropped (stopped) once its gain reaches zero.
    fading: Option<Sink>,
    /// Outgoing gain multiplier (1 to 0) for `fading`.
    fade_out: f32,
    /// Incoming gain multiplier (0 to 1) for the current sink; 1.0 when not fading in.
    fade_in: f32,
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
            crossfade_secs: 0.0,
            fading: None,
            fade_out: 1.0,
            fade_in: 1.0,
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
            // Preamp now lives in the EQ DSP; the sink carries volume × fade, times
            // the crossfade fade-in while a new track is ramping up.
            sink.set_volume(self.volume * self.fade * self.fade_in);
        }
        if let Some(f) = &self.fading {
            f.set_volume(self.volume * self.fade * self.fade_out);
        }
    }

    /// Crossfade length in seconds (0 = disabled). Set from Preferences.
    pub fn set_crossfade(&mut self, secs: f32) {
        self.crossfade_secs = secs.max(0.0);
    }

    pub fn crossfade_secs(&self) -> f32 {
        self.crossfade_secs
    }

    /// True while an outgoing track is still fading out under a crossfade.
    pub fn is_crossfading(&self) -> bool {
        self.fading.is_some()
    }

    /// Advance an in-progress crossfade by `dt` seconds: ramp the incoming track up
    /// and the outgoing one down, dropping the outgoing sink once it's silent. A
    /// no-op when crossfade is off or nothing is fading.
    pub fn tick_crossfade(&mut self, dt: f32) {
        // Only advance while actually playing: a paused crossfade must hold both
        // sinks and both gains where they are, not ramp on (and silently drop the
        // outgoing track) during the pause.
        let (fade_in, fade_out, still_fading, changed) = crossfade_step(
            self.state == PlayState::Playing,
            self.crossfade_secs,
            dt,
            self.fade_in,
            self.fade_out,
            self.fading.is_some(),
        );
        self.fade_in = fade_in;
        self.fade_out = fade_out;
        if !still_fading {
            self.fading = None; // drop = stop the outgoing track once silent
        }
        if changed {
            self.apply_gain();
        }
    }

    /// Shape `source` through the EQ, the analyzer tap and the stereo balance, then
    /// start it on a fresh sink. Shared by file and stream playback so both apply
    /// the same pipeline (and so a new decode path only has to produce a `Source`).
    fn append_source<S>(&mut self, source: S, allow_crossfade: bool) -> Result<()>
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
        // Crossfade: when enabled and a track is already playing, keep the outgoing
        // one on its own sink fading out while the new one fades in, instead of
        // cutting it off. Otherwise (disabled, nothing playing, or this source opts
        // out) swap as before. A CD track opts out: it needs a couple of seconds to
        // spin up, so a crossfade would blend against silence and stutter (#160).
        if allow_crossfade
            && self.crossfade_secs > 0.0
            && self.state == PlayState::Playing
            && self.sink.is_some()
        {
            // At most one outgoing track; if one was already fading, carry its gain so
            // a rapid skip doesn't jump back to full volume.
            self.fade_out = self.fade_in;
            self.fading = self.sink.take();
            self.fade_in = 0.0;
        } else {
            self.fade_in = 1.0;
        }
        self.sink = Some(sink);
        self.apply_gain();
        Ok(())
    }

    /// Load and immediately start playing a file.
    pub fn play_file(&mut self, path: &Path) -> Result<()> {
        // Audio-CD tracks are a `cdda://<device>/<track>` pseudo-path, not a file:
        // read them straight off the drive on a background thread (#160). This is a
        // no-op error on non-Linux builds (handled by skip-unplayable), so the rest
        // of the dispatch is unchanged.
        if let Some(s) = path.to_str() {
            if crate::cdda::is_cdda_path(s) {
                let (device, track) = crate::cdda::parse_track_path(s)
                    .ok_or_else(|| anyhow::anyhow!("bad CD track path: {s}"))?;
                let source = crate::cdda::open_cd_source(&device, track)?;
                self.append_source(source, false)?;
                self.current_path = Some(path.to_path_buf());
                self.current_url = None;
                self.stream_title = None;
                self.state = PlayState::Playing;
                return Ok(());
            }
        }

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
            self.append_source(source, true)?;
        } else if crate::tracker::is_tracker(path) {
            // Tracker modules (.mod/.xm/.s3m/.it) are rendered by xmrs (#158).
            let source = crate::tracker::open_tracker(path)?;
            self.append_source(source, true)?;
        } else {
            let source = crate::decode::open_file(path)?;
            self.append_source(source, true)?;
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
        self.append_source(source, true)?;
        self.current_path = None;
        self.current_url = Some(url);
        self.state = PlayState::Playing;
        Ok(())
    }

    pub fn pause(&mut self) {
        if self.state == PlayState::Playing {
            if let Some(sink) = &self.sink {
                sink.pause();
            }
            // Freeze the outgoing track too, or a pause mid-crossfade would leave
            // it playing on while the incoming one is held.
            if let Some(f) = &self.fading {
                f.pause();
            }
            self.state = PlayState::Paused;
        }
    }

    pub fn resume(&mut self) {
        if self.state == PlayState::Paused {
            if let Some(sink) = &self.sink {
                sink.play();
            }
            if let Some(f) = &self.fading {
                f.play();
            }
            self.state = PlayState::Playing;
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
        if let Some(f) = self.fading.take() {
            f.stop(); // also stop any outgoing crossfade track
        }
        self.state = PlayState::Stopped;
        self.current_url = None;
        self.stream_title = None;
        self.fade = 1.0; // reset so the next track isn't faded
        self.fade_in = 1.0;
        self.fade_out = 1.0;
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

/// Pure ramp step for one crossfade tick. Given the fade length and the `dt`
/// elapsed since the last tick, advance the incoming gain toward 1 and (while a
/// track is fading out) the outgoing gain toward 0. Returns the new incoming and
/// outgoing gains, whether the outgoing track should keep playing, and whether
/// anything moved. Split out from `tick_crossfade` so the ramp math can be tested
/// without an audio device (a real `Player` needs an output stream).
fn crossfade_step(
    playing: bool,
    crossfade_secs: f32,
    dt: f32,
    fade_in: f32,
    fade_out: f32,
    fading: bool,
) -> (f32, f32, bool, bool) {
    if !playing || crossfade_secs <= 0.0 {
        return (fade_in, fade_out, fading, false);
    }
    let step = dt / crossfade_secs;
    let mut new_in = fade_in;
    let mut new_out = fade_out;
    let mut still_fading = fading;
    let mut changed = false;
    if new_in < 1.0 {
        new_in = (new_in + step).min(1.0);
        changed = true;
    }
    if fading {
        new_out -= step;
        if new_out <= 0.0 {
            still_fading = false; // outgoing track is silent: caller drops it
            new_out = 1.0;
        }
        changed = true;
    }
    (new_in, new_out, still_fading, changed)
}

#[cfg(test)]
mod tests {
    use super::crossfade_step;

    #[test]
    fn crossfade_off_is_a_noop() {
        // Zero length means crossfade disabled: gains and state are untouched.
        let (fin, fout, fading, changed) = crossfade_step(true, 0.0, 0.5, 0.3, 0.7, true);
        assert_eq!((fin, fout, fading, changed), (0.3, 0.7, true, false));
    }

    #[test]
    fn crossfade_ramps_in_and_out_over_its_length() {
        // Half a 2s crossfade elapses in one 1s tick: incoming 0 -> 0.5, outgoing
        // 1 -> 0.5, and the outgoing track is still playing.
        let (fin, fout, fading, changed) = crossfade_step(true, 2.0, 1.0, 0.0, 1.0, true);
        assert!((fin - 0.5).abs() < 1e-6, "fade_in {fin}");
        assert!((fout - 0.5).abs() < 1e-6, "fade_out {fout}");
        assert!(fading, "outgoing track should still be fading");
        assert!(changed);
    }

    #[test]
    fn crossfade_drops_the_outgoing_track_once_silent() {
        // A tick long enough to finish the fade takes the outgoing gain to zero,
        // so the caller is told to drop it and the incoming track reaches full gain.
        let (fin, _fout, fading, changed) = crossfade_step(true, 2.0, 2.0, 0.0, 1.0, true);
        assert!((fin - 1.0).abs() < 1e-6, "fade_in should reach 1.0, got {fin}");
        assert!(!fading, "outgoing track should be dropped when silent");
        assert!(changed);
    }

    #[test]
    fn crossfade_without_an_outgoing_track_only_ramps_in() {
        // Incoming track still fading up but nothing fading out: only fade_in moves.
        let (fin, fout, fading, changed) = crossfade_step(true, 4.0, 1.0, 0.5, 1.0, false);
        assert!((fin - 0.75).abs() < 1e-6, "fade_in {fin}");
        assert_eq!(fout, 1.0);
        assert!(!fading);
        assert!(changed);
    }

    #[test]
    fn crossfade_at_rest_reports_no_change() {
        // Incoming already at full gain and nothing fading: a tick changes nothing.
        let (_fin, _fout, _fading, changed) = crossfade_step(true, 3.0, 0.1, 1.0, 1.0, false);
        assert!(!changed);
    }

    #[test]
    fn crossfade_frozen_while_not_playing() {
        // Paused mid-crossfade: the ramp must hold both gains and keep the outgoing
        // track, not advance (which would silently drop it during the pause).
        let (fin, fout, fading, changed) = crossfade_step(false, 2.0, 1.0, 0.4, 0.6, true);
        assert_eq!((fin, fout, fading, changed), (0.4, 0.6, true, false));
    }
}
