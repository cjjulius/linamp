//! Classic 2.x skin renderer: decodes a `.wsz` (ZIP of BMP sprite sheets)
//! and composites the classic windows at exact classic sprite coords (from the
//! webamp project). Serves both headless PNG previews and the live in-app view
//! (uploaded as a nearest-neighbour texture).

use std::collections::HashMap;
use image::RgbaImage;

mod compose;
mod geometry;
pub(crate) use compose::*;
pub(crate) use geometry::*;

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
    /// Blank the time readout this frame, the "off" half of the paused blink
    /// (the classic player flashes the time while paused).
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
    /// falls back to a synthesized green to yellow to red gradient.
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
    /// without extension (example: "normal", "posbar").
    cursors: HashMap<String, crate::cursor::CursorImage>,
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

pub struct EqState {
    pub focused: bool,
    pub on: bool,
    pub auto: bool,
    pub pressed: Option<EqRegion>,
    /// Preamp and 10 bands, each -1.0..=1.0 (bottom..top).
    pub preamp: f32,
    pub bands: [f32; 10],
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
    /// The real scroll offset (index of the first visible row) and the total row count,
    /// used to place the always-visible scrollbar thumb. `rows` is pre-sliced to the
    /// visible window with `first` 0, so these carry the full picture the thumb needs.
    pub scroll_first: usize,
    pub total_rows: usize,
    /// Bottom-bar menu button currently held / with its menu open (or the scrollbar while
    /// it is being dragged, for the pressed thumb sprite).
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
    /// lower bound and the readout gets a trailing `+` (as in the classic player).
    pub total_partial: bool,
    /// Blank the current-track time this frame, the "off" half of the paused
    /// blink (the running total stays put; only the current time flashes).
    pub time_hidden: bool,
}

#[cfg(test)]
mod tests {
    use super::char_cell;
    use super::*;
    use image::Rgba;

    #[test]
    fn drawn_buttons_sit_on_their_hit_targets() {
        // S4: the main-window button sprites are drawn on the rect from the shared
        // REGIONS table (via btn_rect), so a button's draw rect and its click
        // target can't drift apart. Every button the compositor draws must map
        // back to itself when hit-tested at its own centre.
        let drawn = [
            Btn::Prev, Btn::Play, Btn::Pause, Btn::Stop, Btn::Next, Btn::Eject,
            Btn::Shuffle, Btn::Repeat, Btn::Eq, Btn::Pl, Btn::Close, Btn::Minimize,
        ];
        for b in drawn {
            let (x, y, w, h) = super::btn_rect(b).unwrap_or_else(|| panic!("no rect for {b:?}"));
            let (cx, cy) = ((x + w / 2) as f32, (y + h / 2) as f32);
            assert_eq!(region_at(cx, cy), Some(b), "{b:?} draw rect centre doesn't hit it");
        }
        // The transport table covers all six CBUTTONS buttons on distinct sprites.
        assert_eq!(super::CBUTTON_SRC_X.len(), 6);
        let mut xs: Vec<u32> = super::CBUTTON_SRC_X.iter().map(|&(_, sx)| sx).collect();
        xs.sort_unstable();
        xs.dedup();
        assert_eq!(xs.len(), 6, "duplicate source x in CBUTTON_SRC_X");
    }

