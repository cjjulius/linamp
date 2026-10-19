//! Copy the bundled `skins/` folder next to the built executable so a skin
//! edited in the repo is picked up by `target/<profile>/linamp.exe` (which
//! discovers skins relative to itself), keeping the two in sync.

use std::{env, fs, path::PathBuf};

fn main() {
    println!("cargo:rerun-if-changed=skins");

    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let src = manifest.join("skins");
    if !src.is_dir() {
        return;
    }

    // OUT_DIR = target/<profile>/build/<pkg>-<hash>/out → go up to target/<profile>.
    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    let Some(target_dir) = out_dir.ancestors().nth(3) else {
        return;
    };
    let dst = target_dir.join("skins");
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
