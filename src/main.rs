// Hide the console window on Windows release builds (GUI app).
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod balance;
mod cdda;
mod classic;
mod config;
mod cursor;
mod decode;
mod dock;
mod encode;
mod eq;
mod eqf;
mod fileassoc;
mod library;
mod mediakeys;
mod midi;
mod openq;
mod player;
mod playlist;
mod region;
mod selection;
mod skin;
mod stream;
mod tags;
mod text;
mod tracker;
mod tray;
mod viz;
#[cfg(windows)]
mod wincursor;

use app::LinAmpApp;

fn main() -> eframe::Result<()> {
    let args: Vec<String> = std::env::args().collect();

    // Headless cursor-parse check: `linamp --check-cur a.wsz b.wsz ...`
    // Loads each skin (which parses its embedded .cur files) and reports how
    // many cursors came back plus how long the load took, to catch a decoder
    // that hangs or takes an absurd amount of time on a malformed cursor.
    if args.iter().any(|a| a == "--check-cur") {
        for p in args.iter().skip_while(|a| *a != "--check-cur").skip(1) {
            let t = std::time::Instant::now();
            match classic::ClassicSkin::load(std::path::Path::new(p)) {
                Ok(s) => println!(
                    "{p}: {} cursors, has_cursors={} in {:?}",
                    s.cursor_count(),
                    s.has_cursors(),
                    t.elapsed()
                ),
                Err(e) => println!("{p}: load error: {e}"),
            }
        }
        return Ok(());
    }

    // Headless skin colour verifier: `linamp --check-skin a.wsz b.wsz ...`
    if args.iter().any(|a| a == "--check-skin") {
        for p in args.iter().skip_while(|a| *a != "--check-skin").skip(1) {
            skin::describe(std::path::Path::new(p));
        }
        return Ok(());
    }

    // Headless classic-render preview: `linamp --render-skin in.wsz out.png`
    // or `--render-all` for main+EQ+playlist stacked.
    for (flag, all) in [("--render-skin", false), ("--render-all", true)] {
        if let Some(i) = args.iter().position(|a| a == flag) {
            let inp = args.get(i + 1);
            let out = args.get(i + 2);
            if let (Some(inp), Some(out)) = (inp, out) {
                let (inp, out) = (std::path::Path::new(inp), std::path::Path::new(out));
                let r = if all {
                    classic::render_all_png(inp, out)
                } else {
                    classic::render_main_png(inp, out)
                };
                match r {
                    Ok(()) => println!("rendered {} -> {}", inp.display(), out.display()),
                    Err(e) => println!("render error: {e}"),
                }
            } else {
                println!("usage: linamp {flag} <in.wsz> <out.png>");
            }
            return Ok(());
        }
    }

    // Headless Media Library frame preview: `linamp --render-gen in.wsz out.png`.
    if let Some(i) = args.iter().position(|a| a == "--render-gen") {
        if let (Some(inp), Some(out)) = (args.get(i + 1), args.get(i + 2)) {
            let (inp, out) = (std::path::Path::new(inp), std::path::Path::new(out));
            match classic::render_gen_png(inp, out) {
                Ok(()) => println!("rendered gen {} -> {}", inp.display(), out.display()),
                Err(e) => println!("render error: {e}"),
            }
        } else {
            println!("usage: linamp --render-gen <in.wsz> <out.png>");
        }
        return Ok(());
    }

    // Instance model (#193): the lock holder is the "primary" (first launched). It
    // owns the open-request queue, so files always land in one deterministic instance.
    // A launch that passes files (Open with) always routes them to the primary and
    // exits, never opening a rival window. A bare second launch (no files) is a
    // deliberate request for another instance and runs as a secondary. The OS frees
    // the lock on exit or crash, so a stale lock can never wedge startup.
    let launch_files: Vec<std::path::PathBuf> = {
        // Resolve each path against this launcher's cwd: the primary has a different
        // cwd, so a relative path (from a terminal launch) would otherwise resolve
        // against the wrong directory once handed over.
        let cwd = std::env::current_dir().ok();
        args.iter()
            .skip(1)
            .map(|a| {
                let p = std::path::PathBuf::from(a);
                match &cwd {
                    Some(cwd) if p.is_relative() => cwd.join(p),
                    _ => p,
                }
            })
            .collect()
    };
    let (_instance_lock, is_primary) = match acquire_instance_lock() {
        Instance::AlreadyRunning => {
            // A primary is already running. Hand it any files (Open with) through the
            // queue, which only the primary drains, so they open in that one instance.
            if !launch_files.is_empty() {
                openq::enqueue(&launch_files);
                return Ok(());
            }
            // No files: this is a deliberate second launch, so run as a secondary.
            (None, false)
        }
        Instance::Acquired(file) => {
            // This launch owns the lock, so it is the primary. Drop any stale
            // open-requests a previous (crashed) primary left behind, so they aren't
            // replayed as our startup files before the window begins polling the queue.
            openq::clear();
            (Some(file), true)
        }
        // Locking unavailable (e.g. data dir not writable): act as primary, best-effort.
        Instance::Unavailable => (None, true),
    };

    // Register LinAmp as a candidate handler for its audio types (#166), so it
    // shows in Open with / Default Apps and opens files launched with it. Per-user
    // and idempotent; done each launch so it also tracks a moved executable. Only
    // on a real GUI launch (the headless flags above have already returned).
    fileassoc::register();

    // Start borderless at classic skin size (the app sets the real mode on the
    // first frame once it knows whether a skin loaded).
    let mut viewport = egui::ViewportBuilder::default()
        .with_inner_size([550.0, 232.0])
        .with_decorations(false)
        .with_resizable(false)
        .with_title("LinAmp");
    if let Some(icon) = load_icon() {
        viewport = viewport.with_icon(icon);
    }
    // System tray mode (#40) hides the taskbar button, so the tray icon is the
    // app's only presence. egui can only set this at window creation, which is why
    // the window mode applies at launch. A missing/false pref keeps the taskbar.
    if config::Prefs::load().tray_enabled {
        viewport = viewport.with_taskbar(false);
    }
    let options = eframe::NativeOptions {
        viewport,
        // The classic UI uses several borderless child windows rendered as
        // immediate viewports. With vsync on, each frame blocks ~16ms on the
        // first child window's buffer swap, pegging the main thread and making
        // input feel laggy. We throttle repaints ourselves (30fps when active,
        // 4fps idle), so vsync buys nothing here, turn it off.
        vsync: false,
        ..Default::default()
    };

    eframe::run_native(
        "LinAmp",
        options,
        Box::new(move |cc| Ok(Box::new(LinAmpApp::new(cc, is_primary)))),
    )
}

