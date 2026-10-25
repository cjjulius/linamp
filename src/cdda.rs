//! Audio CD (CDDA) support — the device-agnostic groundwork (#160).
//!
//! CD audio is uncompressed 16-bit stereo PCM at 44.1 kHz, so no codec or license
//! is needed to decode it. The work is CD-ROM access: read the table of contents,
//! then read raw CDDA sectors — which is per-OS (DeviceIoControl on Windows, the
//! CDROM ioctls on Linux) and needs a real optical drive to develop and verify.
//! `libcdio` is GPL and incompatible with this app's Apache-2.0 licence, so the
//! device layer uses raw OS reads instead.
//!
//! This module holds the parts that are OS- and hardware-independent (the track
//! model, CDDA timing, and the playlist pseudo-path) so both platforms' device
//! code and the playback source can build on one shared representation.

// The per-OS device layer and playback source are not wired in yet, so these are
// unused in a plain build; keep them without dead-code noise until that lands.
#![allow(dead_code)]

use std::time::Duration;

use anyhow::Result;
use rodio::source::SeekError;
use rodio::Source;

/// CDDA is 16-bit stereo PCM at 44.1 kHz: 2352 bytes per sector, 75 sectors/sec.
pub const CDDA_SECTOR_BYTES: usize = 2352;
pub const CDDA_SECTORS_PER_SEC: u32 = 75;
pub const CDDA_SAMPLE_RATE: u32 = 44_100;
pub const CDDA_CHANNELS: u16 = 2;

/// One audio track from a disc's table of contents. Positions are in sectors
/// (logical block addresses), the native CDDA unit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CdTrack {
    /// 1-based track number, as printed on the disc.
    pub number: u8,
    /// First sector of the track.
    pub start_lba: u32,
    /// Track length in sectors.
    pub sectors: u32,
}

impl CdTrack {
    /// Playing time, derived from the sector count (75 sectors = 1 second).
    pub fn duration(&self) -> Duration {
        Duration::from_secs_f64(self.sectors as f64 / CDDA_SECTORS_PER_SEC as f64)
    }
}

/// The pseudo-path the playlist uses to refer to a CD audio track, which has no
/// file on disk: `cdda://<device>/<track>`, where `<device>` is the OS drive id
/// (e.g. `D:` on Windows, `/dev/sr0` on Linux) and `<track>` is the track number.
pub fn track_path(device: &str, track: u8) -> String {
    format!("cdda://{device}/{track}")
}

/// Parse a [`track_path`] back into `(device, track)`.
pub fn parse_track_path(s: &str) -> Option<(String, u8)> {
    let rest = s.strip_prefix("cdda://")?;
    // The track number is the final `/<n>`; everything before it is the device,
    // which itself may contain `/` (e.g. `/dev/sr0`).
    let (device, track) = rest.rsplit_once('/')?;
    if device.is_empty() {
        return None;
    }
    Some((device.to_string(), track.parse().ok()?))
}

/// True for a CD-track pseudo-path (not a real file on disk).
pub fn is_cdda_path(s: &str) -> bool {
    s.starts_with("cdda://")
}

/// Interleaved f32 samples per CDDA sector (2352 bytes / 2 bytes per 16-bit sample).
pub const CDDA_SAMPLES_PER_SECTOR: usize = CDDA_SECTOR_BYTES / 2;

/// Convert raw CDDA sector bytes (16-bit little-endian stereo PCM, the on-disc
/// form) to interleaved f32 in roughly -1.0..=1.0, which is what the rodio
/// pipeline consumes. The per-OS device layer reads whole 2352-byte sectors; this
/// turns them into audio, so it can share one playback source across platforms.
/// A trailing odd byte (which a correct 2352-byte sector never has) is ignored.
pub fn pcm_s16le_to_f32(bytes: &[u8]) -> Vec<f32> {
    bytes
        .chunks_exact(2)
        .map(|b| i16::from_le_bytes([b[0], b[1]]) as f32 / 32768.0)
        .collect()
}

/// What the per-OS device layer must provide for CD playback. Implementations do
/// the raw device I/O (DeviceIoControl on Windows, the CDROM ioctls on Linux);
/// everything above this trait is platform-independent and already tested.
pub trait CdDrive: Send {
    /// Read the disc's table of contents (the audio tracks, in order).
    fn read_toc(&mut self) -> Result<Vec<CdTrack>>;
    /// Read one 2352-byte CDDA audio sector at logical block address `lba`.
    fn read_sector(&mut self, lba: u32) -> Result<[u8; CDDA_SECTOR_BYTES]>;
}

/// A rodio `Source` that plays one CD track by reading its sectors through a
/// [`CdDrive`] and converting each to f32. Platform-independent: it works against
/// any drive implementation, so only the device I/O is left per-OS.
pub struct CdAudioSource<D: CdDrive> {
    drive: D,
    start_lba: u32,
    next_lba: u32,
    end_lba: u32,
    samples: Vec<f32>,
    pos: usize,
}

impl<D: CdDrive> CdAudioSource<D> {
    /// Play `track` from `drive`.
    pub fn new(drive: D, track: &CdTrack) -> Self {
        Self {
            drive,
            start_lba: track.start_lba,
            next_lba: track.start_lba,
            end_lba: track.start_lba + track.sectors,
            samples: Vec::new(),
            pos: 0,
        }
    }

