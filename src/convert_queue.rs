//! The file-conversion queue (the rip/convert UI, part 1).
//!
//! A single background worker converts queued files one at a time and reports
//! structured progress, so the Media Library's "Converting" pane can show a per-file
//! progress bar. Jobs can be added while the worker runs, and cancelled: cancelling
//! the running file aborts it and deletes its partial output; cancelling a queued file
//! skips it. The worker holds its own library connection (shared WAL db) to index each
//! finished output, and the UI refreshes the library when a job completes.

use std::collections::{HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Mutex};

use crate::encode::{self, ConvertOutcome, ConvertSettings};

/// A queued conversion as the UI tracks it. `location` is the source file, shown in
/// the Location column so rows are identifiable.
pub struct ConvertJob {
    pub id: u64,
    pub artist: String,
    pub title: String,
    pub location: PathBuf,
    pub progress: f32,
    pub state: JobState,
}

/// A job's lifecycle. Cancelled jobs are removed from the list rather than kept, so
/// there is no Cancelled state here.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum JobState {
    Queued,
    Converting,
    Done,
    Failed(String),
}

impl JobState {
    /// A finished job (whether it succeeded or failed), eligible for "Clear completed".
    pub fn is_finished(&self) -> bool {
        matches!(self, JobState::Done | JobState::Failed(_))
    }
}

/// Condense a raw converter error into a few user-facing words for the Failed
/// row's hover tooltip ("File doesn't exist", "Cannot write file", ...). The raw
/// anyhow messages are developer-facing (file paths, codec names), so this maps the
/// common cases and falls back to a trimmed first line. Kept pure for unit testing.
pub fn short_fail_reason(raw: &str) -> String {
    let low = raw.to_ascii_lowercase();
    let not_found = low.contains("os error 2")
        || low.contains("cannot find the file")
        || low.contains("cannot find the path")
        || low.contains("no such file");
    let denied = low.contains("os error 5")
        || low.contains("os error 13")
        || low.contains("access is denied")
        || low.contains("permission denied");
    // The encode arms tag a destination failure with a "create "/"write " context,
    // so a leading one of those means the write side, not the source read.
    let writing = low.starts_with("create ") || low.starts_with("write ");

    if writing {
        return if not_found {
            "Output folder doesn't exist".into()
        } else {
            "Cannot write file".into()
        };
    }
    if not_found {
        return "File doesn't exist".into();
    }
    if denied {
        return "Cannot read file".into();
    }
    if low.starts_with("no audio decoded") {
        return "No audio in file".into();
    }
    if low.contains("unsupported")
        || low.contains("decode")
        || low.contains("format")
        || low.contains("malformed")
    {
        return "Cannot read file".into();
    }
    if low.contains("lame")
        || low.contains("opus")
        || low.contains("vorbis")
        || low.contains("resample")
    {
        return "Conversion failed".into();
    }
    // Last resort: the first line of the raw message, trimmed to a few words.
    let first = raw.lines().next().unwrap_or(raw).trim();
    if first.is_empty() {
        return "Conversion failed".into();
    }
    const MAX: usize = 48;
    if first.chars().count() > MAX {
        let s: String = first.chars().take(MAX).collect();
        format!("{s}\u{2026}")
    } else {
        first.to_string()
    }
}

/// Where a job's audio comes from: an existing file on disk (an ordinary conversion) or
/// a track on an audio CD (a rip). A CD rip carries the tags to write onto the output,
/// since a CD track has no embedded tags to copy.
pub enum JobSource {
    File(PathBuf),
    Cd {
        device: String,
        track: u8,
        tags: crate::encode::RipTags,
    },
}

/// What the UI hands the worker to convert. The destination is given as a folder and a
/// file stem (not a full path): the worker picks the final path just before it writes,
/// so two different sources with the same name don't collide. The extension comes from
/// `settings.format`.
pub struct JobSpec {
    pub id: u64,
    pub source: JobSource,
    pub dir: PathBuf,
    pub stem: String,
    pub settings: ConvertSettings,
}

/// A destination in `dir` for `stem.ext` that does not collide with a file already on
/// disk, bumping to "stem (1).ext", "stem (2).ext", and so on. Called in the worker just
/// before each conversion: because the queue is serial, an earlier job's output already
/// exists by then, so two different sources that share a name (two `song.flac` in
/// different folders, or an album's `01.flac` across discs) convert to distinct files
/// instead of the second overwriting the first.
fn unique_output(dir: &Path, stem: &str, ext: &str) -> PathBuf {
    let first = dir.join(format!("{stem}.{ext}"));
    if !first.exists() {
        return first;
    }
    for n in 1.. {
        let p = dir.join(format!("{stem} ({n}).{ext}"));
        if !p.exists() {
            return p;
        }
    }
    unreachable!()
}

