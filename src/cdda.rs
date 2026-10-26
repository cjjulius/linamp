//! Audio CD (CDDA) support, the device-agnostic groundwork (#160).
//!
//! CD audio is uncompressed 16-bit stereo PCM at 44.1 kHz, so no codec or license
//! is needed to decode it. The work is CD-ROM access: read the table of contents,
//! then read raw CDDA sectors, which is per-OS (DeviceIoControl on Windows, the
//! CDROM ioctls on Linux) and needs a real optical drive to develop and verify.
//! `libcdio` is GPL and incompatible with this app's Apache-2.0 licence, so the
//! device layer uses raw OS reads instead.
//!
//! This module holds the parts that are OS- and hardware-independent (the track
//! model, CDDA timing and the playlist pseudo-path) so both platforms' device
//! code and the playback source can build on one shared representation.

// The Linux device layer ([`linux`]) and the background-buffered playback source
// ([`BufferedCdSource`]) are wired into playback on Linux (#160); on Windows and
// macOS the public entry points below return a clean "Linux only" error, so some
// items here are unused in those builds. Keep them without dead-code noise.
#![allow(dead_code)]

use std::time::Duration;

use anyhow::{anyhow, Result};
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
/// (example: `D:` on Windows, `/dev/sr0` on Linux) and `<track>` is the track number.
pub fn track_path(device: &str, track: u8) -> String {
    format!("cdda://{device}/{track}")
}

/// Parse a [`track_path`] back into `(device, track)`.
pub fn parse_track_path(s: &str) -> Option<(String, u8)> {
    let rest = s.strip_prefix("cdda://")?;
    // The track number is the final `/<n>`; everything before it is the device,
    // which itself may contain `/` (example: `/dev/sr0`).
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
    /// Read `count` consecutive sectors from `lba`, returned as one byte run.
    ///
    /// The default loops `read_sector`, but a device layer should override this to
    /// read the whole run in a single request. Playback reads ~1 second at a time
    /// (see `CdAudioSource`); one request per second keeps the drive streaming,
    /// whereas one request per 2352-byte sector makes it re-seek/spin constantly
    /// and stutter (#160).
    fn read_sectors(&mut self, lba: u32, count: u32) -> Result<Vec<u8>> {
        let mut out = Vec::with_capacity(count as usize * CDDA_SECTOR_BYTES);
        for i in 0..count {
            out.extend_from_slice(&self.read_sector(lba + i)?);
        }
        Ok(out)
    }
}

