//! MIDI (.mid/.midi) playback via rustysynth, a pure-Rust SoundFont synth (#159).
//!
//! Classic Winamp played MIDI through a built-in softsynth. We synthesize with
//! rustysynth and a bundled SoundFont (.sf2), rendering to a lazy rodio `Source`
//! so MIDI flows through the same EQ / analyzer / balance pipeline as everything
//! else. The SoundFont is discovered next to the binary (see [`soundfont_path`],
//! mirroring skin discovery); playback errors cleanly when none is present.

use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, Result};
use rodio::source::SeekError;
use rodio::Source;
use rustysynth::{MidiFile, MidiFileSequencer, SoundFont, Synthesizer, SynthesizerSettings};

/// Render rate for the synth; the rodio mixer resamples to the output device.
const SAMPLE_RATE: u32 = 44_100;
/// Frames rendered per sequencer call.
const BLOCK: usize = 1024;

/// Locate the bundled SoundFont: the first `.sf2` in a `soundfont/` folder in the
/// working directory or next to the executable (same scheme as `skins/`).
pub fn soundfont_path() -> Option<PathBuf> {
    let mut dirs = vec![PathBuf::from("soundfont")];
    if let Ok(exe) = std::env::current_exe() {
        if let Some(d) = exe.parent() {
            dirs.push(d.join("soundfont"));
        }
    }
    for dir in dirs {
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for e in entries.flatten() {
                let p = e.path();
                if p.extension()
                    .and_then(|x| x.to_str())
                    .map(|x| x.eq_ignore_ascii_case("sf2"))
                    .unwrap_or(false)
                {
                    return Some(p);
                }
            }
        }
    }
    None
}

/// A rodio `Source` that renders a MIDI file through rustysynth on demand.
pub struct MidiSource {
    sequencer: MidiFileSequencer,
    total: Option<Duration>,
    /// Interleaved f32 for the current rendered block, and the read cursor.
    buf: Vec<f32>,
    pos: usize,
    /// Reused per-block mono render scratch (left/right).
    left: Vec<f32>,
    right: Vec<f32>,
    done: bool,
}

impl MidiSource {
    /// Render the next block into `buf`; false once the sequence has ended.
    fn fill(&mut self) -> bool {
        if self.done {
            return false;
        }
        self.sequencer.render(&mut self.left, &mut self.right);
        self.buf.clear();
        for (l, r) in self.left.iter().zip(self.right.iter()) {
            self.buf.push(*l);
            self.buf.push(*r);
        }
        self.pos = 0;
        // Stop after the block in which the sequence ends (its release tail is
        // included in that block's render).
        if self.sequencer.end_of_sequence() {
            self.done = true;
        }
        !self.buf.is_empty()
    }
}

impl Iterator for MidiSource {
    type Item = f32;

    fn next(&mut self) -> Option<f32> {
        if self.pos >= self.buf.len() && !self.fill() {
            return None;
        }
        let s = self.buf[self.pos];
        self.pos += 1;
        Some(s)
    }
}

impl Source for MidiSource {
    fn current_frame_len(&self) -> Option<usize> {
        let rem = self.buf.len() - self.pos;
        (rem > 0).then_some(rem)
    }
    fn channels(&self) -> u16 {
        2
    }
    fn sample_rate(&self) -> u32 {
        SAMPLE_RATE
    }
    fn total_duration(&self) -> Option<Duration> {
        self.total
    }
    fn try_seek(&mut self, _pos: Duration) -> Result<(), SeekError> {
        // rustysynth's sequencer can't seek; treat MIDI as non-seekable.
        Err(SeekError::NotSupported { underlying_source: "midi" })
    }
}

/// Open a `.mid`/`.midi` file for playback, synthesized with the SoundFont at
/// `soundfont_path`. Errors cleanly if the SoundFont or MIDI can't be read.
pub fn open_midi(midi_path: &Path, soundfont_path: &Path) -> Result<MidiSource> {
    let mut sf = File::open(soundfont_path)
        .map_err(|e| anyhow!("SoundFont {}: {e}", soundfont_path.display()))?;
    let sound_font =
        Arc::new(SoundFont::new(&mut sf).map_err(|e| anyhow!("bad SoundFont: {e}"))?);

    let mut mf = File::open(midi_path)?;
    let midi_file = Arc::new(MidiFile::new(&mut mf).map_err(|e| anyhow!("bad MIDI file: {e}"))?);
    let total = Some(Duration::from_secs_f64(midi_file.get_length()));

    let settings = SynthesizerSettings::new(SAMPLE_RATE as i32);
    let synth = Synthesizer::new(&sound_font, &settings)
        .map_err(|e| anyhow!("synth init failed: {e}"))?;
    let mut sequencer = MidiFileSequencer::new(synth);
    sequencer.play(&midi_file, false);

    Ok(MidiSource {
        sequencer,
        total,
        buf: Vec::new(),
        pos: 0,
        left: vec![0.0; BLOCK],
        right: vec![0.0; BLOCK],
        done: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_midi_errors_cleanly_without_a_soundfont() {
        // No SoundFont present: a clean error, not a panic (the play path catches it
        // and the skip-unplayable guard handles it).
        let err = open_midi(
            Path::new("whatever.mid"),
            Path::new("definitely-no-soundfont.sf2"),
        );
        assert!(err.is_err());
    }

    #[test]
    fn open_midi_renders_a_note_with_the_bundled_soundfont() {
        // End-to-end: synthesize a one-note MIDI with the bundled SoundFont and
        // confirm it yields audio. Skips if the SoundFont isn't present.
        let sf = Path::new("soundfont/GeneralUser-GS.sf2");
        if !sf.exists() {
            return;
        }
        // A minimal Format-0 MIDI: one note-on, note-off after 96 ticks, end.
        let track: [u8; 12] = [
            0x00, 0x90, 0x3C, 0x64, // t0: note on, middle C, vel 100
            0x60, 0x80, 0x3C, 0x00, // +96: note off
            0x00, 0xFF, 0x2F, 0x00, // t0: end of track
        ];
        let mut mid = Vec::new();
        mid.extend_from_slice(b"MThd");
        mid.extend_from_slice(&6u32.to_be_bytes());
        mid.extend_from_slice(&0u16.to_be_bytes()); // format 0
        mid.extend_from_slice(&1u16.to_be_bytes()); // 1 track
        mid.extend_from_slice(&96u16.to_be_bytes()); // division
        mid.extend_from_slice(b"MTrk");
        mid.extend_from_slice(&(track.len() as u32).to_be_bytes());
        mid.extend_from_slice(&track);
        let p = std::env::temp_dir().join("linamp_midi_test.mid");
        std::fs::write(&p, &mid).unwrap();

        let mut src = open_midi(&p, sf).expect("should open the MIDI with the SoundFont");
        assert_eq!(src.channels(), 2);
        assert_eq!(src.sample_rate(), SAMPLE_RATE);
        // Pull a block's worth of samples; synthesis should produce audio.
        let out: Vec<f32> = (&mut src).take(BLOCK * 2).collect();
        assert!(!out.is_empty(), "MIDI synthesis produced no samples");
        let _ = std::fs::remove_file(&p);
    }
}
