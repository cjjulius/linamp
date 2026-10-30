//! The file-conversion queue (the rip/convert UI, part 1).
//!
//! A single background worker converts queued files one at a time and reports
//! structured progress, so the Media Library's "Converting" pane can show a per-file
//! progress bar. Jobs can be added while the worker runs, and cancelled: cancelling
//! the running file aborts it and deletes its partial output; cancelling a queued file
//! skips it. The worker holds its own library connection (shared WAL db) to index each
//! finished output, and the UI refreshes the library when a job completes.

use std::collections::{HashSet, VecDeque};
use std::path::PathBuf;
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

/// What the UI hands the worker to convert.
pub struct JobSpec {
    pub id: u64,
    pub src: PathBuf,
    pub dst: PathBuf,
    pub settings: ConvertSettings,
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
        let id = spec.id;

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
        let outcome =
            encode::convert_file_cb(&spec.src, &spec.dst, spec.settings, &mut progress, &cancel);
        match outcome {
            Ok(ConvertOutcome::Completed) => {
                if let Some(lib) = &lib {
                    let _ = lib.upsert(&crate::playlist::Track::from_path(spec.dst.clone()));
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