    /// Read the next sector into `samples`; false at end of track / on a read error.
    fn fill(&mut self) -> bool {
        if self.next_lba >= self.end_lba {
            return false;
        }
        match self.drive.read_sector(self.next_lba) {
            Ok(sector) => {
                self.samples = pcm_s16le_to_f32(&sector);
                self.pos = 0;
                self.next_lba += 1;
                !self.samples.is_empty()
            }
            Err(_) => false,
        }
    }
}

impl<D: CdDrive> Iterator for CdAudioSource<D> {
    type Item = f32;

    fn next(&mut self) -> Option<f32> {
        if self.pos >= self.samples.len() && !self.fill() {
            return None;
        }
        let s = self.samples[self.pos];
        self.pos += 1;
        Some(s)
    }
}

impl<D: CdDrive> Source for CdAudioSource<D> {
    fn current_frame_len(&self) -> Option<usize> {
        let rem = self.samples.len() - self.pos;
        (rem > 0).then_some(rem)
    }
    fn channels(&self) -> u16 {
        CDDA_CHANNELS
    }
    fn sample_rate(&self) -> u32 {
        CDDA_SAMPLE_RATE
    }
    fn total_duration(&self) -> Option<Duration> {
        Some(Duration::from_secs_f64(
            (self.end_lba - self.start_lba) as f64 / CDDA_SECTORS_PER_SEC as f64,
        ))
    }
    fn try_seek(&mut self, pos: Duration) -> Result<(), SeekError> {
        // Seek to the sector that holds `pos` (75 sectors = 1 second), clamped to
        // the track, then re-read from there.
        let sector = (pos.as_secs_f64() * CDDA_SECTORS_PER_SEC as f64) as u32;
        self.next_lba = (self.start_lba + sector).min(self.end_lba);
        self.samples.clear();
        self.pos = 0;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn track_duration_is_sectors_over_75() {
        // 75 sectors = 1 second; a 3-minute track is 3*60*75 sectors.
        assert_eq!(CdTrack { number: 1, start_lba: 0, sectors: 75 }.duration(),
                   Duration::from_secs(1));
        let three_min = CdTrack { number: 2, start_lba: 0, sectors: 3 * 60 * 75 };
        assert_eq!(three_min.duration(), Duration::from_secs(180));
    }

    #[test]
    fn track_path_roundtrips_for_both_platforms() {
        for (dev, n) in [("D:", 3u8), ("/dev/sr0", 12), ("E:", 1)] {
            let p = track_path(dev, n);
            assert!(is_cdda_path(&p));
            assert_eq!(parse_track_path(&p), Some((dev.to_string(), n)));
        }
    }

    #[test]
    fn pcm_s16le_converts_to_normalized_f32() {
        // 0 -> 0.0, max -> ~+1, min -> -1; little-endian byte order.
        let bytes = [
            0x00, 0x00, // 0
            0xff, 0x7f, // i16::MAX = 32767
            0x00, 0x80, // i16::MIN = -32768
        ];
        let out = pcm_s16le_to_f32(&bytes);
        assert_eq!(out.len(), 3);
        assert_eq!(out[0], 0.0);
        assert!((out[1] - 0.99997).abs() < 1e-4);
        assert_eq!(out[2], -1.0);
        // A whole sector yields exactly CDDA_SAMPLES_PER_SECTOR samples.
        let sector = vec![0u8; CDDA_SECTOR_BYTES];
        assert_eq!(pcm_s16le_to_f32(&sector).len(), CDDA_SAMPLES_PER_SECTOR);
    }

    /// A drive whose sectors are all silence — enough to exercise the Source's
    /// paging and seeking without real hardware.
    struct FakeDrive;
    impl CdDrive for FakeDrive {
        fn read_toc(&mut self) -> Result<Vec<CdTrack>> {
            Ok(vec![])
        }
        fn read_sector(&mut self, _lba: u32) -> Result<[u8; CDDA_SECTOR_BYTES]> {
            Ok([0u8; CDDA_SECTOR_BYTES])
        }
    }

    #[test]
    fn cd_audio_source_plays_whole_track_then_ends() {
        let track = CdTrack { number: 1, start_lba: 100, sectors: 10 };
        let src = CdAudioSource::new(FakeDrive, &track);
        assert_eq!(src.channels(), CDDA_CHANNELS);
        assert_eq!(src.sample_rate(), CDDA_SAMPLE_RATE);
        assert_eq!(src.total_duration(), Some(Duration::from_secs_f64(10.0 / 75.0)));
        // Exactly 10 sectors of audio, then the iterator ends.
        assert_eq!(src.count(), 10 * CDDA_SAMPLES_PER_SECTOR);
    }

    #[test]
    fn cd_audio_source_seeks_to_a_sector() {
        let track = CdTrack { number: 1, start_lba: 100, sectors: 10 };
        // Seek to sector 8 of 10: sectors 8 and 9 remain.
        let mut src = CdAudioSource::new(FakeDrive, &track);
        src.try_seek(Duration::from_secs_f64(8.0 / 75.0)).unwrap();
        assert_eq!(src.count(), 2 * CDDA_SAMPLES_PER_SECTOR);
        // Seeking past the end yields nothing (clamped to the track end).
        let mut past = CdAudioSource::new(FakeDrive, &track);
        past.try_seek(Duration::from_secs(60)).unwrap();
        assert_eq!(past.count(), 0);
    }

    #[test]
    fn parse_rejects_non_cdda_and_malformed() {
        assert!(!is_cdda_path("/music/song.mp3"));
        assert_eq!(parse_track_path("/music/song.mp3"), None);
        assert_eq!(parse_track_path("cdda://D:/notanumber"), None);
        assert_eq!(parse_track_path("cdda:///3"), None); // empty device
    }
}
