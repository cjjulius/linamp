//! MP3 ID3 tag editing via `lofty`: reads ID3v1 and ID3v2 independently, reads
//! MPEG format info, and writes each tag on its own. The ID3v2 tag is modified
//! in place so frames we don't edit (album art, album artist, disc, BPM, …)
//! survive a save.

use std::path::Path;

use anyhow::{Context, Result};
use lofty::tag::ItemKey;

/// Editable ID3 fields. ID3v1 uses only `track`..=`comment`; ID3v2 uses all.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct Id3Fields {
    pub track: String,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub year: String,
    pub genre: String,
    pub comment: String,
    // ID3v2-only below.
    pub composer: String,
    pub orig_artist: String,
    pub copyright: String,
    pub url: String,
    pub encoded_by: String,
}

/// Read-only format / file info for the Basic Info tab.
#[derive(Clone, Default)]
pub struct FileInfo {
    pub path: String,
    pub size_bytes: u64,
    pub length_secs: u64,
    pub bitrate_kbps: u32,
    pub sample_rate_hz: u32,
    pub mpeg: String,
    pub channel_mode: String,
    pub copyrighted: bool,
    pub original: bool,
    pub emphasis: String,
}

/// Extensions LinAmp can currently *write* tags for. Reading isn't restricted.
pub const EDITABLE_EXTS: &[&str] = &["mp3"];

/// ID3v1 text-field byte cap (title/artist/album/comment); year is 4.
pub const V1_MAX_TEXT: usize = 30;
pub const V1_MAX_YEAR: usize = 4;

/// Standard ID3v1 genre names (index = numeric genre), used for the dropdown.
pub const GENRES: &[&str] = &[
    "Blues", "Classic Rock", "Country", "Dance", "Disco", "Funk", "Grunge",
    "Hip-Hop", "Jazz", "Metal", "New Age", "Oldies", "Other", "Pop", "R&B",
    "Rap", "Reggae", "Rock", "Techno", "Industrial", "Alternative", "Ska",
    "Death Metal", "Pranks", "Soundtrack", "Euro-Techno", "Ambient",
    "Trip-Hop", "Vocal", "Jazz+Funk", "Fusion", "Trance", "Classical",
    "Instrumental", "Acid", "House", "Game", "Sound Clip", "Gospel", "Noise",
    "Alternative Rock", "Bass", "Soul", "Punk", "Space", "Meditative",
    "Instrumental Pop", "Instrumental Rock", "Ethnic", "Gothic", "Darkwave",
    "Techno-Industrial", "Electronic", "Pop-Folk", "Eurodance", "Dream",
    "Southern Rock", "Comedy", "Cult", "Gangsta", "Top 40", "Christian Rap",
    "Pop/Funk", "Jungle", "Native US", "Cabaret", "New Wave", "Psychadelic",
    "Rave", "Showtunes", "Trailer", "Lo-Fi", "Tribal", "Acid Punk",
    "Acid Jazz", "Polka", "Retro", "Musical", "Rock & Roll", "Hard Rock",
    "Folk", "Folk-Rock", "National Folk", "Swing", "Fast Fusion", "Bebob",
    "Latin", "Revival", "Celtic", "Bluegrass", "Avantgarde", "Gothic Rock",
    "Progressive Rock", "Psychedelic Rock", "Symphonic Rock", "Slow Rock",
    "Big Band", "Chorus", "Easy Listening", "Acoustic", "Humour", "Speech",
    "Chanson", "Opera", "Chamber Music", "Sonata", "Symphony", "Booty Bass",
    "Primus", "Porn Groove", "Satire", "Slow Jam", "Club", "Tango", "Samba",
    "Folklore", "Ballad", "Power Ballad", "Rhythmic Soul", "Freestyle",
    "Duet", "Punk Rock", "Drum Solo", "A capella", "Euro-House", "Dance Hall",
];

pub fn can_edit(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| EDITABLE_EXTS.contains(&e.to_lowercase().as_str()))
        .unwrap_or(false)
}

