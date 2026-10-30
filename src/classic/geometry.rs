//! Classic skin geometry: pure hit-testing, coordinate tables, and
//! text/number formatting for the classic windows. No image data or skin
//! state, so it is cheap to test and shared by the compositor
//! (`super::compose`) and the input layer.

/// Native classic main-window size in pixels.
pub const MAIN_W: u32 = 275;
pub const MAIN_H: u32 = 116;
/// Equalizer window size.
pub const EQ_H: u32 = 116;
/// Default playlist window height.
pub const PL_H: u32 = 180;

/// Horizontal positions of the 10 EQ band sliders.
pub const EQ_BAND_X: [i32; 10] = [78, 96, 114, 132, 150, 168, 186, 204, 222, 240];
pub(crate) const EQ_PREAMP_X: i32 = 21;
pub(crate) const EQ_SLIDER_TOP: f32 = 38.0;
pub(crate) const EQ_SLIDER_TRAVEL: f32 = 52.0;
/// Inset of the 11px slider thumb within its 14px band cell. The groove every
/// skin draws is centred at local x ~6, so the thumb (visible centre at its own
/// local x 5) is inset by 1 to sit on it. A `+2` inset left the thumb 1px right
/// of the groove, invisible on wide grooves but obvious on crisp/narrow ones
/// like MacOSXAqua and Osmosis (#149).
pub(crate) const EQ_THUMB_INSET: i32 = 1;

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

/// The `(x, y, w, h)` native-pixel rect for a main-window button, read from the
/// shared `REGIONS` table so the compositor draws each button on exactly its
/// click target (no second copy of the coordinates to keep in sync). `None` for
/// regions that aren't buttons (sliders, title bar, time/viz toggles).
pub(crate) fn btn_rect(b: Btn) -> Option<(i32, i32, i32, i32)> {
    REGIONS
        .iter()
        .find(|(rb, ..)| *rb == b)
        .map(|(_, x, y, w, h)| (*x, *y, *w, *h))
}

/// Transport buttons on CBUTTONS.BMP: each button's source x. Size and dest come
/// from the shared `REGIONS` rect (`btn_rect`), and pressing a button shifts its
/// source down one row (by the button's own height). Keeping only the source x
/// here means a transport-button position is defined once, in `REGIONS`.
pub(crate) const CBUTTON_SRC_X: &[(Btn, u32)] = &[
    (Btn::Prev, 0),
    (Btn::Play, 23),
    (Btn::Pause, 46),
    (Btn::Stop, 69),
    (Btn::Next, 92),
    (Btn::Eject, 114),
];

/// Hover-tooltip label for a main-window button, or `None` for regions that
/// aren't buttons (title bar, sliders, time/viz toggles).
pub(crate) fn btn_label(b: Btn) -> Option<&'static str> {
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
    /// The mini position/seek slider on the collapsed main bar.
    Pos,
}

/// Main windowshade position (seek) bar, measured from Webamp: the track sits at
/// x=226, is 17px wide and 7px tall (y 4..11); the 3px thumb travels the inner
/// `SHADE_POS_W - SHADE_POS_THUMB_W` pixels. Sprites come from TITLEBAR.BMP
/// (background at 0,36; thumb at 20,36; left/right end caps at 17,36 / 23,36).
pub const SHADE_POS_X: f32 = 226.0;
pub const SHADE_POS_W: f32 = 17.0;
pub const SHADE_POS_THUMB_W: f32 = 3.0;

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

/// Hit-testing for the collapsed main window bar, which (unlike the EQ and
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
    // Mini position/seek slider (between the transport buttons and Restore).
    if x >= SHADE_POS_X && x < SHADE_POS_X + SHADE_POS_W {
        return Some(ShadeHit::Pos);
    }
    Some(ShadeHit::TitleBar)
}

