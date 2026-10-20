//! Persistent user preferences, stored as JSON next to the library DB.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Prefs {
    /// true = 2x scaled (readable on modern screens), false = 1x pixel-for-pixel.
    pub double_size: bool,
    /// true = read ID3 tags when files are added; false = lazily as rows are viewed.
    pub tag_on_load: bool,
    /// Path to the skin loaded last; restored on next launch.
    pub last_skin: Option<String>,
    /// true = the Stop action fades out instead of cutting off immediately.
    pub fade_on_stop: bool,
}

impl Default for Prefs {
    fn default() -> Self {
        Self {
            double_size: true,
            tag_on_load: true,
            last_skin: None,
            fade_on_stop: false,
        }
    }
}

impl Prefs {
    pub fn scale(&self) -> f32 {
        if self.double_size {
            2.0
        } else {
            1.0
        }
    }

    fn path() -> PathBuf {
        dirs::data_local_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("LinAmp")
            .join("config.json")
    }

    pub fn load() -> Self {
        std::fs::read_to_string(Self::path())
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) {
        let p = Self::path();
        if let Some(d) = p.parent() {
            std::fs::create_dir_all(d).ok();
        }
        if let Ok(s) = serde_json::to_string_pretty(self) {
            std::fs::write(p, s).ok();
        }
    }
}

/// The last-session state restored on launch: the playlist and where the
/// cursor and playlist window were. Stored separately from [`Prefs`] because it
/// changes far more often.
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Session {
    pub tracks: Vec<String>,
    pub current: Option<usize>,
    pub pl_width: Option<f32>,
    pub pl_height: Option<f32>,
    /// Equalizer state, remembered across runs.
    pub eq_enabled: bool,
    pub eq_auto: bool,
    pub eq_preamp: f32,
    pub eq_bands: Vec<f32>,
}

impl Session {
    fn path() -> PathBuf {
        dirs::data_local_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("LinAmp")
            .join("session.json")
    }

    pub fn load() -> Self {
        std::fs::read_to_string(Self::path())
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) {
        let p = Self::path();
        if let Some(d) = p.parent() {
            std::fs::create_dir_all(d).ok();
        }
        if let Ok(s) = serde_json::to_string_pretty(self) {
            std::fs::write(p, s).ok();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults() {
        let p = Prefs::default();
        assert!(p.double_size && p.tag_on_load);
        assert_eq!(p.scale(), 2.0);
        assert!(p.last_skin.is_none());
    }

    #[test]
    fn scale_1x() {
        let p = Prefs { double_size: false, tag_on_load: false, ..Prefs::default() };
        assert_eq!(p.scale(), 1.0);
    }

    #[test]
    fn json_roundtrip() {
        let p = Prefs {
            double_size: false,
            tag_on_load: true,
            last_skin: Some("C:/skins/x.wsz".into()),
            ..Prefs::default()
        };
        let s = serde_json::to_string(&p).unwrap();
        let q: Prefs = serde_json::from_str(&s).unwrap();
        assert!(!q.double_size && q.tag_on_load);
        assert_eq!(q.last_skin.as_deref(), Some("C:/skins/x.wsz"));
    }

    #[test]
    fn session_roundtrip() {
        let s = Session {
            tracks: vec!["a.mp3".into(), "b.flac".into()],
            current: Some(1),
            pl_width: Some(350.0),
            pl_height: Some(232.0),
            eq_enabled: true,
            eq_auto: true,
            eq_preamp: -3.0,
            eq_bands: vec![1.0; 10],
        };
        let j = serde_json::to_string(&s).unwrap();
        let q: Session = serde_json::from_str(&j).unwrap();
        assert_eq!(q.tracks.len(), 2);
        assert_eq!(q.current, Some(1));
        assert_eq!(q.pl_width, Some(350.0));
        assert!(q.eq_enabled && q.eq_auto);
        assert_eq!(q.eq_bands.len(), 10);
    }
}