/// Read the ID3v1 and ID3v2 tags; `None` when that tag isn't present.
pub fn read_id3(path: &Path) -> (Option<Id3Fields>, Option<Id3Fields>) {
    use lofty::file::TaggedFileExt;
    use lofty::tag::TagType;
    let Ok(tagged) = lofty::read_from_path(path) else {
        return (None, None);
    };
    let v1 = tagged.tag(TagType::Id3v1).map(fields_from_tag);
    let v2 = tagged.tag(TagType::Id3v2).map(fields_from_tag);
    (v1, v2)
}

fn fields_from_tag(tag: &lofty::tag::Tag) -> Id3Fields {
    use lofty::tag::Accessor;
    let key = |k: &ItemKey| tag.get_string(k).map(|s| s.to_string()).unwrap_or_default();
    Id3Fields {
        track: tag.track().map(|t| t.to_string()).unwrap_or_default(),
        title: tag.title().map(|s| s.to_string()).unwrap_or_default(),
        artist: tag.artist().map(|s| s.to_string()).unwrap_or_default(),
        album: tag.album().map(|s| s.to_string()).unwrap_or_default(),
        year: tag.year().map(|y| y.to_string()).unwrap_or_default(),
        genre: tag.genre().map(|s| s.to_string()).unwrap_or_default(),
        comment: tag.comment().map(|s| s.to_string()).unwrap_or_default(),
        composer: key(&ItemKey::Composer),
        orig_artist: key(&ItemKey::OriginalArtist),
        copyright: key(&ItemKey::CopyrightMessage),
        // URL lives in a WOAS locator frame, not a text frame.
        url: tag
            .get(&ItemKey::AudioSourceUrl)
            .and_then(|i| match i.value() {
                lofty::tag::ItemValue::Text(s) | lofty::tag::ItemValue::Locator(s) => {
                    Some(s.clone())
                }
                _ => None,
            })
            .unwrap_or_default(),
        encoded_by: key(&ItemKey::EncodedBy),
    }
}

/// Read MPEG format info for the Basic Info tab (best-effort; blank on failure).
pub fn read_info(path: &Path) -> FileInfo {
    use lofty::config::ParseOptions;
    use lofty::file::AudioFile;
    use lofty::mpeg::{ChannelMode, Emphasis, Layer, MpegFile, MpegVersion};

    let mut info = FileInfo {
        path: path.display().to_string(),
        size_bytes: std::fs::metadata(path).map(|m| m.len()).unwrap_or(0),
        ..Default::default()
    };
    // The MPEG parser only populates this for MPEG audio, but on any other file it
    // scans the whole thing looking for frames before failing — seconds of freeze on
    // a large .wav/.aiff when the tag editor opens. Only run it for MPEG extensions.
    let is_mpeg = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| matches!(e.to_ascii_lowercase().as_str(), "mp3" | "mp2" | "mp1" | "mpa"))
        .unwrap_or(false);
    if is_mpeg {
        if let Ok(mut f) = std::fs::File::open(path) {
        if let Ok(mpeg) = MpegFile::read_from(&mut f, ParseOptions::new()) {
            let p = mpeg.properties();
            info.length_secs = p.duration().as_secs();
            info.bitrate_kbps = p.audio_bitrate();
            info.sample_rate_hz = p.sample_rate();
            let ver = match p.version() {
                MpegVersion::V1 => "MPEG-1",
                MpegVersion::V2 => "MPEG-2",
                MpegVersion::V2_5 => "MPEG-2.5",
                MpegVersion::V4 => "MPEG-4",
            };
            let layer = match p.layer() {
                Layer::Layer1 => "Layer 1",
                Layer::Layer2 => "Layer 2",
                Layer::Layer3 => "Layer 3",
            };
            info.mpeg = format!("{ver} {layer}");
            info.channel_mode = match p.channel_mode() {
                ChannelMode::Stereo => "Stereo",
                ChannelMode::JointStereo => "Joint Stereo",
                ChannelMode::DualChannel => "Dual Channel",
                ChannelMode::SingleChannel => "Mono",
            }
            .to_string();
            info.copyrighted = p.is_copyright();
            info.original = p.is_original();
            info.emphasis = match p.emphasis() {
                None => "None",
                Some(Emphasis::MS5015) => "50/15 ms",
                Some(Emphasis::Reserved) => "Reserved",
                Some(Emphasis::CCIT_J17) => "CCIT J.17",
            }
            .to_string();
            }
        }
    }
    info
}