/// Turn arbitrary text (an album, artist, or track title) into a safe file-name
/// component. Characters that are illegal on Windows, and awkward elsewhere, are
/// converted to similar-looking stand-ins rather than dropped (so "AC/DC" reads as
/// "AC-DC", not "ACDC"); control characters become spaces; runs of whitespace collapse;
/// and leading/trailing dots and spaces are trimmed. Guards the Windows reserved device
/// names. Returns "_" if nothing usable remains. Used for the rip's album folder and
/// track file names.
pub fn sanitize_filename(s: &str) -> String {
    let mapped: String = s
        .chars()
        .map(|c| match c {
            '<' => '(',
            '>' => ')',
            ':' => '-',
            '"' => '\'',
            '/' | '\\' | '|' => '-',
            '?' => '_',
            '*' => '+',
            c if (c as u32) < 0x20 => ' ',
            c => c,
        })
        .collect();
    let collapsed = mapped.split_whitespace().collect::<Vec<_>>().join(" ");
    let trimmed = collapsed.trim_matches(|c| c == '.' || c == ' ');
    if trimmed.is_empty() {
        return "_".to_string();
    }
    // Windows reserves these device names (optionally with an extension), so prefix a
    // "_" when the name would be one of them.
    const RESERVED: &[&str] = &[
        "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7",
        "COM8", "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
    ];
    let base = trimmed.split('.').next().unwrap_or(trimmed).to_ascii_uppercase();
    if RESERVED.contains(&base.as_str()) {
        return format!("_{trimmed}");
    }
    trimmed.to_string()
}

/// A compact UTC timestamp to the second, `YYYYMMDD-HHMMSS`, for disambiguating rip
/// folders and file names (the album folder when one already exists, and tracks with no
/// metadata or a name that is already taken). A whole track takes well over a second to
/// rip, so second precision is enough to stay unique without noisy sub-second suffixes.
pub fn rip_timestamp() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    let (h, m, s) = (rem / 3600, (rem % 3600) / 60, rem % 60);
    let (y, mo, d) = civil_from_days(days);
    format!("{y:04}{mo:02}{d:02}-{h:02}{m:02}{s:02}")
}

/// Convert a day count since the Unix epoch into a `(year, month, day)` civil date
/// (UTC), via Howard Hinnant's `civil_from_days` algorithm. Month and day are 1-based.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32; // [1, 12]
    (y + i64::from(m <= 2), m, d)
}

/// Progress messages the worker sends back to the UI.
pub enum ConvertMsg {
    Started(u64),
    Progress(u64, f32),
    Done(u64),
    Failed(u64, String),
    Cancelled(u64),
}

enum WorkerCmd {
    Add(JobSpec),
}

/// Handle to the background conversion worker: send it jobs, cancel them, and poll
/// for progress. Dropping it lets the worker thread exit once its queue drains.
pub struct ConvertWorker {
    cmd_tx: Sender<WorkerCmd>,
    msg_rx: Receiver<ConvertMsg>,
    /// Ids the UI has cancelled. Shared (not sent over the channel) so a cancel reaches
    /// the running conversion mid-file, where the worker is blocked inside the encoder
    /// and cannot read its command channel.
    cancelled: Arc<Mutex<HashSet<u64>>>,
}

impl Default for ConvertWorker {
    fn default() -> Self {
        Self::new()
    }
}

impl ConvertWorker {
    pub fn new() -> Self {
        let (cmd_tx, cmd_rx) = std::sync::mpsc::channel::<WorkerCmd>();
        let (msg_tx, msg_rx) = std::sync::mpsc::channel::<ConvertMsg>();
        let cancelled: Arc<Mutex<HashSet<u64>>> = Arc::new(Mutex::new(HashSet::new()));
        let cancelled_w = cancelled.clone();
        std::thread::spawn(move || worker_loop(cmd_rx, msg_tx, cancelled_w));
        Self { cmd_tx, msg_rx, cancelled }
    }

    /// Queue a job. Added to the back of the worker's queue, even while it is busy.
    pub fn enqueue(&self, spec: JobSpec) {
        let _ = self.cmd_tx.send(WorkerCmd::Add(spec));
    }

