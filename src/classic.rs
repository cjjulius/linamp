//! Classic Winamp 2.x skin renderer: decodes a `.wsz` (ZIP of BMP sprite sheets)
//! and composites the classic windows at exact Winamp sprite coords (from the
//! webamp project). Serves both headless PNG previews and the live in-app view
//! (uploaded as a nearest-neighbour texture).

use std::collections::HashMap;
use std::fs::File;
use std::io::Read;
use std::path::Path;

use anyhow::Result;
use image::{Rgba, RgbaImage};
use zip::ZipArchive;

/// Native classic main-window size in pixels.
pub const MAIN_W: u32 = 275;
pub const MAIN_H: u32 = 116;
/// Equalizer window size.
pub const EQ_H: u32 = 116;
/// Default playlist window height.
pub const PL_H: u32 = 180;

/// Horizontal positions of the 10 EQ band sliders.
pub const EQ_BAND_X: [i32; 10] = [78, 96, 114, 132, 150, 168, 186, 204, 222, 240];
const EQ_PREAMP_X: i32 = 21;
const EQ_SLIDER_TOP: f32 = 38.0;
const EQ_SLIDER_TRAVEL: f32 = 52.0;

/// Clickable / draggable regions of the main window, in 275x116 space.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Btn {
    Prev,
    Play,
    Pause,
    Stop,
    Next,
    Eject,
    Shuffle,
    Repeat,
    Eq,
    Pl,
    Seek,
    Volume,
    Balance,
    Close,
    Minimize,
    Options,
    TitleBar,
    Time,
    Viz,
    // Clutterbar (the O/A/I/D/V column on the left of the main window).
    ClutterO,
    ClutterA,
    ClutterI,
    ClutterD,
    ClutterV,
}

/// (button, x, y, w, h) hit-rects in main-window space, most-specific first.
pub const REGIONS: &[(Btn, i32, i32, i32, i32)] = &[
    (Btn::Options, 6, 3, 9, 9),
    // Clutterbar column (left edge, below the title bar): O/A/I/D/V, 8px wide,
    // 8px tall on a uniform step. The strip is drawn at dest y22 from TITLEBAR.BMP
    // (304,0); its dim glyphs land at y 24/32/40/48/56, so the hit-rects must too
    // (they used to drift 2–4px up the column, so a click low on one letter fell
    // into the next button's rect).
    (Btn::ClutterO, 10, 24, 8, 8),
    (Btn::ClutterA, 10, 32, 8, 8),
    (Btn::ClutterI, 10, 40, 8, 8),
    (Btn::ClutterD, 10, 48, 8, 8),
    (Btn::ClutterV, 10, 56, 8, 8),
    (Btn::Close, 264, 3, 9, 9),
    (Btn::Minimize, 244, 3, 9, 9),
    (Btn::Prev, 16, 88, 23, 18),
    (Btn::Play, 39, 88, 23, 18),
    (Btn::Pause, 62, 88, 23, 18),
    (Btn::Stop, 85, 88, 23, 18),
    (Btn::Next, 108, 88, 22, 18),
    (Btn::Eject, 136, 89, 22, 16),
    (Btn::Shuffle, 164, 89, 47, 15),
    (Btn::Repeat, 210, 89, 28, 15),
    (Btn::Eq, 219, 58, 23, 12),
    (Btn::Pl, 242, 58, 23, 12),
    (Btn::Seek, 16, 72, 248, 10),
    (Btn::Volume, 107, 57, 68, 13),
    (Btn::Balance, 177, 57, 38, 13),
    (Btn::Time, 36, 26, 64, 14), // click to toggle elapsed/remaining
    (Btn::Viz, 24, 43, 76, 13),  // click to cycle visualizer mode
    (Btn::TitleBar, 0, 0, 275, 14), // fallback: drag
];

/// Which region (if any) contains a point in main-window space.
pub fn region_at(x: f32, y: f32) -> Option<Btn> {
    REGIONS
        .iter()
        .find(|(_, rx, ry, rw, rh)| {
            x >= *rx as f32
                && x < (*rx + *rw) as f32
                && y >= *ry as f32
                && y < (*ry + *rh) as f32
        })
        .map(|(b, ..)| *b)
}

/// Hover-tooltip label for a main-window button, or `None` for regions that
/// aren't buttons (title bar, sliders, time/viz toggles).
fn btn_label(b: Btn) -> Option<&'static str> {
    Some(match b {
        Btn::Prev => "Previous",
        Btn::Play => "Play",
        Btn::Pause => "Pause",
        Btn::Stop => "Stop",
        Btn::Next => "Next",
        Btn::Eject => "Open file(s)",
        Btn::Shuffle => "Shuffle",
        Btn::Repeat => "Repeat",
        Btn::Eq => "Equalizer",
        Btn::Pl => "Playlist",
        Btn::Close => "Close",
        Btn::Minimize => "Minimize",
        Btn::Options => "Options",
        Btn::ClutterO => "Options",
        Btn::ClutterA => "Toggle always on top",
        Btn::ClutterI => "View file info",
        Btn::ClutterD => "Toggle doublesize",
        Btn::ClutterV => "Visualization options",
        _ => return None,
    })
}

/// Tooltip (label, x, y, w, h) rects for the main window's buttons, in native
/// skin pixels. Reuses the click `REGIONS` so hover and hit-testing stay in sync.
pub fn main_tooltips() -> Vec<(&'static str, i32, i32, i32, i32)> {
    REGIONS
        .iter()
        .filter_map(|(b, x, y, w, h)| btn_label(*b).map(|l| (l, *x, *y, *w, *h)))
        .collect()
}

/// Tooltip rects for the equalizer window's buttons, in native skin pixels.
pub fn eq_tooltips() -> Vec<(&'static str, i32, i32, i32, i32)> {
    vec![
        ("On", 14, 18, 26, 12),
        ("Auto", 40, 18, 32, 12),
        ("Presets", 217, 18, 44, 12),
        ("Close", 264, 3, 9, 9),
    ]
}

/// Tooltip rects for the playlist window's mini-player transport buttons, in
/// native skin pixels. Mirrors `PL_MINI_BTNS` / `pl_region_at`.
pub fn pl_tooltips(width: i32, height: i32) -> Vec<(&'static str, i32, i32, i32, i32)> {
    let ox = width - 150;
    PL_MINI_BTNS
        .iter()
        .filter_map(|(region, bx, bw)| {
            let label = match region {
                PlRegion::MiniPrev => "Previous",
                PlRegion::MiniPlay => "Play",
                PlRegion::MiniPause => "Pause",
                PlRegion::MiniStop => "Stop",
                PlRegion::MiniNext => "Next",
                PlRegion::MiniEject => "Open file(s)",
                _ => return None,
            };
            Some((label, ox + *bx, height - 15, *bw, 9))
        })
        .collect()
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum PlayUi {
    Playing,
    Paused,
    Stopped,
}

/// One frame of visualization data for the main-window analyzer area.
pub enum VizFrame {
    /// Per-band bar heights and slowly-falling peak markers, each 0..1.
    Spectrum { bars: Vec<f32>, peaks: Vec<f32> },
    /// Mono samples, -1..1.
    Scope(Vec<f32>),
}

/// Everything the compositor needs to draw one frame.
pub struct MainState {
    pub focused: bool,
    pub play: PlayUi,
    pub pressed: Option<Btn>,
    pub shuffle: bool,
    pub repeat: bool,
    pub eq_on: bool,
    pub pl_on: bool,
    /// Clutterbar toggles: lit when active.
    pub always_on_top: bool,
    pub double_size: bool,
    pub pos: f32,
    pub duration: f32,
    /// Show time as remaining (−mm:ss) instead of elapsed.
    pub time_remaining: bool,
    /// Blank the time readout this frame — the "off" half of the paused blink
    /// (classic Winamp flashes the time while paused).
    pub time_hidden: bool,
    pub volume: f32,
    pub balance: f32,
    pub stereo: bool,
    pub title: String,
    pub kbps: String,
    pub khz: String,
    /// LCD/foreground colour, used for the shade-bar title + time.
    pub fg: [u8; 3],
    /// Live visualization for the analyzer area (None = off).
    pub viz: Option<VizFrame>,
    /// Spectrum gradient from the skin (index 0 = top .. last = bottom); empty
    /// falls back to a synthesized green→yellow→red gradient.
    pub viz_spectrum: Vec<[u8; 3]>,
    /// Peak-marker colour from the skin.
    pub viz_peak: [u8; 3],
}

impl MainState {
    /// The LCD time reading as `(minutes, seconds, is_remaining)`, honoring the
    /// elapsed/remaining toggle. Remaining is only reported when the duration is
    /// known; minutes are capped at 99 to fit the two-digit LCD, matching the
    /// classic skin. Shared by the full and window-shade time displays.
    fn lcd_mmss(&self) -> (u32, u32, bool) {
        let remaining = self.time_remaining && self.duration > 0.0;
        let secs = if remaining {
            (self.duration - self.pos).max(0.0) as u32
        } else {
            self.pos.max(0.0) as u32
        };
        ((secs / 60).min(99), secs % 60, remaining)
    }
}

/// Hit target on a 14px window-shade bar.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ShadeHit {
    Close,
    TitleBar,
    // Main window-shade extras (the EQ/playlist shades only use Close/TitleBar).
    Restore,
    Options,
    Prev,
    Play,
    Pause,
    Stop,
    Next,
    Eject,
}

/// Mini-transport buttons baked into the collapsed main window bar:
/// (hit, x, width) in native skin pixels. Measured from TITLEBAR.BMP's
/// windowshade sprite (dest x167..224, the row at y~3..11).
const MAIN_SHADE_BTNS: &[(ShadeHit, i32, i32)] = &[
    (ShadeHit::Prev, 167, 8),
    (ShadeHit::Play, 176, 9),
    (ShadeHit::Pause, 186, 9),
    (ShadeHit::Stop, 196, 8),
    (ShadeHit::Next, 205, 9),
    (ShadeHit::Eject, 215, 9),
];

pub fn shade_region_at(x: f32, y: f32) -> Option<ShadeHit> {
    if y >= 14.0 {
        return None;
    }
    if x >= 264.0 && x < 273.0 && y >= 3.0 && y < 12.0 {
        Some(ShadeHit::Close)
    } else {
        Some(ShadeHit::TitleBar)
    }
}

/// Hit-testing for the collapsed **main** window bar, which (unlike the EQ and
/// playlist shades) carries the wave/options button, mini-transport buttons, and
/// a restore button. Anything not on a control is the draggable title bar.
pub fn main_shade_region_at(x: f32, y: f32) -> Option<ShadeHit> {
    if y >= 14.0 {
        return None;
    }
    if x >= 264.0 && x < 273.0 && y >= 3.0 && y < 12.0 {
        return Some(ShadeHit::Close);
    }
    if x >= 254.0 && x < 263.0 && y >= 3.0 && y < 12.0 {
        return Some(ShadeHit::Restore);
    }
    // Wave/clutter button at the far left opens the Options menu.
    if x >= 1.0 && x < 18.0 {
        return Some(ShadeHit::Options);
    }
    for (hit, bx, bw) in MAIN_SHADE_BTNS {
        if x >= *bx as f32 && x < (*bx + *bw) as f32 {
            return Some(*hit);
        }
    }
    Some(ShadeHit::TitleBar)
}

/// Decoded sprite sheets from one skin, keyed by lowercased basename.
pub struct ClassicSkin {
    bitmaps: HashMap<String, RgbaImage>,
    /// EQ slider sprites the skin ships (invariant per skin, computed once).
    eq_sprites: Vec<i32>,
    /// Background colour of the bitmap font (TEXT.BMP), so tinted text can tell
    /// the glyph from its backing regardless of whether the font is light-on-dark
    /// or dark-on-light.
    text_bg: [u8; 3],
    /// Window-shape masks from REGION.TXT (empty = rectangular windows).
    pub regions: crate::region::SkinRegions,
    /// Custom cursors from the skin's `.cur` files, keyed by lowercased basename
    /// without extension (e.g. "normal", "posbar").
    cursors: HashMap<String, crate::cursor::CursorImage>,
}