/// Decode the embedded LinAmp logo into an egui window/taskbar icon.
fn load_icon() -> Option<egui::IconData> {
    let bytes = include_bytes!("../assets/linamp_logo_256.png");
    let img = image::load_from_memory(bytes).ok()?.to_rgba8();
    let (width, height) = img.dimensions();
    Some(egui::IconData {
        rgba: img.into_raw(),
        width,
        height,
    })
}

/// Outcome of the single-instance check.
enum Instance {
    /// This process holds the lock; keep the file alive for its lifetime.
    Acquired(std::fs::File),
    /// Another LinAmp already holds the lock, so this launch should exit.
    AlreadyRunning,
    /// The lock couldn't be set up (e.g. the data dir isn't writable); start
    /// anyway rather than refuse to launch over a locking problem.
    Unavailable,
}

/// Try to acquire the single-instance lock at `path`. Uses an advisory file lock,
/// which the OS releases automatically when the file is dropped or the process
/// dies, so a crash never leaves a stale lock behind.
fn acquire_instance_lock_at(path: &std::path::Path) -> Instance {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let file = match std::fs::OpenOptions::new().create(true).write(true).truncate(false).open(path) {
        Ok(f) => f,
        Err(_) => return Instance::Unavailable,
    };
    match file.try_lock() {
        Ok(()) => Instance::Acquired(file),
        Err(std::fs::TryLockError::WouldBlock) => Instance::AlreadyRunning,
        Err(std::fs::TryLockError::Error(_)) => Instance::Unavailable,
    }
}

/// The instance lock's path, in LinAmp's data directory (next to config.json). The
/// lock holder is the primary instance (#193); secondaries re-check it to take over.
pub(crate) fn instance_lock_path() -> std::path::PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join("LinAmp")
        .join("instance.lock")
}

/// The real single-instance lock, in LinAmp's data directory (next to config.json).
fn acquire_instance_lock() -> Instance {
    acquire_instance_lock_at(&instance_lock_path())
}

/// Try to grab the instance lock at `path` without blocking, returning the locked
/// file handle on success. A secondary instance calls this periodically so that when
/// the primary exits (freeing the lock) it can take over as the new primary (#193):
/// keep the returned handle alive to hold the lock. `None` means another instance
/// still holds it, or the lock dir is unwritable.
pub(crate) fn try_lock_instance(path: &std::path::Path) -> Option<std::fs::File> {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let file = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(path)
        .ok()?;
    match file.try_lock() {
        Ok(()) => Some(file),
        Err(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_instance_lock_refuses_a_second_holder() {
        let p = std::env::temp_dir().join("linamp_instance_guard_test.lock");
        let _ = std::fs::remove_file(&p);
        let held = match acquire_instance_lock_at(&p) {
            Instance::Acquired(f) => f,
            _ => panic!("the first launch should acquire the lock"),
        };
        // A second launch while the first holds it is told another instance runs.
        assert!(
            matches!(acquire_instance_lock_at(&p), Instance::AlreadyRunning),
            "a second holder must be refused"
        );
        // Dropping the first (as on exit) frees it for the next launch.
        drop(held);
        assert!(
            matches!(acquire_instance_lock_at(&p), Instance::Acquired(_)),
            "the lock is free again after the first holder exits"
        );
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn try_lock_instance_hands_over_when_the_holder_drops() {
        let p = std::env::temp_dir().join("linamp_handover_test.lock");
        let _ = std::fs::remove_file(&p);
        // The first caller becomes primary.
        let held = try_lock_instance(&p).expect("first caller acquires the lock");
        // A secondary polling while the primary holds it gets nothing.
        assert!(
            try_lock_instance(&p).is_none(),
            "a secondary cannot take the lock while the primary holds it"
        );
        // The primary exits (handle dropped): the next poll takes over as primary.
        drop(held);
        assert!(
            try_lock_instance(&p).is_some(),
            "a secondary takes over once the primary releases the lock"
        );
        let _ = std::fs::remove_file(&p);
    }
}
