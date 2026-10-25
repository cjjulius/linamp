//! Copy the bundled asset folders (`skins/`, `soundfont/`) next to the built
//! executable so files edited in the repo are picked up by
//! `target/<profile>/linamp.exe` (which discovers them relative to itself),
//! keeping the two in sync.

use std::{
    env, fs,
    path::{Path, PathBuf},
};

fn main() {
    emit_build_info();
    embed_windows_icon();

    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    // OUT_DIR = target/<profile>/build/<pkg>-<hash>/out → go up to target/<profile>.
    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    let Some(target_dir) = out_dir.ancestors().nth(3).map(PathBuf::from) else {
        return;
    };
    copy_asset_dir(&manifest, &target_dir, "skins");
    copy_asset_dir(&manifest, &target_dir, "soundfont");
}

/// Mirror `manifest/<name>` into `target_dir/<name>` (top-level files only).
fn copy_asset_dir(manifest: &Path, target_dir: &Path, name: &str) {
    println!("cargo:rerun-if-changed={name}");
    let src = manifest.join(name);
    if !src.is_dir() {
        return;
    }
    let dst = target_dir.join(name);
    let _ = fs::create_dir_all(&dst);
    if let Ok(entries) = fs::read_dir(&src) {
        for e in entries.flatten() {
            let p = e.path();
            if p.is_file() {
                let _ = fs::copy(&p, dst.join(e.file_name()));
            }
        }
    }
}

/// Stamp the git short hash (with a `-dirty` marker when the working tree has
/// uncommitted changes) and the commit date into the binary, so the running app
/// can be matched to an exact source state. Values fall back to "unknown" when
/// git isn't available (e.g. building from a source tarball).
fn emit_build_info() {
    let mut hash = git(&["rev-parse", "--short", "HEAD"]).unwrap_or_else(|| "unknown".into());
    let dirty = git(&["status", "--porcelain"])
        .map(|s| !s.trim().is_empty())
        .unwrap_or(false);
    if dirty {
        hash.push_str("-dirty");
    }
    println!("cargo:rustc-env=GIT_HASH={hash}");

    let date = git(&["show", "-s", "--format=%cd", "--date=short", "HEAD"])
        .unwrap_or_else(|| "unknown".into());
    println!("cargo:rustc-env=COMMIT_DATE={date}");

    // Re-run when the commit changes or the working tree does, so the hash and
    // the -dirty flag stay accurate. `.git/logs/HEAD` (the reflog) is appended on
    // every commit/checkout/reset — unlike `.git/HEAD`, which only records the
    // branch name and so doesn't change when you commit on a branch. `.git/index`
    // covers staging and `src` covers uncommitted edits (the dirty flag).
    println!("cargo:rerun-if-changed=.git/logs/HEAD");
    println!("cargo:rerun-if-changed=.git/HEAD");
    println!("cargo:rerun-if-changed=.git/index");
    println!("cargo:rerun-if-changed=src");
}

/// Run a git command in the manifest dir, returning its trimmed stdout on success.
fn git(args: &[&str]) -> Option<String> {
    let dir = env::var("CARGO_MANIFEST_DIR").ok()?;
    let out = std::process::Command::new("git")
        .current_dir(dir)
        .args(args)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!s.is_empty()).then_some(s)
}

/// On Windows, embed `assets/linamp.ico` into the executable so Explorer and the
/// taskbar show the LinAmp icon. No-op on other platforms.
#[cfg(windows)]
fn embed_windows_icon() {
    println!("cargo:rerun-if-changed=assets/linamp.ico");
    if std::path::Path::new("assets/linamp.ico").exists() {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/linamp.ico");
        let _ = res.compile();
    }
}

#[cfg(not(windows))]
fn embed_windows_icon() {}
