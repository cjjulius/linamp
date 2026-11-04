//! MP3 ID3 tag editing via `lofty`: reads ID3v1 and ID3v2 independently, reads
//! MPEG format info, and writes each tag on its own. The ID3v2 tag is modified
//! in place so frames we don't edit (album art, album artist, disc, BPM, …)
//! survive a save.

use std::path::Path;

use anyhow::{Context, Result};
use lofty::tag::ItemKey;

/// Fallback tag keys for fields lofty maps only for ID3v2. For Vorbis comments,
/// MP4 atoms and APEv2, lofty silently drops these on write (Original Artist and
/// URL have no non-ID3v2 mapping at all; MP4 `cpil` and APE disc/bpm don't
/// round-trip through lofty's typed writers). The mass/single-tag writer stores
/// them under these explicit keys for the formats that need it, and the reader
/// falls back to them, so a mass edit round-trips across every editable format.
/// ID3v2 keeps its standard frames (TOPE/WOAS/TCMP/TPOS/TBPM) and never uses these.
mod fallback {
    pub const ORIG_ARTIST: &str = "ORIGINALARTIST";
    pub const URL: &str = "URL";
    pub const COMPILATION: &str = "COMPILATION";
    pub const DISC: &str = "DISCNUMBER";
    pub const BPM: &str = "BPM";
}

/// The concrete tag key for a fallback field in a given tag type. MP4 stores custom
/// values as freeform `----:mean:name` atoms (lofty only round-trips an unknown MP4
/// key in that form); Vorbis comments and APEv2 take the bare name. Used by both
/// [`apply_v2`] (write) and [`fields_from_tag`] (read) so the two always agree.
fn fallback_key(tt: lofty::tag::TagType, base: &str) -> ItemKey {
    if tt == lofty::tag::TagType::Mp4Ilst {
        ItemKey::Unknown(format!("----:com.apple.iTunes:{base}"))
    } else {
        ItemKey::Unknown(base.to_string())
    }
}

/// Editable ID3 fields. ID3v1 uses only `track`..=`comment`; ID3v2 uses all.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
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
    /// Disc number, like `track` (plain `1` or `1/2`).
    pub disc: String,
    /// Beats per minute (plain integer).
    pub bpm: String,
    /// Part of a compilation (iTunes TCMP / `FlagCompilation`).
    pub compilation: bool,
}

/// Which fields a mass edit changed, so [`write_mass`] applies only those and
/// leaves every other field of each file untouched. Mirrors [`Id3Fields`] one bit
/// per field.
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct FieldMask {
    pub track: bool,
    pub title: bool,
    pub artist: bool,
    pub album: bool,
    pub year: bool,
    pub genre: bool,
    pub comment: bool,
    pub composer: bool,
    pub orig_artist: bool,
    pub copyright: bool,
    pub url: bool,
    pub encoded_by: bool,
    pub disc: bool,
    pub bpm: bool,
    pub compilation: bool,
}

impl FieldMask {
    /// True when at least one field is marked (so a mass save has work to do).
    pub fn any(&self) -> bool {
        let Self {
            track,
            title,
            artist,
            album,
            year,
            genre,
            comment,
            composer,
            orig_artist,
            copyright,
            url,
            encoded_by,
            disc,
            bpm,
            compilation,
        } = *self;
        track
            || title
            || artist
            || album
            || year
            || genre
            || comment
            || composer
            || orig_artist
            || copyright
            || url
            || encoded_by
            || disc
            || bpm
            || compilation
    }

    /// Start from `base` and replace each marked field with `new`'s value, so an
    /// unmarked field keeps the file's own value and a marked one takes the mass
    /// edit's. This is how a mass save merges the changed fields into one file.
    pub fn overlay(&self, base: &Id3Fields, new: &Id3Fields) -> Id3Fields {
        macro_rules! pick {
            ($f:ident) => {
                if self.$f { new.$f.clone() } else { base.$f.clone() }
            };
        }
        Id3Fields {
            track: pick!(track),
            title: pick!(title),
            artist: pick!(artist),
            album: pick!(album),
            year: pick!(year),
            genre: pick!(genre),
            comment: pick!(comment),
            composer: pick!(composer),
            orig_artist: pick!(orig_artist),
            copyright: pick!(copyright),
            url: pick!(url),
            encoded_by: pick!(encoded_by),
            disc: pick!(disc),
            bpm: pick!(bpm),
            compilation: if self.compilation { new.compilation } else { base.compilation },
        }
    }
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
    /// True for MPEG audio (MP3), the only format with the copyright/original/
    /// emphasis flags; the Basic Info tab shows those rows only then.
    pub is_mpeg: bool,
}

/// Extensions LinAmp can currently *write* tags for. Reading isn't restricted.
/// MP2/MP1 are MPEG audio too, so they carry ID3 tags like MP3.
pub const EDITABLE_EXTS: &[&str] =
    &["mp3", "mp2", "mp1", "mpa", "flac", "ogg", "oga", "opus", "m4a", "m4b", "mp4", "ape"];