impl ClassicSkin {
    pub fn load(path: &Path) -> Result<Self> {
        let file = File::open(path)?;
        let mut zip = ZipArchive::new(file)?;
        let mut bitmaps = HashMap::new();
        let mut cursors = HashMap::new();
        let mut region_txt = String::new();
        for i in 0..zip.len() {
            let mut f = zip.by_index(i)?;
            let name = f.name().to_string();
            let base = name
                .rsplit(|c| c == '/' || c == '\\')
                .next()
                .unwrap_or(&name)
                .to_ascii_lowercase();
            if base.ends_with(".bmp") {
                let mut buf = Vec::new();
                if f.read_to_end(&mut buf).is_ok() {
                    if let Ok(img) = image::load_from_memory(&buf) {
                        bitmaps.insert(base, img.to_rgba8());
                    }
                }
            } else if base == "region.txt" {
                let mut buf = Vec::new();
                if f.read_to_end(&mut buf).is_ok() {
                    region_txt = String::from_utf8_lossy(&buf).into_owned();
                }
            } else if base.ends_with(".cur") {
                let mut buf = Vec::new();
                if f.read_to_end(&mut buf).is_ok() {
                    if let Some(cur) = crate::cursor::parse(&buf) {
                        let key = base.trim_end_matches(".cur").to_string();
                        cursors.insert(key, cur);
                    }
                }
            }
        }
        if !bitmaps.contains_key("main.bmp") {
            anyhow::bail!("skin has no MAIN.BMP");
        }
        let regions = crate::region::parse(&region_txt);
        let eq_sprites = bitmaps
            .get("eqmain.bmp")
            .map(eq_valid_sprites)
            .unwrap_or_default();
        let text_bg = bitmaps
            .get("text.bmp")
            .map(dominant_color)
            .unwrap_or([0, 0, 0]);
        Ok(Self {
            bitmaps,
            eq_sprites,
            text_bg,
            regions,
            cursors,
        })
    }

    /// A custom cursor by lowercased basename (no extension), if the skin ships it.
    pub fn cursor(&self, name: &str) -> Option<&crate::cursor::CursorImage> {
        self.cursors.get(name)
    }

    /// Whether the skin ships any custom cursors.
    pub fn has_cursors(&self) -> bool {
        !self.cursors.is_empty()
    }

    /// Number of custom cursors parsed from the skin (diagnostics).
    pub fn cursor_count(&self) -> usize {
        self.cursors.len()
    }

    fn bmp(&self, name: &str) -> Option<&RgbaImage> {
        self.bitmaps.get(name)
    }

    /// Composite the main window for `st` into a fresh 275x116 image.
    pub fn compose_main(&self, st: &MainState) -> RgbaImage {
        let mut c = RgbaImage::new(MAIN_W, MAIN_H);

        if let Some(m) = self.bmp("main.bmp") {
            blit(&mut c, m, 0, 0, 0, 0, MAIN_W, MAIN_H);
        }

        // Title bar + its buttons.
        if let Some(tb) = self.bmp("titlebar.bmp") {
            let sy = if st.focused { 0 } else { 15 };
            blit(&mut c, tb, 0, 0, 27, sy, 275, 14);
            blit(&mut c, tb, 244, 3, 9, 0, 9, 9); // minimize
            blit(&mut c, tb, 254, 3, 0, 18, 9, 9); // shade
            blit(&mut c, tb, 264, 3, 18, 0, 9, 9); // close

            // Clutterbar (O/A/I/D/V) column: the normal strip, then "lit" overlays
            // for the active toggles and whichever button is being pressed.
            // Source rects from TITLEBAR.BMP (canonical Winamp sprite table).
            blit(&mut c, tb, 10, 22, 304, 0, 8, 43); // background strip
            // Lit/pressed overlays replace a dim glyph with its bright version at
            // the same spot: dest y 24/32/40/48/56 (matching the strip) from the
            // active-sprite diagonal at src y 46/54/62/70/78, all 8×8 on a uniform
            // step so the bright glyph lands exactly on the dim one.
            if st.always_on_top {
                blit(&mut c, tb, 10, 32, 312, 54, 8, 8); // A lit
            }
            if st.double_size {
                blit(&mut c, tb, 10, 48, 328, 70, 8, 8); // D lit
            }
            match st.pressed {
                Some(Btn::ClutterO) => blit(&mut c, tb, 10, 24, 304, 46, 8, 8),
                Some(Btn::ClutterA) => blit(&mut c, tb, 10, 32, 312, 54, 8, 8),
                Some(Btn::ClutterI) => blit(&mut c, tb, 10, 40, 320, 62, 8, 8),
                Some(Btn::ClutterD) => blit(&mut c, tb, 10, 48, 328, 70, 8, 8),
                Some(Btn::ClutterV) => blit(&mut c, tb, 10, 56, 336, 78, 8, 8),
                _ => {}
            }
        }

        // Transport buttons.
        if let Some(cb) = self.bmp("cbuttons.bmp") {
            let dn = |b: Btn| if st.pressed == Some(b) { 1 } else { 0 };
            blit(&mut c, cb, 16, 88, 0, dn(Btn::Prev) * 18, 23, 18);
            blit(&mut c, cb, 39, 88, 23, dn(Btn::Play) * 18, 23, 18);
            blit(&mut c, cb, 62, 88, 46, dn(Btn::Pause) * 18, 23, 18);
            blit(&mut c, cb, 85, 88, 69, dn(Btn::Stop) * 18, 23, 18);
            blit(&mut c, cb, 108, 88, 92, dn(Btn::Next) * 18, 22, 18);
            blit(&mut c, cb, 136, 89, 114, dn(Btn::Eject) * 16, 22, 16);
        }

        // Shuffle / repeat / eq / pl.
        if let Some(sr) = self.bmp("shufrep.bmp") {
            let ysel = |on: bool, pressed: bool| match (on, pressed) {
                (true, true) => 45,
                (true, false) => 30,
                (false, true) => 15,
                (false, false) => 0,
            };
            blit(&mut c, sr, 164, 89, 28, ysel(st.shuffle, st.pressed == Some(Btn::Shuffle)), 47, 15);
            blit(&mut c, sr, 210, 89, 0, ysel(st.repeat, st.pressed == Some(Btn::Repeat)), 28, 15);
            blit(&mut c, sr, 219, 58, 0, if st.eq_on { 73 } else { 61 }, 23, 12);
            blit(&mut c, sr, 242, 58, 23, if st.pl_on { 73 } else { 61 }, 23, 12);
        }

        // Play/pause/stop indicator.
        if let Some(pp) = self.bmp("playpaus.bmp") {
            let x = match st.play {
                PlayUi::Playing => 0,
                PlayUi::Paused => 9,
                PlayUi::Stopped => 18,
            };
            blit(&mut c, pp, 26, 28, x, 0, 9, 9);
        }

        // Time digits (elapsed, or remaining with a leading minus). Blanked on the
        // "off" half of the paused blink.
        if !st.time_hidden {
            if let Some(nb) = self.bmp("numbers.bmp").or_else(|| self.bmp("nums_ex.bmp")) {
                let (mm, ss, remaining) = st.lcd_mmss();
                let digits = [mm / 10, mm % 10, ss / 10, ss % 10];
                let xs = [48, 60, 78, 90];
                for (i, &d) in digits.iter().enumerate() {
                    blit(&mut c, nb, xs[i], 26, d * 9, 0, 9, 13);
                }
                if remaining {
                    // Minus sign in the LCD colour, just left of the minutes.
                    fill_rect(&mut c, 40, 32, 5, 2, st.fg);
                }
            }
        }

        // Mono / stereo.
        if let Some(ms) = self.bmp("monoster.bmp") {
            blit(&mut c, ms, 212, 41, 29, if st.stereo { 12 } else { 0 }, 27, 12); // mono
            blit(&mut c, ms, 239, 41, 0, if st.stereo { 0 } else { 12 }, 29, 12); // stereo
        }

        // Position slider.
        if let Some(pb) = self.bmp("posbar.bmp") {
            blit(&mut c, pb, 16, 72, 0, 0, 248, 10);
            if st.duration > 0.0 {
                let frac = (st.pos / st.duration).clamp(0.0, 1.0);
                let tx = 16 + (frac * (248.0 - 29.0)) as i32;
                let sx = if st.pressed == Some(Btn::Seek) { 278 } else { 248 };
                blit(&mut c, pb, tx, 72, sx, 0, 29, 10);
            }
        }

        // Volume.
        if let Some(vb) = self.bmp("volume.bmp") {
            let v = st.volume.clamp(0.0, 1.0);
            let row = (v * 27.0).round() as u32;
            blit(&mut c, vb, 107, 57, 0, row * 15, 68, 13);
            let tx = 107 + (v * (68.0 - 14.0)) as i32;
            let sx = if st.pressed == Some(Btn::Volume) { 0 } else { 15 };
            blit(&mut c, vb, tx, 58, sx, 422, 14, 11);
        }

        // Balance.
        if let Some(bb) = self.bmp("balance.bmp") {
            let amt = (st.balance.abs().clamp(0.0, 1.0) * 27.0).round() as u32;
            blit(&mut c, bb, 177, 57, 9, amt * 15, 38, 13);
            let tx = 177 + ((st.balance * 0.5 + 0.5).clamp(0.0, 1.0) * (38.0 - 14.0)) as i32;
            let sx = if st.pressed == Some(Btn::Balance) { 0 } else { 15 };
            blit(&mut c, bb, tx, 58, sx, 422, 14, 11);
        }

        // Scrolling title, kbps, khz (bitmap font). The title sits centered in
        // its ~12px LCD slot (rows 24-35), so its 6px glyphs start at y=27.
        self.draw_text(&mut c, &st.title, 111, 27, 154);
        self.draw_text(&mut c, &st.kbps, 111, 43, 15);
        self.draw_text(&mut c, &st.khz, 156, 43, 10);

        // Visualizer in the empty LCD slot below the time.
        draw_viz(&mut c, st, 24, 43, 76, 13);

        c
    }

    /// Collapsed main window: a 14px shade bar with scrolling title + time.
    pub fn compose_main_shade(&self, st: &MainState) -> RgbaImage {
        let mut c = RgbaImage::new(275, 14);
        if let Some(tb) = self.bmp("titlebar.bmp") {
            let sy = if st.focused { 29 } else { 42 };
            blit(&mut c, tb, 0, 0, 27, sy, 275, 14);
            blit(&mut c, tb, 254, 3, 0, 18, 9, 9); // shade / restore button
            blit(&mut c, tb, 264, 3, 18, 0, 9, 9); // close button
        }
        // The main windowshade sprite is a self-contained strip (branding, seek,
        // time slot, mini transport, buttons) with no song-title LCD, so we only
        // fill in the time: the digits sit either side of the sprite's baked ":"
        // separator (~x143). Shows remaining (with a leading minus) when the
        // elapsed/remaining toggle is on, matching the expanded LCD.
        if !st.time_hidden {
            let (mm, ss, remaining) = st.lcd_mmss();
            let mm = format!("{:02}", mm);
            let ss = format!("{:02}", ss);
            self.draw_text_tinted(&mut c, &mm, 132, 4, 10, st.fg);
            self.draw_text_tinted(&mut c, &ss, 146, 4, 10, st.fg);
            if remaining {
                fill_rect(&mut c, 126, 7, 4, 2, st.fg); // minus sign before the minutes
            }
        }
        c
    }

    fn draw_text(&self, c: &mut RgbaImage, text: &str, x0: i32, y0: i32, max_w: i32) {
        let Some(tf) = self.bmp("text.bmp") else {
            return;
        };
        let mut x = x0;
        let limit = x0 + max_w;
        for ch in text.chars() {
            if x + 5 > limit {
                break;
            }
            let (col, row) = char_cell(ch);
            blit(c, tf, x, y0, col * 5, row * 6, 5, 6);
            x += 5;
        }
    }
}

/// The generic-window ("gen.bmp"/"genex.bmp") colour scheme a skin defines for
/// its general-purpose windows (the Media Library). Colours are single pixels on
/// the top row of GENEX.BMP; see the GENEX colour map (x=48,50,… at y=0).
#[derive(Clone, Copy, Debug)]
pub struct GenColors {
    pub item_bg: [u8; 3],
    pub item_fg: [u8; 3],
    pub window_bg: [u8; 3],
    pub selection: [u8; 3],
    pub header_bg: [u8; 3],
    pub header_text: [u8; 3],
}

// GEN.BMP sprite rectangles (x, y, w, h), from the webamp project. The active
// ("selected") title-bar row is at y=0; the inactive one at y=21.
const GEN_TB_H: u32 = 20; // title bar height
const GEN_BORDER_L: u32 = 11; // left side wall width
const GEN_BORDER_R: u32 = 8; // right side wall width
const GEN_BORDER_B: u32 = 14; // bottom bar height
const GEN_CORNER_W: u32 = 25; // title-bar corner / bracket width
const GEN_BOTTOM_SIDE_W: u32 = 125; // bottom-left / bottom-right piece width

