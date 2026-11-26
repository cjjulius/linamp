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

/// A console/chiptune format whose native tags LinAmp can edit (write back). The
/// remaining gme formats (NSF/GBS/AY/...) and gzip-compressed `.vgz` are read-only.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NativeKind {
    /// The PSF family: tags live in the appended `[TAG]` block.
    Psf,
    /// SID (PSID/RSID): name/author/released in the fixed header.
    Sid,
    /// VGM (uncompressed `.vgm`): tags in the trailing GD3 block (UTF-16LE).
    Vgm,
    /// SPC (SNES SPC700): tags in the fixed ID666 header.
    Spc,
}

/// The writable native kind for `path`, or `None` when LinAmp has no writer for it
/// (a read-only native format, or not a native file at all). Confirms the format's
/// magic, so a DSD `.dsf` (not a PSF) is never reported writable.
pub fn writable_kind(path: &Path) -> Option<NativeKind> {
    let ext = path.extension().and_then(|e| e.to_str())?.to_ascii_lowercase();
    let magic = |n: usize| -> Vec<u8> {
        std::fs::File::open(path)
            .ok()
            .and_then(|mut f| {
                use std::io::Read;
                let mut b = vec![0u8; n];
                f.read_exact(&mut b).ok().map(|_| b)
            })
            .unwrap_or_default()
    };
    if is_psf_family(&ext) && magic(3) == b"PSF" {
        Some(NativeKind::Psf)
    } else if matches!(ext.as_str(), "sid" | "psid" | "rsid")
        && matches!(magic(4).as_slice(), b"PSID" | b"RSID")
    {
        Some(NativeKind::Sid)
    } else if ext == "vgm" && magic(4) == b"Vgm " {
        // Uncompressed VGM only; a gzip-compressed .vgz stays read-only for now.
        Some(NativeKind::Vgm)
    } else if ext == "spc" && magic(27) == b"SNES-SPC700 Sound File Data" {
        Some(NativeKind::Spc)
    } else {
        None
    }
}

/// Write a native format's editable tags back to the file. The write is atomic (a
/// sibling temp file, then rename), so a failure never leaves a half-written file.
/// Only the fields the format supports are written; the rest of the file (the PSF
/// program and its non-tag `[TAG]` keys like `length`/`_lib`, or the SID body) is
/// preserved byte-for-byte.
pub fn write(path: &Path, t: &NativeTags) -> anyhow::Result<()> {
    match writable_kind(path) {
        Some(NativeKind::Psf) => write_psf(path, t),
        Some(NativeKind::Sid) => write_sid(path, t),
        Some(NativeKind::Vgm) => write_vgm(path, t),
        Some(NativeKind::Spc) => write_spc(path, t),
        None => anyhow::bail!("no native tag writer for {}", path.display()),
    }
}

/// Write `data` to `path` atomically: write a sibling temp file, then rename it over
/// the original (rename replaces the destination on both Windows and Unix).
fn atomic_write(path: &Path, data: &[u8]) -> anyhow::Result<()> {
    use anyhow::Context;
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".linamptmp");
    let tmp = std::path::PathBuf::from(tmp);
    fs::write(&tmp, data).with_context(|| format!("writing {}", tmp.display()))?;
    fs::rename(&tmp, path).with_context(|| format!("replacing {}", path.display()))?;
    Ok(())
}

/// Rewrite a PSF file's `[TAG]` block with the edited fields, preserving the header,
/// the compressed program, and every non-edited tag key (`length`, `fade`, `_lib`,
/// `utf8`, ...) so playback and durations are unaffected.
fn write_psf(path: &Path, t: &NativeTags) -> anyhow::Result<()> {
    use anyhow::anyhow;
    let data = fs::read(path)?;
    if data.len() < 16 || &data[0..3] != b"PSF" {
        return Err(anyhow!("not a PSF file: {}", path.display()));
    }
    let reserved = u32::from_le_bytes([data[4], data[5], data[6], data[7]]) as usize;
    let program = u32::from_le_bytes([data[8], data[9], data[10], data[11]]) as usize;
    let prog_end = 16usize
        .checked_add(reserved)
        .and_then(|x| x.checked_add(program))
        .ok_or_else(|| anyhow!("PSF header sizes overflow"))?;
    if prog_end > data.len() {
        return Err(anyhow!("PSF program runs past end of file"));
    }
    // Existing tag text (the bytes after `[TAG]`), if the file already has one.
    let existing = if data.len() >= prog_end + 5 && &data[prog_end..prog_end + 5] == b"[TAG]" {
        String::from_utf8_lossy(&data[prog_end + 5..]).into_owned()
    } else {
        String::new()
    };
    let new_tag = update_psf_tag(&existing, t);
    let mut out = Vec::with_capacity(prog_end + 5 + new_tag.len());
    out.extend_from_slice(&data[..prog_end]); // header + reserved + program, untouched
    if !new_tag.is_empty() {
        out.extend_from_slice(b"[TAG]");
        out.extend_from_slice(new_tag.as_bytes());
    }
    atomic_write(path, &out)
}

