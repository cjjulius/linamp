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
    fn parse_rejects_non_cdda_and_malformed() {
        assert!(!is_cdda_path("/music/song.mp3"));
        assert_eq!(parse_track_path("/music/song.mp3"), None);
        assert_eq!(parse_track_path("cdda://D:/notanumber"), None);
        assert_eq!(parse_track_path("cdda:///3"), None); // empty device
    }
}
