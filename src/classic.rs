//! Classic Winamp 2.x skin renderer.
//!
//! A `.wsz` is a ZIP of BMP sprite sheets. This module decodes those sheets and
//! composites the classic 275x116 main window from them at exact Winamp sprite
//! coordinates (sourced from the open-source webamp project). The same
//! compositor is used both for a headless PNG preview and for the live in-app
//! view (which uploads the composed frame as a nearest-neighbour texture).

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
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
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
}

/// (button, x, y, w, h) hit-rects in main-window space, most-specific first.
pub const REGIONS: &[(Btn, i32, i32, i32, i32)] = &[
    (Btn::Options, 6, 3, 9, 9),
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

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum PlayUi {
    Playing,
    Paused,
    Stopped,
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
    pub pos: f32,
    pub duration: f32,
    pub volume: f32,
    pub balance: f32,
    pub stereo: bool,
    pub title: String,
    pub kbps: String,
    pub khz: String,
    /// LCD/foreground colour, used for the shade-bar title + time.
    pub fg: [u8; 3],
}

/// Hit target on a 14px window-shade bar.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ShadeHit {
    Close,
    TitleBar,
}

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

/// Decoded sprite sheets from one skin, keyed by lowercased basename.
pub struct ClassicSkin {
    bitmaps: HashMap<String, RgbaImage>,
}

