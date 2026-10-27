//! Cross-instance "open files" queue (#166). LinAmp is single-instance, so a
//! second launch ("Open with", or files dropped on the exe) can't run beside the
//! first. Instead it drops the file paths here and exits, and the running
//! instance picks them up from this queue and opens them.
//!
//! Each request is one small text file (one path per line) published atomically:
//! written to a `.part` temp file, then renamed to `.open`, so the reader never
//! sees a half-written request. The reader deletes each file as it consumes it.

use std::path::{Path, PathBuf};

/// The queue directory, next to config.json / session.json in LinAmp's data dir.
fn dir() -> PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("LinAmp")
        .join("open")
}

/// Drop a request to open `paths` for the running instance. Best-effort: if the
/// write fails, the "Open with" simply does nothing rather than erroring.
pub fn enqueue(paths: &[PathBuf]) {
    enqueue_in(&dir(), paths);
}

/// Take every pending request (oldest first), returning all the paths to open and
/// deleting each request file as it is read.
pub fn drain() -> Vec<PathBuf> {
    drain_in(&dir())
}

fn enqueue_in(d: &Path, paths: &[PathBuf]) {
    if paths.is_empty() || std::fs::create_dir_all(d).is_err() {
        return;
    }
    // Name the file by timestamp then pid so a lexical sort is roughly
    // chronological and two instances never collide on a name.
    let stem = format!("{:020}-{}", nanos(), std::process::id());
    let tmp = d.join(format!("{stem}.part"));
    let done = d.join(format!("{stem}.open"));
    let body = paths
        .iter()
        .map(|p| p.to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("\n");
    if std::fs::write(&tmp, body).is_ok() {
        // Atomic publish: the reader only ever sees a complete `.open` file.
        let _ = std::fs::rename(&tmp, &done);
    }
}

fn drain_in(d: &Path) -> Vec<PathBuf> {
    let Ok(rd) = std::fs::read_dir(d) else {
        return Vec::new();
    };
    let mut files: Vec<PathBuf> = rd
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("open"))
        .collect();
    files.sort();
    let mut paths = Vec::new();
    for f in files {
        if let Ok(body) = std::fs::read_to_string(&f) {
            for line in body.lines() {
                let line = line.trim();
                if !line.is_empty() {
                    paths.push(PathBuf::from(line));
                }
            }
        }
        let _ = std::fs::remove_file(&f);
    }
    paths
}

fn nanos() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("linamp_openq_{}_{}", tag, std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn enqueue_then_drain_returns_paths_and_clears() {
        let d = temp_dir("roundtrip");
        enqueue_in(&d, &[PathBuf::from("/music/a.mp3"), PathBuf::from("/music/b.flac")]);
        let got = drain_in(&d);
        assert_eq!(got, vec![PathBuf::from("/music/a.mp3"), PathBuf::from("/music/b.flac")]);
        // A second drain is empty: each request file is consumed once.
        assert!(drain_in(&d).is_empty());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn multiple_requests_drain_together() {
        let d = temp_dir("multi");
        enqueue_in(&d, &[PathBuf::from("/a.mp3")]);
        enqueue_in(&d, &[PathBuf::from("/b.mp3")]);
        let got = drain_in(&d);
        assert_eq!(got.len(), 2);
        assert!(got.contains(&PathBuf::from("/a.mp3")) && got.contains(&PathBuf::from("/b.mp3")));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn drain_on_missing_dir_is_empty() {
        let d = temp_dir("missing");
        // Never created.
        assert!(drain_in(&d).is_empty());
    }

    #[test]
    fn enqueue_empty_writes_nothing() {
        let d = temp_dir("empty");
        enqueue_in(&d, &[]);
        assert!(drain_in(&d).is_empty());
    }
}