// GEN.BMP title font: uppercase A–Z glyphs (src_x, width) on the bottom rows,
// measured from the base skin. Highlighted row at y=88, normal row at y=96; both
// 7px tall. The cyan key colour marks transparency.
const GEN_FONT_Y_SELECTED: u32 = 88;
const GEN_FONT_Y_NORMAL: u32 = 96;
const GEN_FONT_H: u32 = 7;
const GEN_KEY: [u8; 3] = [0, 198, 255];
const GEN_FONT: [(u32, u32); 26] = [
    (1, 6), (8, 7), (16, 7), (24, 6), (31, 6), (38, 6), (45, 7), (53, 6), (60, 4),
    (65, 6), (72, 7), (80, 5), (86, 8), (95, 6), (102, 6), (109, 6), (116, 7),
    (124, 7), (132, 6), (139, 5), (145, 6), (152, 6), (159, 8), (168, 7), (175, 7),
    (183, 5),
];

fn gen_glyph(c: char) -> Option<(u32, u32)> {
    if c.is_ascii_uppercase() {
        Some(GEN_FONT[(c as u8 - b'A') as usize])
    } else {
        None
    }
}

/// Copy `src[sx..+sw, sy..+sh]` to `dst` at (dx,dy), skipping pixels that match the
/// key colour (transparent).
fn blit_keyed(dst: &mut RgbaImage, src: &RgbaImage, dx: i32, dy: i32, sx: u32, sy: u32, w: u32, h: u32, key: [u8; 3]) {
    // Treat pixels *near* the key colour as transparent too, not just an exact
    // match. This drops faint anti-alias fringe pixels that were blended toward
    // the key (e.g. the base skin's stray ~20%-blend cyan dot above the "Y") —
    // such fringing is an artifact once the glyph is composited onto the title
    // bar's own (non-key) background. The tolerance stays well below any real
    // glyph pixel (white/grey letters), so letters are untouched.
    const TOL: u8 = 60;
    let near_key = |p: &image::Rgba<u8>| {
        p[0].abs_diff(key[0]) <= TOL && p[1].abs_diff(key[1]) <= TOL && p[2].abs_diff(key[2]) <= TOL
    };
    let (sw, sh) = src.dimensions();
    let (dw, dh) = dst.dimensions();
    for j in 0..h {
        let (syy, dyy) = (sy + j, dy + j as i32);
        if syy >= sh || dyy < 0 || dyy >= dh as i32 {
            continue;
        }
        for i in 0..w {
            let (sxx, dxx) = (sx + i, dx + i as i32);
            if sxx >= sw || dxx < 0 || dxx >= dw as i32 {
                continue;
            }
            let p = src.get_pixel(sxx, syy);
            if !near_key(p) {
                dst.put_pixel(dxx as u32, dyy as u32, *p);
            }
        }
    }
}

/// Pixel width of `title` rendered in the gen title font.
fn gen_title_width(title: &str) -> u32 {
    title
        .to_uppercase()
        .chars()
        .map(|c| if c == ' ' { 4 } else { gen_glyph(c).map(|(_, w)| w + 1).unwrap_or(0) })
        .sum::<u32>()
        .saturating_sub(1)
}

/// Per-glyph `(src_x, width)` for A–Z on font row `fy`, measured from the BMP as
/// runs of non-`key` columns. Returns `Some` only when exactly 26 glyphs are found
/// — so a skin whose font layout differs from the base (e.g. Green Dimension's
/// fixed-width grid) renders from its own metrics, while anything unexpected
/// (including the base skin, whose X and Y touch → 25 runs) falls back to the
/// fixed `GEN_FONT` table.
fn gen_font_table(g: &RgbaImage, fy: u32, key: [u8; 3]) -> Option<Vec<(u32, u32)>> {
    if fy + GEN_FONT_H > g.height() {
        return None;
    }
    let col_ink = |x: u32| {
        (0..GEN_FONT_H).any(|j| {
            let p = g.get_pixel(x, fy + j);
            [p[0], p[1], p[2]] != key
        })
    };
    let mut runs = Vec::new();
    let (w, mut x) = (g.width(), 0);
    while x < w {
        if col_ink(x) {
            let start = x;
            while x < w && col_ink(x) {
                x += 1;
            }
            runs.push((start, x - start));
        } else {
            x += 1;
        }
    }
    (runs.len() == 26).then_some(runs)
}

/// Draw `title` (uppercased) centred in the title bar using GEN.BMP's own font.
fn blit_gen_title(dst: &mut RgbaImage, g: &RgbaImage, title: &str, active: bool) {
    let up = title.to_uppercase();
    let fy = if active { GEN_FONT_Y_SELECTED } else { GEN_FONT_Y_NORMAL };
    // The font's transparency key varies per skin (cyan in the base skin, maroon
    // in Green Dimension, …). Sample it from the 1px margin left of the first
    // glyph (A starts at x=1) on this font row; fall back to the common cyan when
    // the skin's GEN.BMP is too short to hold the font rows.
    let key_at = |row: u32| {
        if row < g.height() {
            let p = g.get_pixel(0, row);
            [p[0], p[1], p[2]]
        } else {
            GEN_KEY
        }
    };
    let key = key_at(fy);
    // Measure glyph widths from the SELECTED row for both variants. The glyph
    // columns are identical in the selected and normal rows, but the normal
    // (inactive) row has a stray pixel on some glyphs (e.g. Y) that would widen
    // the measured run; using the clean selected widths excludes it and keeps
    // kerning identical between the two variants (#106).
    let table = gen_font_table(g, GEN_FONT_Y_SELECTED, key_at(GEN_FONT_Y_SELECTED));
    let glyph = |c: char| -> Option<(u32, u32)> {
        c.is_ascii_uppercase().then(|| {
            let i = (c as u8 - b'A') as usize;
            table.as_ref().map(|t| t[i]).unwrap_or(GEN_FONT[i])
        })
    };
    let total: u32 = up
        .chars()
        .map(|c| if c == ' ' { 4 } else { glyph(c).map(|(_, w)| w + 1).unwrap_or(0) })
        .sum::<u32>()
        .saturating_sub(1);
    let dw = dst.width();
    let mut x = (dw / 2).saturating_sub((total + 1) / 2);
    // Nudged up 2px from the geometric centre so the caption lines up with the
    // classic windows' own title text (which sits a little above centre).
    let ty = GEN_TB_H.saturating_sub(GEN_FONT_H) / 2 - 2;
    for c in up.chars() {
        if c == ' ' {
            x += 4;
            continue;
        }
        if let Some((sx, w)) = glyph(c) {
            blit_keyed(dst, g, x as i32, ty as i32, sx, fy, w, GEN_FONT_H, key);
            x += w + 1;
        }
    }
}

/// Horizontally tile `src[sx..+sw, sy..+sh]` across the destination span `[x0, x1)`.
fn tile_h(dst: &mut RgbaImage, src: &RgbaImage, x0: u32, x1: u32, dy: i32, sx: u32, sy: u32, sw: u32, sh: u32) {
    let mut x = x0;
    while x < x1 {
        let w = sw.min(x1 - x);
        blit(dst, src, x as i32, dy, sx, sy, w, sh);
        x += sw;
    }
}

/// Vertically tile `src[sx..+sw, sy..+sh]` down the destination span `[y0, y1)`.
fn tile_v(dst: &mut RgbaImage, src: &RgbaImage, dx: i32, y0: u32, y1: u32, sx: u32, sy: u32, sw: u32, sh: u32) {
    let mut y = y0;
    while y < y1 {
        let h = sh.min(y1 - y);
        blit(dst, src, dx, y as i32, sx, sy, sw, h);
        y += sh;
    }
}

impl ClassicSkin {
    /// Whether this skin ships the general-purpose window art for the Media Library.
    pub fn has_gen(&self) -> bool {
        self.bmp("gen.bmp").is_some() && self.bmp("genex.bmp").is_some()
    }

    /// The Media Library colour scheme from GENEX.BMP, if the skin defines it.
    pub fn gen_colors(&self) -> Option<GenColors> {
        let g = self.bmp("genex.bmp")?;
        let px = |x: u32| -> [u8; 3] {
            if x < g.width() {
                let p = g.get_pixel(x, 0);
                [p[0], p[1], p[2]]
            } else {
                [0, 0, 0]
            }
        };
        Some(GenColors {
            item_bg: px(48),
            item_fg: px(50),
            window_bg: px(52),
            selection: px(60),
            header_bg: px(62),
            header_text: px(64),
        })
    }

    /// Inset (top, right, bottom, left) in native px that the gen frame occupies,
    /// i.e. where the window content area begins.
    pub fn gen_insets() -> (u32, u32, u32, u32) {
        (GEN_TB_H, GEN_BORDER_R, GEN_BORDER_B, GEN_BORDER_L)
    }

    /// Composite the general-purpose window frame at native size `w`×`h`, filling
    /// the interior with the window background colour. `title_w` reserves a centred
    /// span of the title bar for the caption (drawn by the caller). Returns `None`
    /// when the skin has no gen art.
    pub fn compose_gen_window(&self, w: u32, h: u32, active: bool, title: &str) -> Option<RgbaImage> {
        let g = self.bmp("gen.bmp")?;
        let cols = self.gen_colors()?;
        let w = w.max(2 * GEN_CORNER_W + 10);
        let h = h.max(GEN_TB_H + GEN_BORDER_B + 10);
        let mut img = RgbaImage::new(w, h);
        let wb = Rgba([cols.window_bg[0], cols.window_bg[1], cols.window_bg[2], 255]);
        for p in img.pixels_mut() {
            *p = wb;
        }
        let ry = if active { 0 } else { 21 };

        // --- Title bar (y 0..20). Winamp layout, left→right: corner, striped
        // fill running inward, an end-cap, the title (its own centre fill with the
        // caption), the other end-cap, striped fill, corner — so the bars run all
        // the way to the caption like the main window.
        let cap = GEN_CORNER_W; // end-cap / corner width (25)
        let tw = (gen_title_width(title) + 10).min(w.saturating_sub(4 * cap));
        let tleft = (w / 2).saturating_sub(tw / 2);
        let tright = tleft + tw;
        blit(&mut img, g, 0, 0, 0, ry, cap, GEN_TB_H); // TOP_LEFT (corner)
        blit(&mut img, g, (w - cap) as i32, 0, 130, ry, cap, GEN_TB_H); // TOP_RIGHT (corner + close)
        // Striped fill (TOP_LEFT_RIGHT_FILL) between the corners and the end-caps.
        tile_h(&mut img, g, cap, tleft.saturating_sub(cap).max(cap), 0, 104, ry, cap, GEN_TB_H);
        tile_h(&mut img, g, tright + cap, w - cap, 0, 104, ry, cap, GEN_TB_H);
        // End-caps flanking the title, then the title's own centre fill.
        blit(&mut img, g, tleft.saturating_sub(cap) as i32, 0, 26, ry, cap, GEN_TB_H); // TOP_LEFT_END
        blit(&mut img, g, tright as i32, 0, 78, ry, cap, GEN_TB_H); // TOP_RIGHT_END
        tile_h(&mut img, g, tleft, tright, 0, 52, ry, cap, GEN_TB_H); // TOP_CENTER_FILL
        blit_gen_title(&mut img, g, title, active);

        // --- Side walls (tiled vertically between the title bar and bottom bar).
        let side_top = GEN_TB_H;
        let side_bot = h - GEN_BORDER_B;
        tile_v(&mut img, g, 0, side_top, side_bot, 127, 42, GEN_BORDER_L, 29);
        tile_v(&mut img, g, (w - GEN_BORDER_R) as i32, side_top, side_bot, 139, 42, GEN_BORDER_R, 29);

        // --- Bottom bar: left piece, tiled fill, right piece.
        let by = (h - GEN_BORDER_B) as i32;
        blit(&mut img, g, 0, by, 0, 42, GEN_BOTTOM_SIDE_W, GEN_BORDER_B); // BOTTOM_LEFT
        blit(&mut img, g, (w - GEN_BOTTOM_SIDE_W) as i32, by, 0, 57, GEN_BOTTOM_SIDE_W, GEN_BORDER_B); // BOTTOM_RIGHT
        tile_h(&mut img, g, GEN_BOTTOM_SIDE_W, w - GEN_BOTTOM_SIDE_W, by, 127, 72, GEN_CORNER_W, GEN_BORDER_B);

        Some(img)
    }
}

