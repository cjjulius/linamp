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
}

impl Default for Prefs {
    fn default() -> Self {
        Self {
            double_size: true,
            tag_on_load: true,
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