    #[test]
    fn ascii_folds_accented_text_to_base_letters() {
        // Accented and Vietnamese characters render via their base ASCII cell,
        // not as blanks (#97).
        for (accented, base) in [
            ('é', 'e'), ('ñ', 'n'), ('ü', 'u'), ('ç', 'c'), ('ø', 'o'),
            ('Đ', 'd'), ('ư', 'u'), ('ờ', 'o'), ('ế', 'e'), ('ạ', 'a'), ('ý', 'y'),
            ('ğ', 'g'), ('Ğ', 'g'), ('ĥ', 'h'), ('ŋ', 'n'), ('ŧ', 't'), ('ű', 'u'), ('ų', 'u'),
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
    fn playlist_first_clamps_into_range() {
        // An offset within range is untouched.
        assert_eq!(clamp_pl_first(0, 10), 0);
        assert_eq!(clamp_pl_first(5, 10), 5);
        assert_eq!(clamp_pl_first(9, 10), 9);
        // An offset past the end (e.g. after a long playlist is replaced by a short
        // one) clamps to the last row so the list never renders blank.
        assert_eq!(clamp_pl_first(9, 1), 0);
        assert_eq!(clamp_pl_first(50, 3), 2);
        // An empty playlist clamps to 0.
        assert_eq!(clamp_pl_first(7, 0), 0);
    }

    #[test]
    fn max_pl_first_stops_last_row_at_the_bottom() {
        // 10 rows, 4 visible: the furthest scroll shows rows 6..10, so first maxes at 6
        // (no blank rows past the last song).
        assert_eq!(max_pl_first(10, 4), 6);
        // A list that fits the window cannot scroll.
        assert_eq!(max_pl_first(4, 4), 0);
        assert_eq!(max_pl_first(3, 4), 0);
        assert_eq!(max_pl_first(0, 4), 0);
    }

    #[test]
    fn pl_scrollbar_thumb_position_round_trips() {
        // A window tall enough for a real track area; 100 rows, far more than fit.
        let height = 200.0;
        let visible = pl_visible_rows(height as u32);
        let len = 100;
        let max = max_pl_first(len, visible);
        assert!(max > 0);
        // first=0 pins the thumb to the top of the track.
        let top_y = pl_scroll_thumb_y(0, len, visible, height);
        // The max offset pushes the thumb to the bottom of its travel.
        let bot_y = pl_scroll_thumb_y(max, len, visible, height);
        assert!(bot_y > top_y);
        // Grabbing near the top maps back to 0, near the bottom to max.
        assert_eq!(pl_scroll_first_at(top_y, len, visible, height), 0);
        assert_eq!(pl_scroll_first_at(bot_y + 100.0, len, visible, height), max);
        // A list that fits never scrolls and the thumb stays at the top.
        assert_eq!(pl_scroll_first_at(9999.0, visible, visible, height), 0);
        assert_eq!(
            pl_scroll_thumb_y(0, visible, visible, height),
            pl_scroll_thumb_y(0, visible, visible, height)
        );
    }

    #[test]
    fn pl_region_at_hits_the_scrollbar_on_the_right_edge() {
        // The right-edge channel in the list area is the scrollbar, not a list row.
        assert_eq!(
            pl_region_at(275.0 - 8.0, 100.0, 275.0, 180.0),
            Some(PlRegion::Scroll)
        );
        // Just left of the channel is still the list.
        assert_eq!(pl_region_at(100.0, 100.0, 275.0, 180.0), Some(PlRegion::List));
    }

    #[test]
    fn eq_sprite_index_maps_and_snaps() {
        let full: Vec<i32> = (0..28).collect();
        assert_eq!(eq_sprite_index(1.0, &full), Some(0)); // boost to top
        assert_eq!(eq_sprite_index(-1.0, &full), Some(27)); // cut to bottom
        assert_eq!(eq_sprite_index(0.0, &full), Some(14)); // centre
        assert_eq!(eq_sprite_index(2.0, &full), Some(0)); // clamps high
        assert_eq!(eq_sprite_index(-2.0, &full), Some(27)); // clamps low
        assert_eq!(eq_sprite_index(0.5, &[0, 27]), Some(0)); // snap to nearer
        assert_eq!(eq_sprite_index(-0.5, &[0, 27]), Some(27));
        assert_eq!(eq_sprite_index(0.0, &[]), None); // no bank, so thumb only
    }

    #[test]
    fn signed_time_elapsed_and_remaining() {
        assert_eq!(fmt_pl_time_signed(71.0, 200.0, false), "1:11");
        assert_eq!(fmt_pl_time_signed(71.0, 200.0, true), "-2:09");
        assert_eq!(fmt_pl_time_signed(71.0, 0.0, true), "1:11"); // no duration, so elapsed
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
        // Elapsed: 71s to 1:11, not flagged remaining.
        assert_eq!(st(71.0, 200.0, false).lcd_mmss(), (1, 11, false));
        // Remaining: 200 - 71 = 129s to 2:09, flagged remaining.
        assert_eq!(st(71.0, 200.0, true).lcd_mmss(), (2, 9, true));
        // Remaining is suppressed when the duration is unknown.
        assert_eq!(st(71.0, 0.0, true).lcd_mmss(), (1, 11, false));
        // Past the end never shows a negative; it clamps to 0:00.
        assert_eq!(st(250.0, 200.0, true).lcd_mmss(), (0, 0, true));
        // Minutes are capped at 99 for the two-digit LCD (100:40 to 99:40).
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
    fn pl_max_height_caps_at_desktop_bottom() {
        // Desktop 1000px tall, window top at 200, scale 1: room is 800px.
        assert_eq!(pl_max_height(1000.0, 200.0, 1.0), Some(800.0));
        // Double-size (scale 2): the same 800pt of room is 400 skin px.
        assert_eq!(pl_max_height(1000.0, 200.0, 2.0), Some(400.0));
        // No room for even the minimum height, so None (leave the size alone), so a
        // window already near the bottom isn't forcibly shrunk mid-drag.
        assert_eq!(pl_max_height(1000.0, 950.0, 1.0), None);
        // Exactly the minimum is allowed.
        assert_eq!(pl_max_height(200.0 + PL_MIN_H, 200.0, 1.0), Some(PL_MIN_H));
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
            scroll_first: 0,
            total_rows: rows.len(),
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
        // Mini position/seek slider sits between the transport buttons and Restore.
        assert_eq!(main_shade_region_at(SHADE_POS_X, 6.0), Some(ShadeHit::Pos));
        assert_eq!(main_shade_region_at(235.0, 6.0), Some(ShadeHit::Pos));
        assert_eq!(
            main_shade_region_at(SHADE_POS_X + SHADE_POS_W - 1.0, 6.0),
            Some(ShadeHit::Pos)
        );
        // Just outside the slider: the gap before Restore is draggable title bar.
        assert_eq!(
            main_shade_region_at(SHADE_POS_X + SHADE_POS_W, 6.0),
            Some(ShadeHit::TitleBar)
        );
        assert_eq!(main_shade_region_at(SHADE_POS_X - 1.0, 6.0), Some(ShadeHit::TitleBar));
        // The EQ/playlist shade tester has no transport buttons there.
        assert_eq!(shade_region_at(180.0, 6.0), Some(ShadeHit::TitleBar));
        assert_eq!(shade_region_at(SHADE_POS_X, 6.0), Some(ShadeHit::TitleBar));
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
    fn eq_thumb_centres_on_the_groove() {
        // Regression for #149: the 11px slider thumb must sit on the groove every
        // skin draws in its EQMAIN fill sprites. The groove is centred at local
        // x ~6, and the thumb's visible centre lands at EQ_THUMB_INSET + 5, so
        // that must land within a pixel of the groove, otherwise the handle
        // drifts off the bar (obvious on crisp skins like MacOSXAqua / Osmosis).
        let lum = |p: &image::Rgba<u8>| {
            0.299 * p[0] as f32 + 0.587 * p[1] as f32 + 0.114 * p[2] as f32
        };
        // Weighted centre of the groove within a 14px fill-sprite cell, measured
        // as each column's deviation from the cell's edge luminance.
        let groove_centre = |e: &RgbaImage, col: u32| -> f32 {
            let (sx, sy) = (13 + col * 15, 164u32);
            let cols: Vec<f32> = (0..14)
                .map(|lx| {
                    let s: f32 = (8..55).map(|y| lum(e.get_pixel(sx + lx, sy + y))).sum();
                    s / 47.0
                })
                .collect();
            let edge = (cols[0] + cols[1] + cols[12] + cols[13]) / 4.0;
            let dev: Vec<f32> = cols.iter().map(|c| (c - edge).abs()).collect();
            let tot: f32 = dev.iter().sum::<f32>().max(1.0);
            dev.iter().enumerate().map(|(i, d)| i as f32 * d).sum::<f32>() / tot
        };
        let thumb_centre = EQ_THUMB_INSET as f32 + 5.0; // 11px thumb, visible centre at local 5
        for name in ["linamp_base_skin.wsz", "MacOSXAqua1-5.wsz", "Osmosis_2.wsz"] {
            let skin = load_skin(name);
            let e = skin.bmp("eqmain.bmp").unwrap();
            // Average a few fill levels so one noisy sprite can't skew it.
            let g = [3u32, 7, 11].iter().map(|&c| groove_centre(e, c)).sum::<f32>() / 3.0;
            assert!(
                (g - thumb_centre).abs() <= 1.0,
                "{name}: groove centre {g:.1} vs thumb centre {thumb_centre:.1}, thumb off the bar"
            );
        }
    }

    #[test]
    fn eq_shade_fallback_is_readable_with_visible_bars() {
        // A skin missing BOTH eq_ex.bmp and EQMAIN.BMP must still render the
        // collapsed EQ as a readable bar: a solid background plus contrasting
        // volume/balance thumbs and a close marker, never black-on-black (#18).
        let skin = ClassicSkin {
            bitmaps: HashMap::new(),
            eq_sprites: Vec::new(),
            text_bg: [20, 20, 40], // dark skin background
            regions: crate::region::parse(""),
            cursors: HashMap::new(),
        };
        let img = skin.compose_eq_shade(true, 0.5, 0.0);
        assert_eq!(img.dimensions(), (275, 14));
        // Fully opaque, no transparent void.
        assert!(img.pixels().all(|p| p[3] == 255), "shade bar must be opaque");
        // Background is the skin's text colour (sampled off the sliders).
        assert_eq!(*img.get_pixel(5, 2), Rgba([20, 20, 40, 255]));
        // Volume groove and close marker are drawn light so they contrast the dark
        // background (not black-on-black).
        assert!(img.get_pixel(70, 6)[0] > 100, "volume groove must contrast the bar");
        assert!(img.get_pixel(266, 5)[0] > 100, "close marker must contrast the bar");
    }

    #[test]
    fn eq_shade_fallback_contrasts_a_black_background() {
        // Pure-black text_bg is the worst case; controls must still be visible.
        let skin = ClassicSkin {
            bitmaps: HashMap::new(),
            eq_sprites: Vec::new(),
            text_bg: [0, 0, 0],
            regions: crate::region::parse(""),
            cursors: HashMap::new(),
        };
        let img = skin.compose_eq_shade(false, 0.0, 0.0);
        assert!(img.pixels().all(|p| p[3] == 255), "shade bar must be opaque");
        // The close marker is light on the black bar.
        assert!(img.get_pixel(266, 5)[0] > 100, "controls must contrast black");
    }

    #[test]
    fn fallback_slider_tracks_value_without_a_sprite() {
        // VOLUME.BMP / BALANCE.BMP absent: the synthesized slider still shows the
        // level, the bright thumb sits at the left for 0 and the right for 1,
        // with only the dim groove elsewhere (#18).
        let thumb = 200u8;
        let groove = 72u8;

        let mut lo = RgbaImage::new(275, 116);
        draw_fallback_slider(&mut lo, 107, 68, 0.0);
        assert_eq!(lo.get_pixel(107, 63)[0], thumb, "thumb at left for frac 0");
        assert_eq!(lo.get_pixel(107 + 67, 63)[0], groove, "groove at right for frac 0");

        let mut hi = RgbaImage::new(275, 116);
        draw_fallback_slider(&mut hi, 107, 68, 1.0);
        // thumb left edge = 107 + (68 - 14) = 161, matching the click-to-value map
        assert_eq!(hi.get_pixel(161, 63)[0], thumb, "thumb at right for frac 1");
        assert_eq!(hi.get_pixel(107, 63)[0], groove, "groove at left for frac 1");
    }

    #[test]
    fn topaz_segmented_meters_are_detected() {
        // Regression: segmented teal meters once read as blank, so synthesized.
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
