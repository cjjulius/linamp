//! Read-only metadata for the console / chiptune formats whose tags live in
//! format-native fields that lofty and symphonia do not read: the PSF family's
//! `[TAG]` block, the SID (PSID/RSID) header, and the libgme info record. The
//! playlist and the tag editor's Basic Info tab show these so a PSF / SID /
//! chiptune is not stuck displaying its filename.
//!
//! This is read-only. The PSF `[TAG]` and SID header are read straight from the
//! file bytes (no decoder needed, so they work in every build); the libgme info
//! needs the `gme` feature. Writing these tags is a later phase of the roadmap.

use std::fs;
use std::path::Path;

/// Native metadata read from a console / chiptune file. Every field is optional;
/// an absent or blank field is `None`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NativeTags {
    pub title: Option<String>,
    pub artist: Option<String>,
    /// The game / collection the tune belongs to (PSF `game`, libgme `game`, SID
    /// `released`), shown as the album.
    pub album: Option<String>,
    pub year: Option<String>,
    pub genre: Option<String>,
    pub comment: Option<String>,
    pub copyright: Option<String>,
}

impl NativeTags {
    /// True when nothing worth showing was read (no title, artist or album).
    fn is_bare(&self) -> bool {
        self.title.is_none() && self.artist.is_none() && self.album.is_none()
    }
}

/// Read a file's native metadata, routing by extension. `None` for a format with
/// no native tags here, or when nothing readable was found. Pure reads; no decode.
pub fn read(path: &Path) -> Option<NativeTags> {
    let ext = path.extension().and_then(|e| e.to_str())?.to_ascii_lowercase();
    if is_psf_family(&ext) {
        return psf_tags(path);
    }
    if matches!(ext.as_str(), "sid" | "psid" | "rsid") {
        return sid_tags(path);
    }
    #[cfg(feature = "gme")]
    if crate::gme::is_gme(path) {
        return crate::gme::tags(path);
    }
    None
}

/// Extensions of the PSF family (PSF1/PSF2, SSF/DSF, QSF, NCSF, USF, 2SF, GSF),
/// which all share the `[TAG]` block, so one parser serves them. `.dsf` is listed
/// but a DSD `.dsf` (which starts with `DSD `, not `PSF`) is rejected by the magic
/// check in `psf_tags`, so it is never misread as SSF.
fn is_psf_family(ext: &str) -> bool {
    matches!(
        ext,
        "psf" | "minipsf"
            | "psf2"
            | "minipsf2"
            | "ssf"
            | "minissf"
            | "dsf"
            | "minidsf"
            | "dcsf"
            | "minidcsf"
            | "qsf"
            | "miniqsf"
            | "ncsf"
            | "minincsf"
            | "usf"
            | "miniusf"
            | "2sf"
            | "mini2sf"
            | "gsf"
            | "minigsf"
    )
}

/// Parse the PSF `[TAG]` block, the universal PSF-family metadata convention:
/// `[TAG]` followed by UTF-8 `key=value` lines, sitting after the compressed
/// program, so the last `[TAG]` in the file is the real one (mirrors
/// `psf::tagged_length_secs`). The `PSF` magic is required, so a DSD `.dsf` is
/// skipped. `game` maps to the album.
fn psf_tags(path: &Path) -> Option<NativeTags> {
    let data = fs::read(path).ok()?;
    if data.len() < 4 || &data[0..3] != b"PSF" {
        return None;
    }
    let pos = data.windows(5).rposition(|w| w == b"[TAG]")? + 5;
    let text = String::from_utf8_lossy(&data[pos..]);
    let mut t = NativeTags::default();
    for line in text.lines() {
        let Some((key, val)) = line.split_once('=') else {
            continue;
        };
        let val = val.trim();
        if val.is_empty() {
            continue;
        }
        match key.trim().to_ascii_lowercase().as_str() {
            "title" => t.title = Some(val.to_string()),
            "artist" => t.artist = Some(val.to_string()),
            "game" => t.album = Some(val.to_string()),
            "year" => t.year = Some(val.to_string()),
            "genre" => t.genre = Some(val.to_string()),
            "comment" => t.comment = Some(val.to_string()),
            "copyright" => t.copyright = Some(val.to_string()),
            _ => {}
        }
    }
    (!t.is_bare()).then_some(t)
}

/// Read the SID (PSID/RSID) header's fixed metadata: name, author and released,
/// each a 32-byte ISO-8859-1 field at a fixed offset (0x16 / 0x36 / 0x56), present
/// in every PSID version. name -> title, author -> artist, released -> album.
fn sid_tags(path: &Path) -> Option<NativeTags> {
    let data = fs::read(path).ok()?;
    // The three text fields run through 0x76 (0x56 + 32).
    if data.len() < 0x76 || (&data[0..4] != b"PSID" && &data[0..4] != b"RSID") {
        return None;
    }
    // A 32-byte, null-padded ISO-8859-1 field. Latin-1 maps each byte to the same
    // Unicode code point, so `b as char` decodes it directly.
    let field = |off: usize| -> Option<String> {
        let raw = &data[off..off + 32];
        let end = raw.iter().position(|&b| b == 0).unwrap_or(raw.len());
        let s: String = raw[..end].iter().map(|&b| b as char).collect();
        let s = s.trim().to_string();
        (!s.is_empty()).then_some(s)
    };
    let t = NativeTags {
        title: field(0x16),
        artist: field(0x36),
        album: field(0x56),
        ..Default::default()
    };
    (!t.is_bare()).then_some(t)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn psf_tag_block_parses_known_keys() {
        let p = std::env::temp_dir().join("linamp_native_psf_test.psf");
        std::fs::write(
            &p,
            b"PSF\x01....program....[TAG]utf8=1\ntitle=Stage 1\nartist=Follin\ngame=Some Game\nyear=1991\n",
        )
        .unwrap();
        let t = read(&p).expect("psf tags");
        assert_eq!(t.title.as_deref(), Some("Stage 1"));
        assert_eq!(t.artist.as_deref(), Some("Follin"));
        assert_eq!(t.album.as_deref(), Some("Some Game")); // game -> album
        assert_eq!(t.year.as_deref(), Some("1991"));
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn psf_without_magic_is_rejected() {
        // A DSD .dsf starts with "DSD ", not "PSF", so it must not be tag-read even
        // though it carries the .dsf extension and a stray "[TAG]".
        let p = std::env::temp_dir().join("linamp_native_notpsf.dsf");
        std::fs::write(&p, b"DSD \x00\x00\x00\x00[TAG]title=nope\n").unwrap();
        assert!(read(&p).is_none());
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn sid_header_fields_decode() {
        // Minimal PSID: magic + a zeroed header through 0x76, with the three text
        // fields set at their fixed offsets.
        let mut data = vec![0u8; 0x76];
        data[0..4].copy_from_slice(b"PSID");
        let put = |d: &mut Vec<u8>, off: usize, s: &[u8]| d[off..off + s.len()].copy_from_slice(s);
        put(&mut data, 0x16, b"Commando");
        put(&mut data, 0x36, b"Rob Hubbard");
        put(&mut data, 0x56, b"1985 Elite");
        let p = std::env::temp_dir().join("linamp_native_test.sid");
        std::fs::write(&p, &data).unwrap();
        let t = read(&p).expect("sid tags");
        assert_eq!(t.title.as_deref(), Some("Commando"));
        assert_eq!(t.artist.as_deref(), Some("Rob Hubbard"));
        assert_eq!(t.album.as_deref(), Some("1985 Elite")); // released -> album
        let _ = std::fs::remove_file(&p);
    }
}