/// Sectors read per drive access during playback (~1 second of audio). Reading a
/// batch instead of a single 2352-byte sector per audio refill is what keeps CD
/// playback from stuttering (#160).
const READAHEAD_SECTORS: u32 = CDDA_SECTORS_PER_SEC;

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

    /// Read the next batch of sectors into `samples`; false at end of track / on a
    /// read error. Reads up to `READAHEAD_SECTORS` at once so the audio thread hits
    /// the drive about once a second in a streaming burst, not once per 2352-byte
    /// sector (which stutters as the drive re-seeks and spins up) (#160).
    fn fill(&mut self) -> bool {
        if self.next_lba >= self.end_lba {
            return false;
        }
        let count = (self.end_lba - self.next_lba).min(READAHEAD_SECTORS);
        match self.drive.read_sectors(self.next_lba, count) {
            Ok(bytes) if !bytes.is_empty() => {
                self.samples = pcm_s16le_to_f32(&bytes);
                self.pos = 0;
                self.next_lba += count;
                !self.samples.is_empty()
            }
            _ => false,
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

// --- Background-buffered playback source -----------------------------------
//
// Reading CDDA sectors synchronously on the audio thread stutters: an optical
// drive's reads are bursty, and a single ~1-second batch can spike to 2-3 seconds
// when the drive re-reads or renegotiates speed (measured on real hardware). That
// starves the output, the drive idles and spins down, then seeks and spins up to
// resume, the audible "stop, then surprised respin" (#160). The fix is to read on
// a background thread into a deep prefetch buffer, so the audio thread pulls at a
// steady real-time rate while the reader absorbs the spikes and keeps the drive
// streaming (it touches the drive about once a second in steady state, so it never
// idles long enough to spin down).

use std::sync::mpsc::{sync_channel, Receiver, SyncSender};
use std::thread;

/// Prefetch-buffer depth in 1-second batches. Deep enough to ride out the
/// multi-second read spikes a drive produces, while the bounded channel's
/// back-pressure keeps the reader touching the drive roughly once a second so it
/// stays spun up rather than spinning down between reads (#160).
const PREFETCH_BATCHES: usize = 32;

/// Messages the reader thread sends to the playback source. Each carries the seek
/// generation it was produced under, so audio queued before a seek is recognised
/// as stale and dropped instead of played.
enum CdMsg {
    /// A batch of interleaved f32 samples for the given generation.
    Batch(u64, Vec<f32>),
    /// The track played out (reader reached the end) for the given generation.
    End(u64),
}

/// A rodio `Source` that plays one CD track by reading its sectors on a background
/// thread into a bounded prefetch buffer, decoupling the drive's bursty reads from
/// the steady real-time pull of the audio output (#160). Platform-independent: it
/// drives any [`CdDrive`], so only the device I/O stays per-OS.
pub struct BufferedCdSource {
    rx: Receiver<CdMsg>,
    /// Target sector (relative to the track start) for a seek, tagged with the new
    /// generation. The reader jumps there and stamps later batches with that
    /// generation.
    seek_tx: SyncSender<(u64, u32)>,
    /// The generation the source currently accepts. A seek bumps it; batches with
    /// an older generation are dropped as stale.
    gen: u64,
    cur: Vec<f32>,
    pos: usize,
    total: Option<Duration>,
}

impl BufferedCdSource {
    /// Start playing `track` from `drive` on a background reader thread.
    pub fn new<D: CdDrive + 'static>(drive: D, track: &CdTrack) -> Self {
        let (tx, rx) = sync_channel::<CdMsg>(PREFETCH_BATCHES);
        let (seek_tx, seek_rx) = sync_channel::<(u64, u32)>(8);
        let total = Some(Duration::from_secs_f64(
            track.sectors as f64 / CDDA_SECTORS_PER_SEC as f64,
        ));
        let t = track.clone();
        thread::spawn(move || reader_loop(drive, t, tx, seek_rx));
        Self {
            rx,
            seek_tx,
            gen: 0,
            cur: Vec::new(),
            pos: 0,
            total,
        }
    }
}

/// The reader thread: read sectors in ~1-second batches and send them, honouring
/// seek requests. Exits when the track ends with no pending seek, on a read error,
/// or once the source is dropped (the channels close).
fn reader_loop<D: CdDrive>(
    mut drive: D,
    track: CdTrack,
    tx: SyncSender<CdMsg>,
    seek_rx: Receiver<(u64, u32)>,
) {
    let start = track.start_lba;
    let end = track.start_lba + track.sectors;
    let mut gen = 0u64;
    let mut next_lba = start;
    loop {
        // Apply any pending seeks, collapsing a burst to the most recent.
        while let Ok((g, sector)) = seek_rx.try_recv() {
            gen = g;
            next_lba = (start + sector).min(end);
        }
        if next_lba >= end {
            if tx.send(CdMsg::End(gen)).is_err() {
                return; // source dropped
            }
            // Block for a backward seek; exit when the source is gone.
            match seek_rx.recv() {
                Ok((g, sector)) => {
                    gen = g;
                    next_lba = (start + sector).min(end);
                    continue;
                }
                Err(_) => return,
            }
        }
        let count = (end - next_lba).min(READAHEAD_SECTORS);
        match drive.read_sectors(next_lba, count) {
            Ok(bytes) if !bytes.is_empty() => {
                let samples = pcm_s16le_to_f32(&bytes);
                next_lba += count;
                if tx.send(CdMsg::Batch(gen, samples)).is_err() {
                    return; // source dropped
                }
            }
            _ => {
                // A read error ends the track cleanly rather than looping on it.
                let _ = tx.send(CdMsg::End(gen));
                return;
            }
        }
    }
}

impl Iterator for BufferedCdSource {
    type Item = f32;

    fn next(&mut self) -> Option<f32> {
        loop {
            if self.pos < self.cur.len() {
                let s = self.cur[self.pos];
                self.pos += 1;
                return Some(s);
            }
            // Current batch drained: pull the next one, skipping any batch left over
            // from before a seek (a stale generation).
            match self.rx.recv() {
                Ok(CdMsg::Batch(g, v)) if g == self.gen => {
                    self.cur = v;
                    self.pos = 0;
                }
                Ok(CdMsg::End(g)) if g == self.gen => return None,
                Ok(_) => continue, // stale batch/end from before a seek: drop it
                Err(_) => return None, // reader gone
            }
        }
    }
}

impl Source for BufferedCdSource {
    fn current_frame_len(&self) -> Option<usize> {
        None
    }
    fn channels(&self) -> u16 {
        CDDA_CHANNELS
    }
    fn sample_rate(&self) -> u32 {
        CDDA_SAMPLE_RATE
    }
    fn total_duration(&self) -> Option<Duration> {
        self.total
    }
    fn try_seek(&mut self, pos: Duration) -> Result<(), SeekError> {
        // Bump the generation, drop the buffered pre-seek audio, and tell the
        // reader to jump. Batches already queued are discarded by generation in
        // `next`. Treated as always seekable (it's a CD); if the reader has already
        // exited (track over), the request is simply a no-op.
        let sector = (pos.as_secs_f64() * CDDA_SECTORS_PER_SEC as f64) as u32;
        self.gen += 1;
        self.cur.clear();
        self.pos = 0;
        let _ = self.seek_tx.send((self.gen, sector));
        Ok(())
    }
}

// --- Cross-platform entry points ------------------------------------------

/// Read the audio tracks from the disc in `device` (example: `/dev/sr0`). Audio-CD
/// access is Linux-only in this build; other platforms return a clean error.
pub fn read_cd_toc(device: &str) -> Result<Vec<CdTrack>> {
    #[cfg(target_os = "linux")]
    {
        let mut drive = linux::LinuxCdDrive::open(device)?;
        drive.read_toc()
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = device;
        Err(anyhow!("audio CD support is only available on Linux in this build"))
    }
}

/// Open a background-buffered playback source for `track` on `device`. Audio-CD
/// access is Linux-only in this build; other platforms return a clean error.
pub fn open_cd_source(device: &str, track: u8) -> Result<BufferedCdSource> {
    #[cfg(target_os = "linux")]
    {
        let mut drive = linux::LinuxCdDrive::open(device)?;
        let toc = drive.read_toc()?;
        let t = toc
            .into_iter()
            .find(|t| t.number == track)
            .ok_or_else(|| anyhow!("track {track} not found on the disc"))?;
        Ok(BufferedCdSource::new(drive, &t))
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (device, track);
        Err(anyhow!("audio CD support is only available on Linux in this build"))
    }
}

/// The platform's default optical device, used when the UI opens a CD.
pub fn default_cd_device() -> String {
    #[cfg(target_os = "linux")]
    {
        linux::default_device()
    }
    #[cfg(not(target_os = "linux"))]
    {
        "/dev/cdrom".to_string()
    }
}

/// Linux CD-ROM device layer: raw `CDROMREADTOCHDR` / `CDROMREADTOCENTRY` /
/// `CDROMREADAUDIO` ioctls on an optical device, no libcdio (keeps the licence
/// clean). Reads whole 2352-byte CDDA sectors, which the shared source turns into
/// audio (#160).
#[cfg(target_os = "linux")]
mod linux {
    use super::*;
    use std::fs::File;
    use std::os::fd::AsRawFd;

    const CDROMREADTOCHDR: libc::c_ulong = 0x5305;
    const CDROMREADTOCENTRY: libc::c_ulong = 0x5306;
    const CDROMREADAUDIO: libc::c_ulong = 0x530e;
    const CDROM_LBA: u8 = 0x01; // address format: logical block address
    const CDROM_LEADOUT: u8 = 0xAA; // pseudo-track: the disc lead-out (end)
    const CDROM_DATA_TRACK: u8 = 0x04; // ctrl bit set on data (non-audio) tracks

    // These mirror the C structs in <linux/cdrom.h>. `#[repr(C)]` reproduces the
    // same field order and padding the kernel copies into/out of.
    #[repr(C)]
    struct TocHdr {
        trk0: u8,
        trk1: u8,
    }
    #[repr(C)]
    struct TocEntry {
        track: u8,
        adr_ctrl: u8, // adr:4 (high), ctrl:4 (low) packed in one byte
        format: u8,
        addr_lba: i32, // union cdrom_addr, read as the LBA (format = CDROM_LBA)
        datamode: u8,
    }
    #[repr(C)]
    struct ReadAudio {
        addr_lba: i32, // union cdrom_addr: start LBA
        addr_format: u8,
        nframes: i32, // number of 2352-byte frames to read
        buf: *mut u8, // caller-provided buffer, nframes * 2352 bytes
    }

    /// An opened optical device, used read-only through the CD-ROM ioctls.
    pub struct LinuxCdDrive {
        file: File,
    }

    impl LinuxCdDrive {
        /// Open `device` (example: `/dev/sr0`) read-only.
        pub fn open(device: &str) -> Result<Self> {
            let file = File::options()
                .read(true)
                .open(device)
                .map_err(|e| anyhow!("open {device}: {e}"))?;
            Ok(Self { file })
        }

        fn fd(&self) -> libc::c_int {
            self.file.as_raw_fd()
        }

        fn toc_entry(&self, track: u8) -> Option<TocEntry> {
            let mut e = TocEntry {
                track,
                adr_ctrl: 0,
                format: CDROM_LBA,
                addr_lba: 0,
                datamode: 0,
            };
            let r = unsafe {
                libc::ioctl(self.fd(), CDROMREADTOCENTRY, &mut e as *mut _ as *mut libc::c_void)
            };
            (r == 0).then_some(e)
        }
    }

    impl CdDrive for LinuxCdDrive {
        fn read_toc(&mut self) -> Result<Vec<CdTrack>> {
            let mut hdr = TocHdr { trk0: 0, trk1: 0 };
            let r = unsafe {
                libc::ioctl(self.fd(), CDROMREADTOCHDR, &mut hdr as *mut _ as *mut libc::c_void)
            };
            if r != 0 {
                return Err(anyhow!(
                    "read TOC header: {}",
                    std::io::Error::last_os_error()
                ));
            }
            // The lead-out marks the end of the last track.
            let leadout = self.toc_entry(CDROM_LEADOUT).map(|e| e.addr_lba).unwrap_or(0);
            let mut entries = Vec::new();
            for t in hdr.trk0..=hdr.trk1 {
                if let Some(e) = self.toc_entry(t) {
                    let is_data = e.adr_ctrl & CDROM_DATA_TRACK != 0;
                    entries.push((t, e.addr_lba, is_data));
                }
            }
            let mut out = Vec::new();
            for i in 0..entries.len() {
                let (num, lba, is_data) = entries[i];
                if is_data {
                    continue; // CDDA playback: audio tracks only
                }
                let next = entries.get(i + 1).map(|x| x.1).unwrap_or(leadout);
                if next > lba {
                    out.push(CdTrack {
                        number: num,
                        start_lba: lba as u32,
                        sectors: (next - lba) as u32,
                    });
                }
            }
            if out.is_empty() {
                return Err(anyhow!("no audio tracks on the disc"));
            }
            Ok(out)
        }

        fn read_sector(&mut self, lba: u32) -> Result<[u8; CDDA_SECTOR_BYTES]> {
            let v = self.read_sectors(lba, 1)?;
            let mut out = [0u8; CDDA_SECTOR_BYTES];
            let n = v.len().min(CDDA_SECTOR_BYTES);
            out[..n].copy_from_slice(&v[..n]);
            Ok(out)
        }

        fn read_sectors(&mut self, lba: u32, count: u32) -> Result<Vec<u8>> {
            let mut buf = vec![0u8; count as usize * CDDA_SECTOR_BYTES];
            let mut ra = ReadAudio {
                addr_lba: lba as i32,
                addr_format: CDROM_LBA,
                nframes: count as i32,
                buf: buf.as_mut_ptr(),
            };
            let r = unsafe {
                libc::ioctl(self.fd(), CDROMREADAUDIO, &mut ra as *mut _ as *mut libc::c_void)
            };
            if r != 0 {
                return Err(anyhow!(
                    "read audio at lba {lba}: {}",
                    std::io::Error::last_os_error()
                ));
            }
            Ok(buf)
        }
    }

    /// Prefer the `/dev/cdrom` symlink, falling back to `/dev/sr0`.
    pub fn default_device() -> String {
        for d in ["/dev/cdrom", "/dev/sr0"] {
            if std::path::Path::new(d).exists() {
                return d.to_string();
            }
        }
        "/dev/cdrom".to_string()
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
        // 0 to 0.0, max to ~+1, min to -1; little-endian byte order.
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

    /// A drive whose sectors are all silence, enough to exercise the Source's
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
    fn buffered_source_plays_whole_track_then_ends() {
        // The background reader produces every sector of the track exactly once,
        // across the deep prefetch buffer, then the iterator ends (End message).
        let track = CdTrack { number: 1, start_lba: 0, sectors: 50 };
        let src = BufferedCdSource::new(FakeDrive, &track);
        assert_eq!(src.channels(), CDDA_CHANNELS);
        assert_eq!(src.sample_rate(), CDDA_SAMPLE_RATE);
        assert_eq!(src.total_duration(), Some(Duration::from_secs_f64(50.0 / 75.0)));
        assert_eq!(src.count(), 50 * CDDA_SAMPLES_PER_SECTOR);
    }

    #[test]
    fn buffered_source_seek_drops_pre_seek_audio() {
        // A seek bumps the generation; batches the reader produced before it (any
        // interleaving, whether the reader had read ahead or even finished the
        // track) carry the old generation and are dropped, so only the post-seek
        // sectors are played. From sector 60 of 100, 40 sectors remain.
        let track = CdTrack { number: 1, start_lba: 0, sectors: 100 };
        let mut src = BufferedCdSource::new(FakeDrive, &track);
        src.try_seek(Duration::from_secs_f64(60.0 / 75.0)).unwrap();
        assert_eq!(src.count(), 40 * CDDA_SAMPLES_PER_SECTOR);
    }

    /// Records each read_sectors batch size so we can assert the source pages in
    /// ~1-second runs instead of one sector at a time.
    struct CountingDrive {
        batches: std::sync::Arc<std::sync::Mutex<Vec<u32>>>,
    }
    impl CdDrive for CountingDrive {
        fn read_toc(&mut self) -> Result<Vec<CdTrack>> {
            Ok(vec![])
        }
        fn read_sector(&mut self, _lba: u32) -> Result<[u8; CDDA_SECTOR_BYTES]> {
            Ok([0u8; CDDA_SECTOR_BYTES])
        }
        fn read_sectors(&mut self, _lba: u32, count: u32) -> Result<Vec<u8>> {
            self.batches.lock().unwrap().push(count);
            Ok(vec![0u8; count as usize * CDDA_SECTOR_BYTES])
        }
    }

    #[test]
    fn cd_audio_source_reads_sectors_in_batches() {
        // A 200-sector track must be read through read_sectors in ~1-second batches
        // (READAHEAD_SECTORS each), not one sector per read, so the drive streams
        // rather than re-seeking and stuttering (#160).
        let batches = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let drive = CountingDrive { batches: batches.clone() };
        let track = CdTrack { number: 1, start_lba: 0, sectors: 200 };
        let src = CdAudioSource::new(drive, &track);
        assert_eq!(src.count(), 200 * CDDA_SAMPLES_PER_SECTOR);
        let calls = batches.lock().unwrap();
        assert_eq!(
            calls.as_slice(),
            &[READAHEAD_SECTORS, READAHEAD_SECTORS, 200 - 2 * READAHEAD_SECTORS],
            "should read in batches, not per-sector"
        );
    }

    #[test]
    fn parse_rejects_non_cdda_and_malformed() {
        assert!(!is_cdda_path("/music/song.mp3"));
        assert_eq!(parse_track_path("/music/song.mp3"), None);
        assert_eq!(parse_track_path("cdda://D:/notanumber"), None);
        assert_eq!(parse_track_path("cdda:///3"), None); // empty device
    }
}