/// Approximate an accented Latin character with its base ASCII letter, so the
/// ASCII-only bitmap font renders it readably instead of as a blank (#97, example:
/// Vietnamese "Trường" to "Truong"). Returns the char unchanged when unmapped.
/// This is a readability fallback; full non-Latin glyphs would need a font atlas.
/// Also reused when writing ID3v1 (Latin-1 only) to fold characters outside
/// Latin-1 to base ASCII instead of the "?" a lossy encode produces.
pub(crate) fn ascii_fold(ch: char) -> char {
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
        0x11C..=0x123 => 'g', // Ĝ ĝ Ğ ğ Ġ ġ Ģ ģ (Turkish ğ, Esperanto ĝ)
        0x124..=0x127 => 'h', // Ĥ ĥ Ħ ħ
        0x128..=0x131 => 'i',
        0x139..=0x142 => 'l',
        0x143..=0x14B => 'n', // through ŉ Ŋ ŋ
        0x14C..=0x151 | 0x1A0 | 0x1A1 => 'o',
        0x154..=0x159 => 'r',
        0x15A..=0x161 => 's',
        0x162..=0x167 => 't', // through Ŧ ŧ
        0x168..=0x173 | 0x1AF | 0x1B0 => 'u', // through Ű ű Ų ų (Hungarian)
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
pub(crate) fn char_cell(ch: char) -> (u32, u32) {
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
    /// Window-shade volume / balance sliders (collapsed EQ bar; eq_ex.bmp).
    ShadeVolume,
    ShadeBalance,
}

// EQ windowshade volume/balance grooves on the 275px bar (eq_ex.bmp shade
// background), each with a 3px thumb that slides along it. Shared by the
// renderer (compose_eq_shade) and the hit-test/drag.
// Collapsed-EQ slider tracks in base EQ-window coords (the blank channel, not the
// gold EQ-graph bars), from Webamp's shade CSS: volume left 61 width 97, balance
// left 164 width 43. Shared by the compositor and the hit-test so they agree.
pub const EQ_SHADE_VOL_X: f32 = 61.0;
pub const EQ_SHADE_VOL_W: f32 = 97.0;
pub const EQ_SHADE_BAL_X: f32 = 164.0;
pub const EQ_SHADE_BAL_W: f32 = 43.0;
pub const EQ_SHADE_THUMB_W: f32 = 3.0;

/// Hit-test for the collapsed EQ bar: close box, the volume/balance grooves, then
/// the title bar (drag). Separate from `eq_region_at` because the full-window EQ
/// controls aren't present when shaded.
pub fn eq_shade_region_at(x: f32, y: f32) -> Option<EqRegion> {
    let hit = |rx: f32, ry: f32, rw: f32, rh: f32| x >= rx && x < rx + rw && y >= ry && y < ry + rh;
    if hit(264.0, 3.0, 9.0, 9.0) {
        return Some(EqRegion::Close);
    }
    if hit(EQ_SHADE_VOL_X, 0.0, EQ_SHADE_VOL_W, 14.0) {
        return Some(EqRegion::ShadeVolume);
    }
    if hit(EQ_SHADE_BAL_X, 0.0, EQ_SHADE_BAL_W, 14.0) {
        return Some(EqRegion::ShadeBalance);
    }
    if hit(0.0, 0.0, 275.0, 14.0) {
        return Some(EqRegion::TitleBar);
    }
    None
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
    /// The right-edge scrollbar channel, where the thumb travels.
    Scroll,
    // Mini-player transport buttons in the bottom-right sprite.
    MiniPrev,
    MiniPlay,
    MiniPause,
    MiniStop,
    MiniNext,
    MiniEject,
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
pub(crate) const PL_MINI_BTNS: &[(PlRegion, i32, i32)] = &[
    (PlRegion::MiniPrev, 5, 8),
    (PlRegion::MiniPlay, 13, 8),
    (PlRegion::MiniPause, 23, 8),
    (PlRegion::MiniStop, 33, 8),
    (PlRegion::MiniNext, 42, 8),
    (PlRegion::MiniEject, 51, 9),
];


pub(crate) const PL_ROW_H: i32 = 7;
pub(crate) const PL_LIST_X: i32 = 14;
pub(crate) const PL_LIST_Y: i32 = 21;

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
    // Right-edge scrollbar channel (the thumb lives in the right frame), checked before
    // the list so a click on the far right scrolls rather than selecting a row.
    if x >= width - 16.0 && y >= 20.0 && y < height - 38.0 {
        return Some(PlRegion::Scroll);
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

/// Clamp a playlist scroll offset so it never points past the last row. An offset
/// at or beyond the length makes the visible range (`first..end`) empty, which would
/// render the whole list blank, so callers that change the playlist length must
/// re-clamp. Returns 0 for an empty list. This is the loose clamp used on render and
/// after a delete, so the "blank space at the bottom after deleting" case is allowed to
/// persist; a scroll gesture re-applies the tighter [`max_pl_first`] clamp.
pub fn clamp_pl_first(first: usize, len: usize) -> usize {
    first.min(len.saturating_sub(1))
}

/// Fixed height (native px) of the scrollbar thumb sprite (PLEDIT.BMP 52,53 8x18).
pub(crate) const PL_SCROLL_THUMB_H: i32 = 18;

/// The furthest a scroll offset may advance from a scroll gesture: the last row rests at
/// the bottom with no blank rows past it (retro Winamp behaviour). 0 when the list fits
/// the window.
pub fn max_pl_first(len: usize, visible: usize) -> usize {
    len.saturating_sub(visible)
}

/// (top, travel) of the scrollbar thumb's vertical range for a PL window `height` px: the
/// thumb top moves from just under the title bar to `thumb_h` above the bottom bar.
fn pl_scroll_track(height: f32) -> (f32, f32) {
    let top = PL_LIST_Y as f32;
    let bottom = height - 38.0;
    let travel = (bottom - top - PL_SCROLL_THUMB_H as f32).max(0.0);
    (top, travel)
}

/// Top y (native px) of the fixed-size scrollbar thumb for scroll offset `first`. The
/// thumb is at the top when nothing is scrolled off the top (`first == 0`), and at the
/// bottom whenever the end of the list is visible, i.e. the last row is on screen
/// (`first + visible >= len`, which `first >= max_pl_first` captures). In between it is
/// proportional to how far down the scrollable range you are. So growing the window with a
/// non-zero offset walks the thumb down to the bottom as the end comes into view and keeps
/// it there past the end (blank rows below), while the list itself stays put, matching a
/// physical scrollbar.
pub fn pl_scroll_thumb_y(first: usize, len: usize, visible: usize, height: f32) -> f32 {
    let (top, travel) = pl_scroll_track(height);
    let max = max_pl_first(len, visible);
    let frac = if first >= max {
        // The end of the list is visible: pin to the bottom, unless we are also at the
        // very top (whole list fits and nothing is scrolled off), which stays at the top.
        if first == 0 {
            0.0
        } else {
            1.0
        }
    } else {
        first as f32 / max as f32
    };
    top + frac * travel
}

/// The scroll offset a thumb whose top sits at window-y `thumb_y` represents, the inverse
/// of [`pl_scroll_thumb_y`], already clamped to the scrollable range. 0 when the list fits.
pub fn pl_scroll_first_at(thumb_y: f32, len: usize, visible: usize, height: f32) -> usize {
    let max = max_pl_first(len, visible);
    let (top, travel) = pl_scroll_track(height);
    if max == 0 || travel <= 0.0 {
        return 0;
    }
    let frac = ((thumb_y - top) / travel).clamp(0.0, 1.0);
    (frac * max as f32).round() as usize
}

/// Smallest playlist height, in unscaled skin pixels (two title rows + a usable
/// list area); mirrors the floor applied while resizing.
pub const PL_MIN_H: f32 = 87.0;

/// The tallest the playlist may grow (unscaled skin pixels) before its bottom
/// edge would pass `desktop_bottom`, given the window top edge `top_y` and the UI
/// `scale` (both in points; on-screen height is `pl_px_h * scale`). Returns `None`
/// when there isn't even room for the minimum height, the caller then leaves the
/// current height alone rather than shrinking it. Keeps a downward resize from
/// running off the screen bottom, which the WM would otherwise answer by pushing
/// the window's top edge upward (#156).
pub fn pl_max_height(desktop_bottom: f32, top_y: f32, scale: f32) -> Option<f32> {
    let max_h = (desktop_bottom - top_y) / scale.max(0.01);
    (max_h >= PL_MIN_H).then_some(max_h)
}

/// Format a playlist time like the classic player's bottom bar: `M:SS`, or `H:MM:SS` once it
/// reaches an hour. Minutes/hours have no leading zero; seconds (and minutes
/// inside an H:MM:SS) are zero-padded. Negative input clamps to zero.
/// EQ slider sprite index (0 = top/boost .. 27 = bottom/cut) for value `v`
/// (-1..=1), snapped to the nearest sprite the skin ships. None when the skin
/// has no slider bank (only the thumb is drawn).
pub(crate) fn eq_sprite_index(v: f32, valid: &[i32]) -> Option<i32> {
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
