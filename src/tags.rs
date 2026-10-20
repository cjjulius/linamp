//! Track metadata via `lofty`: reading works for every format lofty supports;
//! writing starts with MP3 (ID3v2, see [`EDITABLE_EXTS`]) and extends to more
//! formats by growing that list as each is verified.

use std::path::Path;

use anyhow::{Context, Result};

/// The editable tag fields shared across formats. Numeric fields are kept as
/// strings so the editor's text boxes can round-trip them unchanged.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct TagFields {
    pub title: String,
    pub artist: String,
    pub album: String,
    pub album_artist: String,
    pub year: String,
    pub track: String,
    pub disc: String,
    pub genre: String,
    pub composer: String,
    pub bpm: String,
    pub comment: String,
}

/// Extensions LinAmp can currently *write* tags for. Expand as formats are
/// verified; reading is not restricted to this list.
pub const EDITABLE_EXTS: &[&str] = &["mp3"];

/// Whether tag *writing* is supported for this file yet.
pub fn can_edit(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| EDITABLE_EXTS.contains(&e.to_lowercase().as_str()))
        .unwrap_or(false)
}

/// Read tags into [`TagFields`] (empty fields when a file has no tag).
pub fn read(path: &Path) -> TagFields {
    use lofty::file::TaggedFileExt;
    use lofty::tag::{Accessor, ItemKey};

    let mut f = TagFields::default();
    if let Ok(tagged) = lofty::read_from_path(path) {
        if let Some(tag) = tagged.primary_tag().or_else(|| tagged.first_tag()) {
            f.title = tag.title().map(|s| s.to_string()).unwrap_or_default();
            f.artist = tag.artist().map(|s| s.to_string()).unwrap_or_default();
            f.album = tag.album().map(|s| s.to_string()).unwrap_or_default();
            f.genre = tag.genre().map(|s| s.to_string()).unwrap_or_default();
            f.comment = tag.comment().map(|s| s.to_string()).unwrap_or_default();
            f.year = tag.year().map(|y| y.to_string()).unwrap_or_default();
            f.track = tag.track().map(|t| t.to_string()).unwrap_or_default();
            let key = |k: &ItemKey| tag.get_string(k).map(|s| s.to_string()).unwrap_or_default();
            f.album_artist = key(&ItemKey::AlbumArtist);
            f.disc = key(&ItemKey::DiscNumber);
            f.composer = key(&ItemKey::Composer);
            f.bpm = key(&ItemKey::IntegerBpm);
        }
    }
    f
}

/// Write `fields` back to `path`. Returns an error for formats not yet
/// supported (see [`can_edit`]) or on any I/O / tag failure.
pub fn write(path: &Path, fields: &TagFields) -> Result<()> {
    use lofty::config::WriteOptions;
    use lofty::file::TaggedFileExt;
    use lofty::tag::{Accessor, ItemKey, Tag, TagExt};

    if !can_edit(path) {
        anyhow::bail!(
            "editing tags for this file type isn't supported yet (only {})",
            EDITABLE_EXTS.join(", ")
        );
    }

    let tagged =
        lofty::read_from_path(path).with_context(|| format!("reading {}", path.display()))?;
    let mut tag = tagged
        .primary_tag()
        .cloned()
        .unwrap_or_else(|| Tag::new(tagged.primary_tag_type()));

    // A blank field clears that tag rather than storing an empty string.
    macro_rules! str_field {
        ($val:expr, $set:ident, $remove:ident) => {
            if $val.trim().is_empty() {
                tag.$remove();
            } else {
                tag.$set($val.clone());
            }
        };
    }
    macro_rules! num_field {
        ($val:expr, $set:ident, $remove:ident) => {
            match $val.trim().parse::<u32>() {
                Ok(n) => tag.$set(n),
                Err(_) => tag.$remove(),
            }
        };
    }

    // ItemKey-addressed text frames: clear when blank, else store verbatim.
    macro_rules! key_field {
        ($val:expr, $key:expr) => {
            if $val.trim().is_empty() {
                tag.remove_key(&$key);
            } else {
                tag.insert_text($key, $val.clone());
            }
        };
    }

    str_field!(fields.title, set_title, remove_title);
    str_field!(fields.artist, set_artist, remove_artist);
    str_field!(fields.album, set_album, remove_album);
    str_field!(fields.genre, set_genre, remove_genre);
    str_field!(fields.comment, set_comment, remove_comment);
    num_field!(fields.year, set_year, remove_year);
    num_field!(fields.track, set_track, remove_track);
    key_field!(fields.album_artist, ItemKey::AlbumArtist);
    key_field!(fields.disc, ItemKey::DiscNumber);
    key_field!(fields.composer, ItemKey::Composer);
    key_field!(fields.bpm, ItemKey::IntegerBpm);

    tag.save_to_path(path, WriteOptions::default())
        .with_context(|| format!("writing tags to {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn editable_exts() {
        assert!(can_edit(Path::new("song.mp3")));
        assert!(can_edit(Path::new("SONG.MP3")));
        assert!(!can_edit(Path::new("song.flac")));
        assert!(!can_edit(Path::new("noext")));
    }

    #[test]
    fn write_rejects_unsupported() {
        let err = write(Path::new("x.flac"), &TagFields::default()).unwrap_err();
        assert!(err.to_string().contains("isn't supported"));
    }

    #[test]
    fn fields_default_empty() {
        let f = TagFields::default();
        assert!(f.title.is_empty() && f.year.is_empty());
    }

    #[test]
    fn mp3_write_read_roundtrip() {
        // Uses the bundled sample; skip gracefully if the checkout lacks it.
        let src = Path::new("music/Aria Nocturne - Crystal Harbor.mp3");
        if !src.exists() {
            return;
        }
        let tmp = std::env::temp_dir().join("linamp_tag_roundtrip.mp3");
        std::fs::copy(src, &tmp).unwrap();

        let fields = TagFields {
            title: "Test Title".into(),
            artist: "Test Artist".into(),
            album: "Test Album".into(),
            album_artist: "Various".into(),
            year: "2024".into(),
            track: "3".into(),
            disc: "1".into(),
            genre: "Ambient".into(),
            composer: "A. Composer".into(),
            bpm: "128".into(),
            comment: "a comment".into(),
        };
        write(&tmp, &fields).unwrap();

        let got = read(&tmp);
        assert_eq!(got.title, "Test Title");
        assert_eq!(got.artist, "Test Artist");
        assert_eq!(got.album, "Test Album");
        assert_eq!(got.album_artist, "Various");
        assert_eq!(got.year, "2024");
        assert_eq!(got.track, "3");
        assert_eq!(got.disc, "1");
        assert_eq!(got.genre, "Ambient");
        assert_eq!(got.composer, "A. Composer");
        assert_eq!(got.bpm, "128");
        assert_eq!(got.comment, "a comment");

        // A blank field clears the tag.
        let cleared = TagFields {
            title: String::new(),
            ..fields
        };
        write(&tmp, &cleared).unwrap();
        assert!(read(&tmp).title.is_empty());

        let _ = std::fs::remove_file(&tmp);
    }
}