/// Prepare an ID3v1 text field: trim, drop if empty and cap to `max` bytes on a
/// UTF-8 char boundary. ID3v1 fields are fixed-width; lofty truncates to the cap
/// by byte index and panics if a multibyte character straddles it (#115), which
/// aborts the save mid-write and corrupts the file. Capping here avoids that.
fn v1_text(s: &str, max: usize) -> Option<String> {
    // ID3v1 predates Unicode and carries no encoding flag, and the writer emits
    // these bytes as UTF-8, which a pre-Unicode ("circa 2001") player decodes as
    // Latin-1, turning any multi-byte character into mojibake. To stay in spec and
    // readable everywhere, keep ID3v1 to pure ASCII: fold accents to their base
    // letter (é to e, Ğ to g) and replace anything with no ASCII equivalent with
    // '?'. The full Unicode text is preserved in ID3v2.
    let ascii: String = s
        .chars()
        .map(|c| {
            let f = crate::classic::ascii_fold(c);
            if f.is_ascii() { f } else { '?' }
        })
        .collect();
    let s = ascii.trim();
    if s.is_empty() {
        return None;
    }
    // Pure ASCII now, so byte index == char index; the boundary guard is a no-op
    // kept for safety.
    let mut end = s.len().min(max);
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    Some(s[..end].to_string())
}

/// Write the ID3 tags. For each of v1/v2: `Some` writes/updates it, `None`
/// removes it. The ID3v2 tag is edited in place so other frames survive.
pub fn write_id3(path: &Path, v1: Option<&Id3Fields>, v2: Option<&Id3Fields>) -> Result<()> {
    use lofty::config::WriteOptions;
    use lofty::file::TaggedFileExt;
    use lofty::tag::{Tag, TagExt, TagType};

    if !can_edit(path) {
        anyhow::bail!(
            "editing tags for this file type isn't supported yet (only {})",
            EDITABLE_EXTS.join(", ")
        );
    }
    // ID3v2 first: start from the existing tag so we don't drop other frames.
    match v2 {
        Some(f) => {
            let existing = lofty::read_from_path(path)
                .ok()
                .and_then(|t| t.tag(TagType::Id3v2).cloned());
            let mut tag = existing.unwrap_or_else(|| Tag::new(TagType::Id3v2));
            apply_v2(&mut tag, f);
            tag.save_to_path(path, WriteOptions::default())
                .with_context(|| format!("writing ID3v2 to {}", path.display()))?;
        }
        None => {
            let _ = TagType::Id3v2.remove_from_path(path);
        }
    }
    // ID3v1 is positional (not frame-keyed), so build the concrete tag, the
    // generic Tag's set_year doesn't persist for ID3v1.
    match v1 {
        Some(f) => {
            use lofty::id3::v1::Id3v1Tag;
            let t = Id3v1Tag {
                title: v1_text(&f.title, V1_MAX_TEXT),
                artist: v1_text(&f.artist, V1_MAX_TEXT),
                album: v1_text(&f.album, V1_MAX_TEXT),
                year: v1_text(&f.year, 4),
                // ID3v1.1 reserves the last 2 bytes of the 30-byte comment for the
                // track number, so lofty caps comment at 28, match it here.
                comment: v1_text(&f.comment, V1_MAX_TEXT - 2),
                track_number: f.track.trim().parse::<u8>().ok(),
                genre: GENRES
                    .iter()
                    .position(|g| g.eq_ignore_ascii_case(f.genre.trim()))
                    .map(|i| i as u8),
            };
            t.save_to_path(path, WriteOptions::default())
                .with_context(|| format!("writing ID3v1 to {}", path.display()))?;
        }
        None => {
            let _ = TagType::Id3v1.remove_from_path(path);
        }
    }
    Ok(())
}