/// Merge the edited fields into a PSF `[TAG]` key list parsed from `existing`,
/// keeping every non-edited key in place and dropping an edited field that is now
/// blank. Returns the serialized `key=value\n` block (without the `[TAG]` marker).
fn update_psf_tag(existing: &str, t: &NativeTags) -> String {
    let mut pairs: Vec<(String, String)> = existing
        .lines()
        .filter_map(|l| l.split_once('=').map(|(k, v)| (k.trim().to_string(), v.to_string())))
        .collect();
    // utf8=1: we always write UTF-8, so ensure the flag is set (first, by convention).
    if !pairs.iter().any(|(k, _)| k.eq_ignore_ascii_case("utf8")) {
        pairs.insert(0, ("utf8".to_string(), "1".to_string()));
    }
    let managed: [(&str, &Option<String>); 7] = [
        ("title", &t.title),
        ("artist", &t.artist),
        ("game", &t.album),
        ("year", &t.year),
        ("genre", &t.genre),
        ("comment", &t.comment),
        ("copyright", &t.copyright),
    ];
    for (key, val) in managed {
        // Drop any existing value(s) for this managed key, then re-add if non-blank.
        pairs.retain(|(k, _)| !k.eq_ignore_ascii_case(key));
        if let Some(v) = val {
            let v = v.trim();
            if !v.is_empty() {
                // Values are line-based; fold any newline to a space so one field
                // can't spill into another.
                pairs.push((key.to_string(), v.replace(['\n', '\r'], " ")));
            }
        }
    }
    let mut s = String::new();
    for (k, v) in pairs {
        s.push_str(&k);
        s.push('=');
        s.push_str(v.trim());
        s.push('\n');
    }
    s
}

/// Overwrite a SID file's name / author / released header fields (ISO-8859-1,
/// 32-byte null-padded, at 0x16 / 0x36 / 0x56) in place. The file size and the rest
/// of the header and body are unchanged.
fn write_sid(path: &Path, t: &NativeTags) -> anyhow::Result<()> {
    use anyhow::anyhow;
    let mut data = fs::read(path)?;
    if data.len() < 0x76 || (&data[0..4] != b"PSID" && &data[0..4] != b"RSID") {
        return Err(anyhow!("not a SID file: {}", path.display()));
    }
    let put = |data: &mut [u8], off: usize, s: &Option<String>| {
        let field = &mut data[off..off + 32];
        field.fill(0);
        if let Some(v) = s {
            // ISO-8859-1, capped at 31 bytes so a null terminator always fits.
            let bytes: Vec<u8> = v
                .trim()
                .chars()
                .map(|c| if (c as u32) <= 0xFF { c as u8 } else { b'?' })
                .take(31)
                .collect();
            field[..bytes.len()].copy_from_slice(&bytes);
        }
    };
    put(&mut data, 0x16, &t.title);
    put(&mut data, 0x36, &t.artist);
    put(&mut data, 0x56, &t.album);
    atomic_write(path, &data)
}

/// Read a UTF-16LE null-terminated string at `pos`; returns it and the index just
/// past the terminator (GD3 strings are packed this way).
fn read_utf16z(data: &[u8], pos: usize) -> (String, usize) {
    let mut units = Vec::new();
    let mut p = pos;
    while p + 2 <= data.len() {
        let u = u16::from_le_bytes([data[p], data[p + 1]]);
        p += 2;
        if u == 0 {
            break;
        }
        units.push(u);
    }
    (String::from_utf16_lossy(&units), p)
}

