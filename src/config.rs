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
    /// true = keep the LinAmp windows above other applications.
    pub always_on_top: bool,
    /// true = ignore a skin's custom cursors and use the system cursor instead.
    pub disable_skin_cursors: bool,
}

impl Default for Prefs {
    fn default() -> Self {
        Self {
            // First launch opens at classic 1x size (authentic Winamp scale);
            // the user can switch to 2x, which then persists.
            double_size: false,
            tag_on_load: true,
            last_skin: None,
            fade_on_stop: false,
            always_on_top: false,
            disable_skin_cursors: false,
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
    /// Output volume [0,1] and stereo balance [-1,1]. `None` keeps the player's
    /// default, so a session.json predating these fields isn't forced to silence.
    pub volume: Option<f32>,
    pub balance: Option<f32>,
    pub pl_width: Option<f32>,
    pub pl_height: Option<f32>,
    /// Equalizer state, remembered across runs.
    pub eq_enabled: bool,
    pub eq_auto: bool,
    pub eq_preamp: f32,
    pub eq_bands: Vec<f32>,
    /// Which windows were open, and whether each was collapsed (window-shade), so
    /// the layout is restored on next launch. Positions are not remembered.
    pub eq_visible: bool,
    pub pl_visible: bool,
    pub main_shade: bool,
    pub eq_shade: bool,
    pub pl_shade: bool,
    /// Whether the (dockable, non-shading) Media Library window was open.
    pub lib_visible: bool,
    /// User-resized Media Library column fractions [artist, title]; None = defaults.
    pub lib_col_fr: Option<[f32; 2]>,
    /// Last on-screen top-left (logical points) of each window; `None` until the
    /// window has been positioned. When `main_pos` is present the saved layout is
    /// restored instead of the default dock arrangement.
    pub main_pos: Option<(f32, f32)>,
    pub eq_pos: Option<(f32, f32)>,
    pub pl_pos: Option<(f32, f32)>,
    pub lib_pos: Option<(f32, f32)>,
}

impl Session {
    fn path() -> PathBuf {
        dirs::data_local_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("LinAmp")
            .join("session.json")
    }

    /// A content signature over everything the session persists, used to detect
    /// changes worth re-saving. Hashing the serialized form (rather than a
    /// hand-listed set of fields) means the dirty check can never drift from what
    /// `save` actually writes — the root cause of volume not persisting (#105).
    pub fn sig(&self) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        serde_json::to_string(self).unwrap_or_default().hash(&mut h);
        h.finish()
    }

    /// Whether a saved session exists — false on a truly first launch.
    pub fn exists() -> bool {
        Self::path().exists()
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
    fn session_loads_legacy_json_missing_fields() {
        // An older session.json without the window-shade/visibility fields must
        // still load, defaulting the new fields rather than erroring.
        let json = r#"{"tracks":["a","b"],"current":0,"pl_width":275.0,"pl_height":200.0}"#;
        let s: Session = serde_json::from_str(json).unwrap();
        assert_eq!(s.tracks.len(), 2);
        assert_eq!(s.current, Some(0));
        assert!(!s.eq_visible && !s.pl_visible);
        assert!(!s.main_shade && !s.eq_shade && !s.pl_shade);
        assert!(!s.eq_enabled && !s.eq_auto);
        assert!(s.eq_bands.is_empty());
    }

    #[test]
    fn defaults() {
        let p = Prefs::default();
        // First launch is 1x (classic Winamp size) with tag-on-load.
        assert!(!p.double_size && p.tag_on_load);
        assert_eq!(p.scale(), 1.0);
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
    fn session_sig_tracks_persisted_fields() {
        // The dirty signature must change whenever a persisted field changes, so
        // a save is never skipped (the #105 regression: volume was missing).
        let base = Session::default();
        let b = base.sig();
        let mut v = base.clone();
        v.volume = Some(0.3);
        assert_ne!(v.sig(), b, "volume change not detected");
        let mut bal = base.clone();
        bal.balance = Some(-0.5);
        assert_ne!(bal.sig(), b, "balance change not detected");
        let mut eq = base.clone();
        eq.eq_preamp = 3.0;
        assert_ne!(eq.sig(), b, "eq change not detected");
        // Identical sessions hash identically.
        assert_eq!(base.sig(), Session::default().sig());
    }

    #[test]
    fn session_roundtrip() {
        let s = Session {
            tracks: vec!["a.mp3".into(), "b.flac".into()],
            current: Some(1),
            volume: Some(0.7),
            balance: Some(-0.25),
            pl_width: Some(350.0),
            pl_height: Some(232.0),
            eq_enabled: true,
            eq_auto: true,
            eq_preamp: -3.0,
            eq_bands: vec![1.0; 10],
            eq_visible: true,
            pl_visible: false,
            main_shade: true,
            eq_shade: false,
            pl_shade: true,
            lib_visible: true,
            lib_col_fr: Some([0.3, 0.4]),
            main_pos: Some((100.0, 200.0)),
            eq_pos: Some((100.0, 316.0)),
            pl_pos: None,
            lib_pos: Some((375.0, 200.0)),
        };
        let j = serde_json::to_string(&s).unwrap();
        let q: Session = serde_json::from_str(&j).unwrap();
        assert_eq!(q.tracks.len(), 2);
        assert_eq!(q.current, Some(1));
        assert_eq!(q.volume, Some(0.7));
        assert_eq!(q.balance, Some(-0.25));
        assert_eq!(q.pl_width, Some(350.0));
        assert!(q.eq_enabled && q.eq_auto);
        assert_eq!(q.eq_bands.len(), 10);
        assert!(q.eq_visible && !q.pl_visible);
        assert!(q.main_shade && !q.eq_shade && q.pl_shade);
        assert!(q.lib_visible);
        assert_eq!(q.main_pos, Some((100.0, 200.0)));
        assert_eq!(q.pl_pos, None);
        assert_eq!(q.lib_pos, Some((375.0, 200.0)));
    }
}