impl ClassicSkin {
    pub fn load(path: &Path) -> Result<Self> {
        let file = File::open(path)?;
        let mut zip = ZipArchive::new(file)?;
        let mut bitmaps = HashMap::new();
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
            }
        }
        if !bitmaps.contains_key("main.bmp") {
            anyhow::bail!("skin has no MAIN.BMP");
        }
        Ok(Self { bitmaps })
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

        // Time digits.
        if let Some(nb) = self.bmp("numbers.bmp").or_else(|| self.bmp("nums_ex.bmp")) {
            let secs = st.pos.max(0.0) as u32;
            let mm = (secs / 60).min(99);
            let ss = secs % 60;
            let digits = [mm / 10, mm % 10, ss / 10, ss % 10];
            let xs = [48, 60, 78, 90];
            for (i, &d) in digits.iter().enumerate() {
                blit(&mut c, nb, xs[i], 26, d * 9, 0, 9, 13);
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

        // Scrolling title, kbps, khz (bitmap font).
        self.draw_text(&mut c, &st.title, 111, 24, 154);
        self.draw_text(&mut c, &st.kbps, 111, 43, 15);
        self.draw_text(&mut c, &st.khz, 156, 43, 10);

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
        self.draw_text_tinted(&mut c, &st.title, 6, 4, 150, st.fg);
        let secs = st.pos.max(0.0) as u32;
        let time = format!("{:02}:{:02}", (secs / 60).min(99), secs % 60);
        self.draw_text_tinted(&mut c, &time, 224, 4, 28, st.fg);
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

/// Map a character to its (column, row) cell in TEXT.BMP (5x6 cells).
fn char_cell(ch: char) -> (u32, u32) {
    let c = ch.to_ascii_lowercase();
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

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
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

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
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
}

#[derive(Clone, Copy)]
pub struct PlColors {
    pub bg: [u8; 3],
    pub normal: [u8; 3],
    pub current: [u8; 3],
    pub selbg: [u8; 3],
}

/// One playlist row: display text, is-playing, is-selected.
pub type PlRow = (String, bool, bool);

pub struct PlState<'a> {
    pub focused: bool,
    pub colors: PlColors,
    pub rows: &'a [PlRow],
    pub first: usize,
}

/// Left-anchored bottom-bar menu buttons: (region, x, width). y is
/// `height - 30`, h = 18. The List menu is right-anchored (see `pl_region_at`).
const PL_BOTTOM_BTNS: &[(PlRegion, i32, i32)] = &[
    (PlRegion::AddMenu, 14, 22),
    (PlRegion::RemMenu, 43, 22),
    (PlRegion::SelMenu, 72, 22),
    (PlRegion::MiscMenu, 101, 22),
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

            // Slider thumbs.
            let draw_thumb = |c: &mut RgbaImage, e: &RgbaImage, colx: i32, v: f32, sel: bool| {
                let frac = ((1.0 - v) / 2.0).clamp(0.0, 1.0);
                let ty = EQ_SLIDER_TOP as i32 + (frac * EQ_SLIDER_TRAVEL) as i32;
                let sy = if sel { 176 } else { 164 };
                blit(c, e, colx, ty, 0, sy, 11, 11);
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

            // Bottom bar: left corner (with Add/Rem/Sel/Misc), a tiled plain
            // strip, then the right corner (with the List menu / time display).
            let by = height as i32 - 38;
            let mut bx = 125;
            while bx < w - 150 {
                let seg = (w - 150 - bx).min(25) as u32;
                blit(&mut c, p, bx, by, 126, 72, seg, 38);
                bx += 25;
            }
            blit(&mut c, p, 0, by, 0, 72, 125, 38); // bottom-left
            blit(&mut c, p, w - 150, by, 126, 72, 150, 38); // bottom-right
            blit(&mut c, p, w - 11, 3, 52, 42, 9, 9); // close
        }

        // Track list.
        let visible = pl_visible_rows(height);
        for (i, (text, is_cur, is_sel)) in st.rows.iter().skip(st.first).take(visible).enumerate() {
            let ry = PL_LIST_Y + i as i32 * PL_ROW_H;
            if *is_sel {
                fill_rect(&mut c, PL_LIST_X - 2, ry - 1, w - PL_LIST_X - 22, PL_ROW_H, st.colors.selbg);
            }
            let col = if *is_cur { st.colors.current } else { st.colors.normal };
            self.draw_text_tinted(&mut c, text, PL_LIST_X, ry, w - PL_LIST_X - 24, col);
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

    pub fn compose_pl_shade(&self, focused: bool) -> RgbaImage {
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
            blit_tinted(c, tf, x, y0, col * 5, row * 6, 5, 6, color);
            x += 5;
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
            let lum = p[0] as u32 + p[1] as u32 + p[2] as u32;
            if lum > 150 {
                dst.put_pixel(dxx as u32, dyy as u32, Rgba([color[0], color[1], color[2], 255]));
            }
        }
    }
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
        pos: 125.0,
        duration: 254.0,
        volume: 0.78,
        balance: 0.0,
        stereo: true,
        title: "linamp - smells like winamp".to_string(),
        kbps: "128".to_string(),
        khz: "44".to_string(),
        fg: [0, 255, 120],
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
        pos: 73.0,
        duration: 221.0,
        volume: 0.8,
        balance: 0.0,
        stereo: true,
        title: "linamp - classic skin test".to_string(),
        kbps: "192".to_string(),
        khz: "44".to_string(),
        fg: [0, 255, 120],
    });
    let eq = skin.compose_eq(&EqState {
        focused: true,
        on: true,
        auto: false,
        pressed: None,
        preamp: 0.0,
        bands: [0.6, 0.3, 0.0, -0.2, -0.4, -0.2, 0.1, 0.3, 0.5, 0.7],
    });
    let rows: Vec<PlRow> = vec![
        ("1. akira - kaneda theme".to_string(), true, true),
        ("2. geinoh yamashirogumi - tetsuo".to_string(), false, false),
        ("3. battle angel alita - gally".to_string(), false, false),
        ("4. iria - zeiram ost".to_string(), false, false),
        ("5. evangelion - cruel angel".to_string(), false, false),
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

#[cfg(test)]
mod tests {
    use super::char_cell;
    use super::*;

    #[test]
    fn main_regions() {
        assert_eq!(region_at(50.0, 95.0), Some(Btn::Play));
        assert_eq!(region_at(7.0, 6.0), Some(Btn::Options));
        assert_eq!(region_at(5.0, 5.0), Some(Btn::TitleBar));
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
    fn shade_regions() {
        assert_eq!(shade_region_at(265.0, 5.0), Some(ShadeHit::Close));
        assert_eq!(shade_region_at(50.0, 5.0), Some(ShadeHit::TitleBar));
        assert_eq!(shade_region_at(50.0, 20.0), None);
    }

    #[test]
    fn font_map() {
        assert_eq!(char_cell('a'), (0, 0));
        assert_eq!(char_cell('A'), (0, 0));
        assert_eq!(char_cell('0'), (0, 1));
        assert_eq!(char_cell(' '), (30, 0));
    }
}