/// True when `path` uses ID3 tags (the v1/v2 model), i.e. any MPEG audio layer.
/// Other editable formats (FLAC/Ogg Vorbis comments, MP4 atoms) carry a single
/// native tag instead.
pub fn uses_id3(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| matches!(e.to_ascii_lowercase().as_str(), "mp3" | "mp2" | "mp1" | "mpa"))
        .unwrap_or(false)
}

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
    // Some taggers (example: zing.vn) write UTF-8 bytes into a frame flagged
    // Latin-1, so a spec-correct read yields mojibake. repair_mojibake recovers the
    // intended text when the bytes are disguised UTF-8, and is a no-op otherwise, so
    // the editor shows the real characters (matching the playlist display).
    use crate::playlist::repair_mojibake as fix;
    let text = |k: &ItemKey| fix(&tag.get_string(k).map(|s| s.to_string()).unwrap_or_default());
    // Read a fallback key (see `fallback` and `apply_v2`); empty when absent. The
    // concrete key depends on the tag type (MP4 uses a freeform atom).
    let tt = tag.tag_type();
    let unk = |base: &str| {
        tag.get_string(&fallback_key(tt, base)).map(|s| s.to_string()).unwrap_or_default()
    };
    // Prefer the standard typed value; fall back to the explicit key for formats
    // where lofty can't persist the typed one.
    let orig_artist = {
        let v = text(&ItemKey::OriginalArtist);
        if v.is_empty() { fix(&unk(fallback::ORIG_ARTIST)) } else { v }
    };
    let url = {
        let v = tag
            .get(&ItemKey::AudioSourceUrl)
            .and_then(|i| match i.value() {
                lofty::tag::ItemValue::Text(s) | lofty::tag::ItemValue::Locator(s) => Some(s.clone()),
                _ => None,
            })
            .map(|s| fix(&s))
            .unwrap_or_default();
        if v.is_empty() { fix(&unk(fallback::URL)) } else { v }
    };
    let disc = {
        let v = tag.disk().map(|d| d.to_string()).unwrap_or_default();
        if v.is_empty() { unk(fallback::DISC) } else { v }
    };
    let bpm = {
        let v = tag
            .get_string(&ItemKey::IntegerBpm)
            .or_else(|| tag.get_string(&ItemKey::Bpm))
            .map(|s| s.to_string())
            .unwrap_or_default();
        if v.is_empty() { unk(fallback::BPM) } else { v }
    };
    // Any non-empty, non-"0" value (typed TCMP/cpil or the fallback key) counts as set.
    let compilation = {
        let typed = tag.get_string(&ItemKey::FlagCompilation);
        let set = !matches!(typed, None | Some("") | Some("0"));
        set || !matches!(unk(fallback::COMPILATION).as_str(), "" | "0")
    };
    Id3Fields {
        track: tag.track().map(|t| t.to_string()).unwrap_or_default(),
        title: fix(&tag.title().map(|s| s.to_string()).unwrap_or_default()),
        artist: fix(&tag.artist().map(|s| s.to_string()).unwrap_or_default()),
        album: fix(&tag.album().map(|s| s.to_string()).unwrap_or_default()),
        // Year lives in the TDRC recording-time frame (a full timestamp like
        // "2024-05-01" reads as its 4-digit year); fall back to the Accessor year.
        year: tag
            .get_string(&ItemKey::RecordingDate)
            .and_then(|s| s.get(0..4))
            .filter(|y| y.chars().all(|c| c.is_ascii_digit()))
            .map(|y| y.to_string())
            .or_else(|| tag.year().map(|y| y.to_string()))
            .unwrap_or_default(),
        genre: fix(&tag.genre().map(|s| s.to_string()).unwrap_or_default()),
        comment: fix(&tag.comment().map(|s| s.to_string()).unwrap_or_default()),
        composer: text(&ItemKey::Composer),
        // Original Artist, URL, disc and bpm each prefer the standard frame and fall
        // back to an explicit key for formats lofty can't persist them in (computed above).
        orig_artist,
        copyright: text(&ItemKey::CopyrightMessage),
        url,
        encoded_by: text(&ItemKey::EncodedBy),
        disc,
        bpm,
        compilation,
    }
}

/// Read the single native tag of a non-ID3 file (FLAC/Ogg Vorbis comments, MP4
/// atoms) into the shared field set. Returns the defaults when the file has no
/// tag yet. The same `Id3Fields` is reused because every field maps to a
/// format-agnostic `ItemKey`.
pub fn read_single(path: &Path) -> Id3Fields {
    use lofty::file::TaggedFileExt;
    lofty::read_from_path(path)
        .ok()
        .and_then(|t| t.primary_tag().map(fields_from_tag))
        .unwrap_or_default()
}

/// The metadata block types present in a FLAC byte stream (1 byte each), for the
/// splice check and its test. Empty when `data` is not a FLAC stream. Test-only:
/// the `ensure_flac_writable` splice above reads the header directly, so this is
/// used only to assert the result in [`tests::minimal_flac_gets_a_writable_comments_block`].
#[cfg(test)]
fn flac_block_types(data: &[u8]) -> Vec<u8> {
    let mut types = Vec::new();
    if data.len() < 4 || &data[0..4] != b"fLaC" {
        return types;
    }
    let mut pos = 4;
    while pos + 4 <= data.len() {
        let header = data[pos];
        types.push(header & 0x7f);
        let len = u32::from_be_bytes([0, data[pos + 1], data[pos + 2], data[pos + 3]]) as usize;
        if header & 0x80 != 0 {
            break; // last metadata block
        }
        pos += 4 + len;
    }
    types
}

