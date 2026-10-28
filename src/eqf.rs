//! Winamp `.EQF` equalizer-preset interchange: parse and serialize the classic
//! "Winamp EQ library file v1.1" format, so presets import/export with Winamp and
//! compatible players. Layout (from the webamp `winamp-eqf` reference):
//!   header "Winamp EQ library file v1.1" + 0x1A + "!--", then per preset a
//!   257-byte null-padded name + 10 band bytes (60Hz..16kHz) + 1 preamp byte.
//! Each byte is 0..=63, where 0 = maximum boost and 63 = maximum cut.

use anyhow::{bail, Result};

const HEADER: &[u8] = b"Winamp EQ library file v1.1";
const NAME_LEN: usize = 257;
const VALUES: usize = 11; // 10 bands + preamp
/// LinAmp's band range in dB. (Winamp's own range is wider; imported shapes are
/// scaled into this range, and LinAmp-written files round-trip exactly.)
const MAX_DB: f32 = 12.0;

/// One EQ preset: name, 10 band gains (dB, low to high frequency), preamp (dB).
#[derive(Clone, Debug, PartialEq)]
pub struct EqfPreset {
    pub name: String,
    pub bands: [f32; 10],
    pub preamp: f32,
}

/// On-disk byte (0 = max boost .. 63 = max cut) to dB in -MAX_DB..=MAX_DB.
fn byte_to_db(b: u8) -> f32 {
    ((31.5 - b as f32) / 31.5 * MAX_DB).clamp(-MAX_DB, MAX_DB)
}

/// dB to on-disk byte, clamped to 0..=63.
fn db_to_byte(db: f32) -> u8 {
    (31.5 - (db.clamp(-MAX_DB, MAX_DB) / MAX_DB) * 31.5).round().clamp(0.0, 63.0) as u8
}

/// Parse an `.EQF` file into its presets (a library may hold several). Errors on a
/// missing/invalid header.
pub fn parse(data: &[u8]) -> Result<Vec<EqfPreset>> {
    if data.len() < HEADER.len() + 4 || &data[..HEADER.len()] != HEADER {
        bail!("not a Winamp .EQF file (bad header)");
    }
    let mut i = HEADER.len() + 4; // skip the 0x1A "!--" marker
    let mut out = Vec::new();
    while i + NAME_LEN + VALUES <= data.len() {
        let name_field = &data[i..i + NAME_LEN];
        let end = name_field.iter().position(|&b| b == 0).unwrap_or(NAME_LEN);
        let name = String::from_utf8_lossy(&name_field[..end]).into_owned();
        i += NAME_LEN;
        let mut bands = [0.0f32; 10];
        for band in bands.iter_mut() {
            *band = byte_to_db(data[i]);
            i += 1;
        }
        let preamp = byte_to_db(data[i]);
        i += 1;
        out.push(EqfPreset { name, bands, preamp });
    }
    Ok(out)
}

