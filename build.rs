//! Copy the bundled asset folders (`skins/`, `soundfont/`, `music/`) next to the
//! built executable so files edited in the repo are picked up by
//! `target/<profile>/linamp.exe` (which discovers them relative to itself),
//! keeping the two in sync. The `music/` mirror also means a dev or release run
//! reads its own copy of the demo tracks, so the pristine repo `music/` folder is
//! never written to (the demo loader can rewrite tags on the copy it uses).

use std::{
    env, fs,
    path::{Path, PathBuf},
};

fn main() {
    emit_build_info();
    embed_windows_icon();

    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    // OUT_DIR = target/<profile>/build/<pkg>-<hash>/out, go up to target/<profile>.
    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    let Some(target_dir) = out_dir.ancestors().nth(3).map(PathBuf::from) else {
        return;
    };
    copy_asset_dir(&manifest, &target_dir, "skins");
    copy_asset_dir(&manifest, &target_dir, "soundfont");
    copy_asset_dir(&manifest, &target_dir, "music");
    link_gme(&manifest, &target_dir);
    link_openmpt(&manifest, &target_dir);
    link_adplug(&manifest, &target_dir);
    link_sid(&manifest, &target_dir);
    link_vgmstream(&manifest, &target_dir);
    link_psf(&manifest, &target_dir);
    link_ssf(&manifest, &target_dir);
}