/// lofty 0.21 silently fails to add a tag to a "minimal" FLAC whose only metadata
/// block is STREAMINFO (marked as the last block), which is common in fresh rips:
/// the save returns `Ok` but writes no VorbisComments block, so the edit is lost.
/// Work around it by splicing an empty VorbisComments block in after STREAMINFO, so
/// a following write has a block to update. Only that exact shape is touched; any
/// other FLAC (which lofty writes correctly) and any non-FLAC file is left as is.
fn ensure_flac_writable(path: &Path) -> Result<()> {
    use std::io::Read;
    let mut head = [0u8; 8];
    {
        let mut f =
            std::fs::File::open(path).with_context(|| format!("opening {}", path.display()))?;
        if f.read_exact(&mut head).is_err() {
            return Ok(());
        }
    }
    if &head[0..4] != b"fLaC" {
        return Ok(());
    }
    // The first metadata block is always STREAMINFO (type 0). lofty only chokes when
    // it is also the last block, so there is no other block to carry the tag.
    let streaminfo_is_last = head[4] & 0x80 != 0;
    let block_type = head[4] & 0x7f;
    if !streaminfo_is_last || block_type != 0 {
        return Ok(());
    }
    let data = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    let si_len = u32::from_be_bytes([0, data[5], data[6], data[7]]) as usize;
    let si_end = 8 + si_len; // 4 ("fLaC") + 1 (header) + 3 (length) + STREAMINFO data
    if si_end > data.len() {
        return Ok(()); // malformed; do not touch it
    }
    // "fLaC" + STREAMINFO (last-block flag cleared) + empty VorbisComments (new last
    // block) + the original audio frames.
    let vendor = b"LinAmp";
    let mut body = Vec::with_capacity(8 + vendor.len());
    body.extend_from_slice(&(vendor.len() as u32).to_le_bytes());
    body.extend_from_slice(vendor);
    body.extend_from_slice(&0u32.to_le_bytes()); // zero user comments
    let mut out = Vec::with_capacity(data.len() + body.len() + 4);
    out.extend_from_slice(&data[0..4]);
    out.push(head[4] & 0x7f); // STREAMINFO header, last-block flag cleared
    out.extend_from_slice(&data[5..si_end]); // STREAMINFO length + data
    out.push(0x80 | 4); // VorbisComments, marked as the last metadata block
    out.extend_from_slice(&(body.len() as u32).to_be_bytes()[1..4]); // 24-bit length
    out.extend_from_slice(&body);
    out.extend_from_slice(&data[si_end..]); // original audio frames
    std::fs::write(path, &out).with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}

/// Write the single native tag of a non-ID3 file from the shared field set,
/// preserving items we don't edit. The file's primary tag type is used (Vorbis
/// comments for FLAC/Ogg, the MP4 item list for M4A), so the fields land in the
/// format's own keys via lofty's `ItemKey` mapping.
pub fn write_single(path: &Path, fields: &Id3Fields) -> Result<()> {
    use lofty::config::WriteOptions;
    use lofty::file::TaggedFileExt;
    use lofty::tag::{Tag, TagExt};
    // A minimal FLAC needs a comments block spliced in first (lofty 0.21 can't add
    // one), or the write below silently no-ops. No-op for every other file.
    if path.extension().and_then(|e| e.to_str()).is_some_and(|e| e.eq_ignore_ascii_case("flac")) {
        ensure_flac_writable(path)?;
    }
    let tagged = lofty::read_from_path(path)
        .with_context(|| format!("reading {}", path.display()))?;
    let mut tag = tagged
        .primary_tag()
        .cloned()
        .unwrap_or_else(|| Tag::new(tagged.primary_tag_type()));
    apply_v2(&mut tag, fields);
    tag.save_to_path(path, WriteOptions::default())
        .with_context(|| format!("writing tags to {}", path.display()))?;
    Ok(())
}

/// Read a file's editable fields as one set, for the mass editor's mode pre-fill.
/// MP3 prefers its ID3v2 tag and falls back to ID3v1; other formats read their one
/// native tag. Returns the defaults when the file has no tag yet.
pub fn read_primary(path: &Path) -> Id3Fields {
    if uses_id3(path) {
        let (v1, v2) = read_id3(path);
        v2.or(v1).unwrap_or_default()
    } else {
        read_single(path)
    }
}

/// Write the ID3v2 tag only, leaving any ID3v1 tag in place. Used by the mass
/// editor, which merges changed fields into a file's modern tag without disturbing
/// the legacy ID3v1 trailer. Starts from the existing ID3v2 tag so other frames
/// (album art, album artist, and so on) survive.
pub fn write_id3v2_only(path: &Path, f: &Id3Fields, id3v23: bool) -> Result<()> {
    use lofty::config::WriteOptions;
    use lofty::file::TaggedFileExt;
    use lofty::id3::v2::Id3v2Tag;
    use lofty::tag::{Tag, TagExt, TagType};
    let existing = lofty::read_from_path(path)
        .ok()
        .and_then(|t| t.tag(TagType::Id3v2).cloned());
    let mut tag = existing.unwrap_or_else(|| Tag::new(TagType::Id3v2));
    apply_v2(&mut tag, f);
    let id3: Id3v2Tag = tag.into();
    id3.save_to_path(path, WriteOptions::new().use_id3v23(id3v23))
        .with_context(|| format!("writing ID3v2 to {}", path.display()))?;
    Ok(())
}

/// Merge a mass edit into one file: apply only the fields marked in `mask`,
/// keeping every unedited field and every unmanaged frame. MP3 writes its ID3v2
/// tag (ID3v1 untouched); other formats write their native tag. Returns
/// `Ok(false)` when the overlay leaves the tag unchanged (so the file is skipped),
/// `Ok(true)` when it was written.
pub fn write_mass(path: &Path, new: &Id3Fields, mask: &FieldMask, id3v23: bool) -> Result<bool> {
    if !can_edit(path) {
        anyhow::bail!(
            "editing tags for this file type isn't supported yet (only {})",
            EDITABLE_EXTS.join(", ")
        );
    }
    let cur = read_primary(path);
    let merged = mask.overlay(&cur, new);
    if merged == cur {
        return Ok(false);
    }
    if uses_id3(path) {
        write_id3v2_only(path, &merged, id3v23)?;
    } else {
        write_single(path, &merged)?;
    }
    Ok(true)
}

/// A friendly format name from a file's extension, for the mass editor's format
/// breakdown. Thin public wrapper over the Basic Info label.
pub fn format_name(path: &Path) -> String {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .unwrap_or_default();
    format_label(&ext)
}