/// Serialize presets to `.EQF` bytes (library format).
pub fn write(presets: &[EqfPreset]) -> Vec<u8> {
    let mut buf = Vec::with_capacity(HEADER.len() + 4 + presets.len() * (NAME_LEN + VALUES));
    buf.extend_from_slice(HEADER);
    buf.push(0x1A);
    buf.extend_from_slice(b"!--");
    for p in presets {
        let nb = p.name.as_bytes();
        let n = nb.len().min(NAME_LEN);
        buf.extend_from_slice(&nb[..n]);
        buf.extend(std::iter::repeat(0u8).take(NAME_LEN - n));
        for &b in &p.bands {
            buf.push(db_to_byte(b));
        }
        buf.push(db_to_byte(p.preamp));
    }
    buf
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn byte_db_mapping_endpoints_and_centre() {
        assert!((byte_to_db(0) - MAX_DB).abs() < 1e-4, "byte 0 = max boost");
        assert!((byte_to_db(63) + MAX_DB).abs() < 1e-4, "byte 63 = max cut");
        assert!(byte_to_db(32).abs() < 0.3, "centre is ~0 dB");
        assert_eq!(db_to_byte(MAX_DB), 0);
        assert_eq!(db_to_byte(-MAX_DB), 63);
    }

    #[test]
    fn round_trips_through_write_and_parse() {
        let preset = EqfPreset {
            name: "Test".to_string(),
            bands: [12.0, -12.0, 6.0, -6.0, 0.0, 3.0, -3.0, 9.0, -9.0, 1.5],
            preamp: -4.5,
        };
        let bytes = write(std::slice::from_ref(&preset));
        let got = parse(&bytes).unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].name, "Test");
        // Band values survive the byte quantisation within ~0.4 dB.
        for (a, b) in got[0].bands.iter().zip(preset.bands.iter()) {
            assert!((a - b).abs() < 0.5, "{a} vs {b}");
        }
        assert!((got[0].preamp - preset.preamp).abs() < 0.5);
    }

    #[test]
    fn rejects_non_eqf() {
        assert!(parse(b"not an eqf file at all").is_err());
    }

    #[test]
    fn header_and_layout_sizes_match_winamp() {
        let p = EqfPreset { name: "x".into(), bands: [0.0; 10], preamp: 0.0 };
        let bytes = write(std::slice::from_ref(&p));
        assert!(bytes.starts_with(HEADER));
        // header(27) + 0x1A!--(4) + name(257) + 11 values = 299 bytes for one preset.
        assert_eq!(bytes.len(), HEADER.len() + 4 + NAME_LEN + VALUES);
    }

    #[test]
    fn multi_preset_round_trip() {
        let presets = vec![
            EqfPreset { name: "One".into(), bands: [3.0; 10], preamp: 0.0 },
            EqfPreset { name: "Two".into(), bands: [-6.0; 10], preamp: 2.0 },
            EqfPreset { name: "Three".into(), bands: [0.0; 10], preamp: -3.0 },
        ];
        let got = parse(&write(&presets)).unwrap();
        assert_eq!(got.len(), 3);
        assert_eq!(
            got.iter().map(|p| p.name.clone()).collect::<Vec<_>>(),
            ["One", "Two", "Three"]
        );
    }

    #[test]
    fn header_only_file_parses_to_no_presets() {
        let got = parse(&write(&[])).unwrap();
        assert!(got.is_empty());
    }

    #[test]
    fn trailing_partial_record_is_ignored() {
        let p = EqfPreset { name: "x".into(), bands: [0.0; 10], preamp: 0.0 };
        let mut bytes = write(std::slice::from_ref(&p));
        bytes.extend_from_slice(&[1, 2, 3]); // a truncated trailing record
        let got = parse(&bytes).unwrap();
        // The complete preset parses; the trailing bytes (less than a full record) are skipped.
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].name, "x");
    }

    #[test]
    fn overlong_name_is_clipped_to_the_field() {
        let p = EqfPreset { name: "a".repeat(400), bands: [0.0; 10], preamp: 0.0 };
        let bytes = write(std::slice::from_ref(&p));
        assert_eq!(bytes.len(), HEADER.len() + 4 + NAME_LEN + VALUES); // one record on disk
        let got = parse(&bytes).unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].name.len(), NAME_LEN); // clipped to the 257-byte field
    }

    #[test]
    fn embedded_null_terminates_the_name() {
        let p = EqfPreset { name: "ab\0cd".into(), bands: [0.0; 10], preamp: 0.0 };
        let got = parse(&write(std::slice::from_ref(&p))).unwrap();
        assert_eq!(got[0].name, "ab"); // the null byte ends the name field
    }

    #[test]
    fn parse_tolerates_non_utf8_name_bytes() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(HEADER);
        bytes.push(0x1A);
        bytes.extend_from_slice(b"!--");
        let mut name = vec![0xFFu8]; // an invalid UTF-8 lead byte
        name.resize(NAME_LEN, 0);
        bytes.extend_from_slice(&name);
        bytes.extend(std::iter::repeat(32u8).take(VALUES));
        // A lossy decode keeps the record rather than erroring the whole file.
        let got = parse(&bytes).unwrap();
        assert_eq!(got.len(), 1);
    }
}