/// Rewrite a VGM file's trailing GD3 tag block (UTF-16LE) with the edited English
/// fields, keeping the Japanese variants and the system/date/dumper/notes. Creates
/// a GD3 if the file has none. The GD3 is the last block, so this replaces it and
/// updates the GD3 offset (0x14) and EOF offset (0x04) in the header.
fn write_vgm(path: &Path, t: &NativeTags) -> anyhow::Result<()> {
    use anyhow::anyhow;
    let data = fs::read(path)?;
    if data.len() < 0x40 || &data[0..4] != b"Vgm " {
        return Err(anyhow!("not a VGM file: {}", path.display()));
    }
    // GD3 offset is relative to 0x14; 0 means there is no GD3 yet.
    let gd3_rel = u32::from_le_bytes([data[0x14], data[0x15], data[0x16], data[0x17]]) as usize;
    // The 11 GD3 strings, parsed from the existing block or left blank.
    let mut strings: Vec<String> = vec![String::new(); 11];
    let mut gd3_start = data.len(); // default: append at EOF when there is no GD3
    if gd3_rel != 0 {
        let off = 0x14 + gd3_rel;
        if off + 12 <= data.len() && &data[off..off + 4] == b"Gd3 " {
            gd3_start = off;
            let mut p = off + 12; // past "Gd3 "(4) + version(4) + length(4)
            for s in strings.iter_mut() {
                let (val, next) = read_utf16z(&data, p);
                *s = val;
                p = next;
            }
        }
    }
    // Update only the English fields; an empty edit clears that field.
    let set = |strings: &mut [String], i: usize, v: &Option<String>| {
        strings[i] = v.as_deref().unwrap_or("").trim().replace('\0', " ");
    };
    set(&mut strings, 0, &t.title); // track name (EN)
    set(&mut strings, 2, &t.album); // game name (EN)
    set(&mut strings, 6, &t.artist); // author (EN)

    let mut body: Vec<u8> = Vec::new();
    for s in &strings {
        for u in s.encode_utf16() {
            body.extend_from_slice(&u.to_le_bytes());
        }
        body.extend_from_slice(&[0, 0]); // null terminator
    }
    let mut out = Vec::with_capacity(gd3_start + 12 + body.len());
    out.extend_from_slice(&data[..gd3_start]); // everything before the old GD3
    out.extend_from_slice(b"Gd3 ");
    out.extend_from_slice(&0x0100u32.to_le_bytes()); // GD3 version 1.00
    out.extend_from_slice(&(body.len() as u32).to_le_bytes());
    out.extend_from_slice(&body);
    // Fix up the header offsets (both relative).
    out[0x14..0x18].copy_from_slice(&((gd3_start - 0x14) as u32).to_le_bytes());
    let eof_rel = (out.len() - 0x04) as u32;
    out[0x04..0x08].copy_from_slice(&eof_rel.to_le_bytes());
    atomic_write(path, &out)
}