    /// Cancel a job by id: the worker skips it if still queued, or aborts it and
    /// deletes the partial output if it is the one running.
    pub fn cancel(&self, id: u64) {
        if let Ok(mut set) = self.cancelled.lock() {
            set.insert(id);
        }
    }

    /// Non-blocking poll for the next progress message, if any.
    pub fn try_recv(&self) -> Option<ConvertMsg> {
        self.msg_rx.try_recv().ok()
    }
}

fn is_cancelled(set: &Arc<Mutex<HashSet<u64>>>, id: u64) -> bool {
    set.lock().map(|s| s.contains(&id)).unwrap_or(false)
}

fn worker_loop(
    cmd_rx: Receiver<WorkerCmd>,
    msg_tx: Sender<ConvertMsg>,
    cancelled: Arc<Mutex<HashSet<u64>>>,
) {
    // The worker's own connection, so indexing a finished output doesn't contend with
    // the UI's. If it can't open, conversions still run; the output just isn't indexed.
    let lib = crate::library::Library::open().ok();
    let mut queue: VecDeque<JobSpec> = VecDeque::new();
    loop {
        // Sleep while idle; wake on the next command. Then drain any further commands
        // without blocking so a burst of enqueues all land before the next convert.
        if queue.is_empty() {
            match cmd_rx.recv() {
                Ok(WorkerCmd::Add(spec)) => queue.push_back(spec),
                Err(_) => return, // all handles dropped
            }
        }
        while let Ok(WorkerCmd::Add(spec)) = cmd_rx.try_recv() {
            queue.push_back(spec);
        }
        let Some(spec) = queue.pop_front() else {
            continue;
        };
        let JobSpec { id, source, dir, stem, settings } = spec;

        // Skip a job cancelled before it ever started (no partial file to clean).
        if is_cancelled(&cancelled, id) {
            forget_cancelled(&cancelled, id);
            let _ = msg_tx.send(ConvertMsg::Cancelled(id));
            continue;
        }

        let _ = msg_tx.send(ConvertMsg::Started(id));
        let cancel_set = cancelled.clone();
        let tx = msg_tx.clone();
        let mut last_sent = 0.0f32;
        let mut progress = |f: f32| {
            // Throttle to ~1% steps so a long file doesn't flood the channel.
            if f - last_sent >= 0.01 || f >= 1.0 {
                last_sent = f;
                let _ = tx.send(ConvertMsg::Progress(id, f));
            }
        };
        let cancel = || is_cancelled(&cancel_set, id);
        // Resolve the output path now, not at enqueue time: an earlier same-named job has
        // already written its file, so this one gets a distinct name instead of clobbering.
        let ext = settings.format.ext();
        let dst = unique_output(&dir, &stem, ext);
        let outcome = match &source {
            JobSource::File(src) => {
                encode::convert_file_cb(src, &dst, settings, &mut progress, &cancel)
            }
            JobSource::Cd { device, track, tags } => {
                encode::convert_cd_cb(device, *track, &dst, settings, tags, &mut progress, &cancel)
            }
        };
        match outcome {
            Ok(ConvertOutcome::Completed) => {
                if let Some(lib) = &lib {
                    let _ = lib.upsert(&crate::playlist::Track::from_path(dst.clone()));
                }
                let _ = msg_tx.send(ConvertMsg::Done(id));
            }
            Ok(ConvertOutcome::Cancelled) => {
                forget_cancelled(&cancelled, id);
                let _ = msg_tx.send(ConvertMsg::Cancelled(id));
            }
            Err(e) => {
                let _ = msg_tx.send(ConvertMsg::Failed(id, short_fail_reason(&e.to_string())));
            }
        }
    }
}