/// Apply the editable fields to an ID3v2 `tag`. A blank field clears its frame
/// rather than storing an empty value.
fn apply_v2(tag: &mut lofty::tag::Tag, f: &Id3Fields) {
    use lofty::tag::Accessor;
    macro_rules! str_f {
        ($v:expr, $set:ident, $rm:ident) => {
            if $v.trim().is_empty() {
                tag.$rm();
            } else {
                tag.$set($v.clone());
            }
        };
    }
    macro_rules! num_f {
        ($v:expr, $set:ident, $rm:ident) => {
            match $v.trim().parse::<u32>() {
                Ok(n) => tag.$set(n),
                Err(_) => tag.$rm(),
            }
        };
    }
    macro_rules! key_f {
        ($v:expr, $k:expr) => {
            if $v.trim().is_empty() {
                tag.remove_key(&$k);
            } else {
                tag.insert_text($k, $v.clone());
            }
        };
    }

    str_f!(f.title, set_title, remove_title);
    str_f!(f.artist, set_artist, remove_artist);
    str_f!(f.album, set_album, remove_album);
    str_f!(f.genre, set_genre, remove_genre);
    str_f!(f.comment, set_comment, remove_comment);
    num_f!(f.year, set_year, remove_year);
    num_f!(f.track, set_track, remove_track);
    {
        key_f!(f.composer, ItemKey::Composer);
        key_f!(f.orig_artist, ItemKey::OriginalArtist);
        key_f!(f.copyright, ItemKey::CopyrightMessage);
        key_f!(f.encoded_by, ItemKey::EncodedBy);
        // URL is a WOAS locator frame (not text).
        if f.url.trim().is_empty() {
            tag.remove_key(&ItemKey::AudioSourceUrl);
        } else {
            tag.insert(lofty::tag::TagItem::new(
                ItemKey::AudioSourceUrl,
                lofty::tag::ItemValue::Locator(f.url.clone()),
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn v1_text_is_always_pure_ascii() {
        // Latin Extended accents fold to base ASCII...
        assert_eq!(
            v1_text("\u{11E}\u{171}itar \u{124}\u{171}\u{14B}\u{167}", V1_MAX_TEXT),
            Some("guitar hunt".to_string())
        );
        // ...and so do Latin-1 accents (ID3v1 can't safely hold them).
        assert_eq!(v1_text("Café ñandú", V1_MAX_TEXT), Some("Cafe nandu".to_string()));
        // Characters with no ASCII equivalent become '?'.
        assert_eq!(v1_text("A\u{65E5}B", V1_MAX_TEXT), Some("A?B".to_string()));
        // The result is always pure ASCII; plain text is unchanged; blank to None.
        let out = v1_text("Hello", V1_MAX_TEXT).unwrap();
        assert!(out.is_ascii() && out == "Hello");
        assert_eq!(v1_text("   ", V1_MAX_TEXT), None);
    }

    #[test]
    fn editable_exts() {
        assert!(can_edit(Path::new("song.mp3")));
        assert!(can_edit(Path::new("SONG.MP3")));
        assert!(!can_edit(Path::new("song.flac")));
        assert!(!can_edit(Path::new("noext")));
    }

    #[test]
    fn write_rejects_unsupported() {
        let err = write_id3(Path::new("x.flac"), None, None).unwrap_err();
        assert!(err.to_string().contains("isn't supported"));
    }

    #[test]
    fn genres_start_with_the_standard_list() {
        assert_eq!(GENRES[0], "Blues");
        assert_eq!(GENRES[17], "Rock");
        assert!(GENRES.len() >= 80);
    }

    #[test]
    fn mp3_v1_v2_roundtrip() {
        // Uses the bundled sample; skip gracefully if the checkout lacks it.
        let src = Path::new("music/Aria Nocturne - Crystal Harbor.mp3");
        if !src.exists() {
            return;
        }
        let tmp = std::env::temp_dir().join("linamp_id3_roundtrip.mp3");
        std::fs::copy(src, &tmp).unwrap();

        let v2 = Id3Fields {
            track: "3".into(),
            title: "V2 Title".into(),
            artist: "V2 Artist".into(),
            album: "V2 Album".into(),
            year: "2024".into(),
            genre: "Rock".into(),
            comment: "v2 comment".into(),
            composer: "A. Composer".into(),
            orig_artist: "Orig".into(),
            copyright: "(c) 2024".into(),
            url: "https://example.com".into(),
            encoded_by: "LinAmp".into(),
        };
        let v1 = Id3Fields {
            track: "3".into(),
            title: "V1 Title".into(),
            artist: "V1 Artist".into(),
            album: "V1 Album".into(),
            year: "1999".into(),
            genre: "Jazz".into(),
            comment: "v1 comment".into(),
            ..Default::default()
        };
        write_id3(&tmp, Some(&v1), Some(&v2)).unwrap();

        let (gv1, gv2) = read_id3(&tmp);
        let gv1 = gv1.expect("v1 present");
        let gv2 = gv2.expect("v2 present");
        // Every ID3v2 field round-trips (not just a sample), covering the full
        // editable frame set exposed in the tag editor.
        assert_eq!(gv2.track, "3");
        assert_eq!(gv2.title, "V2 Title");
        assert_eq!(gv2.artist, "V2 Artist");
        assert_eq!(gv2.album, "V2 Album");
        assert_eq!(gv2.year, "2024");
        assert_eq!(gv2.genre, "Rock");
        assert_eq!(gv2.comment, "v2 comment");
        assert_eq!(gv2.composer, "A. Composer");
        assert_eq!(gv2.orig_artist, "Orig");
        assert_eq!(gv2.copyright, "(c) 2024");
        assert_eq!(gv2.url, "https://example.com");
        assert_eq!(gv2.encoded_by, "LinAmp");
        // ID3v1 fields round-trip too.
        assert_eq!(gv1.title, "V1 Title");
        assert_eq!(gv1.artist, "V1 Artist");
        assert_eq!(gv1.year, "1999");

        // Removing ID3v1 leaves ID3v2 intact.
        write_id3(&tmp, None, Some(&v2)).unwrap();
        let (gv1, gv2) = read_id3(&tmp);
        assert!(gv1.is_none());
        assert_eq!(gv2.unwrap().title, "V2 Title");

        let _ = std::fs::remove_file(&tmp);
    }

    #[test]
    fn id3v1_long_multibyte_fields_dont_corrupt() {
        // #115: copying long/multibyte ID3v2 values into ID3v1 and saving must not
        // corrupt the audio. ID3v1 fields are capped at 30 bytes; the value must be
        // truncated on a char boundary (never mid-UTF-8) before writing.
        let src = Path::new("music/Aria Nocturne - Crystal Harbor.mp3");
        if !src.exists() {
            return;
        }
        let tmp = std::env::temp_dir().join("linamp_id3_longfields.mp3");
        std::fs::copy(src, &tmp).unwrap();
        let before = read_info(&tmp).length_secs;

        let long = "Trường Sơn Đông, Trường Sơn Tây, ".repeat(2); // >30 bytes, multibyte
        let v1 = Id3Fields {
            track: "5".into(),
            title: long.clone(),
            artist: long.clone(),
            album: long.clone(),
            year: "2024".into(),
            genre: "Rock".into(),
            comment: long.clone(),
            ..Default::default()
        };
        write_id3(&tmp, Some(&v1), None).unwrap();

        // Audio must survive: same (±1s) duration, still readable.
        let after = read_info(&tmp).length_secs;
        assert!(
            (after as i64 - before as i64).abs() <= 1,
            "duration changed after v1 write: {before} -> {after}"
        );
        let (gv1, _) = read_id3(&tmp);
        let gv1 = gv1.expect("v1 tag present and readable");
        // ID3v1 is a fixed 30-char (latin-1) field, so at most 30 characters survive.
        assert!(gv1.title.chars().count() <= V1_MAX_TEXT, "title: {:?}", gv1.title);
        let _ = std::fs::remove_file(&tmp);
    }
}