/// Overwrite a SPC file's ID666 header tags in place: song title (0x2E), game
/// (0x4E) and comment (0x7E) at fixed offsets, and artist at 0xB1 (text ID666) or
/// 0xB0 (binary ID666). Each is a 32-byte ISO-8859-1 field. The file size and the
/// SPC RAM/DSP state (0x100 on) are untouched.
fn write_spc(path: &Path, t: &NativeTags) -> anyhow::Result<()> {
    use anyhow::anyhow;
    let mut data = fs::read(path)?;
    if data.len() < 0x100 || &data[0..27] != b"SNES-SPC700 Sound File Data" {
        return Err(anyhow!("not a SPC file: {}", path.display()));
    }
    // Mark the header as carrying an ID666 tag (0x23: 26 = present, 27 = absent),
    // since we are writing one.
    data[0x23] = 26;
    let put = |data: &mut [u8], off: usize, len: usize, s: &Option<String>| {
        let field = &mut data[off..off + len];
        field.fill(0);
        if let Some(v) = s {
            let bytes: Vec<u8> = v
                .trim()
                .chars()
                .map(|c| if (c as u32) <= 0xFF { c as u8 } else { b'?' })
                .take(len - 1)
                .collect();
            field[..bytes.len()].copy_from_slice(&bytes);
        }
    };
    put(&mut data, 0x2E, 32, &t.title); // song title
    put(&mut data, 0x4E, 32, &t.album); // game title
    put(&mut data, 0x7E, 32, &t.comment); // comment
    // The artist offset depends on text vs binary ID666 (the length/fade region at
    // 0xA9..0xB0 differs). Text format holds ASCII digits/spaces/nulls there, so
    // pick 0xB1 then, else 0xB0. A misread only nudges the tag, never the audio.
    let text = data[0xA9..0xB1].iter().all(|&b| b.is_ascii_digit() || b == 0 || b == b' ');
    let artist_off = if text { 0xB1 } else { 0xB0 };
    if artist_off + 32 <= data.len() {
        put(&mut data, artist_off, 32, &t.artist);
    }
    atomic_write(path, &data)
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

    #[test]
    fn psf_write_updates_managed_keys_and_preserves_program_and_other_tags() {
        // Minimal PSF: 16-byte header (reserved=0, program=4), a 4-byte program, then
        // a [TAG] with a managed key (title) and non-managed keys (length, _lib) that
        // the edit must keep.
        let program = b"PROG";
        let mut data = Vec::new();
        data.extend_from_slice(b"PSF\x01"); // magic + version
        data.extend_from_slice(&0u32.to_le_bytes()); // reserved size
        data.extend_from_slice(&(program.len() as u32).to_le_bytes()); // program size
        data.extend_from_slice(&0u32.to_le_bytes()); // crc
        data.extend_from_slice(program);
        data.extend_from_slice(b"[TAG]utf8=1\ntitle=Old\nlength=1:23\n_lib=base.psflib\n");
        let p = std::env::temp_dir().join("linamp_psf_write_test.psf");
        std::fs::write(&p, &data).unwrap();

        let edit = NativeTags {
            title: Some("New Title".into()),
            artist: Some("New Artist".into()),
            album: Some("The Game".into()),
            ..Default::default()
        };
        write(&p, &edit).expect("write psf");

        let got = read(&p).expect("read psf");
        assert_eq!(got.title.as_deref(), Some("New Title"));
        assert_eq!(got.artist.as_deref(), Some("New Artist"));
        assert_eq!(got.album.as_deref(), Some("The Game")); // game
        let raw = std::fs::read(&p).unwrap();
        assert_eq!(&raw[16..20], program, "program bytes untouched");
        let text = String::from_utf8_lossy(&raw);
        assert!(text.contains("length=1:23"), "non-managed length tag preserved");
        assert!(text.contains("_lib=base.psflib"), "non-managed _lib tag preserved");
        assert!(!text.contains("title=Old"), "old title replaced, not duplicated");
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn sid_write_round_trips_and_keeps_file_size_and_body() {
        let mut data = vec![0u8; 0x76];
        data[0..4].copy_from_slice(b"PSID");
        data.extend_from_slice(b"\xde\xad\xbe\xef"); // body after the header
        let p = std::env::temp_dir().join("linamp_sid_write_test.sid");
        std::fs::write(&p, &data).unwrap();
        let before = std::fs::metadata(&p).unwrap().len();

        let edit = NativeTags {
            title: Some("New Tune".into()),
            artist: Some("Composer".into()),
            album: Some("2026 Label".into()),
            ..Default::default()
        };
        write(&p, &edit).expect("write sid");
        let got = read(&p).expect("read sid");
        assert_eq!(got.title.as_deref(), Some("New Tune"));
        assert_eq!(got.artist.as_deref(), Some("Composer"));
        assert_eq!(got.album.as_deref(), Some("2026 Label"));
        assert_eq!(before, std::fs::metadata(&p).unwrap().len(), "size unchanged");
        let raw = std::fs::read(&p).unwrap();
        assert_eq!(&raw[0x76..], b"\xde\xad\xbe\xef", "SID body preserved");
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn writable_kind_detects_formats_and_rejects_dsd() {
        let psf = std::env::temp_dir().join("linamp_wk.psf");
        std::fs::write(&psf, b"PSF\x01\0\0\0\0\0\0\0\0\0\0\0\0").unwrap();
        assert_eq!(writable_kind(&psf), Some(NativeKind::Psf));
        let _ = std::fs::remove_file(&psf);

        let sid = std::env::temp_dir().join("linamp_wk.sid");
        std::fs::write(&sid, b"PSID").unwrap();
        assert_eq!(writable_kind(&sid), Some(NativeKind::Sid));
        let _ = std::fs::remove_file(&sid);

        // A DSD .dsf (magic "DSD ") must not be writable as PSF.
        let dsf = std::env::temp_dir().join("linamp_wk.dsf");
        std::fs::write(&dsf, b"DSD \0\0\0\0").unwrap();
        assert_eq!(writable_kind(&dsf), None);
        let _ = std::fs::remove_file(&dsf);

        let vgm = std::env::temp_dir().join("linamp_wk.vgm");
        std::fs::write(&vgm, b"Vgm \0\0\0\0").unwrap();
        assert_eq!(writable_kind(&vgm), Some(NativeKind::Vgm));
        let _ = std::fs::remove_file(&vgm);

        let spc = std::env::temp_dir().join("linamp_wk.spc");
        std::fs::write(&spc, b"SNES-SPC700 Sound File Data v0.30").unwrap();
        assert_eq!(writable_kind(&spc), Some(NativeKind::Spc));
        let _ = std::fs::remove_file(&spc);
    }

    #[test]
    fn vgm_gd3_write_updates_english_fields_and_preserves_the_rest() {
        // Minimal VGM: 0x40 header + 4 bytes of "data", then a GD3 with all 11
        // strings set so the non-edited ones can be checked for survival.
        let mut data = vec![0u8; 0x40];
        data[0..4].copy_from_slice(b"Vgm ");
        let vgm_data = b"DATA";
        data.extend_from_slice(vgm_data);
        let gd3_start = data.len();
        data[0x14..0x18].copy_from_slice(&((gd3_start - 0x14) as u32).to_le_bytes());
        let strings = [
            "TitleEN", "TitleJP", "GameEN", "GameJP", "SysEN", "SysJP", "AuthEN",
            "AuthJP", "2020", "Dumper", "Notes",
        ];
        let mut body = Vec::new();
        for s in strings {
            for u in s.encode_utf16() {
                body.extend_from_slice(&u.to_le_bytes());
            }
            body.extend_from_slice(&[0, 0]);
        }
        data.extend_from_slice(b"Gd3 ");
        data.extend_from_slice(&0x0100u32.to_le_bytes());
        data.extend_from_slice(&(body.len() as u32).to_le_bytes());
        data.extend_from_slice(&body);
        let eof_rel = (data.len() - 0x04) as u32;
        data[0x04..0x08].copy_from_slice(&eof_rel.to_le_bytes());
        let p = std::env::temp_dir().join("linamp_vgm_write.vgm");
        std::fs::write(&p, &data).unwrap();

        let edit = NativeTags {
            title: Some("New Title".into()),
            artist: Some("New Author".into()),
            album: Some("New Game".into()),
            ..Default::default()
        };
        write_vgm(&p, &edit).expect("write vgm");

        let raw = std::fs::read(&p).unwrap();
        assert_eq!(&raw[0x40..0x44], vgm_data, "VGM data preserved");
        let off = 0x14 + u32::from_le_bytes([raw[0x14], raw[0x15], raw[0x16], raw[0x17]]) as usize;
        assert_eq!(&raw[off..off + 4], b"Gd3 ");
        let mut parsed = Vec::new();
        let mut pp = off + 12;
        for _ in 0..11 {
            let (s, n) = read_utf16z(&raw, pp);
            parsed.push(s);
            pp = n;
        }
        assert_eq!(parsed[0], "New Title"); // title EN updated
        assert_eq!(parsed[2], "New Game"); // game EN updated
        assert_eq!(parsed[6], "New Author"); // author EN updated
        assert_eq!(parsed[1], "TitleJP"); // JP preserved
        assert_eq!(parsed[5], "SysJP"); // system JP preserved
        assert_eq!(parsed[9], "Dumper"); // dumper preserved
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn spc_id666_text_write_updates_fields_and_keeps_audio() {
        // Minimal SPC: 0x100 header (text ID666) + a 4-byte "RAM" body.
        let mut data = vec![0u8; 0x100];
        data[0..27].copy_from_slice(b"SNES-SPC700 Sound File Data");
        data[0x23] = 26; // has ID666
        for b in data[0xA9..0xB1].iter_mut() {
            *b = b'0'; // text length/fade region, so artist is read at 0xB1
        }
        data.extend_from_slice(b"RAM!");
        let p = std::env::temp_dir().join("linamp_spc_write.spc");
        std::fs::write(&p, &data).unwrap();
        let before = std::fs::metadata(&p).unwrap().len();

        let edit = NativeTags {
            title: Some("Song".into()),
            artist: Some("Composer".into()),
            album: Some("Game".into()),
            comment: Some("Hi".into()),
            ..Default::default()
        };
        write_spc(&p, &edit).expect("write spc");

        let raw = std::fs::read(&p).unwrap();
        let field = |off: usize, len: usize| -> String {
            let b = &raw[off..off + len];
            let end = b.iter().position(|&x| x == 0).unwrap_or(len);
            b[..end].iter().map(|&x| x as char).collect()
        };
        assert_eq!(field(0x2E, 32), "Song");
        assert_eq!(field(0x4E, 32), "Game");
        assert_eq!(field(0x7E, 32), "Hi");
        assert_eq!(field(0xB1, 32), "Composer"); // text ID666 -> artist at 0xB1
        assert_eq!(before, std::fs::metadata(&p).unwrap().len(), "size unchanged");
        assert_eq!(&raw[0x100..0x104], b"RAM!", "SPC body preserved");
        let _ = std::fs::remove_file(&p);
    }
}
