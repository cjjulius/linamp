//! Classic skin compositor: decodes the `.wsz` sprite sheets and blits the
//! classic windows. Uses the coordinate tables in `super::geometry` and the
//! shared state structs in `super`.

use std::collections::HashMap;
use std::fs::File;
use std::io::Read;
use std::path::Path;

use anyhow::Result;
use image::{Rgba, RgbaImage};
use zip::ZipArchive;

use super::*;

/// Decode a skin bitmap with a sane size cap, so a crafted BMP header can't force
/// a huge allocation. Returns None (the sprite is skipped) on any failure.
fn decode_bmp_capped(buf: &[u8]) -> Option<image::RgbaImage> {
    let mut reader = image::ImageReader::new(std::io::Cursor::new(buf))
        .with_guessed_format()
        .ok()?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(8192);
    limits.max_image_height = Some(8192);
    reader.limits(limits);
    Some(reader.decode().ok()?.to_rgba8())
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
                    if let Some(img) = decode_bmp_capped(&buf) {
                        bitmaps.insert(base, img);
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

    pub(crate) fn bmp(&self, name: &str) -> Option<&RgbaImage> {
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
        } else {
            // Skin ships no VOLUME.BMP: show the level with a synthesized slider
            // instead of an empty recess (#18).
            draw_fallback_slider(&mut c, 107, 68, st.volume.clamp(0.0, 1.0));
        }

        // Balance.
        if let Some(bb) = self.bmp("balance.bmp") {
            let amt = (st.balance.abs().clamp(0.0, 1.0) * 27.0).round() as u32;
            blit(&mut c, bb, 177, 57, 9, amt * 15, 38, 13);
            let tx = 177 + ((st.balance * 0.5 + 0.5).clamp(0.0, 1.0) * (38.0 - 14.0)) as i32;
            let sx = if st.pressed == Some(Btn::Balance) { 0 } else { 15 };
            blit(&mut c, bb, tx, 58, sx, 422, 14, 11);
        } else {
            // Skin ships no BALANCE.BMP: synthesized pan indicator, centre = 0 (#18).
            draw_fallback_slider(&mut c, 177, 38, st.balance * 0.5 + 0.5);
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
        // Mini position/seek slider: the track background plus a thumb at the
        // current position, mirroring the expanded posbar. Draws from TITLEBAR.BMP
        // (background 0,36 17x7; thumb 20,36 3x7, with end-cap variants at the
        // extremes). st.pos already reflects a live seek-drag preview.
        if let Some(tb) = self.bmp("titlebar.bmp") {
            let x = SHADE_POS_X as i32;
            blit(&mut c, tb, x, 4, 0, 36, 17, 7);
            let frac = if st.duration > 0.0 {
                (st.pos / st.duration).clamp(0.0, 1.0)
            } else {
                0.0
            };
            let travel = SHADE_POS_W - SHADE_POS_THUMB_W;
            let tx = x + (frac * travel).round() as i32;
            // End-cap thumb sprites at the extremes, middle thumb in between.
            let sx = if frac <= 0.0 {
                17
            } else if frac >= 1.0 {
                23
            } else {
                20
            };
            blit(&mut c, tb, tx, 4, sx, 36, 3, 7);
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
    // the key (example: the base skin's stray ~20%-blend cyan dot above the "Y"), 
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
/// runs of non-`key` columns. Returns `Some` only when exactly 26 glyphs are found,
/// so a skin whose font layout differs from the base (example: Green Dimension's
/// fixed-width grid) renders from its own metrics, while anything unexpected
/// (including the base skin, whose X and Y touch, giving 25 runs) falls back to the
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
    // (inactive) row has a stray pixel on some glyphs (example: Y) that would widen
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
    /// ex: where the window content area begins.
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

        // --- Title bar (y 0..20). Winamp layout, left to right: corner, striped
        // fill running inward, an end-cap, the title (its own centre fill with the
        // caption), the other end-cap, striped fill, corner, so the bars run all
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
                // value (−1..1, bottom..top) to graph row 0..=18 (top..bottom).
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
                blit(c, e, colx + EQ_THUMB_INSET, ty, 0, sy, 11, 11);
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
                // line does NOT blink, only the standalone current-time box below.
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

    pub fn compose_eq_shade(&self, focused: bool, volume: f32, balance: f32) -> RgbaImage {
        let mut c = RgbaImage::new(275, 14);
        // Prefer the dedicated EQ-shade sheet (eq_ex.bmp): it carries the volume
        // and balance mini-sliders, which mirror the main window's. Skins without
        // it fall back to the plain EQMAIN shade bar (no sliders).
        if let Some(e) = self.bmp("eq_ex.bmp") {
            let sy = if focused { 0 } else { 15 };
            blit(&mut c, e, 0, 0, 0, sy, 275, 14);
            // A 3×7 thumb slides along the slider track (the blank channel, not the
            // gold EQ-graph bars); its sprite (left/centre/right at eq_ex y30) is
            // chosen by which third the value falls in. Track bounds from Webamp's
            // shade CSS: volume left 61 width 97, balance left 164 width 43, top 4.
            let slider = |c: &mut RgbaImage, frac: f32, lo: i32, w: i32, sx: [u32; 3]| {
                let frac = frac.clamp(0.0, 1.0);
                let tx = lo + (frac * (w - 3).max(0) as f32).round() as i32;
                let s = if frac < 1.0 / 3.0 {
                    sx[0]
                } else if frac < 2.0 / 3.0 {
                    sx[1]
                } else {
                    sx[2]
                };
                blit(c, e, tx, 4, s, 30, 3, 7);
            };
            slider(
                &mut c,
                volume.clamp(0.0, 1.0),
                EQ_SHADE_VOL_X as i32,
                EQ_SHADE_VOL_W as i32,
                [1, 4, 7],
            );
            slider(
                &mut c,
                balance.clamp(-1.0, 1.0) * 0.5 + 0.5,
                EQ_SHADE_BAL_X as i32,
                EQ_SHADE_BAL_W as i32,
                [11, 14, 17],
            );
            blit(&mut c, e, 264, 3, 11, 38, 9, 9); // close
        } else if let Some(e) = self.bmp("eqmain.bmp") {
            let sy = if focused { 134 } else { 149 };
            blit(&mut c, e, 0, 0, 0, sy, 275, 14);
            blit(&mut c, e, 264, 3, 0, 116, 9, 9); // close
        } else {
            // Neither eq_ex.bmp nor EQMAIN.BMP is present, so the skin ships no
            // shade-bar graphics. Draw a minimal but usable bar from the skin's
            // colours: a solid background plus synthesized volume/balance thumbs and
            // a close marker, in a tone that contrasts the background so it never
            // renders unreadable (example: black-on-black) (#18).
            let bg = self.text_bg;
            fill_rect(&mut c, 0, 0, 275, 14, bg);
            // Light controls on a dark bar, dark controls on a light bar.
            let lum = (bg[0] as u32 * 299 + bg[1] as u32 * 587 + bg[2] as u32 * 114) / 1000;
            let (groove, thumb) = if lum < 128 {
                ([120u8, 120, 120], [220u8, 220, 220])
            } else {
                ([110u8, 110, 110], [30u8, 30, 30])
            };
            // Volume and balance grooves with a thumb at the current value, at the
            // same track bounds eq_ex.bmp would use so the hit-testing still lines up.
            let track = |c: &mut RgbaImage, frac: f32, lo: i32, w: i32| {
                let frac = frac.clamp(0.0, 1.0);
                fill_rect(c, lo, 6, w, 2, groove);
                let tx = lo + (frac * (w - 3).max(0) as f32).round() as i32;
                fill_rect(c, tx, 4, 3, 7, thumb);
            };
            track(&mut c, volume.clamp(0.0, 1.0), EQ_SHADE_VOL_X as i32, EQ_SHADE_VOL_W as i32);
            track(
                &mut c,
                balance.clamp(-1.0, 1.0) * 0.5 + 0.5,
                EQ_SHADE_BAL_X as i32,
                EQ_SHADE_BAL_W as i32,
            );
            // Close marker (the close region is hit-tested regardless; this just
            // makes it visible).
            fill_rect(&mut c, 265, 4, 7, 7, thumb);
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
/// Bounds-checked: a short EQMAIN (example: Alita, no slider bank) yields an empty list.
pub(crate) fn eq_valid_sprites(e: &RgbaImage) -> Vec<i32> {
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

/// Classic Winamp-style analyzer gradient by vertical fraction (0=bottom green,
/// 0.5=yellow, 1=top red).
pub(crate) fn viz_gradient(f: f32) -> [u8; 3] {
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
/// rect (x,y,w,h). Bars use a green to yellow to red gradient by height; the
/// oscilloscope uses the LCD colour.
fn draw_viz(c: &mut RgbaImage, st: &MainState, x: i32, y: i32, w: i32, h: i32) {
    let peak_col = st.viz_peak;
    let hf = (h as f32 - 1.0).max(1.0);
    // Colour at vertical fraction f (0=bottom, 1=top): the skin's spectrum
    // gradient if present, else a synthesized green to yellow to red gradient.
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
        Some(VizFrame::Scope(samples)) if samples.len() >= 2 => {
            let n = samples.len();
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
        // A 0 or 1 sample scope frame (startup, silence, a buffer gap) has nothing
        // to plot; draw nothing instead of indexing out of bounds.
        Some(VizFrame::Scope(_)) | None => {}
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

/// Minimal volume / balance indicator for skins that omit VOLUME.BMP / BALANCE.BMP:
/// a dim groove with a brighter thumb at `frac` (0..1), so the current level is
/// still visible instead of a dead recess (#18). `x`/`w` are the slider track's
/// destination bounds in the main window; the geometry matches the real sprites
/// (thumb y=58, height 11; the 7px thumb reaches the right edge at frac 1).
pub(crate) fn draw_fallback_slider(c: &mut RgbaImage, x: i32, w: i32, frac: f32) {
    const GROOVE: [u8; 3] = [72, 72, 72];
    const THUMB: [u8; 3] = [200, 200, 200];
    // The thumb is 14px wide with a (w - 14) travel, matching the real VOLUME.BMP
    // / BALANCE.BMP thumb and, crucially, the click-to-value handler
    // (apply at `Btn::Volume`/`Btn::Balance`), so the drawn thumb sits exactly
    // where a click on it maps.
    const THUMB_W: i32 = 14;
    fill_rect(c, x, 62, w, 2, GROOVE);
    let frac = frac.clamp(0.0, 1.0);
    let tx = x + (frac * (w - THUMB_W).max(0) as f32).round() as i32;
    fill_rect(c, tx, 58, THUMB_W, 11, THUMB);
}

/// Blit a glyph, recolouring its lit pixels to `color` (transparent elsewhere).
/// Most common colour in an image, used as the bitmap font's background.
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