/// Copy a sub-rectangle of `src` to `dst` at (dx,dy), clipping to both.
fn blit(dst: &mut RgbaImage, src: &RgbaImage, dx: i32, dy: i32, sx: u32, sy: u32, w: u32, h: u32) {
    let (sw, sh) = src.dimensions();
    let (dw, dh) = dst.dimensions();
    for j in 0..h {
        let syy = sy + j;
        if syy >= sh {
            break;
        }
        let dyy = dy + j as i32;
        if dyy < 0 || dyy >= dh as i32 {
            continue;
        }
        for i in 0..w {
            let sxx = sx + i;
            if sxx >= sw {
                break;
            }
            let dxx = dx + i as i32;
            if dxx < 0 || dxx >= dw as i32 {
                continue;
            }
            let p = *src.get_pixel(sxx, syy);
            dst.put_pixel(dxx as u32, dyy as u32, p);
        }
    }
}

/// Approximate an accented Latin character with its base ASCII letter, so the
/// ASCII-only bitmap font renders it readably instead of as a blank (#97, e.g.
/// Vietnamese "Trường" -> "Truong"). Returns the char unchanged when unmapped.
/// This is a readability fallback; full non-Latin glyphs would need a font atlas.
fn ascii_fold(ch: char) -> char {
    match ch as u32 {
        // Latin-1 Supplement
        0xC0..=0xC6 | 0xE0..=0xE6 => 'a',
        0xC7 | 0xE7 => 'c',
        0xC8..=0xCB | 0xE8..=0xEB => 'e',
        0xCC..=0xCF | 0xEC..=0xEF => 'i',
        0xD0 | 0xF0 => 'd',
        0xD1 | 0xF1 => 'n',
        0xD2..=0xD6 | 0xD8 | 0xF2..=0xF6 | 0xF8 => 'o',
        0xD9..=0xDC | 0xF9..=0xFC => 'u',
        0xDD | 0xFD | 0xFF => 'y',
        0xDF => 's',
        // Latin Extended-A/B: common accented letters and Vietnamese ơ/ư/đ/ă
        0x100..=0x105 => 'a',
        0x106..=0x10D => 'c',
        0x110 | 0x111 => 'd',
        0x112..=0x11B => 'e',
        0x128..=0x131 => 'i',
        0x139..=0x142 => 'l',
        0x143..=0x148 => 'n',
        0x14C..=0x151 | 0x1A0 | 0x1A1 => 'o',
        0x154..=0x159 => 'r',
        0x15A..=0x161 => 's',
        0x162..=0x165 => 't',
        0x168..=0x16F | 0x1AF | 0x1B0 => 'u',
        0x174 | 0x175 => 'w',
        0x176..=0x178 => 'y',
        0x179..=0x17E => 'z',
        // Vietnamese tone marks (Latin Extended Additional)
        0x1EA0..=0x1EB7 => 'a',
        0x1EB8..=0x1EC7 => 'e',
        0x1EC8..=0x1ECB => 'i',
        0x1ECC..=0x1EE3 => 'o',
        0x1EE4..=0x1EF1 => 'u',
        0x1EF2..=0x1EF9 => 'y',
        _ => ch,
    }
}

/// Map a character to its (column, row) cell in TEXT.BMP (5x6 cells).
fn char_cell(ch: char) -> (u32, u32) {
    let c = ascii_fold(ch).to_ascii_lowercase();
    match c {
        'a'..='z' => (c as u32 - 'a' as u32, 0),
        '0'..='9' => (c as u32 - '0' as u32, 1),
        '"' => (26, 0),
        '@' => (27, 0),
        '.' => (11, 1),
        ':' => (12, 1),
        '(' => (13, 1),
        ')' => (14, 1),
        '-' => (15, 1),
        '\'' => (16, 1),
        '!' => (17, 1),
        '_' => (18, 1),
        '+' => (19, 1),
        '\\' => (20, 1),
        '/' => (21, 1),
        '[' | '<' | '{' => (22, 1),
        ']' | '>' | '}' => (23, 1),
        '^' => (24, 1),
        '&' => (25, 1),
        '%' => (26, 1),
        ',' => (27, 1),
        '=' => (28, 1),
        '$' => (29, 1),
        '#' => (30, 1),
        '?' => (3, 2),
        '*' => (4, 2),
        _ => (30, 0), // space
    }
}

// ======================= Equalizer window ===================================

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum EqRegion {
    On,
    Auto,
    Presets,
    Close,
    TitleBar,
    Preamp,
    Band(usize),
}

pub struct EqState {
    pub focused: bool,
    pub on: bool,
    pub auto: bool,
    pub pressed: Option<EqRegion>,
    /// Preamp and 10 bands, each -1.0..=1.0 (bottom..top).
    pub preamp: f32,
    pub bands: [f32; 10],
}

pub fn eq_region_at(x: f32, y: f32) -> Option<EqRegion> {
    let hit = |rx: f32, ry: f32, rw: f32, rh: f32| x >= rx && x < rx + rw && y >= ry && y < ry + rh;
    if hit(264.0, 3.0, 9.0, 9.0) {
        return Some(EqRegion::Close);
    }
    if hit(14.0, 18.0, 26.0, 12.0) {
        return Some(EqRegion::On);
    }
    if hit(40.0, 18.0, 32.0, 12.0) {
        return Some(EqRegion::Auto);
    }
    if hit(217.0, 18.0, 44.0, 12.0) {
        return Some(EqRegion::Presets);
    }
    if hit(EQ_PREAMP_X as f32, EQ_SLIDER_TOP, 14.0, 63.0) {
        return Some(EqRegion::Preamp);
    }
    for (i, &bx) in EQ_BAND_X.iter().enumerate() {
        if hit(bx as f32, EQ_SLIDER_TOP, 14.0, 63.0) {
            return Some(EqRegion::Band(i));
        }
    }
    if hit(0.0, 0.0, 275.0, 14.0) {
        return Some(EqRegion::TitleBar);
    }
    None
}

/// Convert a y within a band slider column to a value in -1.0..=1.0.
pub fn eq_value_at_y(y: f32) -> f32 {
    let frac = ((y - EQ_SLIDER_TOP) / EQ_SLIDER_TRAVEL).clamp(0.0, 1.0);
    1.0 - 2.0 * frac
}

// ======================= Playlist window ====================================

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum PlRegion {
    Close,
    TitleBar,
    List,
    AddMenu,
    RemMenu,
    SelMenu,
    MiscMenu,
    ListMenu,
    Resize,
    // Mini-player transport buttons in the bottom-right sprite.
    MiniPrev,
    MiniPlay,
    MiniPause,
    MiniStop,
    MiniNext,
    MiniEject,
}

#[derive(Clone, Copy)]
pub struct PlColors {
    pub bg: [u8; 3],
    pub normal: [u8; 3],
    pub current: [u8; 3],
    pub selbg: [u8; 3],
}

/// One playlist row: display text, is-playing, is-selected, track length in
/// seconds (None when not yet known). The length is drawn right-aligned.
pub type PlRow = (String, bool, bool, Option<f32>);

pub struct PlState<'a> {
    pub focused: bool,
    pub colors: PlColors,
    pub rows: &'a [PlRow],
    pub first: usize,
    /// Bottom-bar menu button currently held / with its menu open.
    pub pressed: Option<PlRegion>,
    /// Elapsed time of the current track, in seconds (0 when stopped).
    pub elapsed_secs: f32,
    /// Duration of the current track, for the remaining-time toggle (0 if unknown).
    pub cur_duration: f32,
    /// Show the current-track time as remaining (−m:ss) instead of elapsed.
    pub time_remaining: bool,
    /// Total running time of the whole playlist, in seconds (sum of the known
    /// track durations).
    pub total_secs: f32,
    /// True when at least one track's length is unknown, so `total_secs` is a
    /// lower bound and the readout gets a trailing `+` (as in Winamp).
    pub total_partial: bool,
    /// Blank the current-track time this frame — the "off" half of the paused
    /// blink (the running total stays put; only the current time flashes).
    pub time_hidden: bool,
}

/// Left-anchored bottom-bar menu buttons: (region, x, width). y is
/// `height - 30`, h = 18. The List menu is right-anchored (see `pl_region_at`).
const PL_BOTTOM_BTNS: &[(PlRegion, i32, i32)] = &[
    (PlRegion::AddMenu, 14, 22),
    (PlRegion::RemMenu, 43, 22),
    (PlRegion::SelMenu, 72, 22),
    (PlRegion::MiscMenu, 101, 22),
];

/// Mini-player transport buttons in the bottom-right sprite: (region, x, width)
/// offset from the sprite origin (`width - 150`), row at `height - 15..-7`.
/// Measured from PLEDIT.BMP's bottom-right glyphs (source x132..184).
const PL_MINI_BTNS: &[(PlRegion, i32, i32)] = &[
    (PlRegion::MiniPrev, 5, 8),
    (PlRegion::MiniPlay, 13, 8),
    (PlRegion::MiniPause, 23, 8),
    (PlRegion::MiniStop, 33, 8),
    (PlRegion::MiniNext, 42, 8),
    (PlRegion::MiniEject, 51, 9),
];


const PL_ROW_H: i32 = 7;
const PL_LIST_X: i32 = 14;
const PL_LIST_Y: i32 = 21;

pub fn pl_region_at(x: f32, y: f32, width: f32, height: f32) -> Option<PlRegion> {
    if x >= width - 11.0 && x < width - 2.0 && y >= 3.0 && y < 12.0 {
        return Some(PlRegion::Close);
    }
    if y < 20.0 {
        return Some(PlRegion::TitleBar);
    }
    // Resize grip (bottom-right corner).
    if x >= width - 20.0 && y >= height - 20.0 {
        return Some(PlRegion::Resize);
    }
    if y < height - 38.0 {
        return Some(PlRegion::List);
    }
    // Mini-player transport buttons, in the lower strip of the bottom-right
    // sprite (checked before the menu band, which overlaps in y but not in x).
    if y >= height - 15.0 && y < height - 6.0 {
        let ox = width - 150.0;
        for (region, bx, bw) in PL_MINI_BTNS {
            let x0 = ox + *bx as f32;
            if x >= x0 && x < x0 + *bw as f32 {
                return Some(*region);
            }
        }
    }
    // Bottom-bar menu buttons: the Add/Rem/Sel/Misc cluster is anchored left,
    // the List menu is anchored to the right edge.
    let by = height - 30.0;
    if y >= by && y < by + 18.0 {
        for (region, bx, bw) in PL_BOTTOM_BTNS {
            if x >= *bx as f32 && x < (*bx + *bw) as f32 {
                return Some(*region);
            }
        }
        if x >= width - 44.0 && x < width - 22.0 {
            return Some(PlRegion::ListMenu);
        }
    }
    None
}

pub fn pl_row_at(y: f32, first: usize) -> usize {
    first + (((y - PL_LIST_Y as f32) / PL_ROW_H as f32).max(0.0) as usize)
}

pub fn pl_visible_rows(height: u32) -> usize {
    ((height as i32 - PL_LIST_Y - 38 - 1).max(0) / PL_ROW_H) as usize
}