/// Link libsidplayfp for the `sid` feature: compile the C++ shim (src/sid_shim.cpp) that gives
/// libsidplayfp a C API, link it, and bundle the shared library next to the binary (runtime
/// resolution via an `$ORIGIN/lib` rpath), the same self-contained pattern as the other native
/// features. The library and headers are found at `$SIDPLAYFP_DIR` (a prefix with
/// `lib/libsidplayfp.so` and `include/sidplayfp/*.h`), defaulting to the in-repo
/// `build/sidplayfp-min` that `packaging/linux/build-sidplayfp-min.sh` produces. No-op without the
/// feature.
fn link_sid(manifest: &Path, target_dir: &Path) {
    println!("cargo:rerun-if-env-changed=SIDPLAYFP_DIR");
    if env::var_os("CARGO_FEATURE_SID").is_none() {
        return;
    }
    println!("cargo:rerun-if-changed=src/sid_shim.cpp");
    let dir = env::var_os("SIDPLAYFP_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| manifest.join("build/sidplayfp-min"));
    let incdir = dir.join("include");
    let libdir = dir.join("lib");

    cc::Build::new()
        .cpp(true)
        .file("src/sid_shim.cpp")
        .include(&incdir)
        .compile("sid_shim");

    println!("cargo:rustc-link-search=native={}", libdir.display());
    println!("cargo:rustc-link-lib=dylib=sidplayfp");

    #[cfg(not(windows))]
    {
        println!("cargo:rustc-link-arg=-Wl,-rpath,$ORIGIN/lib");
        println!("cargo:rustc-link-arg=-Wl,--disable-new-dtags");
        let dst = target_dir.join("lib");
        let _ = fs::create_dir_all(&dst);
        if let Ok(entries) = fs::read_dir(&libdir) {
            for e in entries.flatten() {
                let name = e.file_name();
                if name.to_string_lossy().contains("libsidplayfp.so") {
                    let _ = fs::copy(e.path(), dst.join(&name));
                }
            }
        }
    }
    #[cfg(windows)]
    let _ = target_dir;
}

/// Link AdPlug for the `adplug` feature: compile the small C++ shim (src/adplug_shim.cpp) that
/// gives AdPlug a C API, link libadplug + libbinio, and bundle both shared libraries next to the
/// binary (runtime resolution via an `$ORIGIN/lib` rpath), the same self-contained pattern as
/// libgme/libopenmpt. The libraries and headers are found at `$ADPLUG_DIR` (a prefix with
/// `lib/libadplug.so` and `include/adplug/*.h`), defaulting to the in-repo `build/adplug-min`
/// that `packaging/linux/build-adplug-min.sh` produces. No-op without the feature.
fn link_adplug(manifest: &Path, target_dir: &Path) {
    println!("cargo:rerun-if-env-changed=ADPLUG_DIR");
    if env::var_os("CARGO_FEATURE_ADPLUG").is_none() {
        return;
    }
    println!("cargo:rerun-if-changed=src/adplug_shim.cpp");
    let dir = env::var_os("ADPLUG_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| manifest.join("build/adplug-min"));
    let incdir = dir.join("include");
    let libdir = dir.join("lib");

    // Compile the C++ shim into a static lib linked into the crate. cc emits the link directive
    // for the shim and pulls in the C++ standard library. AdPlug's headers include <binio.h>,
    // which the libbinio package installs under include/libbinio, so both are on the include path.
    cc::Build::new()
        .cpp(true)
        .file("src/adplug_shim.cpp")
        .include(&incdir)
        .include(incdir.join("libbinio"))
        .compile("adplug_shim");

    println!("cargo:rustc-link-search=native={}", libdir.display());
    println!("cargo:rustc-link-lib=dylib=adplug");
    println!("cargo:rustc-link-lib=dylib=binio");

    #[cfg(not(windows))]
    {
        println!("cargo:rustc-link-arg=-Wl,-rpath,$ORIGIN/lib");
        println!("cargo:rustc-link-arg=-Wl,--disable-new-dtags");
        let dst = target_dir.join("lib");
        let _ = fs::create_dir_all(&dst);
        if let Ok(entries) = fs::read_dir(&libdir) {
            for e in entries.flatten() {
                let name = e.file_name();
                let n = name.to_string_lossy();
                if (n.contains("libadplug") || n.contains("libbinio")) && n.contains(".so") {
                    let _ = fs::copy(e.path(), dst.join(&name));
                }
            }
        }
    }
    #[cfg(windows)]
    let _ = target_dir;
}

/// Link libgme for the `gme` feature and copy its shared library next to the binary (into
/// `target/<profile>/lib`, resolved at runtime via an `$ORIGIN/lib` rpath), so dev and release
/// runs are self-contained in the same way the FFmpeg feature bundles libav*. The library is
/// found at `$GME_DIR` (a prefix with `lib/libgme.so`), defaulting to the in-repo
/// `build/gme-min` that `packaging/linux/build-gme-min.sh` produces. No-op without the feature.
fn link_gme(manifest: &Path, target_dir: &Path) {
    println!("cargo:rerun-if-env-changed=GME_DIR");
    if env::var_os("CARGO_FEATURE_GME").is_none() {
        return;
    }
    let dir = env::var_os("GME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| manifest.join("build/gme-min"));
    let libdir = dir.join("lib");
    println!("cargo:rustc-link-search=native={}", libdir.display());
    println!("cargo:rustc-link-lib=dylib=gme");

    // Non-Windows: resolve the bundled libgme.so next to the executable at runtime, and copy
    // it there so a dev/release run works without a system libgme. (Windows packaging ships
    // the DLL beside the exe, handled by the installer, so this host branch is Unix-only.)
    #[cfg(not(windows))]
    {
        println!("cargo:rustc-link-arg=-Wl,-rpath,$ORIGIN/lib");
        println!("cargo:rustc-link-arg=-Wl,--disable-new-dtags");
        let dst = target_dir.join("lib");
        let _ = fs::create_dir_all(&dst);
        if let Ok(entries) = fs::read_dir(&libdir) {
            for e in entries.flatten() {
                let name = e.file_name();
                if name.to_string_lossy().contains("libgme.so") {
                    let _ = fs::copy(e.path(), dst.join(&name));
                }
            }
        }
    }
    #[cfg(windows)]
    let _ = target_dir;
}

/// Link libopenmpt for the `openmpt` feature and copy its shared library next to the binary,
/// exactly as `link_gme` does for libgme (runtime resolution via an `$ORIGIN/lib` rpath). The
/// library is found at `$OPENMPT_DIR` (a prefix with `lib/libopenmpt.so`), defaulting to the
/// in-repo `build/openmpt-min` that `packaging/linux/build-openmpt-min.sh` produces. No-op
/// without the feature.
fn link_openmpt(manifest: &Path, target_dir: &Path) {
    println!("cargo:rerun-if-env-changed=OPENMPT_DIR");
    if env::var_os("CARGO_FEATURE_OPENMPT").is_none() {
        return;
    }
    let dir = env::var_os("OPENMPT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| manifest.join("build/openmpt-min"));
    let libdir = dir.join("lib");
    println!("cargo:rustc-link-search=native={}", libdir.display());
    println!("cargo:rustc-link-lib=dylib=openmpt");

    #[cfg(not(windows))]
    {
        println!("cargo:rustc-link-arg=-Wl,-rpath,$ORIGIN/lib");
        println!("cargo:rustc-link-arg=-Wl,--disable-new-dtags");
        let dst = target_dir.join("lib");
        let _ = fs::create_dir_all(&dst);
        if let Ok(entries) = fs::read_dir(&libdir) {
            for e in entries.flatten() {
                let name = e.file_name();
                if name.to_string_lossy().contains("libopenmpt.so") {
                    // Follow the symlink and copy the real shared object under each name, so a
                    // dev link into a system libopenmpt bundles a usable file (not a dangling
                    // link) next to the binary.
                    let _ = fs::copy(e.path(), dst.join(&name));
                }
            }
        }
    }
    #[cfg(windows)]
    let _ = target_dir;
}

/// Link libvgmstream for the `vgmstream` feature and copy its shared library next to the binary,
/// exactly as `link_gme` does for libgme (runtime resolution via an `$ORIGIN/lib` rpath).
/// libvgmstream exposes a C API directly, so there is no shim to compile. We link the core-only
/// build (no ffmpeg/vorbis/mpg123 externals), which is fully permissive (ISC) and self-contained
/// (depends only on libc/libm). The library and headers are found at `$VGMSTREAM_DIR` (a prefix
/// with `lib/libvgmstream.so` and `include/libvgmstream.h`), defaulting to the in-repo
/// `build/vgmstream-min` that `packaging/linux/build-vgmstream-min.sh` produces. No-op without the
/// feature.
fn link_vgmstream(manifest: &Path, target_dir: &Path) {
    println!("cargo:rerun-if-env-changed=VGMSTREAM_DIR");
    if env::var_os("CARGO_FEATURE_VGMSTREAM").is_none() {
        return;
    }
    let dir = env::var_os("VGMSTREAM_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| manifest.join("build/vgmstream-min"));
    let libdir = dir.join("lib");
    println!("cargo:rustc-link-search=native={}", libdir.display());
    println!("cargo:rustc-link-lib=dylib=vgmstream");

    #[cfg(not(windows))]
    {
        println!("cargo:rustc-link-arg=-Wl,-rpath,$ORIGIN/lib");
        println!("cargo:rustc-link-arg=-Wl,--disable-new-dtags");
        let dst = target_dir.join("lib");
        let _ = fs::create_dir_all(&dst);
        if let Ok(entries) = fs::read_dir(&libdir) {
            for e in entries.flatten() {
                let name = e.file_name();
                if name.to_string_lossy().contains("libvgmstream.so") {
                    let _ = fs::copy(e.path(), dst.join(&name));
                }
            }
        }
    }
    #[cfg(windows)]
    let _ = target_dir;
}

/// Link libupse for the `psf` feature: compile the small C shim (src/psf_shim.c) that wires stdio
/// into libupse's I/O callbacks and exposes a tiny API, link libupse, and bundle the shared library
/// next to the binary (runtime resolution via an `$ORIGIN/lib` rpath), the same self-contained
/// pattern as the other native features. The library and headers are found at `$UPSE_DIR` (a prefix
/// with `lib/libupse.so` and `include/upse.h`), defaulting to the in-repo `build/upse-min` that
/// `packaging/linux/build-upse-min.sh` produces. libupse is ROM-less (its own HLE BIOS; no PlayStation
/// BIOS). No-op without the feature.
fn link_psf(manifest: &Path, target_dir: &Path) {
    println!("cargo:rerun-if-env-changed=UPSE_DIR");
    if env::var_os("CARGO_FEATURE_PSF").is_none() {
        return;
    }
    println!("cargo:rerun-if-changed=src/psf_shim.c");
    let dir = env::var_os("UPSE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| manifest.join("build/upse-min"));
    let incdir = dir.join("include");
    let libdir = dir.join("lib");

    // Compile the C shim into a static lib linked into the crate; it includes libupse's headers.
    cc::Build::new()
        .file("src/psf_shim.c")
        .include(&incdir)
        .compile("psf_shim");

    println!("cargo:rustc-link-search=native={}", libdir.display());
    println!("cargo:rustc-link-lib=dylib=upse");

    #[cfg(not(windows))]
    {
        println!("cargo:rustc-link-arg=-Wl,-rpath,$ORIGIN/lib");
        println!("cargo:rustc-link-arg=-Wl,--disable-new-dtags");
        let dst = target_dir.join("lib");
        let _ = fs::create_dir_all(&dst);
        if let Ok(entries) = fs::read_dir(&libdir) {
            for e in entries.flatten() {
                let name = e.file_name();
                if name.to_string_lossy().contains("libupse.so") {
                    let _ = fs::copy(e.path(), dst.join(&name));
                }
            }
        }
    }
    #[cfg(windows)]
    let _ = target_dir;
}

/// Link Highly_Theoretical + psflib for the `ssf` feature: compile the C shim (src/ssf_shim.c) that
/// drives psflib (container/`_lib` parsing) and the Highly_Theoretical Sega sound-chip emulator and
/// exposes a tiny API, link both shared libraries, and bundle them next to the binary (runtime
/// resolution via an `$ORIGIN/lib` rpath), the same self-contained pattern as the other native
/// features. The emulator is found at `$HIGHLYTHEORETICAL_DIR` (`lib/libhighlytheoretical.so` +
/// `include/sega.h`) and the shared container parser at `$PSFLIB_DIR` (`lib/libpsflib.so` +
/// `include/psflib.h`), defaulting to the in-repo `build/highlytheoretical-min` and `build/psflib-min`
/// that `packaging/linux/build-highly-theoretical-min.sh` and `build-psflib-min.sh` produce.
/// Highly_Theoretical is ROM-less (emulates only the SCSP/AICA; no console BIOS). No-op without the
/// feature.
fn link_ssf(manifest: &Path, target_dir: &Path) {
    println!("cargo:rerun-if-env-changed=HIGHLYTHEORETICAL_DIR");
    println!("cargo:rerun-if-env-changed=PSFLIB_DIR");
    if env::var_os("CARGO_FEATURE_SSF").is_none() {
        return;
    }
    println!("cargo:rerun-if-changed=src/ssf_shim.c");
    let ht = env::var_os("HIGHLYTHEORETICAL_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| manifest.join("build/highlytheoretical-min"));
    let psflib = env::var_os("PSFLIB_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| manifest.join("build/psflib-min"));

    // Compile the C shim; it includes both the emulator and psflib headers.
    cc::Build::new()
        .file("src/ssf_shim.c")
        .include(ht.join("include"))
        .include(psflib.join("include"))
        .compile("ssf_shim");

    println!("cargo:rustc-link-search=native={}", ht.join("lib").display());
    println!("cargo:rustc-link-search=native={}", psflib.join("lib").display());
    println!("cargo:rustc-link-lib=dylib=highlytheoretical");
    println!("cargo:rustc-link-lib=dylib=psflib");
    println!("cargo:rustc-link-lib=dylib=z");

    #[cfg(not(windows))]
    {
        println!("cargo:rustc-link-arg=-Wl,-rpath,$ORIGIN/lib");
        println!("cargo:rustc-link-arg=-Wl,--disable-new-dtags");
        let dst = target_dir.join("lib");
        let _ = fs::create_dir_all(&dst);
        for libdir in [ht.join("lib"), psflib.join("lib")] {
            if let Ok(entries) = fs::read_dir(&libdir) {
                for e in entries.flatten() {
                    let name = e.file_name();
                    let n = name.to_string_lossy();
                    if n.contains("libhighlytheoretical.so") || n.contains("libpsflib.so") {
                        let _ = fs::copy(e.path(), dst.join(&name));
                    }
                }
            }
        }
    }
    #[cfg(windows)]
    let _ = target_dir;
}

/// Mirror `manifest/<name>` into `target_dir/<name>` (top-level files only),
/// pruning files that no longer exist in the source so stale or test-only assets
/// can't linger in the shipped folder (example: test skins kept in qa-tools must
/// never reach `target/<profile>/skins`).
fn copy_asset_dir(manifest: &Path, target_dir: &Path, name: &str) {
    println!("cargo:rerun-if-changed={name}");
    let src = manifest.join(name);
    if !src.is_dir() {
        return;
    }
    let dst = target_dir.join(name);
    let _ = fs::create_dir_all(&dst);
    let mut want = std::collections::HashSet::new();
    if let Ok(entries) = fs::read_dir(&src) {
        for e in entries.flatten() {
            let p = e.path();
            if p.is_file() {
                let _ = fs::copy(&p, dst.join(e.file_name()));
                want.insert(e.file_name());
            }
        }
    }
    // Delete any file in the target that isn't a current source asset.
    if let Ok(entries) = fs::read_dir(&dst) {
        for e in entries.flatten() {
            let p = e.path();
            if p.is_file() && !want.contains(&e.file_name()) {
                let _ = fs::remove_file(&p);
            }
        }
    }
}

/// Stamp the git short hash (with a `-dirty` marker when the working tree has
/// uncommitted changes) and the commit date into the binary, so the running app
/// can be matched to an exact source state. Values fall back to "unknown" when
/// git isn't available (example: building from a source tarball).
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
    // every commit/checkout/reset, unlike `.git/HEAD`, which only records the
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