/// A friendly format name from the file extension, so the Basic Info tab names
/// the format even for types lofty doesn't parse (MIDI, tracker modules).
fn format_label(ext: &str) -> String {
    match ext {
        "mp3" | "mp2" | "mp1" | "mpa" => "MP3",
        "flac" => "FLAC",
        "ogg" | "oga" => "Ogg Vorbis",
        "opus" => "Opus",
        "m4a" | "m4b" | "mp4" => "MPEG-4 / AAC",
        "wav" | "wave" => "WAV",
        "aif" | "aiff" | "aifc" => "AIFF",
        "mid" | "midi" | "rmi" | "kar" => "MIDI",
        "mod" | "xm" | "it" | "s3m" | "mtm" | "umx" => "Tracker module",
        "" => return String::new(),
        other => return other.to_uppercase(),
    }
    .to_string()
}

fn channel_label(ch: u8) -> String {
    match ch {
        1 => "Mono".to_string(),
        2 => "Stereo".to_string(),
        n => format!("{n} channels"),
    }
}

/// Read file / format info for the Basic Info tab (best-effort; fields stay blank
/// when unavailable). Works for every format: a label comes from the extension,
/// and duration / bitrate / sample rate / channels come from lofty for the audio
/// formats it parses (MIDI and tracker modules get the label and size only).
pub fn read_info(path: &Path) -> FileInfo {
    use lofty::config::ParseOptions;
    use lofty::file::AudioFile;
    use lofty::mpeg::{ChannelMode, Emphasis, Layer, MpegFile, MpegVersion};

    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .unwrap_or_default();
    let mut info = FileInfo {
        path: path.display().to_string(),
        size_bytes: std::fs::metadata(path).map(|m| m.len()).unwrap_or(0),
        mpeg: format_label(&ext),
        ..Default::default()
    };
    let is_mpeg = matches!(ext.as_str(), "mp3" | "mp2" | "mp1" | "mpa");
    if is_mpeg {
        info.is_mpeg = true;
        // MP3 reads the MPEG-specific details (version/layer, channel mode, and the
        // copyright/original/emphasis flags) the generic properties don't carry.
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
    } else if lofty_parses(&ext) {
        // FLAC/Ogg/Opus/MP4/WAV/AIFF: lofty's own parser gives the generic audio
        // properties fast (no frame scan).
        if let Ok(tagged) = lofty::read_from_path(path) {
            let p = tagged.properties();
            info.length_secs = p.duration().as_secs();
            info.bitrate_kbps = p.audio_bitrate().or_else(|| p.overall_bitrate()).unwrap_or(0);
            info.sample_rate_hz = p.sample_rate().unwrap_or(0);
            info.channel_mode = p.channels().map(channel_label).unwrap_or_default();
        }
    } else {
        // MIDI and tracker modules aren't parsed by lofty, so reuse the same length
        // readers the playlist uses (SoundFont-free), so at least the duration shows.
        if let Some(d) = crate::midi::duration(path)
            .or_else(|| crate::tracker::duration(path))
            .or_else(|| crate::ape::duration(path))
            .or_else(|| crate::ac3::duration(path))
        {
            info.length_secs = d.as_secs();
        }
        // Enrich the MIDI Format row with the SMF type (0/1/2) and track count,
        // a cheap 14-byte header read.
        if matches!(ext.as_str(), "mid" | "midi" | "rmi" | "kar") {
            if let Some((fmt, tracks)) = midi_header(path) {
                info.mpeg = format!("MIDI (type {fmt}, {tracks} track(s))");
            }
        }
    }
    info
}

/// Read a Standard MIDI File header (`MThd`): the format type (0/1/2) and track
/// count, for the Basic Info tab. `None` when the header isn't a valid `MThd`.
fn midi_header(path: &Path) -> Option<(u16, u16)> {
    let bytes = std::fs::read(path).ok()?;
    if bytes.len() < 14 || &bytes[0..4] != b"MThd" {
        return None;
    }
    let fmt = u16::from_be_bytes([bytes[8], bytes[9]]);
    let tracks = u16::from_be_bytes([bytes[10], bytes[11]]);
    Some((fmt, tracks))
}

/// Whether lofty can parse this extension for audio properties (so we don't ask
/// it to open MIDI / tracker modules it doesn't understand).
fn lofty_parses(ext: &str) -> bool {
    matches!(
        ext,
        "flac" | "ogg" | "oga" | "opus" | "spx" | "m4a" | "m4b" | "mp4"
            | "aac" | "wav" | "wave" | "aif" | "aiff" | "aifc" | "ape" | "wv"
    )
}

/// Prepare an ID3v1 text field: trim, drop if empty and cap to `max` bytes on a
/// UTF-8 char boundary. ID3v1 fields are fixed-width; lofty truncates to the cap
/// by byte index and panics if a multibyte character straddles it (#115), which
/// aborts the save mid-write and corrupts the file. Capping here avoids that.
/// The ID3v1 numeric genre index for a genre name (case-insensitive), or None when the
/// name is empty or not one of the standard [`GENRES`]. ID3v1 genre is a single byte
/// into that fixed list, so a free-form name (common from MusicBrainz) has no index and
/// is left unset; the full name still lives in ID3v2.
pub(crate) fn genre_index(name: &str) -> Option<u8> {
    let name = name.trim();
    if name.is_empty() {
        return None;
    }
    GENRES.iter().position(|g| g.eq_ignore_ascii_case(name)).map(|i| i as u8)
}