/// Drop a handled id from the cancelled set, so it can't grow without bound over a
/// long session.
fn forget_cancelled(set: &Arc<Mutex<HashSet<u64>>>, id: u64) {
    if let Ok(mut s) = set.lock() {
        s.remove(&id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn job_state_finished_classifies_terminal_states() {
        assert!(!JobState::Queued.is_finished());
        assert!(!JobState::Converting.is_finished());
        assert!(JobState::Done.is_finished());
        assert!(JobState::Failed("x".into()).is_finished());
    }

    #[test]
    fn unique_output_does_not_clobber_same_named_outputs() {
        // Two different sources that share a name (e.g. two song.flac) convert to the
        // same folder and format. Because the worker resolves the path serially, once the
        // first output exists the second must get a distinct name, not overwrite it.
        let dir = std::env::temp_dir().join(format!("linamp_uniq_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let _ = std::fs::remove_file(dir.join("song.ogg"));
        let _ = std::fs::remove_file(dir.join("song (1).ogg"));

        let p1 = unique_output(&dir, "song", "ogg");
        assert_eq!(p1, dir.join("song.ogg"));
        std::fs::write(&p1, b"first output").unwrap(); // first job wrote its file

        let p2 = unique_output(&dir, "song", "ogg");
        assert_ne!(p1, p2, "second same-named output must not reuse the first path");
        assert_eq!(p2, dir.join("song (1).ogg"));

        let _ = std::fs::remove_file(&p1);
        let _ = std::fs::remove_dir(&dir);
    }

    #[test]
    fn sanitize_filename_converts_reserved_chars() {
        assert_eq!(sanitize_filename("AC/DC"), "AC-DC");
        assert_eq!(sanitize_filename("Where Are You?"), "Where Are You_");
        assert_eq!(sanitize_filename("a:b*c|d\"e<f>g\\h"), "a-b+c-d'e(f)g-h");
        // Control characters become spaces, runs collapse, ends trimmed.
        assert_eq!(sanitize_filename("  a\tb  "), "a b");
        assert_eq!(sanitize_filename("trailing dots..."), "trailing dots");
        // Nothing usable left.
        assert_eq!(sanitize_filename("   "), "_");
        assert_eq!(sanitize_filename("..."), "_");
        // Windows reserved device name gets a guard prefix.
        assert_eq!(sanitize_filename("CON"), "_CON");
        assert_eq!(sanitize_filename("nul.mp3"), "_nul.mp3");
    }

    #[test]
    fn rip_timestamp_is_sortable_and_formed() {
        let ts = rip_timestamp();
        // YYYYMMDD-HHMMSS: 15 chars with a dash at index 8, all else digits.
        assert_eq!(ts.len(), 15);
        assert_eq!(&ts[8..9], "-");
        assert!(ts[..8].chars().all(|c| c.is_ascii_digit()));
        assert!(ts[9..].chars().all(|c| c.is_ascii_digit()));
    }

    #[test]
    fn civil_from_days_matches_known_dates() {
        assert_eq!(civil_from_days(0), (1970, 1, 1)); // epoch
        assert_eq!(civil_from_days(18_993), (2022, 1, 1)); // 2022-01-01
        // 1700000000s / 86400 = 19675 days -> 2023-11-14 (UTC).
        assert_eq!(civil_from_days(19_675), (2023, 11, 14));
    }

    #[test]
    fn short_fail_reason_maps_common_cases() {
        // Missing source (Windows and Unix phrasings of os error 2).
        assert_eq!(
            short_fail_reason("The system cannot find the file specified. (os error 2)"),
            "File doesn't exist"
        );
        assert_eq!(
            short_fail_reason("No such file or directory (os error 2)"),
            "File doesn't exist"
        );
        // Permission reading the source.
        assert_eq!(
            short_fail_reason("Permission denied (os error 13)"),
            "Cannot read file"
        );
        // Destination write failures carry a "create "/"write " context.
        assert_eq!(
            short_fail_reason("create C:\\out\\x.mp3: Access is denied. (os error 5)"),
            "Cannot write file"
        );
        assert_eq!(
            short_fail_reason("create C:\\missing\\x.mp3: The system cannot find the path specified. (os error 3)"),
            "Output folder doesn't exist"
        );
        assert_eq!(
            short_fail_reason("write C:\\out\\x.aiff: disk full"),
            "Cannot write file"
        );
        // Decode / unsupported source.
        assert_eq!(short_fail_reason("no audio decoded from a.bin"), "No audio in file");
        assert_eq!(short_fail_reason("unsupported codec"), "Cannot read file");
        // Encoder/resampler internals collapse to a generic reason.
        assert_eq!(short_fail_reason("LAME encode: BadValue"), "Conversion failed");
        assert_eq!(short_fail_reason("opus init: bad argument"), "Conversion failed");
    }

    #[test]
    fn short_fail_reason_falls_back_to_trimmed_first_line() {
        assert_eq!(short_fail_reason("something odd went wrong"), "something odd went wrong");
        let long = "x".repeat(80);
        let out = short_fail_reason(&long);
        assert!(out.ends_with('\u{2026}'));
        assert!(out.chars().count() <= 49);
    }
}