impl ClassicSkin {
    pub fn compose_eq(&self, st: &EqState) -> RgbaImage {
        let mut c = RgbaImage::new(275, EQ_H);
        if let Some(e) = self.bmp("eqmain.bmp") {
            blit(&mut c, e, 0, 0, 0, 0, 275, 116); // background
            let ty = if st.focused { 134 } else { 149 };
            blit(&mut c, e, 0, 0, 0, ty, 275, 14); // title bar
            blit(&mut c, e, 264, 3, 0, 116, 9, 9); // close

            // On button.
            let (ox, oy) = match (st.on, st.pressed == Some(EqRegion::On)) {
                (true, true) => (187, 119),
                (true, false) => (69, 119),
                (false, true) => (128, 119),
                (false, false) => (10, 119),
            };
            blit(&mut c, e, 14, 18, ox, oy, 26, 12);
            // Auto button.
            let (ax, ay) = match (st.auto, st.pressed == Some(EqRegion::Auto)) {
                (true, true) => (213, 119),
                (true, false) => (95, 119),
                (false, true) => (154, 119),
                (false, false) => (36, 119),
            };
            blit(&mut c, e, 40, 18, ax, ay, 32, 12);
            // Presets button.
            let py = if st.pressed == Some(EqRegion::Presets) { 176 } else { 164 };
            blit(&mut c, e, 217, 18, 224, py, 44, 12);
            // Graph background.
            blit(&mut c, e, 86, 17, 0, 294, 113, 19);

            // EQ response curve through the 10 bands in the graph box (86,17 /
            // 113×19), coloured by EQMAIN's 19-row gradient at x=115,y=294 (red
            // boost top, green cut bottom). Skipped when EQMAIN is too short (Alita).
            if e.width() > 115 && e.height() >= 294 + 19 {
                const GX: i32 = 86;
                const GY: i32 = 17;
                const GW: i32 = 113;
                const GH: i32 = 19;
                let bands = st.bands;
                // Catmull-Rom sample of the band values at fx in 0.0..=9.0, so
                // the curve passes smoothly through each slider's value.
                let sample = |fx: f32| -> f32 {
                    let i = fx.floor() as i32;
                    let t = fx - i as f32;
                    let g = |k: i32| bands[k.clamp(0, 9) as usize];
                    let (p0, p1, p2, p3) = (g(i - 1), g(i), g(i + 1), g(i + 2));
                    0.5 * (2.0 * p1
                        + (-p0 + p2) * t
                        + (2.0 * p0 - 5.0 * p1 + 4.0 * p2 - p3) * t * t
                        + (-p0 + 3.0 * p1 - 3.0 * p2 + p3) * t * t * t)
                };
                // value (−1..1, bottom..top) -> graph row 0..=18 (top..bottom).
                let row_of = |v: f32| -> i32 {
                    (((1.0 - v) * 0.5) * (GH - 1) as f32)
                        .round()
                        .clamp(0.0, (GH - 1) as f32) as i32
                };
                let mut prev: Option<i32> = None;
                for gx in 0..GW {
                    let fx = gx as f32 / (GW - 1) as f32 * 9.0;
                    let row = row_of(sample(fx).clamp(-1.0, 1.0));
                    // Fill the span to the previous column so steep sections of
                    // the curve stay a continuous line rather than dotted.
                    let (lo, hi) = prev.map_or((row, row), |p| (row.min(p), row.max(p)));
                    for r in lo..=hi {
                        let px = *e.get_pixel(115, (294 + r) as u32);
                        c.put_pixel((GX + gx) as u32, (GY + r) as u32, px);
                    }
                    prev = Some(row);
                }
            }

            // Which of the 28 slider sprites the skin actually provides. A
            // requested position with no sprite snaps to the nearest one the
            // skin does have, so a partially-skinned EQ keeps the skin's look.
            // When the skin ships no slider bank at all we draw only the thumb
            // over the background rather than synthesize garish bars.
            let valid = &self.eq_sprites;
            let draw_thumb = |c: &mut RgbaImage, e: &RgbaImage, colx: i32, v: f32, sel: bool| {
                let frac = ((1.0 - v) / 2.0).clamp(0.0, 1.0);
                if let Some(ui) = eq_sprite_index(v, valid) {
                    let (row, col) = (ui / 14, ui % 14);
                    blit(
                        c,
                        e,
                        colx,
                        EQ_SLIDER_TOP as i32,
                        (13 + col * 15) as u32,
                        (164 + row * 65) as u32,
                        14,
                        63,
                    );
                }
                let ty = EQ_SLIDER_TOP as i32 + (frac * EQ_SLIDER_TRAVEL) as i32;
                let sy = if sel { 176 } else { 164 };
                blit(c, e, colx + 2, ty, 0, sy, 11, 11);
            };
            draw_thumb(&mut c, e, EQ_PREAMP_X, st.preamp, st.pressed == Some(EqRegion::Preamp));
            for (i, &bx) in EQ_BAND_X.iter().enumerate() {
                draw_thumb(&mut c, e, bx, st.bands[i], st.pressed == Some(EqRegion::Band(i)));
            }
        }
        c
    }

    pub fn compose_playlist(&self, st: &PlState, width: u32, height: u32) -> RgbaImage {
        let w = width.max(MAIN_W) as i32;
        let mut c = RgbaImage::new(w as u32, height);
        // Inner background fill.
        fill_rect(&mut c, 12, 20, w - 12 - 20, height as i32 - 20 - 38, st.colors.bg);

        if let Some(p) = self.bmp("pledit.bmp") {
            let sy = if st.focused { 0 } else { 21 };
            blit(&mut c, p, 0, 0, 0, sy, 25, 20); // top-left corner
            blit(&mut c, p, w - 25, 0, 153, sy, 25, 20); // top-right corner
            let mut x = 25;
            while x < w - 25 {
                blit(&mut c, p, x, 0, 127, sy, 25, 20);
                x += 25;
            }
            blit(&mut c, p, (w - 100) / 2, 0, 26, sy, 100, 20); // title bar center

            // Left/right window edges, tiled vertically.
            let mut y = 20;
            let mid_end = height as i32 - 38;
            while y < mid_end {
                blit(&mut c, p, 0, y, 0, 42, 12, 29);
                blit(&mut c, p, w - 20, y, 31, 42, 20, 29);
                y += 29;
            }

            // Bottom bar: left corner (Add/Rem/Sel/Misc), the tiled stretch, then
            // the right corner (mini-player / List / time). Stretch tiles the 4px
            // plain column at pledit x126 (a wider slice duplicates the buttons).
            let by = height as i32 - 38;
            let mut bx = 125;
            while bx < w - 150 {
                let seg = (w - 150 - bx).min(4) as u32;
                blit(&mut c, p, bx, by, 126, 72, seg, 38);
                bx += 4;
            }
            blit(&mut c, p, 0, by, 0, 72, 125, 38); // bottom-left
            blit(&mut c, p, w - 150, by, 126, 72, 150, 38); // bottom-right
            blit(&mut c, p, w - 11, 3, 52, 42, 9, 9); // close

            // Depress the held bottom-bar menu button. pledit has no pressed
            // sprite for these, so darken the button region to show the press.
            if let Some(bx) = st.pressed.and_then(|pr| match pr {
                PlRegion::AddMenu => Some(14),
                PlRegion::RemMenu => Some(43),
                PlRegion::SelMenu => Some(72),
                PlRegion::MiscMenu => Some(101),
                PlRegion::ListMenu => Some(w - 44),
                _ => None,
            }) {
                darken_rect(&mut c, bx, height as i32 - 30, 22, 18, 0.55);
            }

            // Time readouts in the bottom-right sprite (at w-150): the
            // "<cur>/<total>" summary in the wide top bar and the current time in
            // the small box, like Winamp's "6:46/52:48:36+" over "02:29". A
            // trailing + means the total omits tracks of unknown length.
            if st.total_secs > 0.0 || st.elapsed_secs > 0.0 {
                // Sprite origin; its two black boxes sit at these local offsets:
                // the wide summary bar at x5..96 / y8..17 and the small elapsed
                // box at x63..96 / y22..29 (colon baked at x83).
                let sx0 = w - 150;
                let cur = fmt_pl_time_signed(st.elapsed_secs, st.cur_duration, st.time_remaining);
                let summary = format!(
                    "{}/{}{}",
                    cur,
                    fmt_pl_time(st.total_secs),
                    if st.total_partial { "+" } else { "" },
                );
                // Left-aligned and vertically centred in the top bar. The summary
                // line does NOT blink — only the standalone current-time box below.
                self.draw_text_tinted(&mut c, &summary, sx0 + 7, by + 10, 89, st.colors.normal);
                // Current time in the small box, its colon aligned onto the one
                // baked into the sprite. Blanked on the paused-blink "off" half.
                if !st.time_hidden {
                    let el = cur.clone();
                    if let Some(ci) = el.rfind(':') {
                        let tx = sx0 + 83 - el[..ci].chars().count() as i32 * 5;
                        self.draw_text_tinted(&mut c, &el, tx, by + 22, sx0 + 98 - tx, st.colors.normal);
                    }
                }
            }
        }

        // Track list. Each row shows the title on the left and, when known, the
        // track length right-aligned (as in Winamp), with the title clipped so
        // the two never overlap.
        let visible = pl_visible_rows(height);
        for (i, (text, is_cur, is_sel, dur)) in st.rows.iter().skip(st.first).take(visible).enumerate() {
            let ry = PL_LIST_Y + i as i32 * PL_ROW_H;
            if *is_sel {
                fill_rect(&mut c, PL_LIST_X - 2, ry - 1, w - PL_LIST_X - 22, PL_ROW_H, st.colors.selbg);
            }
            let col = if *is_cur { st.colors.current } else { st.colors.normal };
            let time = dur.map(|d| fmt_pl_time(d));
            // Default title width (no time): up to the right frame.
            let mut title_w = w - PL_LIST_X - 24;
            if let Some(t) = &time {
                let tw = t.chars().count() as i32 * 5;
                let tx = w - 24 - tw; // right-aligned, just left of the frame
                self.draw_text_tinted(&mut c, t, tx, ry, tw, col);
                title_w = tx - PL_LIST_X - 6; // leave a gap before the time
            }
            self.draw_text_tinted(&mut c, text, PL_LIST_X, ry, title_w.max(0), col);
        }
        c
    }

    pub fn compose_eq_shade(&self, focused: bool) -> RgbaImage {
        let mut c = RgbaImage::new(275, 14);
        if let Some(e) = self.bmp("eqmain.bmp") {
            let sy = if focused { 134 } else { 149 };
            blit(&mut c, e, 0, 0, 0, sy, 275, 14);
            blit(&mut c, e, 264, 3, 0, 116, 9, 9); // close
        }
        c
    }

    /// Collapsed (window-shade) playlist bar. Shows the current track and its
    /// length on the single line, Winamp-style; `text` empty means nothing is
    /// playing, so only the bar is drawn.
    pub fn compose_pl_shade(
        &self,
        focused: bool,
        text: &str,
        time: &str,
        color: [u8; 3],
    ) -> RgbaImage {
        let mut c = RgbaImage::new(275, 14);
        if let Some(p) = self.bmp("pledit.bmp") {
            blit(&mut c, p, 0, 0, 72, 42, 25, 14); // left end
            let mut x = 25;
            while x < 225 {
                blit(&mut c, p, x, 0, 72, 57, 25, 14); // centre tile
                x += 25;
            }
            let ry = if focused { 42 } else { 57 };
            blit(&mut c, p, 225, 0, 99, ry, 50, 14); // right end (with buttons)
            blit(&mut c, p, 264, 3, 52, 42, 9, 9); // close

            // Shade-bar track + time: title at `PL_LIST_X` (aligned with the
            // expanded rows), time right-aligned before the button cluster, title
            // clipped so they never overlap.
            let mut title_w = 225 - PL_LIST_X - 4;
            if !time.is_empty() {
                let tw = time.chars().count() as i32 * 5;
                let tx = 214 - tw;
                self.draw_text_tinted(&mut c, time, tx, 4, tw, color);
                title_w = tx - PL_LIST_X - 4;
            }
            if !text.is_empty() {
                self.draw_text_tinted(&mut c, text, PL_LIST_X, 4, title_w.max(0), color);
            }
        }
        c
    }

    fn draw_text_tinted(&self, c: &mut RgbaImage, text: &str, x0: i32, y0: i32, max_w: i32, color: [u8; 3]) {
        let Some(tf) = self.bmp("text.bmp") else {
            return;
        };
        let mut x = x0;
        let limit = x0 + max_w;
        for ch in text.chars() {
            if x + 5 > limit {
                break;
            }
            let (col, row) = char_cell(ch);
            blit_tinted(c, tf, x, y0, col * 5, row * 6, 5, 6, color, self.text_bg);
            x += 5;
        }
    }
}

/// Which of the 28 EQ slider sprites (two rows of 14 in EQMAIN.BMP, 14×63 at
/// x=13+col*15, y=164+row*65) a skin ships, index 0..28 (0=boost, 27=cut). Skins
/// differ wildly, so a sprite counts as present on colour OR light/dark variation.
/// Bounds-checked: a short EQMAIN (e.g. Alita, no slider bank) yields an empty list.
fn eq_valid_sprites(e: &RgbaImage) -> Vec<i32> {
    let (ew, eh) = e.dimensions();
    let present = |col: i32, row: i32| -> bool {
        let (x0, y0) = (13 + col * 15, 164 + row * 65);
        if x0 < 0 || y0 < 0 || x0 as u32 + 14 > ew || y0 as u32 + 63 > eh {
            return false;
        }
        let (mut max_sat, mut min_b, mut max_b) = (0i32, 255i32, 0i32);
        let mut y = y0;
        while y < y0 + 63 {
            for dx in [4, 7, 10] {
                let p = e.get_pixel((x0 + dx) as u32, y as u32);
                let (mx, mn) = (
                    p[0].max(p[1]).max(p[2]) as i32,
                    p[0].min(p[1]).min(p[2]) as i32,
                );
                max_sat = max_sat.max(mx - mn);
                let b = (p[0] as i32 + p[1] as i32 + p[2] as i32) / 3;
                min_b = min_b.min(b);
                max_b = max_b.max(b);
            }
            y += 2;
        }
        max_sat > 40 || (max_b - min_b) > 60
    };
    (0..28).filter(|&i| present(i % 14, i / 14)).collect()
}