pub(crate) fn v1_text(s: &str, max: usize) -> Option<String> {
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
pub fn write_id3(
    path: &Path,
    v1: Option<&Id3Fields>,
    v2: Option<&Id3Fields>,
    id3v23: bool,
) -> Result<()> {
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
            // Save through the concrete ID3v2 tag so the recording-time frame is a
            // proper timestamp: lofty writes TDRC for 2.4 and splits it into TYER
            // for 2.3 (a generic text TDRC would be dropped on a 2.3 write). 2.3 is
            // the most compatible; 2.4 is the newer standard. Reading handles both.
            use lofty::id3::v2::Id3v2Tag;
            let id3: Id3v2Tag = tag.into();
            id3.save_to_path(path, WriteOptions::new().use_id3v23(id3v23))
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
            let t = $v.trim();
            if t.is_empty() {
                tag.$rm();
            } else if let Ok(n) = t.parse::<u32>() {
                tag.$set(n);
            }
            // A non-empty value that isn't a plain number (e.g. "2024 (remaster)"
            // or "3/12") is left as-is rather than silently dropping the frame.
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
    // Year goes to the TDRC recording-time frame: lofty writes it as TDRC for
    // ID3v2.4 and splits it into TYER for ID3v2.3 (the generic `set_year` frame is
    // dropped on a 2.3 write, so a plain year would vanish). Empty clears it; a
    // non-numeric value is left as-is rather than dropped.
    {
        let y = f.year.trim();
        if y.is_empty() {
            tag.remove_year();
            tag.remove_key(&ItemKey::RecordingDate);
        } else if let Ok(n) = y.parse::<u32>() {
            tag.remove_year(); // drop any legacy TYER so the TDRC year is read
            tag.remove_key(&ItemKey::RecordingDate);
            if tag.tag_type() == lofty::tag::TagType::Ape {
                // APEv2 has no RecordingDate mapping in lofty; it keeps the year under
                // the "Year" item (the Accessor year), so write it there instead.
                tag.set_year(n);
            } else {
                tag.insert_text(ItemKey::RecordingDate, y.to_string());
            }
        }
    }
    num_f!(f.track, set_track, remove_track);
    num_f!(f.disc, set_disk, remove_disk);
    {
        key_f!(f.composer, ItemKey::Composer);
        key_f!(f.orig_artist, ItemKey::OriginalArtist);
        key_f!(f.copyright, ItemKey::CopyrightMessage);
        key_f!(f.encoded_by, ItemKey::EncodedBy);
        // BPM: ID3v2 and MP4 use the integer frame (TBPM / tmpo), Vorbis comments
        // use "BPM". Write both so whichever the format accepts is set (lofty drops
        // the key it doesn't map).
        if f.bpm.trim().is_empty() {
            tag.remove_key(&ItemKey::IntegerBpm);
            tag.remove_key(&ItemKey::Bpm);
        } else {
            tag.insert_text(ItemKey::IntegerBpm, f.bpm.clone());
            tag.insert_text(ItemKey::Bpm, f.bpm.clone());
        }
        // Compilation is a boolean flag; iTunes stores it as "1" (TCMP).
        if f.compilation {
            tag.insert_text(ItemKey::FlagCompilation, "1".to_string());
        } else {
            tag.remove_key(&ItemKey::FlagCompilation);
        }
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

    // Cross-format fallbacks. lofty maps Original Artist and URL only for ID3v2, and
    // MP4 `cpil` / APE disc+bpm don't round-trip through its typed writers, so those
    // fields silently vanish on a non-ID3v2 save. Mirror them into explicit keys for
    // the formats that need it (the reader prefers the standard frame, then these).
    // ID3v2 keeps its clean standard frames and never gets these extra items.
    let tt = tag.tag_type();
    let set_fallback = |tag: &mut lofty::tag::Tag, base: &str, val: &str| {
        let k = fallback_key(tt, base);
        tag.remove_key(&k);
        if !val.trim().is_empty() {
            // `insert` re-maps the key and silently drops `ItemKey::Unknown` (no
            // mapping), so use the unchecked insert, as lofty's docs direct.
            tag.insert_unchecked(lofty::tag::TagItem::new(
                k,
                lofty::tag::ItemValue::Text(val.to_string()),
            ));
        }
    };
    if tt != lofty::tag::TagType::Id3v2 {
        // No non-ID3v2 mapping exists for these two in lofty.
        set_fallback(tag, fallback::ORIG_ARTIST, &f.orig_artist);
        set_fallback(tag, fallback::URL, &f.url);
    }
    if tt == lofty::tag::TagType::Mp4Ilst {
        // `cpil` written as text "1" does not read back; keep a parallel flag.
        set_fallback(tag, fallback::COMPILATION, if f.compilation { "1" } else { "" });
    }
    if tt == lofty::tag::TagType::Ape {
        // APEv2 disc/bpm don't survive lofty's typed writers; store them explicitly.
        set_fallback(tag, fallback::DISC, &f.disc);
        set_fallback(tag, fallback::BPM, &f.bpm);
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
        assert!(can_edit(Path::new("song.mp2"))); // MP2 is MPEG audio, ID3-taggable
        assert!(uses_id3(Path::new("song.mp2")));
        assert!(can_edit(Path::new("song.flac")));
        assert!(can_edit(Path::new("tune.ogg")));
        assert!(can_edit(Path::new("tune.m4a")));
        assert!(!can_edit(Path::new("sound.wav")));
        assert!(!can_edit(Path::new("noext")));
        // Only MP3 uses the ID3 v1/v2 model; the others carry a single tag.
        assert!(uses_id3(Path::new("song.mp3")));
        assert!(!uses_id3(Path::new("song.flac")));
    }

    #[test]
    fn write_rejects_unsupported() {
        let err = write_id3(Path::new("x.wav"), None, None, true).unwrap_err();
        assert!(err.to_string().contains("isn't supported"));
    }



    #[test]
    fn apply_and_read_round_trips_across_tag_types() {
        // The field mapping works for every editable format, since each field maps
        // to a format-agnostic ItemKey. VorbisComments (FLAC/Ogg) and the MP4 item
        // list are exercised in memory, so no audio file is needed.
        use lofty::tag::{Tag, TagType};
        let f = Id3Fields {
            track: "4".into(),
            title: "T".into(),
            artist: "A".into(),
            album: "Al".into(),
            year: "2021".into(),
            genre: "Jazz".into(),
            comment: "c".into(),
            composer: "Comp".into(),
            disc: "1".into(),
            bpm: "90".into(),
            compilation: true,
            ..Default::default()
        };
        // APEv2 is covered by the file round-trip test below; it maps the core fields
        // but not every extended one (bpm/compilation), so it is not in this strict set.
        for tt in [TagType::VorbisComments, TagType::Mp4Ilst] {
            let mut tag = Tag::new(tt);
            apply_v2(&mut tag, &f);
            let g = fields_from_tag(&tag);
            // Core fields every format maps round-trip.
            assert_eq!(g.title, "T", "{tt:?}");
            assert_eq!(g.artist, "A", "{tt:?}");
            assert_eq!(g.album, "Al", "{tt:?}");
            assert_eq!(g.year, "2021", "{tt:?}");
            assert_eq!(g.track, "4", "{tt:?}");
            assert_eq!(g.disc, "1", "{tt:?}");
            assert_eq!(g.genre, "Jazz", "{tt:?}");
            assert_eq!(g.composer, "Comp", "{tt:?}");
            assert_eq!(g.bpm, "90", "{tt:?}");
            assert!(g.compilation, "{tt:?}");
        }
    }

    #[test]
    fn ape_tags_write_and_read_back() {
        // Full path through lofty's Monkey's Audio writer (APEv2), gated on a local
        // sample (qa-music is gitignored, so this skips on CI). Confirms .ape joins the
        // tag-editable formats, not just the in-memory field mapping above.
        let src = std::path::Path::new("qa-music/format-samples/sample.ape");
        if !src.exists() {
            return;
        }
        assert!(can_edit(src), ".ape must be tag-editable");
        let tmp = std::env::temp_dir().join("linamp_ape_tag_test.ape");
        std::fs::copy(src, &tmp).unwrap();
        let fields = Id3Fields {
            title: "Scale Test".into(),
            artist: "LinAmp".into(),
            album: "QA".into(),
            year: "2026".into(),
            ..Default::default()
        };
        write_single(&tmp, &fields).expect("write APEv2 tags");
        let back = read_single(&tmp);
        assert_eq!(back.title, "Scale Test");
        assert_eq!(back.artist, "LinAmp");
        assert_eq!(back.album, "QA");
        assert_eq!(back.year, "2026");
        let _ = std::fs::remove_file(&tmp);
    }

    #[test]
    fn genres_start_with_the_standard_list() {
        assert_eq!(GENRES[0], "Blues");
        assert_eq!(GENRES[17], "Rock");
        assert!(GENRES.len() >= 80);
    }

    #[test]
    fn genre_index_maps_standard_names() {
        assert_eq!(genre_index("Rock"), Some(17));
        assert_eq!(genre_index("rock"), Some(17)); // case-insensitive
        assert_eq!(genre_index("Blues"), Some(0));
        assert_eq!(genre_index(""), None);
        assert_eq!(genre_index("Pop Punk"), None); // not a standard ID3v1 genre
    }

    #[test]
    fn apply_v2_and_read_back_round_trips_fields() {
        // Exercises the ID3v2 write mapping (apply_v2) and read mapping
        // (fields_from_tag) together, with no MP3 file needed, so this runs in a
        // clean checkout/CI where the bundled sample is absent.
        use lofty::tag::{Tag, TagType};
        let f = Id3Fields {
            track: "3".into(),
            title: "The Title".into(),
            artist: "The Artist".into(),
            album: "The Album".into(),
            year: "2024".into(),
            genre: "Rock".into(),
            comment: "a comment".into(),
            composer: "A. Composer".into(),
            orig_artist: "Orig".into(),
            copyright: "(c) 2024".into(),
            url: "https://example.com".into(),
            encoded_by: "LinAmp".into(),
            disc: "2".into(),
            bpm: "128".into(),
            compilation: true,
        };
        let mut tag = Tag::new(TagType::Id3v2);
        apply_v2(&mut tag, &f);
        assert_eq!(fields_from_tag(&tag), f);
    }

    #[test]
    fn apply_v2_keeps_a_non_numeric_year() {
        use lofty::tag::{Accessor, Tag, TagType};
        let mut tag = Tag::new(TagType::Id3v2);
        tag.set_year(1999);
        // A non-numeric year ("2024 (remaster)") leaves the existing frame intact,
        // rather than silently dropping the user's value.
        apply_v2(&mut tag, &Id3Fields { year: "2024 (remaster)".into(), ..Default::default() });
        assert_eq!(tag.year(), Some(1999));
        // An empty value still clears the frame.
        apply_v2(&mut tag, &Id3Fields { year: String::new(), ..Default::default() });
        assert_eq!(tag.year(), None);
    }

    #[test]
    fn fields_from_tag_repairs_mojibake() {
        use lofty::tag::{Accessor, Tag, TagType};
        let mut tag = Tag::new(TagType::Id3v2);
        tag.set_title("Clean Title".to_string());
        // UTF-8 bytes mistakenly stored in a Latin-1-flagged frame (what some
        // taggers write); the editor must show the intended characters.
        let garbled: String = "Café del Mar".bytes().map(|b| b as char).collect();
        tag.set_artist(garbled);
        let f = fields_from_tag(&tag);
        assert_eq!(f.title, "Clean Title");
        assert_eq!(f.artist, "Café del Mar");
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
            disc: "2".into(),
            bpm: "128".into(),
            compilation: true,
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
        write_id3(&tmp, Some(&v1), Some(&v2), true).unwrap();

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
        assert_eq!(gv2.disc, "2");
        assert_eq!(gv2.bpm, "128");
        assert!(gv2.compilation);
        // ID3v1 fields round-trip too.
        assert_eq!(gv1.title, "V1 Title");
        assert_eq!(gv1.artist, "V1 Artist");
        assert_eq!(gv1.year, "1999");

        // Removing ID3v1 leaves ID3v2 intact.
        write_id3(&tmp, None, Some(&v2), true).unwrap();
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
        write_id3(&tmp, Some(&v1), None, true).unwrap();

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



    #[test]
    fn id3v2_version_follows_the_pref() {
        // The `id3v23` flag selects the ID3v2 major version written (2.3 vs 2.4),
        // read back from the raw tag header (the 4th byte after "ID3").
        let src = Path::new("music/Aria Nocturne - Crystal Harbor.mp3");
        if !src.exists() {
            return;
        }
        let major = |path: &Path| -> Option<u8> {
            let b = std::fs::read(path).ok()?;
            (b.len() >= 4 && &b[0..3] == b"ID3").then(|| b[3])
        };
        let v2 = Id3Fields { title: "V".into(), ..Default::default() };
        let tmp = std::env::temp_dir().join("linamp_id3_version.mp3");

        std::fs::copy(src, &tmp).unwrap();
        write_id3(&tmp, None, Some(&v2), true).unwrap();
        assert_eq!(major(&tmp), Some(3), "id3v23=true should write ID3v2.3");

        std::fs::copy(src, &tmp).unwrap();
        write_id3(&tmp, None, Some(&v2), false).unwrap();
        assert_eq!(major(&tmp), Some(4), "id3v23=false should write ID3v2.4");

        let _ = std::fs::remove_file(&tmp);
    }

    #[test]
    fn field_mask_any_and_overlay() {
        let mut m = FieldMask::default();
        assert!(!m.any(), "an empty mask has no work");
        m.artist = true;
        m.compilation = true;
        assert!(m.any());

        let base = Id3Fields {
            title: "Keep Me".into(),
            artist: "Old".into(),
            compilation: false,
            ..Default::default()
        };
        let new = Id3Fields {
            title: "Ignored".into(),
            artist: "New".into(),
            compilation: true,
            ..Default::default()
        };
        let merged = m.overlay(&base, &new);
        // Marked fields take the new value; unmarked keep the base value.
        assert_eq!(merged.artist, "New");
        assert!(merged.compilation);
        assert_eq!(merged.title, "Keep Me", "unmarked field is preserved");
    }

    #[test]
    fn write_mass_writes_only_changed_fields_and_skips_noops() {
        // Full path through the merge writer on the bundled sample; skips in a
        // checkout without it. Confirms a marked field is applied, an unmarked field
        // is preserved, and a no-op overlay reports Ok(false).
        let src = Path::new("music/Aria Nocturne - Crystal Harbor.mp3");
        if !src.exists() {
            return;
        }
        let tmp = std::env::temp_dir().join("linamp_mass_write.mp3");
        std::fs::copy(src, &tmp).unwrap();
        // Seed a known starting point.
        let seed = Id3Fields {
            title: "Seed Title".into(),
            artist: "Seed Artist".into(),
            album: "Seed Album".into(),
            ..Default::default()
        };
        write_id3v2_only(&tmp, &seed, true).unwrap();

        // Change only the album.
        let mask = FieldMask { album: true, ..Default::default() };
        let new = Id3Fields { album: "Mass Album".into(), artist: "Should Not Apply".into(), ..Default::default() };
        assert_eq!(write_mass(&tmp, &new, &mask, true).unwrap(), true, "a real change writes");
        let got = read_primary(&tmp);
        assert_eq!(got.album, "Mass Album", "marked field applied");
        assert_eq!(got.title, "Seed Title", "unmarked field preserved");
        assert_eq!(got.artist, "Seed Artist", "unmarked field preserved even when new carries a value");

        // Re-applying the same album is a no-op and is skipped.
        assert_eq!(write_mass(&tmp, &new, &mask, true).unwrap(), false, "no net change is skipped");

        // An empty mask never writes.
        assert_eq!(
            write_mass(&tmp, &new, &FieldMask::default(), true).unwrap(),
            false,
            "empty mask is a no-op"
        );
        let _ = std::fs::remove_file(&tmp);
    }

    #[test]
    fn minimal_flac_gets_a_writable_comments_block() {
        // A FLAC whose only metadata block is STREAMINFO, marked last. lofty 0.21
        // cannot add a tag to this shape; ensure_flac_writable splices an empty
        // VorbisComments block so a later write lands. Built by hand so it runs on CI.
        let streaminfo = [0u8; 34]; // contents don't matter for the splice logic
        let mut data = Vec::new();
        data.extend_from_slice(b"fLaC");
        data.push(0x80); // last-block flag set, type 0 (STREAMINFO)
        data.extend_from_slice(&[0, 0, 34]); // 24-bit length
        data.extend_from_slice(&streaminfo);
        data.extend_from_slice(&[0xDE, 0xAD, 0xBE, 0xEF]); // stand-in audio frames
        let tmp = std::env::temp_dir().join("linamp_minimal.flac");
        std::fs::write(&tmp, &data).unwrap();

        ensure_flac_writable(&tmp).unwrap();
        let out = std::fs::read(&tmp).unwrap();
        // STREAMINFO is no longer the last block, a VorbisComments block now exists,
        // and the audio frames are preserved at the end.
        assert!(flac_block_types(&out).contains(&4u8), "VorbisComments block present");
        assert_eq!(out[4] & 0x80, 0, "STREAMINFO last-block flag cleared");
        assert_eq!(&out[out.len() - 4..], &[0xDE, 0xAD, 0xBE, 0xEF], "frames preserved");

        // A second call is a no-op (a comments block already exists).
        let before = std::fs::read(&tmp).unwrap();
        ensure_flac_writable(&tmp).unwrap();
        assert_eq!(std::fs::read(&tmp).unwrap(), before, "idempotent");
        let _ = std::fs::remove_file(&tmp);
    }

    #[test]
    fn minimal_flac_round_trips_tags_after_fix() {
        // End-to-end on a real minimal (rip-style) FLAC: without the fix the write is
        // silently dropped. Gated on a local sample (skips on CI).
        let src = Path::new("qa-music/rip_testing/Green Day - Bobby Sox.flac");
        if !src.exists() {
            return;
        }
        let tmp = std::env::temp_dir().join("linamp_minflac_rt.flac");
        std::fs::copy(src, &tmp).unwrap();
        let mask = FieldMask { artist: true, album: true, ..Default::default() };
        let new = Id3Fields { artist: "Green Day".into(), album: "Saviors".into(), ..Default::default() };
        assert_eq!(write_mass(&tmp, &new, &mask, true).unwrap(), true);
        let back = read_single(&tmp);
        assert_eq!(back.artist, "Green Day");
        assert_eq!(back.album, "Saviors");
        let _ = std::fs::remove_file(&tmp);
    }

    #[test]
    fn mass_edit_round_trips_every_field_across_formats() {
        // Real-file matrix: write the full field set through the mass path
        // (write_mass -> read_primary) for every editable format, and assert nothing
        // is dropped. This is the regression guard for the cross-format fallback keys
        // (see `apply_v2`). The non-MP3 samples live in the gitignored qa-music, so
        // each missing sample is skipped (CI runs the bundled MP3; a dev checkout with
        // qa-music covers all). Two passes with distinct values confirm an edit also
        // overwrites a prior value (the "set everything to 1234, then 5678" check).
        let samples: &[(&str, &str)] = &[
            ("mp3", "music/Aria Nocturne - Crystal Harbor.mp3"),
            ("flac", "qa-music/format-samples/hypno-this.flac"),
            ("ogg", "qa-music/format-samples/hypno-this.ogg"),
            ("opus", "qa-music/format-samples/hypno-this.opus"),
            ("m4a", "qa-music/format-samples/hypno-this.m4a"),
            ("mp4", "qa-music/format-samples/hypno-this.mp4"),
            ("ape", "qa-music/format-samples/sample.ape"),
        ];
        let all = FieldMask {
            track: true, title: true, artist: true, album: true, year: true, genre: true,
            comment: true, composer: true, orig_artist: true, copyright: true, url: true,
            encoded_by: true, disc: true, bpm: true, compilation: true,
        };
        // Two value sets; genre stays a standard name so ID3v1's index survives too.
        let passes = [
            Id3Fields {
                track: "1".into(), title: "T1".into(), artist: "Ar1".into(), album: "Al1".into(),
                year: "2001".into(), genre: "Rock".into(), comment: "C1".into(),
                composer: "Cm1".into(), orig_artist: "OA1".into(), copyright: "Cp1".into(),
                url: "http://one".into(), encoded_by: "E1".into(), disc: "1".into(),
                bpm: "111".into(), compilation: true,
            },
            Id3Fields {
                track: "2".into(), title: "T2".into(), artist: "Ar2".into(), album: "Al2".into(),
                year: "2002".into(), genre: "Jazz".into(), comment: "C2".into(),
                composer: "Cm2".into(), orig_artist: "OA2".into(), copyright: "Cp2".into(),
                url: "http://two".into(), encoded_by: "E2".into(), disc: "2".into(),
                bpm: "222".into(), compilation: false,
            },
        ];
        let mut ran = 0usize;
        for (ext, src) in samples {
            let src = Path::new(src);
            if !src.exists() {
                continue;
            }
            ran += 1;
            let tmp =
                std::env::temp_dir().join(format!("linamp_matrix_{ext}.{ext}"));
            std::fs::copy(src, &tmp).unwrap();
            for f in &passes {
                write_mass(&tmp, f, &all, true).unwrap();
                let g = read_primary(&tmp);
                assert_eq!(&g.track, &f.track, "{ext} track");
                assert_eq!(&g.title, &f.title, "{ext} title");
                assert_eq!(&g.artist, &f.artist, "{ext} artist");
                assert_eq!(&g.album, &f.album, "{ext} album");
                assert_eq!(&g.year, &f.year, "{ext} year");
                assert_eq!(&g.genre, &f.genre, "{ext} genre");
                assert_eq!(&g.comment, &f.comment, "{ext} comment");
                assert_eq!(&g.composer, &f.composer, "{ext} composer");
                assert_eq!(&g.orig_artist, &f.orig_artist, "{ext} orig_artist");
                assert_eq!(&g.copyright, &f.copyright, "{ext} copyright");
                assert_eq!(&g.url, &f.url, "{ext} url");
                assert_eq!(&g.encoded_by, &f.encoded_by, "{ext} encoded_by");
                assert_eq!(&g.disc, &f.disc, "{ext} disc");
                assert_eq!(&g.bpm, &f.bpm, "{ext} bpm");
                assert_eq!(g.compilation, f.compilation, "{ext} compilation");
            }
            let _ = std::fs::remove_file(&tmp);
        }
        assert!(ran >= 1, "expected at least the bundled MP3 sample to run");
    }

    #[test]
    fn format_name_labels_by_extension() {
        assert_eq!(format_name(Path::new("a.mp3")), "MP3");
        assert_eq!(format_name(Path::new("a.flac")), "FLAC");
        assert_eq!(format_name(Path::new("a.ogg")), "Ogg Vorbis");
    }
}