/// Format a playlist time like Winamp's bottom bar: `M:SS`, or `H:MM:SS` once it
/// reaches an hour. Minutes/hours have no leading zero; seconds (and minutes
/// inside an H:MM:SS) are zero-padded. Negative input clamps to zero.
/// EQ slider sprite index (0 = top/boost .. 27 = bottom/cut) for value `v`
/// (-1..=1), snapped to the nearest sprite the skin ships. None when the skin
/// has no slider bank (only the thumb is drawn).
fn eq_sprite_index(v: f32, valid: &[i32]) -> Option<i32> {
    let frac = ((1.0 - v) / 2.0).clamp(0.0, 1.0);
    let idx = (frac * 27.0).round() as i32;
    valid.iter().min_by_key(|&&vi| (vi - idx).abs()).copied()
}

pub fn fmt_pl_time(secs: f32) -> String {
    let total = secs.max(0.0) as u64;
    let (h, m, s) = (total / 3600, (total % 3600) / 60, total % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

/// Current-track time as elapsed, or `-remaining` when the remaining-time toggle
/// is on and the duration is known.
pub fn fmt_pl_time_signed(pos: f32, duration: f32, remaining: bool) -> String {
    if remaining && duration > 0.0 {
        format!("-{}", fmt_pl_time((duration - pos).max(0.0)))
    } else {
        fmt_pl_time(pos)
    }
}

/// Classic Winamp-style analyzer gradient by vertical fraction (0=bottom green,
/// 0.5=yellow, 1=top red).
fn viz_gradient(f: f32) -> [u8; 3] {
    let lerp = |a: [u8; 3], b: [u8; 3], t: f32| {
        [
            (a[0] as f32 + (b[0] as f32 - a[0] as f32) * t) as u8,
            (a[1] as f32 + (b[1] as f32 - a[1] as f32) * t) as u8,
            (a[2] as f32 + (b[2] as f32 - a[2] as f32) * t) as u8,
        ]
    };
    let f = f.clamp(0.0, 1.0);
    let green = [40, 220, 60];
    let yellow = [220, 210, 40];
    let red = [220, 70, 40];
    if f < 0.5 {
        lerp(green, yellow, f / 0.5)
    } else {
        lerp(yellow, red, (f - 0.5) / 0.5)
    }
}

/// Draw the main-window visualizer (spectrum bars or oscilloscope) into the
/// rect (x,y,w,h). Bars use a green→yellow→red gradient by height; the
/// oscilloscope uses the LCD colour.
fn draw_viz(c: &mut RgbaImage, st: &MainState, x: i32, y: i32, w: i32, h: i32) {
    let peak_col = st.viz_peak;
    let hf = (h as f32 - 1.0).max(1.0);
    // Colour at vertical fraction f (0=bottom, 1=top): the skin's spectrum
    // gradient if present, else a synthesized green→yellow→red gradient.
    let grad = |f: f32| -> [u8; 3] {
        if st.viz_spectrum.is_empty() {
            viz_gradient(f)
        } else {
            let n = st.viz_spectrum.len();
            let idx = (((1.0 - f.clamp(0.0, 1.0)) * (n - 1) as f32).round() as usize).min(n - 1);
            st.viz_spectrum[idx]
        }
    };
    match &st.viz {
        Some(VizFrame::Spectrum { bars, peaks }) => {
            let n = bars.len().max(1) as i32;
            let bw = (w as f32 / n as f32).max(2.0);
            for (i, &b) in bars.iter().enumerate() {
                let b = b.clamp(0.0, 1.0);
                let x0 = x + (i as f32 * bw) as i32;
                let bar_w = (bw as i32 - 1).max(1); // 1px gap between bars
                let bh = (b * h as f32).round() as i32;
                // Draw the bar row by row so it fades up its height.
                for k in 0..bh {
                    let col = grad(k as f32 / hf);
                    fill_rect(c, x0, y + h - 1 - k, bar_w, 1, col);
                }
                // Slowly-falling peak dash, drawn only above the bar.
                let pk = peaks.get(i).copied().unwrap_or(0.0).clamp(0.0, 1.0);
                let ph = (pk * h as f32).round() as i32;
                if ph > bh {
                    fill_rect(c, x0, y + h - ph, bar_w, 1, peak_col);
                }
            }
        }
        Some(VizFrame::Scope(samples)) => {
            let n = samples.len().max(2);
            let mid = y + h / 2;
            let amp = (h as f32 / 2.0 - 1.0).max(1.0);
            let yat = |s: f32| mid - (s.clamp(-1.0, 1.0) * amp) as i32;
            let mut prev = yat(samples[0]);
            for i in 1..n {
                let px = x + (i as i32 * w / n as i32);
                let cur = yat(samples[i]);
                let (lo, hi) = (prev.min(cur), prev.max(cur));
                fill_rect(c, px, lo, 1, (hi - lo + 1).max(1), st.fg);
                prev = cur;
            }
        }
        None => {}
    }
}

/// Multiply the RGB of every pixel in a rect by `factor` (0..1) for a quick
/// "pressed"/shaded look where no dedicated sprite exists.
fn darken_rect(dst: &mut RgbaImage, x: i32, y: i32, w: i32, h: i32, factor: f32) {
    let (dw, dh) = dst.dimensions();
    for j in 0..h {
        let yy = y + j;
        if yy < 0 || yy >= dh as i32 {
            continue;
        }
        for i in 0..w {
            let xx = x + i;
            if xx < 0 || xx >= dw as i32 {
                continue;
            }
            let p = dst.get_pixel_mut(xx as u32, yy as u32);
            p[0] = (p[0] as f32 * factor) as u8;
            p[1] = (p[1] as f32 * factor) as u8;
            p[2] = (p[2] as f32 * factor) as u8;
        }
    }
}

fn fill_rect(dst: &mut RgbaImage, x: i32, y: i32, w: i32, h: i32, col: [u8; 3]) {
    let (dw, dh) = dst.dimensions();
    for j in 0..h {
        let yy = y + j;
        if yy < 0 || yy >= dh as i32 {
            continue;
        }
        for i in 0..w {
            let xx = x + i;
            if xx < 0 || xx >= dw as i32 {
                continue;
            }
            dst.put_pixel(xx as u32, yy as u32, Rgba([col[0], col[1], col[2], 255]));
        }
    }
}

/// Blit a glyph, recolouring its lit pixels to `color` (transparent elsewhere).
/// Most common colour in an image — used as the bitmap font's background.
fn dominant_color(img: &RgbaImage) -> [u8; 3] {
    let mut counts: HashMap<[u8; 3], u32> = HashMap::new();
    for p in img.pixels() {
        *counts.entry([p[0], p[1], p[2]]).or_insert(0) += 1;
    }
    counts
        .into_iter()
        .max_by_key(|&(_, n)| n)
        .map(|(c, _)| c)
        .unwrap_or([0, 0, 0])
}

/// Blit the lit pixels of a bitmap-font glyph in `color`. A pixel counts as part
/// of the glyph when it differs enough from the font background `bg`; for the
/// usual light-on-black font this is the same as "bright", but it also handles
/// dark-on-light fonts (which otherwise paint as solid blocks).
fn blit_tinted(
    dst: &mut RgbaImage,
    src: &RgbaImage,
    dx: i32,
    dy: i32,
    sx: u32,
    sy: u32,
    w: u32,
    h: u32,
    color: [u8; 3],
    bg: [u8; 3],
) {
    let (sw, sh) = src.dimensions();
    let (dw, dh) = dst.dimensions();
    for j in 0..h {
        let syy = sy + j;
        if syy >= sh {
            break;
        }
        let dyy = dy + j as i32;
        if dyy < 0 || dyy >= dh as i32 {
            continue;
        }
        for i in 0..w {
            let sxx = sx + i;
            if sxx >= sw {
                break;
            }
            let dxx = dx + i as i32;
            if dxx < 0 || dxx >= dw as i32 {
                continue;
            }
            let p = src.get_pixel(sxx, syy);
            let diff = (p[0] as i32 - bg[0] as i32).abs()
                + (p[1] as i32 - bg[1] as i32).abs()
                + (p[2] as i32 - bg[2] as i32).abs();
            if diff > 150 {
                dst.put_pixel(dxx as u32, dyy as u32, Rgba([color[0], color[1], color[2], 255]));
            }
        }
    }
}

/// Base-skin-like spectrum gradient (top red .. bottom green) for previews.
fn sample_palette() -> Vec<[u8; 3]> {
    vec![
        [239, 49, 16], [214, 90, 0], [214, 115, 0], [222, 165, 24],
        [214, 181, 33], [189, 222, 41], [148, 222, 33], [41, 206, 16],
        [57, 181, 16], [24, 132, 8],
    ]
}

/// Sample spectrum bars + peaks for the headless previews.
fn sample_viz() -> (Vec<f32>, Vec<f32>) {
    let bars: Vec<f32> = (0..19).map(|i| 0.3 + 0.6 * (i as f32 * 0.7).sin().abs()).collect();
    let peaks: Vec<f32> = bars.iter().map(|b| (b + 0.18).min(1.0)).collect();
    (bars, peaks)
}

/// Headless: render a skin's main window to a PNG (used for `--render-skin`).
pub fn render_main_png(skin_path: &Path, out: &Path) -> Result<()> {
    let skin = ClassicSkin::load(skin_path)?;
    let st = MainState {
        focused: true,
        play: PlayUi::Playing,
        pressed: None,
        shuffle: false,
        repeat: true,
        eq_on: false,
        pl_on: true,
        always_on_top: false,
        double_size: false,
        pos: 125.0,
        duration: 254.0,
        time_remaining: false,
        time_hidden: false,
        volume: 0.78,
        balance: 0.0,
        stereo: true,
        title: "linamp - smells like winamp".to_string(),
        kbps: "128".to_string(),
        khz: "44".to_string(),
        fg: [0, 255, 120],
        viz: Some({ let (bars, peaks) = sample_viz(); VizFrame::Spectrum { bars, peaks } }),
        viz_spectrum: sample_palette(),
        viz_peak: [150, 150, 150],
    };
    skin.compose_main(&st).save(out)?;
    Ok(())
}

/// Headless: render main + EQ + playlist stacked, to inspect all three windows.
pub fn render_all_png(skin_path: &Path, out: &Path) -> Result<()> {
    let skin = ClassicSkin::load(skin_path)?;
    let main = skin.compose_main(&MainState {
        focused: true,
        play: PlayUi::Playing,
        pressed: None,
        shuffle: true,
        repeat: true,
        eq_on: true,
        pl_on: true,
        always_on_top: false,
        double_size: false,
        pos: 73.0, // 1:13
        duration: 325.0,
        time_remaining: false,
        time_hidden: false,
        volume: 0.8,
        balance: 0.0,
        stereo: true,
        title: "aria nocturne - crystal harbor".to_string(),
        kbps: "192".to_string(),
        khz: "44".to_string(),
        fg: [0, 255, 120],
        viz: Some({ let (bars, peaks) = sample_viz(); VizFrame::Spectrum { bars, peaks } }),
        viz_spectrum: sample_palette(),
        viz_peak: [150, 150, 150],
    });
    // Rock preset (band gains 8,5,-4,-7,-3,2,6,9,10,10 dB over ±12) so the
    // colored sliders show their green-boost / red-cut range.
    let eq = skin.compose_eq(&EqState {
        focused: true,
        on: true,
        auto: false,
        pressed: None,
        preamp: 0.0,
        bands: [0.667, 0.417, -0.333, -0.583, -0.25, 0.167, 0.5, 0.75, 0.833, 0.833],
    });
    let rows: Vec<PlRow> = vec![
        ("1. aria nocturne - crystal harbor".to_string(), true, true, Some(325.0)),
        ("2. midnight ramen - neon skyline".to_string(), false, false, Some(242.0)),
        ("3. lo-fi samurai - paper lanterns".to_string(), false, false, Some(198.0)),
        ("4. nova drift - blue hour".to_string(), false, false, Some(276.0)),
        ("5. the wandering synths - midnight train".to_string(), false, false, Some(263.0)),
    ];
    let pl = skin.compose_playlist(
        &PlState {
            focused: true,
            colors: PlColors {
                bg: [0, 0, 0],
                normal: [0, 255, 0],
                current: [255, 255, 255],
                selbg: [0, 0, 180],
            },
            rows: &rows,
            first: 0,
            pressed: None,
            elapsed_secs: 73.0, // 1:13, matching the main display
            cur_duration: 325.0,
            time_remaining: false,
            total_secs: 1304.0, // 21:44 across the five tracks
            total_partial: false,
            time_hidden: false,
        },
        MAIN_W,
        PL_H,
    );

    let total_h = MAIN_H + EQ_H + PL_H;
    let mut canvas = RgbaImage::new(275, total_h);
    blit(&mut canvas, &main, 0, 0, 0, 0, 275, MAIN_H);
    blit(&mut canvas, &eq, 0, MAIN_H as i32, 0, 0, 275, EQ_H);
    blit(&mut canvas, &pl, 0, (MAIN_H + EQ_H) as i32, 0, 0, 275, PL_H);
    canvas.save(out)?;
    Ok(())
}

/// Headless preview of the general-purpose (Media Library) window frame at a
/// sample size, for eyeballing the gen.bmp chrome without the GUI.
pub fn render_gen_png(skin_path: &Path, out: &Path) -> Result<()> {
    let skin = ClassicSkin::load(skin_path)?;
    if !skin.has_gen() {
        anyhow::bail!("skin has no GEN.BMP/GENEX.BMP");
    }
    let frame = skin
        .compose_gen_window(550, 300, true, "LinAmp Library")
        .ok_or_else(|| anyhow::anyhow!("failed to compose gen window"))?;
    frame.save(out)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::char_cell;
    use super::*;

    #[test]
    fn ascii_folds_accented_text_to_base_letters() {
        // Accented and Vietnamese characters render via their base ASCII cell,
        // not as blanks (#97).
        for (accented, base) in [
            ('é', 'e'), ('ñ', 'n'), ('ü', 'u'), ('ç', 'c'), ('ø', 'o'),
            ('Đ', 'd'), ('ư', 'u'), ('ờ', 'o'), ('ế', 'e'), ('ạ', 'a'), ('ý', 'y'),
        ] {
            assert_eq!(char_cell(accented), char_cell(base), "{accented} -> {base}");
            assert_ne!(char_cell(accented), char_cell(' '), "{accented} folded to blank");
        }
        // ASCII is unchanged.
        assert_eq!(char_cell('A'), char_cell('a'));
    }

    #[test]
    fn clutterbar_regions_map_to_buttons() {
        // A point inside each 8px-tall button (rows 24/32/40/48/56) maps to it.
        assert_eq!(region_at(13.0, 27.0), Some(Btn::ClutterO));
        assert_eq!(region_at(13.0, 35.0), Some(Btn::ClutterA));
        assert_eq!(region_at(13.0, 43.0), Some(Btn::ClutterI));
        assert_eq!(region_at(13.0, 51.0), Some(Btn::ClutterD));
        assert_eq!(region_at(13.0, 59.0), Some(Btn::ClutterV));
    }

    #[test]
    fn eq_regions_cover_all_controls() {
        assert_eq!(eq_region_at(268.0, 7.0), Some(EqRegion::Close));
        assert_eq!(eq_region_at(20.0, 24.0), Some(EqRegion::On));
        assert_eq!(eq_region_at(50.0, 24.0), Some(EqRegion::Auto));
        assert_eq!(eq_region_at(230.0, 24.0), Some(EqRegion::Presets));
        assert_eq!(eq_region_at(25.0, 50.0), Some(EqRegion::Preamp));
        assert_eq!(eq_region_at(80.0, 50.0), Some(EqRegion::Band(0)));
        assert_eq!(eq_region_at(245.0, 50.0), Some(EqRegion::Band(9)));
        assert_eq!(eq_region_at(5.0, 5.0), Some(EqRegion::TitleBar));
        assert_eq!(eq_region_at(150.0, 35.0), None); // between buttons and sliders
    }

    #[test]
    fn main_slider_regions() {
        assert_eq!(region_at(140.0, 77.0), Some(Btn::Seek));
        assert_eq!(region_at(141.0, 63.0), Some(Btn::Volume));
        assert_eq!(region_at(196.0, 63.0), Some(Btn::Balance));
    }

    #[test]
    fn playlist_visible_rows_and_hit() {
        assert_eq!(pl_visible_rows(0), 0);
        assert_eq!(pl_visible_rows(60), 0);
        assert_eq!(pl_visible_rows(67), 1);
        assert_eq!(pl_visible_rows(PL_H), 17);
        assert_eq!(pl_row_at(PL_LIST_Y as f32, 0), 0);
        assert_eq!(pl_row_at(PL_LIST_Y as f32 + PL_ROW_H as f32, 0), 1);
        assert_eq!(pl_row_at(0.0, 0), 0); // above the list clamps
        assert_eq!(pl_row_at(PL_LIST_Y as f32, 5), 5); // offset by `first`
        assert_eq!(pl_row_at(PL_LIST_Y as f32 + 2.0 * PL_ROW_H as f32, 2), 4);
    }

    #[test]
    fn eq_sprite_index_maps_and_snaps() {
        let full: Vec<i32> = (0..28).collect();
        assert_eq!(eq_sprite_index(1.0, &full), Some(0)); // boost -> top
        assert_eq!(eq_sprite_index(-1.0, &full), Some(27)); // cut -> bottom
        assert_eq!(eq_sprite_index(0.0, &full), Some(14)); // centre
        assert_eq!(eq_sprite_index(2.0, &full), Some(0)); // clamps high
        assert_eq!(eq_sprite_index(-2.0, &full), Some(27)); // clamps low
        assert_eq!(eq_sprite_index(0.5, &[0, 27]), Some(0)); // snap to nearer
        assert_eq!(eq_sprite_index(-0.5, &[0, 27]), Some(27));
        assert_eq!(eq_sprite_index(0.0, &[]), None); // no bank -> thumb only
    }

    #[test]
    fn signed_time_elapsed_and_remaining() {
        assert_eq!(fmt_pl_time_signed(71.0, 200.0, false), "1:11");
        assert_eq!(fmt_pl_time_signed(71.0, 200.0, true), "-2:09");
        assert_eq!(fmt_pl_time_signed(71.0, 0.0, true), "1:11"); // no duration -> elapsed
        assert_eq!(fmt_pl_time_signed(250.0, 200.0, true), "-0:00"); // past end clamps
    }

    #[test]
    fn viz_gradient_anchors() {
        assert_eq!(viz_gradient(0.0), [40, 220, 60]); // green
        assert_eq!(viz_gradient(0.5), [220, 210, 40]); // yellow
        assert_eq!(viz_gradient(1.0), [220, 70, 40]); // red
        assert_eq!(viz_gradient(-1.0), [40, 220, 60]); // clamps low
        assert_eq!(viz_gradient(2.0), [220, 70, 40]); // clamps high
    }

    #[test]
    fn char_cell_punctuation_and_case() {
        assert_eq!(char_cell('.'), (11, 1));
        assert_eq!(char_cell(':'), (12, 1));
        assert_eq!(char_cell('/'), (21, 1));
        assert_eq!(char_cell('+'), (19, 1));
        assert_eq!(char_cell('?'), (3, 2));
        assert_eq!(char_cell('*'), (4, 2));
        assert_eq!(char_cell('<'), (22, 1));
        assert_eq!(char_cell('9'), (9, 1));
        assert_eq!(char_cell('Z'), (25, 0)); // case-folded to 'z'
    }

    #[test]
    fn main_regions() {
        assert_eq!(region_at(50.0, 95.0), Some(Btn::Play));
        assert_eq!(region_at(7.0, 6.0), Some(Btn::Options));
        assert_eq!(region_at(5.0, 5.0), Some(Btn::TitleBar));
        assert_eq!(region_at(60.0, 31.0), Some(Btn::Time)); // time display
        assert_eq!(region_at(60.0, 48.0), Some(Btn::Viz)); // visualizer
        assert_eq!(region_at(1000.0, 1000.0), None);
    }

    #[test]
    fn eq_regions() {
        assert_eq!(eq_region_at(20.0, 20.0), Some(EqRegion::On));
        assert!(matches!(eq_region_at(80.0, 60.0), Some(EqRegion::Band(0))));
    }

    #[test]
    fn pl_regions() {
        assert_eq!(pl_region_at(5.0, 5.0, 275.0, 180.0), Some(PlRegion::TitleBar));
        assert_eq!(pl_region_at(100.0, 100.0, 275.0, 180.0), Some(PlRegion::List));
        assert_eq!(pl_region_at(260.0, 165.0, 275.0, 180.0), Some(PlRegion::Resize));
        // Resize grip and List menu track the right edge when widened.
        assert_eq!(pl_region_at(360.0, 165.0, 375.0, 180.0), Some(PlRegion::Resize));
        assert_eq!(pl_region_at(375.0 - 30.0, 180.0 - 20.0, 375.0, 180.0), Some(PlRegion::ListMenu));
    }

    #[test]
    fn pl_mini_transport_regions() {
        // The mini-player buttons sit in the bottom-right sprite's lower strip.
        let (w, h) = (275.0, 180.0);
        assert_eq!(pl_region_at(142.0, 169.0, w, h), Some(PlRegion::MiniPlay));
        assert_eq!(pl_region_at(162.0, 169.0, w, h), Some(PlRegion::MiniStop));
        // They track the right edge when the window is widened.
        assert_eq!(pl_region_at(142.0 + 100.0, 169.0, w + 100.0, h), Some(PlRegion::MiniPlay));
        // Just above the strip is the menu band, not a transport button.
        assert_ne!(pl_region_at(142.0, 155.0, w, h), Some(PlRegion::MiniPlay));
    }

    #[test]
    fn every_mini_transport_button_is_hittable() {
        // Each PL_MINI_BTNS entry's centre must resolve back to that region, and
        // none may collide with the resize grip at the right edge. Guards the
        // bottom-bar geometry on both platforms.
        for &(region, bx, bw) in PL_MINI_BTNS {
            for &(w, h) in &[(275.0f32, 180.0f32), (420.0, 240.0)] {
                let cx = (w - 150.0) + bx as f32 + bw as f32 / 2.0;
                let cy = h - 11.0; // middle of the height-15..height-6 strip
                assert_eq!(
                    pl_region_at(cx, cy, w, h),
                    Some(region),
                    "mini button {region:?} not hittable at {w}x{h}"
                );
            }
        }
    }

    #[test]
    fn tooltips_present_and_in_bounds() {
        let main = main_tooltips();
        let labels: Vec<&str> = main.iter().map(|t| t.0).collect();
        for l in [
            "Previous", "Play", "Pause", "Stop", "Next", "Open file(s)", "Shuffle", "Repeat",
            "Equalizer", "Playlist", "Close", "Minimize", "Options",
        ] {
            assert!(labels.contains(&l), "main tooltip missing {l}");
        }
        for (_, x, y, w, h) in &main {
            assert!(
                *x >= 0 && *y >= 0 && (x + w) as u32 <= MAIN_W && (y + h) as u32 <= MAIN_H,
                "main tooltip rect out of bounds"
            );
        }
        let eq: Vec<&str> = eq_tooltips().iter().map(|t| t.0).collect();
        for l in ["On", "Auto", "Presets", "Close"] {
            assert!(eq.contains(&l), "eq tooltip missing {l}");
        }
        assert_eq!(pl_tooltips(275, 180).len(), 6);
    }

    #[test]
    fn main_tooltips_sit_on_their_buttons() {
        // Every tooltip rect must cover the button it names, so hover labels and
        // click targets can't drift apart.
        for (label, x, y, w, h) in main_tooltips() {
            let (cx, cy) = ((x + w / 2) as f32, (y + h / 2) as f32);
            assert_eq!(
                region_at(cx, cy).and_then(btn_label),
                Some(label),
                "tooltip {label:?} centre doesn't hit its button"
            );
        }
    }

    #[test]
    fn pl_tooltips_track_the_right_edge() {
        let narrow = pl_tooltips(275, 180);
        let wide = pl_tooltips(375, 180);
        assert_eq!(narrow.len(), wide.len());
        for (n, w) in narrow.iter().zip(&wide) {
            assert_eq!(n.0, w.0, "label changed");
            assert_eq!(w.1 - n.1, 100, "{} did not follow the right edge", n.0);
        }
    }

    #[test]
    fn main_shade_ignores_title() {
        // Regression: the collapsed main bar used to draw the song title over the
        // baked branding, producing garbled text. It must not draw the title now,
        // so the output is identical regardless of what the title is.
        let skin = load_skin("linamp_base_skin.wsz");
        let mk = |title: &str| MainState {
            focused: true,
            play: PlayUi::Playing,
            pressed: None,
            shuffle: false,
            repeat: false,
            eq_on: false,
            pl_on: false,
            always_on_top: false,
            double_size: false,
            pos: 71.0,
            duration: 200.0,
            time_remaining: false,
            time_hidden: false,
            volume: 0.5,
            balance: 0.0,
            stereo: true,
            title: title.to_string(),
            kbps: "128".to_string(),
            khz: "44".to_string(),
            fg: [0, 255, 0],
            viz: None,
            viz_spectrum: Vec::new(),
            viz_peak: [0, 0, 0],
        };
        let a = skin.compose_main_shade(&mk("")).into_raw();
        let b = skin.compose_main_shade(&mk("A VERY LONG SONG TITLE 1234567890")).into_raw();
        assert_eq!(a, b, "the collapsed main bar must not draw the song title");
    }

    #[test]
    fn lcd_mmss_honors_elapsed_remaining_and_caps_minutes() {
        let st = |pos: f32, duration: f32, time_remaining: bool| MainState {
            focused: true,
            play: PlayUi::Playing,
            pressed: None,
            shuffle: false,
            repeat: false,
            eq_on: false,
            pl_on: false,
            always_on_top: false,
            double_size: false,
            pos,
            duration,
            time_remaining,
            time_hidden: false,
            volume: 0.5,
            balance: 0.0,
            stereo: true,
            title: String::new(),
            kbps: "128".to_string(),
            khz: "44".to_string(),
            fg: [0, 255, 0],
            viz: None,
            viz_spectrum: Vec::new(),
            viz_peak: [0, 0, 0],
        };
        // Elapsed: 71s -> 1:11, not flagged remaining.
        assert_eq!(st(71.0, 200.0, false).lcd_mmss(), (1, 11, false));
        // Remaining: 200 - 71 = 129s -> 2:09, flagged remaining.
        assert_eq!(st(71.0, 200.0, true).lcd_mmss(), (2, 9, true));
        // Remaining is suppressed when the duration is unknown.
        assert_eq!(st(71.0, 0.0, true).lcd_mmss(), (1, 11, false));
        // Past the end never shows a negative; it clamps to 0:00.
        assert_eq!(st(250.0, 200.0, true).lcd_mmss(), (0, 0, true));
        // Minutes are capped at 99 for the two-digit LCD (100:40 -> 99:40).
        assert_eq!(st(6040.0, 0.0, false).lcd_mmss(), (99, 40, false));
    }

    #[test]
    fn pl_shade_shows_track_and_time() {
        // #80: the collapsed playlist bar draws the current track + length; with
        // no current track it draws only the frame.
        let skin = load_skin("linamp_base_skin.wsz");
        let color = [0, 255, 0];
        let text_px = |img: &RgbaImage| -> usize {
            let mut n = 0;
            for y in 0..14 {
                for x in 27..214u32 {
                    let p = img.get_pixel(x, y);
                    if p[0] < 80 && p[1] > 180 && p[2] < 80 {
                        n += 1;
                    }
                }
            }
            n
        };
        let shown = skin.compose_pl_shade(true, "1. a song", "6:12", color);
        assert!(text_px(&shown) > 0, "shaded bar should draw the track and time");
        let empty = skin.compose_pl_shade(true, "", "", color);
        assert_eq!(text_px(&empty), 0, "no current track -> no text on the bar");
    }

    #[test]
    fn playlist_row_draws_track_time() {
        // #73: a row with a known length renders the time right-aligned; a row
        // with no length renders none. Checks for normal-coloured text pixels in
        // the right-hand time column.
        let skin = load_skin("linamp_base_skin.wsz");
        let colors = PlColors {
            bg: [0, 0, 0],
            normal: [0, 255, 0],
            current: [255, 255, 255],
            selbg: [0, 0, 180],
        };
        let rows: Vec<PlRow> = vec![
            ("track one".to_string(), false, false, Some(125.0)), // 2:05
            ("track two".to_string(), false, false, None),
        ];
        let st = PlState {
            focused: true,
            colors,
            rows: &rows,
            first: 0,
            pressed: None,
            elapsed_secs: 0.0,
            cur_duration: 0.0,
            time_remaining: false,
            total_secs: 0.0,
            total_partial: false,
            time_hidden: false,
        };
        let w = MAIN_W;
        let img = skin.compose_playlist(&st, w, PL_H);
        let time_pixels = |row: usize| -> usize {
            let ry = PL_LIST_Y + row as i32 * PL_ROW_H;
            let mut n = 0;
            for yy in ry..ry + PL_ROW_H {
                for xx in (w as i32 - 60)..(w as i32 - 22) {
                    let p = img.get_pixel(xx as u32, yy as u32);
                    if p[0] < 80 && p[1] > 180 && p[2] < 80 {
                        n += 1;
                    }
                }
            }
            n
        };
        assert!(time_pixels(0) > 0, "row with a duration should draw its time");
        assert_eq!(time_pixels(1), 0, "row without a duration should draw no time");
    }

    #[test]
    fn main_shade_regions() {
        // Collapsed main bar: wave/options, mini-transport, restore, close; the
        // branding/seek area in between is the draggable title bar.
        assert_eq!(main_shade_region_at(9.0, 6.0), Some(ShadeHit::Options));
        assert_eq!(main_shade_region_at(171.0, 6.0), Some(ShadeHit::Prev));
        assert_eq!(main_shade_region_at(180.0, 6.0), Some(ShadeHit::Play));
        assert_eq!(main_shade_region_at(200.0, 6.0), Some(ShadeHit::Stop));
        assert_eq!(main_shade_region_at(219.0, 6.0), Some(ShadeHit::Eject));
        assert_eq!(main_shade_region_at(258.0, 6.0), Some(ShadeHit::Restore));
        assert_eq!(main_shade_region_at(268.0, 6.0), Some(ShadeHit::Close));
        assert_eq!(main_shade_region_at(100.0, 6.0), Some(ShadeHit::TitleBar));
        // The EQ/playlist shade tester has no transport buttons there.
        assert_eq!(shade_region_at(180.0, 6.0), Some(ShadeHit::TitleBar));
    }

    #[test]
    fn shade_regions() {
        assert_eq!(shade_region_at(265.0, 5.0), Some(ShadeHit::Close));
        assert_eq!(shade_region_at(50.0, 5.0), Some(ShadeHit::TitleBar));
        assert_eq!(shade_region_at(50.0, 20.0), None);
    }

    #[test]
    fn playlist_time_format() {
        // M:SS with no leading zero on minutes; seconds always padded.
        assert_eq!(fmt_pl_time(0.0), "0:00");
        assert_eq!(fmt_pl_time(9.0), "0:09");
        assert_eq!(fmt_pl_time(406.0), "6:46"); // the issue's current-time example
        assert_eq!(fmt_pl_time(65.4), "1:05"); // fractions truncate
        // Rolls over to H:MM:SS at an hour, hours unpadded, mins/secs padded.
        assert_eq!(fmt_pl_time(3600.0), "1:00:00");
        assert_eq!(
            fmt_pl_time(52.0 * 3600.0 + 48.0 * 60.0 + 36.0),
            "52:48:36" // the issue's total-time example
        );
        // Negative clamps to zero rather than underflowing.
        assert_eq!(fmt_pl_time(-5.0), "0:00");
    }

    #[test]
    fn font_map() {
        assert_eq!(char_cell('a'), (0, 0));
        assert_eq!(char_cell('A'), (0, 0));
        assert_eq!(char_cell('0'), (0, 1));
        assert_eq!(char_cell(' '), (30, 0));
    }

    // EQ slider sprite detection, using bundled skins as fixtures. Locks three
    // bugs: Topaz/Iria bars mis-detected as "missing" (saturation-only test), and
    // Alita's short EQMAIN panicking the sampler.

    fn skins_dir() -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("skins")
    }

    fn load_skin(name: &str) -> ClassicSkin {
        ClassicSkin::load(&skins_dir().join(name)).unwrap()
    }

    fn valid_count(name: &str) -> usize {
        let skin = load_skin(name);
        eq_valid_sprites(skin.bmp("eqmain.bmp").unwrap()).len()
    }

    const EQ_TEST_STATE: EqState = EqState {
        focused: true,
        on: true,
        auto: false,
        pressed: None,
        preamp: 0.0,
        bands: [0.6, 0.3, 0.0, -0.2, -0.4, -0.2, 0.1, 0.3, 0.5, 0.7],
    };

    #[test]
    fn bundled_base_and_iria_skins_load() {
        // The shipped base + Iria skins must decode (have MAIN.BMP) and
        // composite their windows without panicking.
        for name in ["linamp_base_skin.wsz", "Iria_Amp02.wsz"] {
            let skin = load_skin(name); // unwraps: fails loudly if it won't load
            assert!(skin.bmp("main.bmp").is_some(), "{name} missing MAIN.BMP");
            let _ = skin.compose_eq(&EQ_TEST_STATE);
        }
    }

    #[test]
    fn skins_load_cursors_including_animated() {
        // Standard .cur (base) and animated .ani-in-.cur (Nucleo) both decode,
        // including the "normal" base cursor used on the auxiliary windows.
        for name in ["linamp_base_skin.wsz", "Nucleo_NLog.wsz"] {
            let skin = load_skin(name);
            assert!(skin.has_cursors(), "{name} should load custom cursors");
            assert!(skin.cursor("normal").is_some(), "{name} missing a normal cursor");
        }
    }

    #[test]
    fn base_skin_has_full_slider_bank() {
        // The reference skin ships all 28 gradient sprites.
        assert_eq!(valid_count("linamp_base_skin.wsz"), 28);
    }

    #[test]
    fn base_skin_draws_eq_curve() {
        // The response curve lives in the graph box (dest 86,17 / 113×19) and is
        // coloured by EQMAIN's line gradient at x=115, y=294 (row 0 = top). With
        // every band maxed the curve hugs the top row; fully cut, the bottom row.
        let skin = load_skin("linamp_base_skin.wsz");
        let eqmain = skin.bmp("eqmain.bmp").unwrap();
        let grad_top = *eqmain.get_pixel(115, 294);
        let grad_bot = *eqmain.get_pixel(115, 294 + 18);

        let mut st = EQ_TEST_STATE;
        st.bands = [1.0; 10];
        let img = skin.compose_eq(&st);
        for gx in [100u32, 140, 180] {
            assert_eq!(*img.get_pixel(gx, 17), grad_top, "top-row curve at x={gx}");
        }

        st.bands = [-1.0; 10];
        let img = skin.compose_eq(&st);
        for gx in [100u32, 140, 180] {
            assert_eq!(*img.get_pixel(gx, 35), grad_bot, "bottom-row curve at x={gx}");
        }
    }

    #[test]
    fn topaz_segmented_meters_are_detected() {
        // Regression: segmented teal meters once read as blank -> synthesized.
        // The empty bottom-of-travel sprite may legitimately not count, so allow
        // a little slack but require nearly the whole bank.
        assert!(valid_count("TopazAmp1-2.wsz") >= 25);
    }

    #[test]
    fn iria_greyscale_bars_are_detected() {
        // Regression: greyscale (low-saturation) bars once read as blank, so good
        // positions snapped to Iria's red sprites ("red at random").
        assert!(valid_count("Iria_Amp02.wsz") >= 25);
    }

    #[test]
    fn alita_short_eqmain_has_no_slider_bank() {
        // Regression: Alita's EQMAIN is too short to hold the slider bank; the old
        // sampler read past the bottom and panicked. It must report zero sprites
        // (so only the thumb is drawn) and never synthesize garish bars.
        let skin = load_skin("Battle_Angel_Alita.wsz");
        let eqmain = skin.bmp("eqmain.bmp").unwrap();
        assert!(eqmain.height() < 227, "fixture changed: Alita EQMAIN grew");
        assert_eq!(eq_valid_sprites(eqmain).len(), 0);
    }

    #[test]
    fn every_bundled_skin_composes_eq_without_panic() {
        // Composing the EQ for any bundled skin must not panic and must yield a
        // correctly sized window. Guards the Alita-style out-of-bounds crash and
        // anything similar a new skin might trip.
        let mut checked = 0;
        for entry in std::fs::read_dir(skins_dir()).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().and_then(|e| e.to_str()) != Some("wsz") {
                continue;
            }
            let Ok(skin) = ClassicSkin::load(&path) else {
                continue;
            };
            let img = skin.compose_eq(&EQ_TEST_STATE);
            assert_eq!((img.width(), img.height()), (275, EQ_H), "{path:?}");
            // A skin that has an EQMAIN tall enough for the bank should detect at
            // least some sprites, so we never silently fall back to thumb-only.
            if let Some(e) = skin.bmp("eqmain.bmp") {
                if e.height() >= 227 {
                    assert!(
                        !eq_valid_sprites(e).is_empty(),
                        "no slider sprites detected for {path:?}",
                    );
                }
            }
            checked += 1;
        }
        assert!(checked >= 5, "expected several bundled skins, found {checked}");
    }
}
