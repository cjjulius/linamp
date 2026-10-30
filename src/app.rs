//! The `eframe::App`: application state, the per-frame update loop and the
//! window focus / shutdown / double-size logic. The multi-window model, the
//! fingerprint-cached render pipeline and the platform FFI boundary are
//! documented in docs/ARCHITECTURE.md.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use egui::{Color32, Id, Key, RichText, Stroke};
use rustfft::{Fft, FftPlanner};

use crate::classic::{self, Btn, ClassicSkin, MainState, PlayUi};
use crate::config::{Prefs, Session};
use crate::dock::{
    dock_group_from_main, docked_group_below, nudge_onto, push_off, rects_docked, snap_rect,
};
use crate::library::Library;
use crate::player::{PlayState, Player};
use crate::playlist::{save_playlist_file, Playlist};
use crate::skin::{self, Theme};
use crate::text::{fmt_time, marquee, scroll_text};
use crate::viz::{self, VizShared};

mod tag_ui;
mod menu;
mod library_ui;
mod modern;
mod theme;
mod input;
mod platform_win;
mod window;
mod render;
pub(crate) use platform_win::*;
use theme::{apply_theme, menu_palette};

/// Point size for menu text, shared by the egui classic menus and the native
/// tray menu (#40) so every menu renders at the same size. This matches egui's
/// default `Button` text style, so the classic menus look unchanged.
pub(crate) const MENU_FONT_PT: f32 = 12.5;
use menu::Popup;

const FFT_SIZE: usize = 2048;
const VIZ_BANDS: usize = 28;
/// Fewer, wider bars for the small classic analyzer (classic-style).
const CLASSIC_BANDS: usize = 19;

/// A right-click Cut / Copy / Paste / Select all menu for a single-line text
/// buffer, attached to a `TextEdit`'s response. egui has no built-in text context
/// menu and no API to read the clipboard on demand, and not everyone uses Ctrl+C/V.
/// Clipboard access goes through arboard (the crate eframe already uses); the
/// cut/paste position comes from the field's own cursor state.
fn text_edit_context_menu(resp: &egui::Response, ctx: &egui::Context, buf: &mut String) {
    use egui::text::{CCursor, CCursorRange};
    use egui::text_edit::TextEditState;

    // The current selection as a sorted char range, if the field has focus/state.
    let sel = TextEditState::load(ctx, resp.id)
        .and_then(|s| s.cursor.char_range())
        .map(|r| {
            let [a, b] = r.sorted();
            (a.index, b.index)
        });
    let has_sel = sel.map(|(a, b)| a != b).unwrap_or(false);
    // Char index to byte offset into `buf` (cursor indices are chars, not bytes).
    let byte_at = |s: &str, chars: usize| {
        s.char_indices().nth(chars).map(|(i, _)| i).unwrap_or(s.len())
    };
    let set_cursor = |ctx: &egui::Context, idx: usize| {
        if let Some(mut st) = TextEditState::load(ctx, resp.id) {
            st.cursor.set_char_range(Some(CCursorRange::one(CCursor::new(idx))));
            st.store(ctx, resp.id);
        }
    };

    resp.context_menu(|ui| {
        if ui.add_enabled(has_sel, egui::Button::new("Cut")).clicked() {
            if let Some((a, b)) = sel {
                let (ba, bb) = (byte_at(buf, a), byte_at(buf, b));
                // Copy via eframe's clipboard (persists reliably, unlike a one-shot
                // arboard owner on X11); arboard is only used to read on Paste.
                ui.ctx().copy_text(buf[ba..bb].to_string());
                buf.replace_range(ba..bb, "");
                set_cursor(ui.ctx(), a);
            }
            ui.close_menu();
        }
        if ui.add_enabled(has_sel, egui::Button::new("Copy")).clicked() {
            if let Some((a, b)) = sel {
                let (ba, bb) = (byte_at(buf, a), byte_at(buf, b));
                ui.ctx().copy_text(buf[ba..bb].to_string());
            }
            ui.close_menu();
        }
        if ui.button("Paste").clicked() {
            if let Ok(text) = arboard::Clipboard::new().and_then(|mut c| c.get_text()) {
                // Single-line field: drop any newlines a pasted value carries.
                let text: String = text.chars().filter(|c| *c != '\n' && *c != '\r').collect();
                let count = buf.chars().count();
                let (a, b) = sel.unwrap_or((count, count));
                let (ba, bb) = (byte_at(buf, a), byte_at(buf, b));
                buf.replace_range(ba..bb, &text);
                set_cursor(ui.ctx(), a + text.chars().count());
            }
            ui.close_menu();
        }
        if ui.button("Select all").clicked() {
            let count = buf.chars().count();
            if let Some(mut st) = TextEditState::load(ui.ctx(), resp.id) {
                st.cursor.set_char_range(Some(CCursorRange::two(
                    CCursor::new(0),
                    CCursor::new(count),
                )));
                st.store(ui.ctx(), resp.id);
            }
            ui.close_menu();
        }
    });
}

/// Register a broad-Unicode fallback font so egui-rendered text (the Media
/// Library, the modern UI, dialogs) can display scripts the default font lacks, 
/// example: Vietnamese. It is appended as a fallback, so the default look is kept for
/// characters it already covers; only missing glyphs fall through to Noto Sans.
/// The classic skin windows draw text from the skin's own bitmap font and are
/// unaffected by this.
fn install_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    fonts.font_data.insert(
        "noto_sans".to_owned(),
        egui::FontData::from_static(include_bytes!("../assets/fonts/NotoSans-Regular.ttf")),
    );
    for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
        fonts
            .families
            .entry(family)
            .or_default()
            .push("noto_sans".to_owned());
    }
    ctx.set_fonts(fonts);
}

/// Open the folder containing `path` in the system file manager.
fn open_in_file_manager(path: &std::path::Path) {
    let dir = path.parent().unwrap_or(path);
    #[cfg(target_os = "linux")]
    let cmd = "xdg-open";
    #[cfg(target_os = "macos")]
    let cmd = "open";
    #[cfg(target_os = "windows")]
    let cmd = "explorer";
    #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
    let _ = std::process::Command::new(cmd).arg(dir).spawn();
}

/// The bundled demo `music/` folder (next to the exe for an installed build, or
/// in the working directory for a dev run), if present. Used only on first run.
fn first_run_music_dir() -> Option<std::path::PathBuf> {
    // Prefer the copy next to the executable (mirrored there at build time, or
    // installed alongside it): a dev or release run then reads its own copy and
    // never the pristine repo music/ folder. Fall back to a working-directory
    // "music" only when there's no sibling copy.
    let mut dirs = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(d) = exe.parent() {
            dirs.push(d.join("music"));
        }
    }
    dirs.push(std::path::PathBuf::from("music"));
    dirs.into_iter().find(|d| d.is_dir())
}

fn win_idx(w: ClWin) -> usize {
    match w {
        ClWin::Main => 0,
        ClWin::Eq => 1,
        ClWin::Pl => 2,
    }
}

fn idx_win(i: usize) -> ClWin {
    match i {
        0 => ClWin::Main,
        1 => ClWin::Eq,
        _ => ClWin::Pl,
    }
}

/// Core of "Play Next": move the rows in `sel` to sit just after `current` (or to
/// the front when nothing is playing), preserving their ascending order. Mutates
/// `tracks` and returns the current track's new index, the index where the moved
/// block starts, and how many rows actually moved (duplicates/out-of-range
/// ignored) so the caller can re-select the block. Pure, single-pass, and
/// index-based so it can be unit-tested.
fn reorder_play_next<T>(
    tracks: &mut Vec<T>,
    current: Option<usize>,
    sel: &[usize],
) -> (Option<usize>, usize, usize) {
    let n = tracks.len();
    let mut want = vec![false; n];
    let mut n_sel = 0usize;
    for &i in sel {
        if i < n && !std::mem::replace(&mut want[i], true) {
            n_sel += 1;
        }
    }
    if n_sel == 0 {
        return (current, 0, 0);
    }
    let cur_selected = current.map(|c| want[c]).unwrap_or(false);
    // One pass: split into the moved block (selected rows, in order) and the kept
    // rows, tracking where the surviving current track lands among the kept.
    let mut moved = Vec::with_capacity(n_sel);
    let mut kept: Vec<T> = Vec::with_capacity(n - n_sel);
    let mut cur_in_kept = None;
    for (i, t) in std::mem::take(tracks).into_iter().enumerate() {
        if want[i] {
            moved.push(t);
        } else {
            if Some(i) == current {
                cur_in_kept = Some(kept.len());
            }
            kept.push(t);
        }
    }
    // Insert after the current track, or at the front when it's gone/absent.
    let insert_at = if cur_selected {
        0
    } else {
        cur_in_kept.map(|c| c + 1).unwrap_or(0)
    };
    let tail = kept.split_off(insert_at);
    kept.extend(moved);
    kept.extend(tail);
    *tracks = kept;
    let final_cur = if cur_selected {
        // The current track moved to the front block; its offset there is its rank
        // among the selected rows.
        Some(want[..current.unwrap()].iter().filter(|&&x| x).count())
    } else {
        // Inserting right after the kept current track leaves its index unchanged.
        cur_in_kept
    };
    (final_cur, insert_at, n_sel)
}

/// If `r` overlaps `keep` (the main window), the position `r` should move to so it
/// no longer covers it, pushed to `keep`'s nearest edge (least movement). `None`
/// when they don't overlap (flush/touching counts as not overlapping).
#[derive(Clone, Copy, PartialEq, Eq)]
enum UiMode {
    Classic,
    Modern,
}

/// Which classic window a pointer gesture is on.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ClWin {
    Main,
    Eq,
    Pl,
}

/// An in-progress title-bar drag of a classic window. On Windows (global cursor
/// readable) we move it each frame so docked windows track live; elsewhere we
/// fall back to the OS window-drag. `win_pos` is authoritative, snapped on release.
struct Dragging {
    win: ClWin,
    /// true = manual per-frame move (live tandem); false = OS drag fallback.
    manual: bool,
    /// Manual: offset of the global cursor from the window origin (points).
    grab: egui::Vec2,
    /// Window position at press (to tell a click from a drag).
    press_pos: egui::Pos2,
    /// OS fallback: screen pos at press (drag threshold) + whether started.
    press_screen: egui::Pos2,
    started_os: bool,
    moved: bool,
    /// Docked windows that move with this one, by offset from it.
    group: Vec<(usize, egui::Vec2)>,
    /// When the main window carries a docked library, its offset from main so it
    /// rides along too (the library lives outside `win_pos`).
    lib_offset: Option<egui::Vec2>,
}

const EQ_LABELS: [&str; 10] = [
    "60", "170", "310", "600", "1K", "3K", "6K", "12K", "14K", "16K",
];

#[derive(Clone, Copy, PartialEq, Eq)]
enum View {
    Playlist,
    Library,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SortKey {
    Custom,
    Title,
    Artist,
    Album,
    Duration,
    Filename,
    Path,
}

impl SortKey {
    fn label(self) -> &'static str {
        match self {
            SortKey::Custom => "Manual order",
            SortKey::Title => "Title",
            SortKey::Artist => "Artist",
            SortKey::Album => "Album",
            SortKey::Duration => "Duration",
            SortKey::Filename => "Filename",
            SortKey::Path => "Path",
        }
    }
}

/// Lowercased file name of a path, for case-insensitive filename sorting.
fn file_name_key(p: &std::path::Path) -> String {
    p.file_name()
        .map(|n| n.to_string_lossy().to_lowercase())
        .unwrap_or_default()
}

/// Categories in the Preferences window's left sidebar. The classic player groups
/// its many options in a tree; LinAmp uses a flat category list that the
/// buildout can extend (Title display, Audio, Skins, File types, Library…).
#[derive(Clone, Copy, PartialEq, Eq)]
enum PrefTab {
    Display,
    Playback,
    Conversion,
    Playlist,
    Tags,
    OptionalFeatures,
    About,
}

impl PrefTab {
    const ALL: [PrefTab; 7] = [
        PrefTab::Display,
        PrefTab::Playback,
        PrefTab::Conversion,
        PrefTab::Playlist,
        PrefTab::Tags,
        PrefTab::OptionalFeatures,
        PrefTab::About,
    ];
    fn label(self) -> &'static str {
        match self {
            PrefTab::Display => "Display",
            PrefTab::Playback => "Playback",
            PrefTab::Conversion => "Conversion",
            PrefTab::Playlist => "Playlist",
            PrefTab::Tags => "Tags",
            PrefTab::OptionalFeatures => "Optional Features",
            PrefTab::About => "About",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum VizMode {
    Spectrum,
    Oscilloscope,
    Off,
}

impl VizMode {
    fn next(self) -> Self {
        match self {
            VizMode::Spectrum => VizMode::Oscilloscope,
            VizMode::Oscilloscope => VizMode::Off,
            VizMode::Off => VizMode::Spectrum,
        }
    }
}

/// Equalizer state, grouped so an EQ change touches one field of `LinAmpApp`
/// instead of several scattered ones (the first step of decomposing the app
/// state, see docs/CODE-REVIEW.md C1).
struct EqState {
    enabled: bool,
    preamp_db: f32,
    bands: [f32; 10],
    auto: bool,
    /// The EQ slider / region currently held (classic view).
    active: Option<classic::EqRegion>,
    /// User-named EQ presets, persisted to eq_presets.json.
    presets: crate::config::EqPresets,
    /// When `Some`, the "Save EQ preset as…" name dialog is open with this buffer.
    save_name: Option<String>,
}

/// Per-window classic-render caches (composite textures, fingerprints, X11 shape
/// masks and cursor state), grouped so the render pipeline's state lives in one
/// place (C1).
struct ClassicRenderState {
    classic_tex: Option<egui::TextureHandle>,
    eq_tex: Option<egui::TextureHandle>,
    pl_tex: Option<egui::TextureHandle>,
    /// Per-window (main/eq/pl) X11 shape fingerprint, so REGION.TXT masks are only
    /// re-applied on change.
    shape_sig: [Option<u64>; 3],
    /// Last custom cursor name applied per window (main/eq/pl), to switch the X
    /// cursor only when the hovered region changes.
    cur_cursor: [Option<&'static str>; 3],
    /// Last composed fingerprint per window, to skip re-compositing an unchanged frame.
    tex_fp: [u64; 3],
    active_region: Option<Btn>,
    pressed_btn: Option<Btn>,
}

/// Classic playlist view + selection state (C1).
struct PlaylistViewState {
    /// Index of the first visible row (scroll position).
    first: usize,
    selection: HashSet<usize>,
    /// Fixed end for shift-range selection; set by a plain/ctrl click or nav.
    anchor: Option<usize>,
    /// The focus ("lead") row keyboard navigation moves from; the highlight follows
    /// it along with the rest of the selection.
    lead: Option<usize>,
    last_click: Option<(usize, Instant)>,
    /// The row a playlist right-click targeted, so the track menu's Play / Edit act
    /// on exactly that row rather than the selection's min / current track.
    ctx_row: Option<usize>,
    /// Playlist window size in unscaled skin pixels.
    px_w: f32,
    px_h: f32,
    /// Bottom-bar menu button currently held / with its menu open.
    menu_press: Option<classic::PlRegion>,
}

/// Media Library 3-pane browser, search, sort, selection and background-scan
/// state (C1). The library database itself is `LinAmpApp::library`.
struct LibraryViewState {
    query: String,
    /// When set, a search is pending: run it once the clock passes this instant,
    /// so fast typing coalesces into one query instead of a LIKE scan per
    /// keystroke (#96).
    search_at: Option<std::time::Instant>,
    results: Vec<crate::playlist::Track>,
    /// Star rating (0-5) for each row in `results`, parallel-indexed. Only
    /// populated for the Top Rated view, where the list shows the stars.
    ratings: Vec<i64>,
    /// Cached (path, rating) for the selected row, so the bottom bar's star
    /// display doesn't query SQLite every frame.
    rating_cache: Option<(PathBuf, i64)>,
    /// User-resizable track-table column fractions [artist, title]; the third
    /// column (album/rating) takes the remainder.
    col_fr: [f32; 2],
    /// Active track-table sort: (column 0=artist 1=title 2=album/rating 3=time,
    /// ascending). None = the view's natural order.
    sort: Option<(u8, bool)>,
    nav: crate::library::LibView,
    artists: Vec<(String, i64)>,
    albums: Vec<(String, i64)>,
    sel_artists: Vec<String>,
    sel_albums: Vec<String>,
    sel_tracks: std::collections::BTreeSet<usize>,
    /// Active background scan receiver + its latest progress text.
    scan: Option<std::sync::mpsc::Receiver<crate::library::ScanMsg>>,
    scan_text: String,
    /// The conversion queue shown in the "Converting" node: active and finished jobs,
    /// in enqueue order. The node appears in the nav only while this is non-empty.
    convert_queue: Vec<crate::convert_queue::ConvertJob>,
    /// Monotonic id for the next queued conversion, matching jobs to worker messages.
    convert_next_id: u64,
}

/// Window geometry and focus (C1): the classic windows' positions, the library
/// window's position/drag, drag/resize state, window-shade flags and per-window
/// focus.
struct WindowGeom {
    /// Top-left of each classic window (main/eq/pl).
    win_pos: [egui::Pos2; 3],
    positions_init: bool,
    /// true when the last session's positions were restored, so frame 1 keeps them
    /// (clamped on-screen) instead of applying the default layout.
    has_pos_memory: bool,
    /// Media Library window position, whether it has been placed, and its
    /// title-bar grab offset while dragging.
    lib_pos: egui::Pos2,
    lib_pos_init: bool,
    lib_drag: Option<egui::Vec2>,
    /// Window-shade (collapsed) flags per classic window.
    main_shade: bool,
    eq_shade: bool,
    pl_shade: bool,
    /// App-level focus (true while any LinAmp window is focused) plus per-window
    /// focus, OR'd each frame.
    app_focused: bool,
    foc_root: bool,
    foc_eq: bool,
    foc_pl: bool,
    foc_lib: bool,
    /// Active window drag, and the playlist resize grab offsets (width, height) px.
    drag: Option<Dragging>,
    resize_pl: Option<egui::Vec2>,
    /// The root window's last (pos, size), for drag tracking.
    root_last: Option<(egui::Pos2, egui::Vec2)>,
}

/// Classic right-click / options popup state (C1): the open popup, a queued one,
/// and the rounded-region / fresh-window bookkeeping that keeps each menu sized
/// correctly.
struct PopupState {
    /// The currently open popup (kind + absolute screen anchor of its top-left).
    cur: Option<(Popup, egui::Pos2)>,
    /// A popup to open once the current one closes (classic popups aren't nested).
    pending: Option<(Popup, egui::Pos2)>,
    /// Rows for an open dynamic menu (`Popup::LibMenu`), built per click because
    /// the library menus carry per-row names and the live selection.
    dynamic: Option<Vec<library_ui::DynRow>>,
    /// Library rows the Rate flyout (`Popup::LibRate`) acts on.
    rate_rows: Vec<usize>,
    /// Item rows for a generic flyout (`Popup::LibSub`), e.g. the Convert formats.
    sub_rows: Vec<library_ui::DynRow>,
    /// The open one-level flyout (the Rate submenu): its kind and absolute screen
    /// anchor. Drawn in a second viewport beside the parent, which stays open.
    sub: Option<(Popup, egui::Pos2)>,
    /// Opening frame for the flyout window (mirrors `fresh` for the parent).
    sub_fresh: bool,
    /// Whether the (persistent) flyout window is currently shown. Once a flyout is
    /// opened, its OS window is kept alive for the rest of the menu session and only
    /// toggled visible/hidden as the pointer moves on and off the submenu rows, so it
    /// is never recreated per hover (recreating it flashed a title bar on Windows).
    sub_visible: bool,
    /// Window height the flyout's rounded region was last built for.
    sub_rounded_h: Option<f32>,
    fresh: bool,
    /// Whether the popup window's rounded region has been applied for the current
    /// popup (applied once, retrying only until it succeeds). The window stays hidden
    /// until this is set so its clipped-away title bar is never revealed.
    rounded: bool,
    /// Window height the rounded-corner region was last built for, so it rebuilds
    /// only when a different-sized menu opens (#154).
    rounded_h: Option<f32>,
    /// Bumped on every popup open so each menu gets a fresh viewport id/window (#154).
    serial: u64,
}

/// What a running fade-out settles into when it reaches silence: a full stop, or
/// a pause (so pausing can fade out like stopping does, instead of cutting) (#171).
#[derive(Clone, Copy, PartialEq, Eq)]
enum FadeEnd {
    Stop,
    Pause,
}

pub struct LinAmpApp {
    player: Player,
    playlist: Playlist,
    library: Option<Library>,

    shuffle: bool,
    repeat_all: bool,
    repeat_one: bool,

    // seek bar drag state
    seeking: bool,
    seek_preview: f32,

    // equalizer
    eq: EqState,

    // playlist / library views
    view: View,
    pl_search: String,
    sort_key: SortKey,
    /// Media Library 3-pane browser, search, sort, selection and scan state.
    lib: LibraryViewState,
    /// Window geometry and focus (positions, drag/resize, shades, per-window focus).
    geom: WindowGeom,

    // visualization
    viz: Arc<VizShared>,
    viz_mode: VizMode,
    viz_bars: Vec<f32>,
    /// Classic analyzer bar heights + slowly-falling peak markers.
    cviz_bars: Vec<f32>,
    cviz_peaks: Vec<f32>,
    viz_samples: Vec<f32>,
    fft: Arc<dyn Fft<f32>>,

    // album art
    album_art: Option<egui::TextureHandle>,
    art_bytes: Option<Vec<u8>>,
    art_dirty: bool,

    // theming / skins
    theme: Theme,
    skins: Vec<PathBuf>,

    // classic skin renderer
    ui_mode: UiMode,
    classic: Option<ClassicSkin>,
    /// Per-window composite textures, fingerprints, shape masks and cursor caches.
    render: ClassicRenderState,
    /// Composed GEN.BMP library-window frame + a fingerprint (skin, active, size)
    /// so it is only re-composited when something changes.
    lib_tex: Option<egui::TextureHandle>,
    lib_tex_sig: Option<(String, bool, u32, u32)>,
    /// Whether any X11 shape was ever set, to reset windows to rectangular when
    /// switching to a skin without regions.
    shaped_any: bool,
    classic_seek_preview: Option<f32>,
    balance: f32,
    time_remaining: bool,
    /// True on the "off" half of the paused-time blink (the classic player flashes
    /// the time while paused). Recomputed each frame in `update`.
    time_blink_hidden: bool,
    /// Stop when the current track ends instead of advancing (Ctrl+V).
    stop_after_current: bool,
    /// Remaining fade-out time in seconds and what the fade ends in (stop or
    /// pause) while fading out (Shift+V, or the "fade out when stopping or
    /// pausing" preference).
    fade: Option<(f32, FadeEnd)>,
    mode_init: bool,
    eq_visible: bool,
    pl_visible: bool,
    /// Classic playlist view + selection state.
    plview: PlaylistViewState,
    /// Classic right-click / options popup state (open popup, queued popup, the
    /// rounded-region and fresh-window bookkeeping).
    popups: PopupState,
    /// Alt+I bookmarks, persisted to bookmarks.json.
    bookmarks: crate::config::Bookmarks,
    tb_last_click: Option<(ClWin, Instant)>,
    /// Hash of the last-persisted session, so we only write on actual changes.
    session_sig: Option<u64>,
    /// Frames remaining in a graceful shutdown (set when the WM asks to close),
    /// used to tear down child viewports before the root to avoid a winit crash.
    quit_countdown: Option<u8>,
    /// Last always-on-top state pushed to the root window (so we only send the
    /// window-level command when it changes).
    on_top_last: Option<bool>,
    prefs: Prefs,
    /// The background conversion worker (the rip/convert queue). Created lazily on the
    /// first conversion; its progress drains into `lib.convert_queue` each frame.
    convert_worker: Option<crate::convert_queue::ConvertWorker>,
    /// Whether this process is the primary instance (holds the instance lock). Only
    /// the primary drains the open-request queue, so files handed over by other
    /// launches always land in one deterministic instance (#193).
    is_primary: bool,
    /// The instance lock, held once this process has become primary by taking it over
    /// (the launch-time primary holds its own copy in `main`). Keeping the handle alive
    /// holds the lock; the OS frees it when this instance exits so the next can take over.
    instance_lock: Option<std::fs::File>,
    /// Where the instance lock lives, so a secondary can poll to take over if the primary exits.
    lock_path: std::path::PathBuf,
    /// Last time a secondary polled the instance lock to take over the primary role (#193).
    last_lock_try: Instant,
    /// System-tray icon (kept alive for its lifetime) and the channel its clicks
    /// arrive on, drained once per frame (#40). `hidden_to_tray` tracks whether
    /// the window is currently hidden so a tray left-click toggles it.
    _tray: crate::tray::Tray,
    tray_rx: std::sync::mpsc::Receiver<crate::tray::TrayAction>,
    /// Global media-key listener (kept alive for its lifetime) and the channel
    /// its presses arrive on (#165). Reuses `TrayAction`, so presses route
    /// through the same `handle_tray_action` as the tray's transport buttons.
    _media_keys: crate::mediakeys::MediaKeys,
    media_keys_rx: std::sync::mpsc::Receiver<crate::tray::TrayAction>,
    hidden_to_tray: bool,
    /// Time of the last tray-icon toggle, to debounce a double-click. A desktop that
    /// reports Activate for both clicks would otherwise toggle twice (hide then show,
    /// a net no-op), so a second toggle in quick succession is ignored.
    last_tray_toggle: Option<Instant>,
    /// Visibility of the (eq, playlist, library) windows before hiding to the
    /// tray, so restoring brings back exactly the windows that were open.
    tray_prev_vis: (bool, bool, bool),
    /// Whether this session runs in System tray mode: the pref, gated on the tray
    /// actually running. Drives whether minimize and the close (X) button hide to
    /// the tray instead of their usual behaviour. Set at launch and updated live
    /// by `set_window_mode` when the pref changes.
    tray_mode: bool,
    /// Startup deadline for re-applying the X11 taskbar-skip hint (Linux). winit's
    /// `with_taskbar(false)` doesn't set `_NET_WM_STATE_SKIP_TASKBAR` on X11, so in
    /// tray mode we send the EWMH hint ourselves. The window isn't in the WM's client
    /// list the instant we launch, so we re-send (keeping the event loop awake) until
    /// this deadline, which reliably beats the race. `None` once done or in Normal mode.
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    tray_taskbar_deadline: Option<Instant>,
    /// Set by a genuine quit (main-menu Exit or the tray's Exit) so the close
    /// handler really quits instead of hiding to the tray in System tray mode.
    force_quit: bool,
    /// Path of the currently loaded skin (persisted as `last_skin`).
    skin_path: Option<PathBuf>,
    scale: f32,
    prefs_open: bool,
    /// Which Preferences category is selected in the sidebar.
    prefs_tab: PrefTab,
    /// Preferences window position (None when closed, so it re-anchors to the
    /// main window on open) and the grab offset while its title bar is dragged.
    prefs_pos: Option<egui::Pos2>,
    prefs_drag: Option<egui::Vec2>,
    lib_visible: bool,
    /// Whether the Minibrowser window (a themed nod to the classic player's SHOUTcast pane)
    /// is open. Transient, it opens from the Options menu each session.
    mb_visible: bool,
    tag_editor: Option<tag_ui::TagEditor>,
    /// Jump-to-file search state (Some while the dialog is open).
    jump_file: Option<JumpFile>,
    /// Jump-to-time input buffer (Some while the dialog is open).
    jump_time: Option<String>,
    /// Open-location (stream URL) input buffer (Some while the dialog is open).
    location_entry: Option<String>,
    /// A stream being opened off the UI thread: (pending, url, started-at). Polled
    /// each frame until the format is known, then handed to the player.
    pending_stream: Option<(crate::stream::Pending, String, Instant)>,

    marquee_offset: f32,
    last_frame: Instant,
    last_device_check: Instant,
    last_session_check: Instant,
    /// Throttle for polling the cross-instance "Open with" queue (#166).
    last_open_check: Instant,
    status: String,
}

/// Jump-to-file dialog state: the search query and the highlighted result row.
#[derive(Default)]
struct JumpFile {
    query: String,
    sel: usize,
}

/// Parse a time string ("m:ss", "mm:ss" or plain seconds) into seconds.
fn parse_time(s: &str) -> Option<f32> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    if let Some((m, sec)) = s.split_once(':') {
        let m: f32 = m.trim().parse().ok()?;
        let sec: f32 = sec.trim().parse().ok()?;
        if sec < 0.0 || m < 0.0 {
            return None;
        }
        Some(m * 60.0 + sec)
    } else {
        s.parse::<f32>().ok().filter(|v| *v >= 0.0)
    }
}

/// Whether a near-end crossfade should begin this frame. Pure, so the gate is
/// unit-tested without a live Player. It holds back when crossfade is off, the
/// player is not playing, a crossfade is already running, a one-shot mode is set
/// (stop-after-current or repeat-one), or a stop-fade is in progress (the user is
/// stopping, so it must not start the next track). Otherwise it fires once the
/// position reaches the crossfade window before the end, and skips tracks shorter
/// than the fade so they still play in full.
#[allow(clippy::too_many_arguments)]
fn crossfade_due(
    secs: f32,
    playing: bool,
    is_crossfading: bool,
    stop_after_current: bool,
    repeat_one: bool,
    stop_fading: bool,
    duration: Option<f32>,
    position: f32,
) -> bool {
    if secs <= 0.0
        || !playing
        || is_crossfading
        || stop_after_current
        || repeat_one
        || stop_fading
    {
        return false;
    }
    match duration {
        Some(d) if d > secs => position >= d - secs,
        _ => false,
    }
}

impl LinAmpApp {
    pub fn new(cc: &eframe::CreationContext<'_>, is_primary: bool) -> Self {
        install_fonts(&cc.egui_ctx);
        let player = Player::new();
        let viz = player.viz();
        let library = Library::open().ok();
        let fft = FftPlanner::<f32>::new().plan_fft_forward(FFT_SIZE);
        let prefs = Prefs::load();
        let scale = prefs.scale();
        let (tray, tray_rx) = crate::tray::Tray::spawn(&cc.egui_ctx, prefs.tray_enabled);
        // Global media keys (#165) are independent of tray mode, so they always
        // spawn; the listener is a no-op where unsupported.
        let (media_keys, media_keys_rx) = crate::mediakeys::MediaKeys::spawn(&cc.egui_ctx);
        // Run in tray mode only when the pref asks for it and the tray actually
        // started, so we never hide the window to a tray that isn't there.
        let tray_mode = prefs.tray_enabled && tray.is_active();

        let mut app = Self {
            player,
            playlist: Playlist::default(),
            library,
            shuffle: false,
            repeat_all: true,
            repeat_one: false,
            seeking: false,
            seek_preview: 0.0,
            eq: EqState {
                enabled: false,
                preamp_db: 0.0,
                bands: [0.0; 10],
                auto: false,
                active: None,
                presets: crate::config::EqPresets::load(),
                save_name: None,
            },
            view: View::Playlist,
            pl_search: String::new(),
            sort_key: SortKey::Custom,
            lib: LibraryViewState {
                query: String::new(),
                search_at: None,
                results: Vec::new(),
                ratings: Vec::new(),
                rating_cache: None,
                col_fr: [0.28, 0.42],
                sort: None,
                nav: crate::library::LibView::Audio,
                artists: Vec::new(),
                albums: Vec::new(),
                sel_artists: Vec::new(),
                sel_albums: Vec::new(),
                sel_tracks: std::collections::BTreeSet::new(),
                scan: None,
                scan_text: String::new(),
                convert_queue: Vec::new(),
                convert_next_id: 0,
            },
            geom: WindowGeom {
                win_pos: [egui::pos2(100.0, 100.0); 3],
                positions_init: false,
                has_pos_memory: false,
                lib_pos: egui::Pos2::ZERO,
                lib_pos_init: false,
                lib_drag: None,
                main_shade: false,
                eq_shade: false,
                pl_shade: false,
                app_focused: true,
                foc_root: true,
                foc_eq: false,
                foc_pl: false,
                foc_lib: false,
                drag: None,
                resize_pl: None,
                root_last: None,
            },
            lib_tex: None,
            lib_tex_sig: None,
            shaped_any: false,
            viz,
            viz_mode: VizMode::Spectrum,
            viz_bars: vec![0.0; VIZ_BANDS],
            cviz_bars: vec![0.0; CLASSIC_BANDS],
            cviz_peaks: vec![0.0; CLASSIC_BANDS],
            viz_samples: Vec::new(),
            fft,
            album_art: None,
            art_bytes: None,
            art_dirty: false,
            theme: Theme::default(),
            skins: Vec::new(),
            ui_mode: UiMode::Modern,
            classic: None,
            render: ClassicRenderState {
                classic_tex: None,
                eq_tex: None,
                pl_tex: None,
                shape_sig: [None; 3],
                cur_cursor: [None; 3],
                tex_fp: [0; 3],
                active_region: None,
                pressed_btn: None,
            },
            classic_seek_preview: None,
            balance: 0.0,
            time_remaining: false,
            time_blink_hidden: false,
            stop_after_current: false,
            fade: None,
            mode_init: true,
            eq_visible: false,
            pl_visible: false,
            plview: PlaylistViewState {
                first: 0,
                selection: HashSet::new(),
                anchor: None,
                lead: None,
                last_click: None,
                ctx_row: None,
                px_w: classic::MAIN_W as f32,
                px_h: classic::PL_H as f32,
                menu_press: None,
            },
            popups: PopupState {
                cur: None,
                pending: None,
                dynamic: None,
                rate_rows: Vec::new(),
                sub_rows: Vec::new(),
                sub: None,
                sub_fresh: false,
                sub_visible: false,
                sub_rounded_h: None,
                fresh: false,
                rounded: false,
                rounded_h: None,
                serial: 0,
            },
            bookmarks: crate::config::Bookmarks::load(),
            tb_last_click: None,
            session_sig: None,
            quit_countdown: None,
            on_top_last: None,
            prefs,
            convert_worker: None,
            is_primary,
            instance_lock: None,
            lock_path: crate::instance_lock_path(),
            last_lock_try: Instant::now(),
            _tray: tray,
            tray_rx,
            _media_keys: media_keys,
            media_keys_rx,
            hidden_to_tray: false,
            last_tray_toggle: None,
            tray_prev_vis: (false, false, false),
            tray_mode,
            // Re-apply the taskbar-skip hint for a few seconds after launch (Linux),
            // until the window is in the WM's client list. Only when starting hidden
            // from the taskbar; a plain window needs nothing.
            tray_taskbar_deadline: tray_mode
                .then(|| Instant::now() + std::time::Duration::from_secs(3)),
            force_quit: false,
            skin_path: None,
            scale,
            prefs_open: false,
            prefs_tab: PrefTab::Display,
            prefs_pos: None,
            prefs_drag: None,
            lib_visible: false,
            mb_visible: false,
            tag_editor: None,
            jump_file: None,
            jump_time: None,
            location_entry: None,
            pending_stream: None,
            marquee_offset: 0.0,
            last_frame: Instant::now(),
            last_device_check: Instant::now(),
            last_session_check: Instant::now(),
            last_open_check: Instant::now(),
            status: "Ready. Press L to open files.".to_string(),
        };

        // A deviceless launch (no audio output device) still runs; say why playback
        // does nothing until one is connected. poll_device reacquires one later.
        if !app.player.has_device() {
            app.status =
                "No audio output device found; playback is off until one is connected.".into();
        }

        // Restore the last session's playlist, volume and window layout. This
        // runs whether or not files were passed on the command line, so "Open
        // with" can enqueue into the current playlist and keep the window layout;
        // a truly fresh install is handled by `first_run` below. Playback is
        // started (if at all) by the command-line / first-run logic that follows,
        // not here.
        {
            let first_run = !Session::exists();
            let session = Session::load();
            for t in &session.tracks {
                app.playlist.add_file(PathBuf::from(t));
            }
            app.playlist.current = session.current.filter(|&i| i < app.playlist.len());
            // Restore volume/balance; absent (older session) keeps the default.
            if let Some(v) = session.volume {
                app.player.set_volume(v.clamp(0.0, 1.0));
            }
            if let Some(b) = session.balance {
                app.balance = b.clamp(-1.0, 1.0);
                app.player.set_balance(app.balance);
            }
            if app.prefs.tag_on_load {
                app.playlist.load_all();
            }
            if let Some(w) = session.pl_width {
                app.plview.px_w = w.max(classic::MAIN_W as f32);
            }
            if let Some(h) = session.pl_height {
                app.plview.px_h = h.max(87.0);
            }
            // Restore which windows were open and their collapsed/full state.
            // The saved window positions (if any) are applied just below and
            // placed on frame 1; absent them, positions_init docks the windows.
            app.eq_visible = session.eq_visible;
            app.pl_visible = session.pl_visible;
            app.geom.main_shade = session.main_shade;
            app.geom.eq_shade = session.eq_shade;
            app.geom.pl_shade = session.pl_shade;
            app.lib_visible = session.lib_visible;
            if let Some(fr) = session.lib_col_fr {
                app.lib.col_fr = fr;
            }
            // Restore the saved window layout (absolute top-lefts). When present
            // it replaces the default dock arrangement; frame 1 clamps it on-
            // screen. Positions are absolute so docking is preserved exactly.
            if let Some((x, y)) = session.main_pos {
                let p = |o: Option<(f32, f32)>, d: egui::Pos2| {
                    o.map(|(x, y)| egui::pos2(x, y)).unwrap_or(d)
                };
                app.geom.win_pos[0] = egui::pos2(x, y);
                app.geom.win_pos[1] = p(session.eq_pos, app.geom.win_pos[0]);
                app.geom.win_pos[2] = p(session.pl_pos, app.geom.win_pos[0]);
                app.geom.lib_pos = p(session.lib_pos, app.geom.win_pos[0]);
                app.geom.lib_pos_init = true;
                app.geom.has_pos_memory = true;
            }

            // Files passed on the command line ("Open with", or files dropped on
            // the exe). The "Open with" pref decides whether they replace the
            // playlist (Clear) or add to it (Enqueue, the default); either way the
            // first opened item starts playing. When none were passed, a fresh
            // install cues the bundled demo instead.
            let cli: Vec<PathBuf> = std::env::args().skip(1).map(PathBuf::from).collect();
            if !cli.is_empty() {
                app.open_paths(&cc.egui_ctx, cli, false);
            } else if first_run {
                // Fresh install (no saved session): the classic trio open and
                // stacked (at the 1x default), the library closed and the bundled
                // demo song cued. The base skin loads by default.
                app.eq_visible = true;
                app.pl_visible = true;
                app.lib_visible = false;
                if let Some(dir) = first_run_music_dir() {
                    app.playlist.add_folder(&dir);
                    if app.prefs.tag_on_load {
                        app.playlist.load_all();
                    }
                    if !app.playlist.is_empty() {
                        app.play_index(0);
                    }
                }
            }
        }
        app.refresh_library();
        app.discover_skins();
        app.apply_crossfade_pref();

        // Restore equalizer state from the last session and push it to the DSP.
        let eq_session = Session::load();
        if !eq_session.eq_bands.is_empty() {
            app.eq.enabled = eq_session.eq_enabled;
            app.eq.auto = eq_session.eq_auto;
            app.eq.preamp_db = eq_session.eq_preamp;
            for (i, b) in eq_session.eq_bands.iter().take(10).enumerate() {
                app.eq.bands[i] = *b;
            }
            app.apply_eq();
        }

        // Open with the LinAmp-branded default skin (fall back to any skin found).
        let stem = |p: &std::path::Path| {
            p.file_stem().map(|s| s.to_string_lossy().to_lowercase()).unwrap_or_default()
        };
        let startup = app
            .prefs
            .last_skin
            .as_ref()
            .map(PathBuf::from)
            .filter(|p| p.exists())
            .or_else(|| app.skins.iter().find(|p| stem(p) == "linamp_base_skin").cloned())
            .or_else(|| app.skins.iter().find(|p| stem(p).contains("base")).cloned())
            .or_else(|| app.skins.first().cloned());
        if let Some(p) = startup {
            if let Ok(cs) = ClassicSkin::load(&p) {
                app.classic = Some(cs);
                if let Ok(t) = skin::load_wsz(&p) {
                    app.theme = t;
                }
                app.skin_path = Some(p.clone());
                app.ui_mode = UiMode::Classic;
            }
        }

        apply_theme(&cc.egui_ctx, &app.theme);
        app.push_tray_theme();
        app
    }

    /// Open files handed over by "Open with" (at launch, or forwarded from a
    /// second instance). Honors the `open_with` pref: Clear replaces the playlist,
    /// Enqueue (the default) adds to it. The first opened item starts playing.
    /// When `focus` is set (a forwarded open to the already-running instance), the
    /// window is surfaced so the user sees what they opened.
    fn open_paths(&mut self, ctx: &egui::Context, paths: Vec<PathBuf>, focus: bool) {
        if paths.is_empty() {
            return;
        }
        let clear = self.prefs.open_with == crate::config::OpenWith::Clear;
        if clear {
            self.playlist.clear();
        }
        // Where the opened items land, so playback starts on what was opened
        // rather than the top of an enqueued-into playlist.
        let first_new = self.playlist.len();
        for p in paths {
            if p.is_dir() {
                self.playlist.add_folder(&p);
            } else if matches!(
                p.extension().and_then(|e| e.to_str()).map(|e| e.to_lowercase()).as_deref(),
                Some("m3u") | Some("m3u8") | Some("pls")
            ) {
                self.playlist.load_file(&p);
            } else {
                self.playlist.add_file(p);
            }
        }
        if self.playlist.len() > first_new {
            // Read tags for just the opened items; the rest were already loaded at
            // restore (or load lazily), so don't re-scan the whole playlist.
            if self.prefs.tag_on_load {
                for i in first_new..self.playlist.len() {
                    self.playlist.ensure_loaded(i);
                }
            }
            self.play_index(first_new);
        } else if clear {
            // Clear mode but nothing openable was added: the playlist is now empty,
            // so stop the track that was playing instead of leaving it sounding.
            self.stop_playback();
        }
        if focus {
            // Surface the window for a forwarded open so the user sees the result.
            if self.hidden_to_tray {
                self.show_from_tray(ctx);
            } else {
                ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
                ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
            }
        }
    }

    // ---- transport helpers -------------------------------------------------

    /// Replace the playlist with `tracks` and start playing the first.
    fn play_tracks(&mut self, tracks: Vec<crate::playlist::Track>) {
        if tracks.is_empty() {
            return;
        }
        self.playlist.tracks = tracks;
        self.playlist.current = None;
        self.play_index(0);
    }

    /// Append `tracks` to the playlist without changing what's playing.
    fn enqueue_tracks(&mut self, tracks: Vec<crate::playlist::Track>) {
        if tracks.is_empty() {
            return;
        }
        self.playlist.tracks.extend(tracks);
        self.status = format!("{} track(s) in playlist.", self.playlist.len());
    }

    /// Play track `idx`, skipping forward over any that can't be opened/decoded
    /// (missing files, unsupported/corrupt audio) until one plays. Bounded to one
    /// pass over the playlist so an all-unplayable list can't loop forever.
    fn play_index(&mut self, idx: usize) {
        let len = self.playlist.len();
        if len == 0 {
            return;
        }
        let mut i = idx.min(len - 1);
        let mut skipped = 0;
        for _ in 0..len {
            self.playlist.ensure_loaded(i);
            let Some(track) = self.playlist.tracks.get(i).cloned() else {
                break;
            };
            match self.player.play_file(&track.path) {
                Ok(()) => {
                    self.playlist.current = Some(i);
                    // Scroll the playlist so the now-playing track is in view (it may
                    // sit outside the current scroll window, e.g. after replacing the
                    // list from the library).
                    self.pl_scroll_to(i);
                    self.seek_preview = 0.0;
                    self.marquee_offset = 0.0;
                    self.apply_eq();
                    self.art_bytes = crate::playlist::read_cover(&track.path);
                    self.art_dirty = true;
                    self.status = if skipped == 0 {
                        format!("Playing: {}", track.display_name())
                    } else {
                        format!("Playing: {} (skipped {skipped} unplayable)", track.display_name())
                    };
                    // Record the play for the library's Most/Recently Played views
                    // (no-op when the track isn't in the library).
                    if let Some(lib) = &self.library {
                        lib.increment_play(&track.path);
                    }
                    return;
                }
                // Can't open/decode this one, skip to the next (wrapping).
                Err(_) => {
                    skipped += 1;
                    i = (i + 1) % len;
                }
            }
        }
        // Nothing in the playlist could be played, just stop, silently (#128);
        // leave the prior status as-is rather than showing a message.
        self.player.stop();
        self.playlist.current = None;
    }

    fn play_pause(&mut self) {
        match self.player.state() {
            PlayState::Stopped => {
                if !self.playlist.is_empty() {
                    let start = self.playlist.current.unwrap_or(0);
                    self.play_index(start);
                }
            }
            // Pausing fades out (when the pref is on); resuming is immediate.
            PlayState::Playing => self.pause_playback(),
            PlayState::Paused => self.player.toggle_pause(),
        }
    }

    fn next(&mut self) {
        if let Some(i) = self.playlist.next_index(self.shuffle, self.repeat_all) {
            self.play_index(i);
        } else {
            self.player.stop();
            self.status = "End of playlist.".to_string();
        }
    }

    fn prev(&mut self) {
        if let Some(i) = self.playlist.prev_index(self.shuffle, self.repeat_all) {
            self.play_index(i);
        }
    }

    /// Act on a click from the system-tray icon (#40).
    fn handle_tray_action(&mut self, ctx: &egui::Context, action: crate::tray::TrayAction) {
        use crate::tray::TrayAction;
        match action {
            TrayAction::PlayPause => self.play_pause(),
            TrayAction::Stop => self.stop_playback(),
            TrayAction::Prev => self.prev(),
            TrayAction::Next => self.next(),
            TrayAction::Toggle => {
                // Collapse a double-click: some desktops deliver Activate for both
                // clicks, which would toggle twice (hide then show, a net no-op). A
                // second toggle within 400ms is ignored, so single and double click
                // both do one show/hide.
                let now = Instant::now();
                let recent = self
                    .last_tray_toggle
                    .is_some_and(|t| now.duration_since(t) < Duration::from_millis(400));
                if recent {
                    return;
                }
                self.last_tray_toggle = Some(now);
                if self.hidden_to_tray {
                    self.show_from_tray(ctx);
                } else {
                    self.hide_to_tray(ctx);
                }
            }
            TrayAction::Restore => self.show_from_tray(ctx),
            TrayAction::Quit => self.request_exit(ctx),
        }
    }

    /// Switch window mode live (#40): add or remove the tray icon, flip the
    /// taskbar button, and update the behaviour, so the choice applies without a
    /// restart. Windows toggles the taskbar button through ITaskbarList; X11 is
    /// best-effort.
    fn set_window_mode(&mut self, ctx: &egui::Context, enabled: bool) {
        self.prefs.tray_enabled = enabled;
        self.prefs.save();
        // Re-create the tray to match: spawning disabled drops the icon, enabled
        // adds it, and the old tray's Drop removes the previous icon.
        let (tray, rx) = crate::tray::Tray::spawn(ctx, enabled);
        self._tray = tray;
        self.tray_rx = rx;
        self.tray_mode = enabled && self._tray.is_active();
        // The fresh tray starts on default colours; push the current skin's.
        self.push_tray_theme();
        set_taskbar_button("LinAmp", !self.tray_mode);
        // Leaving tray mode while the window is hidden would strand it; bring it back.
        if !self.tray_mode && self.hidden_to_tray {
            self.show_from_tray(ctx);
        }
    }

    /// Push the current skin's menu palette to the tray so its native menu matches
    /// LinAmp's own menus (#40). Cheap, and a no-op in effect off Windows.
    fn push_tray_theme(&self) {
        let (bg, text, hover_bg, hover_text, border) = menu_palette(&self.theme);
        self._tray
            .set_menu_colors(bg, text, hover_bg, hover_text, border);
    }

    /// Genuinely quit the app (main-menu Exit or the tray's Exit). Marks the quit
    /// so the close handler shuts down even in tray mode, then starts the normal
    /// close. The window may be minimized to the tray, but eframe still runs the
    /// teardown while minimized, so there's no need to restore it first.
    fn request_exit(&mut self, ctx: &egui::Context) {
        self.force_quit = true;
        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
    }

    /// Hide every LinAmp window and keep running in the tray. The child windows
    /// go with the main one so none are left floating, and their state is saved
    /// so a later restore reopens exactly what was showing.
    fn hide_to_tray(&mut self, ctx: &egui::Context) {
        if self.hidden_to_tray {
            return;
        }
        self.tray_prev_vis = (self.eq_visible, self.pl_visible, self.lib_visible);
        self.eq_visible = false;
        self.pl_visible = false;
        self.lib_visible = false;
        self.hidden_to_tray = true;
        // Minimize rather than hide: a window hidden with Visible(false) stops
        // getting repaints on Windows, so eframe never runs update() again and the
        // tray's clicks (Restore, Play/Pause, Exit) are never drained. eframe keeps
        // running update() for a minimized window, so the tray stays responsive. In
        // tray mode the taskbar button is already removed, so nothing shows.
        ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
    }

    /// Bring LinAmp back from the tray: restore the windows that were open and
    /// focus the main one. A no-op visibility restore when it is already shown.
    fn show_from_tray(&mut self, ctx: &egui::Context) {
        if self.hidden_to_tray {
            let (eq, pl, lib) = self.tray_prev_vis;
            self.eq_visible = eq;
            self.pl_visible = pl;
            self.lib_visible = lib;
            self.hidden_to_tray = false;
        }
        ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
        ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
        ctx.request_repaint();
    }

    /// Push the crossfade length (or 0 = off) from Preferences to the player.
    fn apply_crossfade_pref(&mut self) {
        let secs = if self.prefs.crossfade { self.prefs.crossfade_secs } else { 0.0 };
        self.player.set_crossfade(secs);
    }

    /// True when the current track has reached its crossfade window and the next one
    /// should start overlapping it. Only for finite-length local tracks (streams have
    /// no length to fade from), and not while already crossfading or in a one-shot
    /// mode (stop-after / repeat-one).
    fn should_start_crossfade(&self) -> bool {
        crossfade_due(
            self.player.crossfade_secs(),
            self.player.state() == PlayState::Playing,
            self.player.is_crossfading(),
            self.stop_after_current,
            self.repeat_one,
            self.fade.is_some(),
            self.playlist.current_track().and_then(|t| t.duration).map(|d| d.as_secs_f32()),
            self.player.position().as_secs_f32(),
        )
    }

    fn adjust_volume(&mut self, delta: f32) {
        let v = (self.player.volume() + delta).clamp(0.0, 1.0);
        self.player.set_volume(v);
    }

    fn seek_relative(&mut self, secs: f32) {
        let pos = self.player.position().as_secs_f32() + secs;
        self.player.seek(Duration::from_secs_f32(pos.max(0.0)));
    }

    /// Keep `current` pointing at the same track after a reorder/sort.
    fn restore_current(&mut self, path: Option<PathBuf>) {
        if let Some(p) = path {
            self.playlist.current = self.playlist.tracks.iter().position(|t| t.path == p);
        }
    }

    fn move_track(&mut self, from: usize, mut to: usize) {
        if from >= self.playlist.tracks.len() {
            return;
        }
        let cur = self.playlist.current_track().map(|t| t.path.clone());
        let track = self.playlist.tracks.remove(from);
        if from < to {
            to -= 1;
        }
        let to = to.min(self.playlist.tracks.len());
        self.playlist.tracks.insert(to, track);
        self.sort_key = SortKey::Custom;
        self.restore_current(cur);
    }

    fn sort_playlist(&mut self) {
        let cur = self.playlist.current_track().map(|t| t.path.clone());
        let t = &mut self.playlist.tracks;
        match self.sort_key {
            SortKey::Custom => {}
            SortKey::Title => t.sort_by(|a, b| a.title.to_lowercase().cmp(&b.title.to_lowercase())),
            SortKey::Artist => t.sort_by(|a, b| {
                (a.artist.to_lowercase(), a.album.to_lowercase(), a.title.to_lowercase()).cmp(&(
                    b.artist.to_lowercase(),
                    b.album.to_lowercase(),
                    b.title.to_lowercase(),
                ))
            }),
            SortKey::Album => t.sort_by(|a, b| {
                (a.album.to_lowercase(), a.title.to_lowercase())
                    .cmp(&(b.album.to_lowercase(), b.title.to_lowercase()))
            }),
            SortKey::Duration => t.sort_by(|a, b| a.duration.cmp(&b.duration)),
            SortKey::Filename => {
                t.sort_by(|a, b| file_name_key(&a.path).cmp(&file_name_key(&b.path)))
            }
            SortKey::Path => t.sort_by(|a, b| {
                a.path
                    .to_string_lossy()
                    .to_lowercase()
                    .cmp(&b.path.to_string_lossy().to_lowercase())
            }),
        }
        self.restore_current(cur);
    }

    // ---- file dialogs ------------------------------------------------------

    /// Run a blocking native file dialog with always-on-top suspended, so the
    /// dialog comes to the front instead of being trapped behind our topmost
    /// windows (#172). No-op cost when always-on-top is off.
    fn pick_dialog<T>(&self, pick: impl FnOnce() -> T) -> T {
        // Restore always-on-top from a Drop guard, so an unwind out of `pick` still
        // re-raises the windows. A bare restore after `pick()` would be skipped on a
        // panic, leaving every window stuck non-topmost (#172).
        struct TopmostGuard(bool);
        impl Drop for TopmostGuard {
            fn drop(&mut self) {
                if self.0 {
                    set_windows_topmost(true);
                }
            }
        }
        let restore = self.prefs.always_on_top;
        if restore {
            set_windows_topmost(false);
        }
        let _guard = TopmostGuard(restore);
        pick()
    }

    fn open_files(&mut self) {
        hide_popup_window();
        if let Some(paths) = self.pick_dialog(|| {
            rfd::FileDialog::new()
                // Single source of truth: the same list `is_supported` gates on, so
                // adding a format only touches `SUPPORTED_EXTS`.
                .add_filter("Audio", crate::playlist::SUPPORTED_EXTS)
                .pick_files()
        }) {
            let was_empty = self.playlist.is_empty();
            for p in paths {
                self.playlist.add_file(p);
            }
            self.maybe_scan_tags();
            if was_empty && !self.playlist.is_empty() {
                self.play_index(0);
            }
            self.status = format!("{} track(s) in playlist.", self.playlist.len());
        }
    }

    /// Read the audio CD in the default optical drive and play it: append its
    /// tracks to the playlist (as `cdda://` entries) and start the first one. The
    /// native drive reads keep the audio gapless, unlike playing the desktop's
    /// `cdda://` GVfs mount as WAV files (#160). Linux (verified) and Windows
    /// (best-effort, pending hardware verification).
    #[cfg(any(target_os = "linux", target_os = "windows"))]
    fn open_audio_cd(&mut self) {
        hide_popup_window();
        let device = crate::cdda::default_cd_device();
        match crate::cdda::read_cd_toc(&device) {
            Ok(toc) if !toc.is_empty() => {
                let start = self.playlist.len();
                for t in &toc {
                    self.playlist.tracks.push(crate::playlist::Track::cd_track(&device, t));
                }
                self.status = format!("Audio CD: {} track(s) added.", toc.len());
                self.play_index(start);
            }
            Ok(_) => self.status = "Audio CD: no audio tracks on the disc.".to_string(),
            Err(e) => self.status = format!("Audio CD: {e}"),
        }
    }

    fn maybe_scan_tags(&mut self) {
        if self.prefs.tag_on_load {
            self.playlist.load_all();
        }
    }

    /// Append any files/folders/playlists dropped onto this viewport's window.
    /// Add dropped files/folders/playlists. `win` is the classic window the drop
    /// landed on (None in the modern fallback). A drop on the playlist inserts at the
    /// row under the cursor; anywhere else appends. Unsupported files are skipped,
    /// just like adding from the menu.
    fn handle_dropped(&mut self, ctx: &egui::Context, win: Option<ClWin>) {
        let dropped: Vec<PathBuf> = ctx.input(|i| {
            i.raw.dropped_files.iter().filter_map(|f| f.path.clone()).collect()
        });
        if dropped.is_empty() {
            return;
        }
        // Where to insert: a drop on the (expanded) playlist window lands at the row
        // under the release point; every other target appends (row = None).
        let drop_row = if win == Some(ClWin::Pl) && !self.geom.pl_shade {
            ctx.input(|i| i.pointer.hover_pos().or(i.pointer.interact_pos()))
                .map(|p| classic::pl_row_at(p.y / self.scale, self.plview.first))
        } else {
            None
        };
        let was_empty = self.playlist.is_empty();
        let before = self.playlist.len();
        for p in dropped {
            let is_pl = matches!(
                p.extension().and_then(|e| e.to_str()).map(|e| e.to_lowercase()).as_deref(),
                Some("m3u") | Some("m3u8") | Some("pls")
            );
            if p.is_dir() {
                self.playlist.add_folder(&p);
            } else if is_pl {
                self.playlist.load_file(&p);
            } else {
                self.playlist.add_file(p);
            }
        }
        // Move the just-appended block to the drop row (no-op when appending).
        if let Some(row) = drop_row {
            let added = self.playlist.len() - before;
            if let Some(at) = self.playlist.move_tail_to(added, row) {
                // Rows at/after the insertion point shifted down by `added`; keep the
                // selection pointing at the same tracks (the dropped ones stay
                // unselected).
                self.plview.selection = self
                    .plview.selection
                    .iter()
                    .map(|&i| if i >= at { i + added } else { i })
                    .collect();
            }
        }
        self.maybe_scan_tags();
        if was_empty && !self.playlist.is_empty() {
            self.play_index(0);
        }
        self.status = format!("{} track(s) in playlist.", self.playlist.len());
    }

    fn open_folder(&mut self) {
        hide_popup_window();
        if let Some(dir) = self.pick_dialog(|| rfd::FileDialog::new().pick_folder()) {
            let was_empty = self.playlist.is_empty();
            self.playlist.add_folder(&dir);
            self.maybe_scan_tags();
            if was_empty && !self.playlist.is_empty() {
                self.play_index(0);
            }
            self.status = format!("{} track(s) in playlist.", self.playlist.len());
        }
    }

    fn save_playlist_dialog(&mut self) {
        if self.playlist.is_empty() {
            self.status = "Playlist is empty.".to_string();
            return;
        }
        hide_popup_window();
        if let Some(path) = self.pick_dialog(|| {
            rfd::FileDialog::new()
                .add_filter("M3U playlist", &["m3u"])
                .add_filter("PLS playlist", &["pls"])
                .set_file_name("playlist.m3u")
                .save_file()
        }) {
            match save_playlist_file(&path, &self.playlist.tracks) {
                Ok(()) => self.status = format!("Saved {}", path.display()),
                Err(e) => self.status = format!("Save error: {e}"),
            }
        }
    }

    fn load_playlist_dialog(&mut self) {
        hide_popup_window();
        if let Some(path) = self.pick_dialog(|| {
            rfd::FileDialog::new()
                .add_filter("Playlists", &["m3u", "m3u8", "pls"])
                .pick_file()
        }) {
            let was_empty = self.playlist.is_empty();
            let n = self.playlist.load_file(&path);
            self.maybe_scan_tags();
            if was_empty && !self.playlist.is_empty() {
                self.play_index(0);
            }
            self.status = format!("Loaded {n} track(s).");
        }
    }

    // ---- library -----------------------------------------------------------

    /// Fixed window size of the Media Library (logical points; unaffected by the
    /// 1x/2x "double size" toggle, like the other egui dialogs).
    /// The library window's base (1x) size in points. Height is exactly four
    /// main-window heights so that, docked to the right of the Main/EQ/Playlist
    /// stack, the group reads as one rectangle.
    fn lib_base_size(&self) -> egui::Vec2 {
        egui::vec2(640.0, 4.0 * classic::MAIN_H as f32)
    }

    /// The library window's on-screen size, scaled with the double-size setting
    /// so it grows with the classic windows.
    fn lib_size(&self) -> egui::Vec2 {
        self.lib_base_size() * self.scale
    }

    /// Show/hide the Media Library window.
    fn toggle_library(&mut self) {
        self.lib_visible = !self.lib_visible;
        if self.lib_visible {
            if !self.geom.lib_pos_init {
                self.geom.lib_pos = self.library_default_pos();
                self.geom.lib_pos_init = true;
            }
            self.refresh_library();
        }
    }

    /// Default library placement: just below the current window stack.
    /// Default position when the library is first opened: docked to the right edge
    /// of the main window, top-aligned with it.
    fn library_default_pos(&self) -> egui::Pos2 {
        let base = self.geom.win_pos[0];
        egui::pos2(base.x + classic::MAIN_W as f32 * self.scale, base.y)
    }

    // ---- skins / theming ---------------------------------------------------

    fn discover_skins(&mut self) {
        self.skins.clear();
        let mut dirs = vec![PathBuf::from("skins")];
        if let Ok(exe) = std::env::current_exe() {
            if let Some(d) = exe.parent() {
                dirs.push(d.join("skins"));
            }
        }
        let mut seen = std::collections::HashSet::new();
        for d in dirs {
            if let Ok(rd) = std::fs::read_dir(&d) {
                for e in rd.flatten() {
                    let p = e.path();
                    let is_skin = p
                        .extension()
                        .and_then(|x| x.to_str())
                        .map(|x| matches!(x.to_lowercase().as_str(), "wsz" | "zip"))
                        .unwrap_or(false);
                    // De-dup by file name (first dir searched, ./skins, wins) so
                    // a skin found next to the exe too isn't listed twice.
                    let name = p.file_name().map(|n| n.to_os_string());
                    if is_skin {
                        if let Some(n) = name {
                            if seen.insert(n) {
                                self.skins.push(p);
                            }
                        }
                    }
                }
            }
        }
        self.skins.sort();
    }

    fn apply_skin_path(&mut self, ctx: &egui::Context, path: &std::path::Path) {
        match skin::load_wsz(path) {
            Ok(t) => {
                self.theme = t;
                apply_theme(ctx, &self.theme);
                self.push_tray_theme();
                self.status = format!("Loaded skin: {}", self.theme.name);
            }
            Err(e) => self.status = format!("Skin error: {e}"),
        }
        // Load the classic sprite sheets for the same skin. Drop every cached
        // window texture so all three re-composite with the new skin.
        self.classic = ClassicSkin::load(path).ok();
        self.render.classic_tex = None;
        self.render.eq_tex = None;
        self.render.pl_tex = None;
        // Destroy the previous skin's cached native cursors (Windows) so they don't
        // leak across skin changes, then reset the desired cursors so a skin with
        // none doesn't keep showing the old skin's; the per-frame cursor update
        // re-applies for the new skin if it ships any.
        #[cfg(windows)]
        crate::wincursor::clear_cache();
        self.clear_skin_cursors();
        // Remember this skin for next launch.
        self.skin_path = Some(path.to_path_buf());
        self.prefs.last_skin = Some(path.display().to_string());
        self.prefs.save();
    }

    fn set_mode(&mut self, ctx: &egui::Context, mode: UiMode) {
        self.ui_mode = mode;
        match mode {
            UiMode::Classic => {
                ctx.send_viewport_cmd(egui::ViewportCommand::Decorations(false));
                ctx.send_viewport_cmd(egui::ViewportCommand::Resizable(false));
                ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(
                    self.win_inner_size(ClWin::Main),
                ));
                self.geom.root_last = None;
            }
            UiMode::Modern => {
                ctx.send_viewport_cmd(egui::ViewportCommand::Decorations(true));
                ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(egui::vec2(420.0, 640.0)));
                ctx.send_viewport_cmd(egui::ViewportCommand::Resizable(true));
            }
        }
    }

    // ---- classic skin view -------------------------------------------------

    fn make_main_state(&self) -> MainState {
        let play = match self.player.state() {
            PlayState::Playing => PlayUi::Playing,
            PlayState::Paused => PlayUi::Paused,
            PlayState::Stopped => PlayUi::Stopped,
        };
        let (mut title, mut kbps, mut khz, mut stereo, mut dur) =
            (String::from("linamp"), String::new(), String::new(), true, 0.0);
        if let Some(url) = self.player.current_url() {
            // Streaming "Open location": the stream is what's actually playing, so
            // it wins over any still-selected playlist track. Show the live ICY
            // stream title ("now playing") when the stream reports one, otherwise
            // the location itself. Bitrate/rate/duration are unknown for a live
            // stream, so they stay at their defaults.
            title = match self.player.stream_title() {
                Some(t) => t,
                None => crate::text::display_url(url),
            };
        } else if let Some(t) = self.playlist.current_track() {
            title = t.display_name();
            kbps = t.bitrate.map(|b| b.to_string()).unwrap_or_default();
            khz = t.sample_rate.map(|s| (s / 1000).to_string()).unwrap_or_default();
            stereo = t.channels.map(|c| c >= 2).unwrap_or(true);
            dur = t.duration.map(|d| d.as_secs_f32()).unwrap_or(0.0);
        }
        let pos = self
            .classic_seek_preview
            .unwrap_or_else(|| self.player.position().as_secs_f32());

        MainState {
            focused: self.geom.app_focused,
            play,
            pressed: self.render.pressed_btn,
            shuffle: self.shuffle,
            repeat: self.repeat_all,
            eq_on: self.eq_visible,
            pl_on: self.pl_visible,
            always_on_top: self.prefs.always_on_top,
            double_size: self.prefs.double_size,
            pos,
            duration: dur,
            time_remaining: self.time_remaining,
            time_hidden: self.time_blink_hidden,
            volume: self.player.volume(),
            balance: self.balance,
            stereo,
            title: scroll_text(&title, self.marquee_offset, 30),
            kbps,
            khz,
            fg: [self.theme.lcd.r(), self.theme.lcd.g(), self.theme.lcd.b()],
            viz: match self.viz_mode {
                VizMode::Spectrum => Some(classic::VizFrame::Spectrum {
                    bars: self.cviz_bars.clone(),
                    peaks: self.cviz_peaks.clone(),
                }),
                VizMode::Oscilloscope => Some(classic::VizFrame::Scope(self.viz_samples.clone())),
                VizMode::Off => None,
            },
            viz_spectrum: self
                .theme
                .viz_spectrum
                .iter()
                .map(|c| [c.r(), c.g(), c.b()])
                .collect(),
            viz_peak: [
                self.theme.viz_peak.r(),
                self.theme.viz_peak.g(),
                self.theme.viz_peak.b(),
            ],
        }
    }

    /// Update the visualization data (spectrum bars / oscilloscope samples) for
    /// the classic analyzer, once per frame.
    fn step_viz(&mut self) {
        match self.viz_mode {
            // Spectrum: run the FFT only while playing; on stop let the bars fall
            // to zero (no FFT); while paused freeze them where they are.
            VizMode::Spectrum => match self.player.state() {
                PlayState::Playing => {
                    self.viz.snapshot(&mut self.viz_samples, FFT_SIZE);
                    let rate = self.viz.rate();
                    let bars = viz::compute_bars(
                        &self.viz_samples,
                        &*self.fft,
                        FFT_SIZE,
                        CLASSIC_BANDS,
                        rate,
                    );
                    for i in 0..CLASSIC_BANDS {
                        let target = bars.get(i).copied().unwrap_or(0.0);
                        let cur = self.cviz_bars[i];
                        // Smoothed rise, steady fall, so it isn't jumpy or sticky.
                        let b = if target > cur {
                            cur + (target - cur) * 0.55
                        } else {
                            (cur - 0.045).max(target)
                        };
                        self.cviz_bars[i] = b;
                        self.cviz_peaks[i] = if b > self.cviz_peaks[i] {
                            b
                        } else {
                            (self.cviz_peaks[i] - 0.009).max(0.0)
                        };
                    }
                }
                PlayState::Stopped => {
                    for i in 0..CLASSIC_BANDS {
                        self.cviz_bars[i] = (self.cviz_bars[i] - 0.045).max(0.0);
                        self.cviz_peaks[i] = (self.cviz_peaks[i] - 0.009).max(0.0);
                    }
                }
                PlayState::Paused => {}
            },
            // Scope: live while playing; flat (cleared ring) when stopped; frozen
            // when paused.
            VizMode::Oscilloscope => {
                if self.player.state() != PlayState::Paused {
                    self.viz.snapshot(&mut self.viz_samples, 76);
                }
            }
            VizMode::Off => {}
        }
    }

    /// Whether the classic UI has live motion and should keep repainting at
    /// ~30fps. When false the app idles (egui still wakes on input). A stopped
    /// player with its spectrum bars still falling counts as animating.
    fn is_animating(&self) -> bool {
        self.player.state() == PlayState::Playing
            // Paused flashes the time (see `time_blink_hidden`), so keep repainting.
            || self.player.state() == PlayState::Paused
            || self.fade.is_some()
            || self.geom.drag.is_some()
            || self.classic_seek_preview.is_some()
            || self.pending_stream.is_some() // keep polling while a stream connects
            || self.popups.cur.is_some()
            || self.quit_countdown.is_some()
            // Keep repainting while a background scan runs so its progress and the
            // final refresh show up without waiting on the next input.
            || self.lib.scan.is_some()
            || (self.player.state() == PlayState::Stopped
                && self.viz_mode == VizMode::Spectrum
                // Bars *and* peak markers, the peaks fall slower, so without
                // them the falloff would stall when the repaint rate drops.
                && self
                    .cviz_bars
                    .iter()
                    .chain(self.cviz_peaks.iter())
                    .any(|&b| b > 0.001))
    }

    /// Height in native px of each classic window given shade state.
    fn main_h(&self) -> f32 {
        if self.geom.main_shade { 14.0 } else { classic::MAIN_H as f32 }
    }
    fn eq_h(&self) -> f32 {
        if self.geom.eq_shade { 14.0 } else { classic::EQ_H as f32 }
    }
    fn apply_classic_slider(&mut self, region: Btn, lx: f32) {
        match region {
            Btn::Seek => {
                let dur = self
                    .playlist
                    .current_track()
                    .and_then(|t| t.duration)
                    .map(|d| d.as_secs_f32())
                    .unwrap_or(0.0);
                if dur > 0.0 {
                    let frac = if self.geom.main_shade {
                        // Collapsed main bar uses the shorter mini-posbar track.
                        (lx - classic::SHADE_POS_X)
                            / (classic::SHADE_POS_W - classic::SHADE_POS_THUMB_W)
                    } else {
                        (lx - 16.0) / (248.0 - 29.0)
                    };
                    self.classic_seek_preview = Some(frac.clamp(0.0, 1.0) * dur);
                }
            }
            Btn::Volume => {
                let frac = ((lx - 107.0) / (68.0 - 14.0)).clamp(0.0, 1.0);
                self.player.set_volume(frac);
            }
            Btn::Balance => {
                let t = ((lx - 177.0) / (38.0 - 14.0)).clamp(0.0, 1.0);
                let b = t * 2.0 - 1.0;
                // Snap to dead-centre, as the classic player does.
                self.balance = if b.abs() < 0.06 { 0.0 } else { b };
                self.player.set_balance(self.balance);
            }
            _ => {}
        }
    }

    /// Set the shared volume/balance from the collapsed-EQ shade sliders (`lx` in
    /// base EQ-window coords), mirroring `apply_classic_slider` so the main window
    /// and the shaded EQ move in tandem.
    fn apply_eq_shade_slider(&mut self, region: classic::EqRegion, lx: f32) {
        use classic::{
            EqRegion, EQ_SHADE_BAL_W, EQ_SHADE_BAL_X, EQ_SHADE_THUMB_W, EQ_SHADE_VOL_W,
            EQ_SHADE_VOL_X,
        };
        match region {
            EqRegion::ShadeVolume => {
                let frac =
                    ((lx - EQ_SHADE_VOL_X) / (EQ_SHADE_VOL_W - EQ_SHADE_THUMB_W)).clamp(0.0, 1.0);
                self.player.set_volume(frac);
            }
            EqRegion::ShadeBalance => {
                let t =
                    ((lx - EQ_SHADE_BAL_X) / (EQ_SHADE_BAL_W - EQ_SHADE_THUMB_W)).clamp(0.0, 1.0);
                let b = t * 2.0 - 1.0;
                self.balance = if b.abs() < 0.06 { 0.0 } else { b };
                self.player.set_balance(self.balance);
            }
            _ => {}
        }
    }

    /// Stop playback, fading out first when the "fade out when stopping" pref is
    /// on (otherwise an immediate cut). Shift+V always forces a fade regardless.
    fn stop_playback(&mut self) {
        if self.prefs.fade_on_stop && self.player.state() != PlayState::Stopped {
            self.fade = Some((self.prefs.fade_on_stop_secs, FadeEnd::Stop));
            self.status = "Fading out…".into();
        } else {
            self.player.stop();
            self.fade = None;
        }
    }

    /// Pause playback, fading out first when the "fade out when stopping or
    /// pausing" pref is on (otherwise an immediate pause). Mirrors `stop_playback`
    /// so a pause from the tray, a button or a key fades like a stop instead of
    /// cutting abruptly (#171). Resuming plays at full volume.
    fn pause_playback(&mut self) {
        if self.prefs.fade_on_stop && self.player.state() == PlayState::Playing {
            self.fade = Some((self.prefs.fade_on_stop_secs, FadeEnd::Pause));
            self.status = "Fading out…".into();
        } else {
            self.player.pause();
        }
    }

    /// Bookmark the current track (the classic player Alt+I). No-op with nothing
    /// playing, and a duplicate path is skipped.
    fn add_bookmark(&mut self) {
        let (path, title) = match self.playlist.current_track() {
            Some(t) => (t.path.to_string_lossy().into_owned(), t.display_name()),
            None => {
                self.status = "Nothing playing to bookmark".into();
                return;
            }
        };
        if self.bookmarks.add(path, title.clone()) {
            self.bookmarks.save();
            self.status = format!("Bookmarked: {title}");
            self.refresh_bookmarks_view_if_shown();
        } else {
            self.status = "Already bookmarked".into();
        }
    }

    /// Play a bookmarked track, appending it to the playlist first if it isn't
    /// already there.
    fn play_bookmark(&mut self, idx: usize) {
        let Some(bm) = self.bookmarks.items.get(idx) else {
            return;
        };
        let path = PathBuf::from(&bm.path);
        let title = bm.title.clone();
        match self.playlist.tracks.iter().position(|t| t.path == path) {
            Some(i) => self.play_index(i),
            None if path.exists() => {
                self.playlist.tracks.push(crate::playlist::Track::new(path));
                let last = self.playlist.len() - 1;
                self.play_index(last);
            }
            // A moved/deleted bookmark shouldn't drop a permanently-unplayable row
            // into the playlist, just report it. (URL bookmarks wait on #15.)
            None => self.status = format!("Bookmark file not found: {title}"),
        }
    }

    /// Path of the currently-playing track as stored in bookmarks, if any.
    fn current_bookmark_path(&self) -> Option<String> {
        self.playlist
            .current_track()
            .map(|t| t.path.to_string_lossy().into_owned())
    }

    /// Whether the current track is already bookmarked.
    fn current_is_bookmarked(&self) -> bool {
        match self.current_bookmark_path() {
            Some(p) => self.bookmarks.items.iter().any(|b| b.path == p),
            None => false,
        }
    }

    /// Remove the current track's bookmark.
    fn remove_current_bookmark(&mut self) {
        if let Some(p) = self.current_bookmark_path() {
            self.bookmarks.remove_path(&p);
            self.bookmarks.save();
            self.status = "Removed bookmark".into();
            self.refresh_bookmarks_view_if_shown();
        }
    }

    /// Clear every bookmark.
    fn clear_bookmarks(&mut self) {
        self.bookmarks.items.clear();
        self.bookmarks.save();
        self.status = "Bookmarks cleared".into();
        self.refresh_bookmarks_view_if_shown();
    }

    /// Rebuild the Media Library Bookmarks list after the bookmark store changes,
    /// so an open library on that view reflects the add/remove/clear immediately.
    fn refresh_bookmarks_view_if_shown(&mut self) {
        if self.lib.nav == crate::library::LibView::Bookmarks {
            self.refresh_library();
        }
    }

    /// The persistable last-session snapshot (playlist, window layout, EQ).
    fn build_session(&self) -> Session {
        Session {
            tracks: self
                .playlist
                .tracks
                .iter()
                .map(|t| t.path.display().to_string())
                .collect(),
            current: self.playlist.current,
            volume: Some(self.player.volume()),
            balance: Some(self.balance),
            pl_width: Some(self.plview.px_w),
            pl_height: Some(self.plview.px_h),
            eq_enabled: self.eq.enabled,
            eq_auto: self.eq.auto,
            eq_preamp: self.eq.preamp_db,
            eq_bands: self.eq.bands.to_vec(),
            eq_visible: self.eq_visible,
            pl_visible: self.pl_visible,
            main_shade: self.geom.main_shade,
            eq_shade: self.geom.eq_shade,
            pl_shade: self.geom.pl_shade,
            lib_visible: self.lib_visible,
            lib_col_fr: Some(self.lib.col_fr),
            // Remember the window layout once it's been established on frame 1.
            main_pos: self.geom.positions_init.then_some((self.geom.win_pos[0].x, self.geom.win_pos[0].y)),
            eq_pos: self.geom.positions_init.then_some((self.geom.win_pos[1].x, self.geom.win_pos[1].y)),
            pl_pos: self.geom.positions_init.then_some((self.geom.win_pos[2].x, self.geom.win_pos[2].y)),
            lib_pos: (self.geom.positions_init && self.geom.lib_pos_init)
                .then_some((self.geom.lib_pos.x, self.geom.lib_pos.y)),
        }
    }

    /// Persist the session when any of it has changed since the last write. Run
    /// every frame; it only touches disk on an actual change, so it doesn't rely
    /// on eframe's save-on-exit (which isn't invoked here) or its periodic timer.
    fn persist_session_if_changed(&mut self) {
        // Don't overwrite the saved layout while shutting down (we hide the child
        // windows for teardown, which must not be persisted as "closed").
        if self.quit_countdown.is_some() {
            return;
        }
        // Likewise while hidden to the tray: hide_to_tray closes the EQ/playlist/
        // library windows and minimizes the main one, which must not be saved as
        // the layout. Skipping keeps the last on-screen layout, so exiting from the
        // tray (and reopening) restores the windows and positions that were open.
        if self.hidden_to_tray {
            return;
        }
        // Don't churn the file every 300ms during a drag; the final resting
        // position is saved on the next cycle after the button is released.
        if self.geom.drag.is_some() || self.geom.lib_drag.is_some() {
            return;
        }
        // Change detection hashes every track path, so don't run it every frame.
        let now = Instant::now();
        if now.duration_since(self.last_session_check).as_millis() < 300 {
            return;
        }
        self.last_session_check = now;
        // Hash the session we would save, so the change detection can never drift
        // from what is actually persisted (the #105 bug: volume/balance were saved
        // but absent from a hand-listed signature, so changes weren't written).
        let session = self.build_session();
        let sig = session.sig();
        if self.session_sig != Some(sig) {
            self.session_sig = Some(sig);
            session.save();
        }
    }

    fn trigger_classic(&mut self, ctx: &egui::Context, region: Btn) {
        match region {
            Btn::Prev => self.prev(),
            Btn::Play => self.play_pause(),
            Btn::Pause => self.pause_playback(),
            Btn::Stop => self.stop_playback(),
            Btn::Next => self.next(),
            Btn::Eject => self.open_files(),
            Btn::Shuffle => self.shuffle = !self.shuffle,
            Btn::Repeat => self.repeat_all = !self.repeat_all,
            Btn::Eq => {
                self.eq_visible = !self.eq_visible;
                if self.eq_visible {
                    self.restack();
                }
            }
            Btn::Pl => {
                self.pl_visible = !self.pl_visible;
                if self.pl_visible {
                    self.restack();
                }
            }
            Btn::Close => {
                // Tray mode: the close (X) button hides to the tray; otherwise it
                // quits.
                if self.tray_mode {
                    self.hide_to_tray(ctx);
                } else {
                    self.request_exit(ctx);
                }
            }
            Btn::Minimize => {
                // Tray mode: minimize hides to the tray; otherwise iconify normally.
                if self.tray_mode {
                    self.hide_to_tray(ctx);
                } else {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
                }
            }
            Btn::Time => self.time_remaining = !self.time_remaining,
            Btn::Viz => self.viz_mode = self.viz_mode.next(),
            // Clutterbar: A=always-on-top, I=file info, D=doublesize, V=viz mode.
            // O opens the Options menu (handled on release in window.rs).
            Btn::ClutterA => {
                self.prefs.always_on_top = !self.prefs.always_on_top;
                self.prefs.save();
            }
            Btn::ClutterI => self.open_tag_editor(),
            Btn::ClutterD => {
                let dbl = !self.prefs.double_size;
                self.set_double_size(ctx, dbl);
            }
            Btn::ClutterV => self.viz_mode = self.viz_mode.next(),
            Btn::Seek | Btn::Volume | Btn::Balance | Btn::TitleBar | Btn::Options
            | Btn::ClutterO => {}
        }
    }

    fn make_eq_state(&self) -> classic::EqState {
        classic::EqState {
            focused: self.geom.app_focused,
            on: self.eq.enabled,
            auto: self.eq.auto,
            pressed: self.eq.active,
            preamp: (self.eq.preamp_db / 12.0).clamp(-1.0, 1.0),
            bands: self.eq.bands.map(|d| (d / 12.0).clamp(-1.0, 1.0)),
        }
    }

    /// Build the playlist rows for the visible window `first..end` only (numbered
    /// by absolute position), so a multi-thousand-track playlist doesn't format a
    /// string per track every recomposite for rows that are never drawn.
    fn build_pl_rows(&self, first: usize, end: usize) -> Vec<classic::PlRow> {
        let cur = self.playlist.current;
        (first..end)
            .filter_map(|i| self.playlist.tracks.get(i).map(|t| (i, t)))
            .map(|(i, t)| {
                (
                    format!("{}. {}", i + 1, t.display_name()),
                    Some(i) == cur,
                    self.plview.selection.contains(&i),
                    t.duration.map(|d| d.as_secs_f32()),
                )
            })
            .collect()
    }

    fn pl_select_all(&mut self) {
        self.plview.selection = (0..self.playlist.len()).collect();
        self.plview.anchor = Some(0);
        self.plview.lead = self.playlist.len().checked_sub(1);
    }

    /// Apply a selection gesture landing on `target`, honoring Ctrl and Shift.
    /// `ctrl_mode` differs by input source: mouse ctrl-click toggles a row,
    /// keyboard ctrl-nav adds it. The lead row is recorded and scrolled in.
    fn pl_select_with(
        &mut self,
        target: usize,
        ctrl: bool,
        shift: bool,
        ctrl_mode: crate::selection::CtrlMode,
    ) {
        if self.playlist.is_empty() {
            return;
        }
        let max = self.playlist.len() - 1;
        let target = target.min(max);
        // Clamp a stale anchor (the list may have shrunk since it was set) so a
        // shift-range can't run past the end.
        let anchor = self.plview.anchor.map(|a| a.min(max));
        self.plview.anchor = Some(crate::selection::apply(
            &mut self.plview.selection,
            anchor,
            target,
            ctrl,
            shift,
            ctrl_mode,
        ));
        self.plview.lead = Some(target);
        self.pl_scroll_to(target);
    }

    /// Keyboard navigation by `delta` rows from the lead, with Ctrl/Shift
    /// semantics (Shift extends the range, Ctrl adds the new row).
    fn pl_navigate(&mut self, delta: i32, ctrl: bool, shift: bool) {
        if self.playlist.is_empty() {
            return;
        }
        let len = self.playlist.len() as i32;
        let from = self
            .plview.lead
            .or(self.plview.anchor)
            .map(|b| b as i32)
            // Nothing focused yet: enter from the near end.
            .unwrap_or(if delta < 0 { len } else { -1 });
        let target = (from + delta).clamp(0, len - 1) as usize;
        self.pl_select_with(target, ctrl, shift, crate::selection::CtrlMode::Add);
    }

    /// Jump the selection to an absolute row (Home / End), with Ctrl/Shift.
    fn pl_navigate_to(&mut self, target: usize, ctrl: bool, shift: bool) {
        self.pl_select_with(target, ctrl, shift, crate::selection::CtrlMode::Add);
    }

    /// Adjust the playlist scroll offset so row `idx` is visible.
    fn pl_scroll_to(&mut self, idx: usize) {
        let visible = classic::pl_visible_rows(self.plview.px_h as u32).max(1);
        if idx < self.plview.first {
            self.plview.first = idx;
        } else if idx >= self.plview.first + visible {
            self.plview.first = idx + 1 - visible;
        }
    }

    fn pl_invert_selection(&mut self) {
        self.plview.selection = (0..self.playlist.len())
            .filter(|i| !self.plview.selection.contains(i))
            .collect();
    }

    /// "Play Next": move the selected track(s) to play right after the current
    /// one (or to the front when nothing is playing), keeping their order. The
    /// moved block becomes the new selection.
    fn play_next_selected(&mut self) {
        if self.plview.selection.is_empty() {
            return;
        }
        let sel: Vec<usize> = self.plview.selection.iter().copied().collect();
        let (cur, start, count) =
            reorder_play_next(&mut self.playlist.tracks, self.playlist.current, &sel);
        if count == 0 {
            return;
        }
        self.playlist.current = cur;
        self.plview.selection = (start..start + count).collect();
        self.sort_key = SortKey::Custom;
    }

    /// Remove tracks: when `keep` is true keep only selected (crop), else remove
    /// the selected ones. Preserves the current track by path where possible.
    fn pl_remove(&mut self, keep: bool) {
        if self.plview.selection.is_empty() {
            return;
        }
        let cur_path = self.playlist.current_track().map(|t| t.path.clone());
        let sel = self.plview.selection.clone();
        let mut i = 0usize;
        self.playlist.tracks.retain(|_| {
            let selected = sel.contains(&i);
            i += 1;
            if keep {
                selected
            } else {
                !selected
            }
        });
        self.plview.selection.clear();
        self.plview.anchor = None;
        self.plview.lead = None;
        self.playlist.current =
            cur_path.and_then(|p| self.playlist.tracks.iter().position(|t| t.path == p));
        self.plview.first = self.plview.first.min(self.playlist.len().saturating_sub(1));
        self.status = format!("{} track(s) in playlist.", self.playlist.len());
    }

    fn pl_colors(&self) -> classic::PlColors {
        let c = |x: Color32| [x.r(), x.g(), x.b()];
        classic::PlColors {
            bg: c(self.theme.list_bg),
            normal: c(self.theme.list_normal),
            current: c(self.theme.list_current),
            selbg: c(self.theme.list_sel_bg),
        }
    }

    fn trigger_eq(&mut self, _ctx: &egui::Context, region: classic::EqRegion) {
        use classic::EqRegion as R;
        match region {
            R::On => {
                self.eq.enabled = !self.eq.enabled;
                self.apply_eq();
            }
            R::Auto => {
                self.eq.auto = !self.eq.auto;
                self.apply_eq_auto_gain();
                self.apply_eq();
            }
            R::Close => {
                self.eq_visible = false;
            }
            // Applied on press/drag, nothing to do on release.
            R::Presets
            | R::TitleBar
            | R::Preamp
            | R::Band(_)
            | R::ShadeVolume
            | R::ShadeBalance => {}
        }
    }

    /// Apply a built-in preset (by index into [`eq::PRESETS`]).
    fn apply_eq_preset(&mut self, idx: usize) {
        if let Some((_, preamp, bands)) = crate::eq::PRESETS.get(idx) {
            self.eq.bands = *bands;
            self.eq.preamp_db = *preamp;
            if !self.eq.enabled {
                self.eq.enabled = true;
            }
            self.apply_eq_auto_gain();
            self.apply_eq();
        }
    }

    /// Apply a user-named preset from the in-app library (by index).
    fn load_named_eq_preset(&mut self, idx: usize) {
        let Some((bands, preamp, name)) = self
            .eq.presets
            .items
            .get(idx)
            .map(|p| (p.bands, p.preamp, p.name.clone()))
        else {
            return;
        };
        self.eq.bands = bands;
        self.eq.preamp_db = preamp;
        self.eq.auto = false; // an explicit preset is a manual override
        self.eq.enabled = true;
        self.apply_eq();
        self.status = format!("Loaded EQ preset: {name}");
    }

    /// Delete a saved preset from the in-app library (by index).
    fn delete_named_eq_preset(&mut self, idx: usize) {
        let Some(name) = self.eq.presets.items.get(idx).map(|p| p.name.clone()) else {
            return;
        };
        self.eq.presets.remove(&name);
        self.eq.presets.save_to_disk();
        self.status = format!("Deleted EQ preset: {name}");
    }

    /// Save the current EQ curve as a named preset (replacing one of the same name).
    fn commit_eq_preset(&mut self, name: String) {
        let name = name.trim().to_string();
        if name.is_empty() {
            return;
        }
        self.eq.presets
            .save_preset(name.clone(), self.eq.preamp_db, self.eq.bands);
        self.eq.presets.save_to_disk();
        self.status = format!("Saved EQ preset: {name}");
    }

    /// Import a classic (.EQF) preset file and apply its first preset.
    fn import_eqf(&mut self) {
        hide_popup_window();
        let Some(path) = self.pick_dialog(|| {
            rfd::FileDialog::new()
                .add_filter("EQ preset", &["eqf", "q1"])
                .pick_file()
        }) else {
            return;
        };
        match std::fs::read(&path).map_err(anyhow::Error::from).and_then(|b| crate::eqf::parse(&b)) {
            Ok(presets) => match presets.into_iter().next() {
                Some(p) => {
                    self.eq.bands = p.bands;
                    self.eq.preamp_db = p.preamp;
                    self.eq.auto = false; // an explicit preset is a manual override
                    self.eq.enabled = true;
                    self.apply_eq();
                    self.status = format!("Loaded EQ preset: {}", p.name);
                }
                None => self.status = "EQ preset file has no presets".into(),
            },
            Err(e) => self.status = format!("Couldn't load EQ preset: {e}"),
        }
    }

    /// Export the current EQ settings to a classic `.EQF` file.
    fn export_eqf(&mut self) {
        hide_popup_window();
        let Some(path) = self.pick_dialog(|| {
            rfd::FileDialog::new()
                .add_filter("EQ preset", &["eqf"])
                .set_file_name("linamp.eqf")
                .save_file()
        }) else {
            return;
        };
        let preset = crate::eqf::EqfPreset {
            name: "LinAmp".to_string(),
            bands: self.eq.bands,
            preamp: self.eq.preamp_db,
        };
        match std::fs::write(&path, crate::eqf::write(std::slice::from_ref(&preset))) {
            Ok(()) => self.status = "Saved EQ preset".into(),
            Err(e) => self.status = format!("Couldn't save EQ preset: {e}"),
        }
    }

    /// When Auto is on, set the preamp to offset the loudest boosted band so the
    /// EQ is less likely to clip.
    fn apply_eq_auto_gain(&mut self) {
        if self.eq.auto {
            self.eq.preamp_db = crate::eq::auto_preamp(&self.eq.bands);
        }
    }

    fn win_native_h(&self, win: ClWin) -> f32 {
        match win {
            ClWin::Main => self.main_h(),
            ClWin::Eq => self.eq_h(),
            ClWin::Pl => {
                if self.geom.pl_shade {
                    14.0
                } else {
                    self.plview.px_h
                }
            }
        }
    }

    fn win_inner_size(&self, win: ClWin) -> egui::Vec2 {
        let w = match win {
            ClWin::Pl if !self.geom.pl_shade => self.plview.px_w,
            _ => classic::MAIN_W as f32,
        };
        egui::vec2(w * self.scale, self.win_native_h(win) * self.scale)
    }

    fn close_window(&mut self, ctx: &egui::Context, win: ClWin) {
        match win {
            // Closing the main window follows the window mode: hide to the tray in
            // tray mode, quit otherwise.
            ClWin::Main => {
                if self.tray_mode {
                    self.hide_to_tray(ctx);
                } else {
                    self.request_exit(ctx);
                }
            }
            ClWin::Eq => self.eq_visible = false,
            ClWin::Pl => self.pl_visible = false,
        }
    }

    /// Target OS window level for the player windows (always-on-top or normal).
    fn window_level(&self) -> egui::WindowLevel {
        if self.prefs.always_on_top {
            egui::WindowLevel::AlwaysOnTop
        } else {
            egui::WindowLevel::Normal
        }
    }

    /// Whether the library window is currently edge-docked to the player group.
    fn library_docked(&self) -> bool {
        if !self.lib_visible {
            return false;
        }
        let lib = egui::Rect::from_min_size(self.geom.lib_pos, self.lib_size());
        rects_docked(lib, self.win_rect_pts(0))
            || (self.eq_visible && rects_docked(lib, self.win_rect_pts(1)))
            || (self.pl_visible && rects_docked(lib, self.win_rect_pts(2)))
    }

    fn ui_prefs(&mut self, ctx: &egui::Context) {
        if !self.prefs_open {
            // Re-anchor to the main window the next time it opens.
            self.prefs_pos = None;
            return;
        }
        // Render in its own real window so it is never clipped by the (small)
        // borderless main window. Borderless with a skin-drawn title bar (like the
        // classic windows and the Library) so the skin cursor covers the whole
        // window, including the title bar, with no OS chrome to break it.
        let default_pos = self.geom.win_pos[0] + egui::vec2(24.0, 24.0);
        let pos = *self.prefs_pos.get_or_insert(default_pos);
        let builder = egui::ViewportBuilder::default()
            .with_title("LinAmp Preferences")
            .with_position(pos)
            .with_inner_size(egui::vec2(440.0, 300.0))
            .with_decorations(false)
            .with_resizable(false)
            .with_taskbar(false)
            .with_window_level(self.window_level());
        let id = egui::ViewportId::from_hash_of("linamp_prefs");
        let mut close = false;
        ctx.show_viewport_immediate(id, builder, |ctx, _| {
            // Top: skin-accented title bar (the window is borderless). Drag it to
            // move the window; the × closes it. Mirrors the Library's title bar.
            egui::TopBottomPanel::top("prefs_titlebar")
                .exact_height(20.0)
                .frame(egui::Frame::none())
                .show(ctx, |ui| {
                    let (bar, bar_resp) = ui.allocate_exact_size(
                        egui::vec2(ui.available_width(), 20.0),
                        egui::Sense::click_and_drag(),
                    );
                    let accent = self.theme.accent;
                    let txt = crate::skin::contrast_text(accent);
                    ui.painter().rect_filled(bar, 0.0, accent);
                    ui.painter().text(
                        bar.left_center() + egui::vec2(6.0, 0.0),
                        egui::Align2::LEFT_CENTER,
                        "LinAmp Preferences",
                        egui::FontId::proportional(13.0),
                        txt,
                    );
                    // Close box on the right.
                    let x_rect = egui::Rect::from_min_size(
                        egui::pos2(bar.right() - 18.0, bar.center().y - 7.0),
                        egui::vec2(14.0, 14.0),
                    );
                    let x_resp =
                        ui.interact(x_rect, ui.id().with("prefs_close"), egui::Sense::click());
                    ui.painter()
                        .rect_stroke(x_rect, 2.0, egui::Stroke::new(1.0_f32, txt));
                    ui.painter().text(
                        x_rect.center(),
                        egui::Align2::CENTER_CENTER,
                        "×",
                        egui::FontId::proportional(13.0),
                        txt,
                    );
                    if x_resp.clicked() {
                        close = true;
                    }
                    // Drag the title bar to move the window (skip when over the ×).
                    if bar_resp.drag_started() && !x_resp.hovered() {
                        if let Some(g) = global_cursor_points(ctx) {
                            self.prefs_drag = Some(g - pos);
                        }
                    }
                    if bar_resp.dragged() {
                        if let (Some(g), Some(grab)) = (global_cursor_points(ctx), self.prefs_drag) {
                            let np = g - grab;
                            self.prefs_pos = Some(np);
                            ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(np));
                        }
                    }
                    if bar_resp.drag_stopped() {
                        self.prefs_drag = None;
                    }
                });
            // Left: the category list (the classic player's Preferences tree, flattened
            // to a list that the buildout can extend).
            egui::SidePanel::left("prefs_cats")
                .resizable(false)
                .exact_width(104.0)
                .show(ctx, |ui| {
                    ui.add_space(6.0);
                    for tab in PrefTab::ALL {
                        if ui
                            .selectable_label(self.prefs_tab == tab, tab.label())
                            .clicked()
                        {
                            self.prefs_tab = tab;
                        }
                    }
                });
            // Bottom: Close on the left, a compact version on the right (always
            // visible, whichever category is open).
            egui::TopBottomPanel::bottom("prefs_footer").show(ctx, |ui| {
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    if ui.button("Close").clicked() {
                        close = true;
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(
                            egui::RichText::new(format!("LinAmp {}", env!("CARGO_PKG_VERSION")))
                                .small()
                                .color(Color32::GRAY),
                        );
                    });
                });
                ui.add_space(4.0);
            });
            // Right: the selected category's options.
            // Right: the selected category's page, in a scroll area so a category
            // can grow tall without the fixed window clipping it. Each arm just
            // dispatches to a `prefs_page_*` method, see `pref_section` for the
            // pattern that keeps every page consistent as options are added.
            egui::CentralPanel::default().show(ctx, |ui| {
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| match self.prefs_tab {
                        PrefTab::Display => self.prefs_page_display(ctx, ui),
                        PrefTab::Playback => self.prefs_page_playback(ui),
                        PrefTab::Conversion => self.prefs_page_conversion(ui),
                        PrefTab::Playlist => self.prefs_page_playlist(ui),
                        PrefTab::Tags => self.prefs_page_tags(ui),
                        PrefTab::OptionalFeatures => self.prefs_page_optional_features(ui),
                        PrefTab::About => self.prefs_page_about(ui),
                    });
            });
            self.apply_base_cursor(ctx, "LinAmp Preferences");
            if ctx.input(|i| i.viewport().close_requested() || i.key_pressed(egui::Key::Escape)) {
                close = true;
            }
        });
        if close {
            self.prefs_open = false;
        }
    }

    /// One titled group of related settings, drawn with uniform spacing so every
    /// Preferences page reads the same however many groups it grows.
    ///
    /// Extending the Preferences window:
    /// * add a setting: drop a widget into the relevant `pref_section` below;
    /// * add a group: another `pref_section(ui, "Title", |ui| { … })`;
    /// * add a category: a [`PrefTab`] variant (wired into `ALL` + `label`)
    ///   and a matching `prefs_page_*` method dispatched from `ui_prefs`.
    fn pref_section(ui: &mut egui::Ui, title: &str, add: impl FnOnce(&mut egui::Ui)) {
        ui.add_space(2.0);
        ui.label(egui::RichText::new(title).strong());
        ui.add_space(2.0);
        add(ui);
        ui.add_space(12.0);
    }

    fn prefs_page_display(&mut self, ctx: &egui::Context, ui: &mut egui::Ui) {
        Self::pref_section(ui, "Window size", |ui| {
            let mut dbl = self.prefs.double_size;
            ui.radio_value(&mut dbl, false, "1x, pixel-for-pixel");
            ui.radio_value(&mut dbl, true, "2x, scaled for modern screens (Ctrl+D)");
            if dbl != self.prefs.double_size {
                self.set_double_size(ctx, dbl);
            }
        });
        Self::pref_section(ui, "Window", |ui| {
            let mut aot = self.prefs.always_on_top;
            if ui.checkbox(&mut aot, "Always on top").changed() {
                self.prefs.always_on_top = aot;
                self.prefs.save();
            }
            let mut disable_cur = self.prefs.disable_skin_cursors;
            if ui
                .checkbox(&mut disable_cur, "Disable skin cursors")
                .changed()
            {
                self.prefs.disable_skin_cursors = disable_cur;
                self.prefs.save();
                // Drop the custom cursors immediately when turning the option off;
                // re-enabling reapplies them as windows are hovered.
                if disable_cur {
                    self.clear_skin_cursors();
                } else {
                    self.render.cur_cursor = [None; 3];
                }
            }
        });
        Self::pref_section(ui, "System tray", |ui| {
            let mut tray = self.prefs.tray_enabled;
            ui.radio_value(&mut tray, false, "Normal: taskbar button, no tray icon");
            ui.radio_value(
                &mut tray,
                true,
                "System tray: tray icon, no taskbar button, hide to tray on minimize and close",
            );
            if tray != self.prefs.tray_enabled {
                self.set_window_mode(ctx, tray);
            }
            ui.add_space(4.0);
            ui.weak("Minimize and the close button hide to the tray. Exit from the main menu always quits.");
        });
    }

    fn prefs_page_optional_features(&mut self, ui: &mut egui::Ui) {
        ui.add_space(2.0);
        ui.colored_label(
            egui::Color32::from_rgb(200, 150, 60),
            "These are obsolete or incomplete features included for \"full parity\". \
             They may not function exactly as expected, enable at your own risk.",
        );
        ui.add_space(10.0);
        Self::pref_section(ui, "Minibrowser", |ui| {
            let mut on = self.prefs.minibrowser_enabled;
            if ui.checkbox(&mut on, "Enable the Minibrowser").changed() {
                self.prefs.minibrowser_enabled = on;
                if !on {
                    self.mb_visible = false;
                }
                self.prefs.save();
            }
            ui.add_enabled_ui(self.prefs.minibrowser_enabled, |ui| {
                ui.label("Address (opens in your default browser):");
                let mut url = self.prefs.minibrowser_url.clone();
                if ui
                    .add(egui::TextEdit::singleline(&mut url).desired_width(260.0))
                    .changed()
                {
                    self.prefs.minibrowser_url = url;
                    self.prefs.save();
                }
            });
        });
        Self::pref_section(ui, "Numeric keypad", |ui| {
            let mut on = self.prefs.numpad_transport;
            if ui
                .checkbox(&mut on, "Bind the numeric keypad to transport controls")
                .changed()
            {
                self.prefs.numpad_transport = on;
                self.prefs.save();
            }
        });
    }

    fn prefs_page_playback(&mut self, ui: &mut egui::Ui) {
        Self::pref_section(ui, "On stop or pause", |ui| {
            // Only genuine, persistent settings belong in Preferences. One-shot
            // actions ("Stop after current", Ctrl+V) and view toggles ("time
            // remaining", Ctrl+T) keep their shortcuts but are deliberately not
            // shown here, a settings checkbox implies it sticks, and they don't.
            let mut fade = self.prefs.fade_on_stop;
            let mut changed = ui
                .checkbox(&mut fade, "Fade out when stopping or pausing (Shift+V to force)")
                .changed();
            let mut secs = self.prefs.fade_on_stop_secs;
            ui.add_enabled_ui(fade, |ui| {
                if ui
                    .add(egui::Slider::new(&mut secs, 1.0..=12.0).suffix(" s"))
                    .changed()
                {
                    changed = true;
                }
            });
            if changed {
                self.prefs.fade_on_stop = fade;
                self.prefs.fade_on_stop_secs = secs;
                self.prefs.save();
            }
        });

        Self::pref_section(ui, "Crossfade", |ui| {
            // Overlaps the end of one track with the start of the next. Off by
            // default (the classic player's default is a gapless cut).
            let mut on = self.prefs.crossfade;
            let mut changed = ui
                .checkbox(&mut on, "Crossfade between tracks")
                .changed();
            let mut secs = self.prefs.crossfade_secs;
            ui.add_enabled_ui(on, |ui| {
                if ui
                    .add(egui::Slider::new(&mut secs, 1.0..=12.0).suffix(" s"))
                    .changed()
                {
                    changed = true;
                }
            });
            if changed {
                self.prefs.crossfade = on;
                self.prefs.crossfade_secs = secs;
                self.prefs.save();
                self.apply_crossfade_pref();
            }
        });
    }

    /// The rip / convert settings page. Format first, then Bitrate and Sample rate as
    /// dropdowns whose options depend on the chosen format (bitrate only for the lossy
    /// formats; Opus is fixed at 48 kHz), then the output folder. These are global and
    /// applied to every conversion for now.
    fn prefs_page_conversion(&mut self, ui: &mut egui::Ui) {
        use crate::encode::OutputFormat;
        ui.add_space(2.0);
        ui.weak("Applied to every file conversion. Per-track settings may come later.");
        ui.add_space(10.0);

        Self::pref_section(ui, "Format", |ui| {
            let mut fmt = self.prefs.convert_format;
            egui::ComboBox::from_id_salt("convert_format")
                .selected_text(fmt.display())
                .show_ui(ui, |ui| {
                    for &f in OutputFormat::ALL {
                        ui.selectable_value(&mut fmt, f, f.display());
                    }
                });
            if fmt != self.prefs.convert_format {
                self.prefs.convert_format = fmt;
                // Keep the bitrate valid for the newly chosen format.
                if fmt.is_lossy() {
                    self.prefs.convert_bitrate_kbps =
                        fmt.clamp_bitrate(self.prefs.convert_bitrate_kbps);
                }
                self.prefs.save();
            }
        });

        let fmt = self.prefs.convert_format;

        Self::pref_section(ui, "Bitrate", |ui| {
            if fmt.is_lossy() {
                let mut br = fmt.clamp_bitrate(self.prefs.convert_bitrate_kbps);
                egui::ComboBox::from_id_salt("convert_bitrate")
                    .selected_text(format!("{br} kbps"))
                    .show_ui(ui, |ui| {
                        for &p in fmt.bitrate_presets() {
                            ui.selectable_value(&mut br, p, format!("{p} kbps"));
                        }
                    });
                if br != self.prefs.convert_bitrate_kbps {
                    self.prefs.convert_bitrate_kbps = br;
                    self.prefs.save();
                }
            } else {
                ui.weak(format!("{} is lossless; bitrate does not apply.", fmt.display()));
            }
        });

        Self::pref_section(ui, "Sample rate", |ui| {
            if fmt == OutputFormat::Opus {
                ui.weak("Opus always encodes at 48000 Hz.");
            } else {
                let mut rate = self.prefs.convert_sample_rate;
                let label = match rate {
                    Some(r) => format!("{r} Hz"),
                    None => "Same as source".to_string(),
                };
                egui::ComboBox::from_id_salt("convert_rate")
                    .selected_text(label)
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut rate, None, "Same as source");
                        for &r in fmt.sample_rate_presets() {
                            ui.selectable_value(&mut rate, Some(r), format!("{r} Hz"));
                        }
                    });
                if rate != self.prefs.convert_sample_rate {
                    self.prefs.convert_sample_rate = rate;
                    self.prefs.save();
                }
            }
        });

        Self::pref_section(ui, "Output folder", |ui| {
            ui.horizontal(|ui| {
                if ui.button("Browse...").clicked() {
                    if let Some(dir) = self.pick_dialog(|| rfd::FileDialog::new().pick_folder()) {
                        self.prefs.convert_dir = Some(dir);
                        self.prefs.save();
                    }
                }
                if self.prefs.convert_dir.is_some() && ui.button("Clear").clicked() {
                    self.prefs.convert_dir = None;
                    self.prefs.save();
                }
            });
            ui.add_space(2.0);
            let cur = self
                .prefs
                .convert_dir
                .as_ref()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| "Not set; you will be asked when you convert.".to_string());
            ui.weak(cur);
        });
    }

    fn prefs_page_playlist(&mut self, ui: &mut egui::Ui) {
        Self::pref_section(ui, "ID3 tag loading", |ui| {
            let mut onload = self.prefs.tag_on_load;
            ui.radio_value(&mut onload, true, "On load, scan when files are added");
            ui.radio_value(&mut onload, false, "On view, lazily as rows scroll in");
            if onload != self.prefs.tag_on_load {
                self.prefs.tag_on_load = onload;
                self.prefs.save();
                if onload {
                    self.playlist.load_all();
                }
            }
        });
        Self::pref_section(ui, "Open with", |ui| {
            let mut mode = self.prefs.open_with;
            ui.radio_value(
                &mut mode,
                crate::config::OpenWith::Enqueue,
                "Enqueue: add opened files to the current playlist",
            );
            ui.radio_value(
                &mut mode,
                crate::config::OpenWith::Clear,
                "Clear queue: replace the playlist with the opened files",
            );
            if mode != self.prefs.open_with {
                self.prefs.open_with = mode;
                self.prefs.save();
            }
            ui.add_space(4.0);
            ui.weak("Applies when you open audio files with LinAmp from your file manager.");
        });
    }

    fn prefs_page_tags(&mut self, ui: &mut egui::Ui) {
        Self::pref_section(ui, "ID3v2 version", |ui| {
            let mut v23 = self.prefs.id3v23;
            ui.radio_value(&mut v23, true, "ID3v2.3 (most compatible)");
            ui.radio_value(&mut v23, false, "ID3v2.4 (newer standard)");
            if v23 != self.prefs.id3v23 {
                self.prefs.id3v23 = v23;
                self.prefs.save();
            }
            ui.add_space(4.0);
            ui.weak("The version written when you save MP3 tags. Reading handles both regardless.");
        });
    }

    fn prefs_page_about(&mut self, ui: &mut egui::Ui) {
        Self::pref_section(ui, "LinAmp", |ui| {
            ui.label(format!("Version {}", env!("CARGO_PKG_VERSION")));
            ui.label(
                egui::RichText::new(format!("build {} - {}", env!("GIT_HASH"), env!("COMMIT_DATE")))
                    .small()
                    .color(Color32::GRAY),
            );
            ui.add_space(6.0);
            ui.hyperlink_to(
                "github.com/cjjulius/linamp",
                "https://github.com/cjjulius/linamp",
            );
        });
    }

    /// Jump-to-file: type to filter the playlist, Enter/click to play a match.
    fn ui_jump_file(&mut self, ctx: &egui::Context) {
        if self.jump_file.is_none() {
            return;
        }
        let screen = self.geom.win_pos[0] + egui::vec2(28.0, 28.0);
        let builder = egui::ViewportBuilder::default()
            .with_title("LinAmp, Jump to file")
            .with_position(screen)
            .with_inner_size(egui::vec2(360.0, 300.0))
            .with_min_inner_size(egui::vec2(260.0, 160.0))
            .with_taskbar(false)
            .with_window_level(self.window_level());
        let id = egui::ViewportId::from_hash_of("linamp_jumpfile");
        let mut close = false;
        let mut play: Option<usize> = None;
        ctx.show_viewport_immediate(id, builder, |ctx, _| {
            let (up, down, enter, esc) = ctx.input(|i| {
                (
                    i.key_pressed(egui::Key::ArrowUp),
                    i.key_pressed(egui::Key::ArrowDown),
                    i.key_pressed(egui::Key::Enter),
                    i.key_pressed(egui::Key::Escape),
                )
            });
            egui::CentralPanel::default().show(ctx, |ui| {
                // Search box (keeps focus for typing).
                let (q, changed) = {
                    let jf = self.jump_file.as_mut().unwrap();
                    ui.label("Jump to file (type to filter - Up/Down + Enter):");
                    let resp = ui.add(
                        egui::TextEdit::singleline(&mut jf.query)
                            .hint_text("title / artist / file"),
                    );
                    resp.request_focus();
                    (jf.query.to_lowercase(), resp.changed())
                };
                ui.separator();

                // Filter the playlist to matching indices.
                let filtered: Vec<usize> = self
                    .playlist
                    .tracks
                    .iter()
                    .enumerate()
                    .filter(|(_, t)| {
                        if q.is_empty() {
                            return true;
                        }
                        let hay = format!(
                            "{} {}",
                            t.display_name(),
                            t.path.file_name().map(|s| s.to_string_lossy()).unwrap_or_default()
                        )
                        .to_lowercase();
                        hay.contains(&q)
                    })
                    .map(|(i, _)| i)
                    .collect();

                // Move the highlight (reset to top when the query changes).
                let sel = {
                    let jf = self.jump_file.as_mut().unwrap();
                    if changed {
                        jf.sel = 0;
                    }
                    if filtered.is_empty() {
                        jf.sel = 0;
                    } else {
                        if down {
                            jf.sel = (jf.sel + 1).min(filtered.len() - 1);
                        }
                        if up {
                            jf.sel = jf.sel.saturating_sub(1);
                        }
                        jf.sel = jf.sel.min(filtered.len() - 1);
                    }
                    jf.sel
                };

                egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                    for (row, &i) in filtered.iter().enumerate() {
                        let selected = row == sel;
                        let resp = ui.selectable_label(
                            selected,
                            self.playlist.tracks[i].display_name(),
                        );
                        if resp.clicked() {
                            play = Some(i);
                        }
                        if resp.double_clicked() {
                            play = Some(i);
                        }
                        if selected && (up || down) {
                            resp.scroll_to_me(None);
                        }
                    }
                });

                if enter && !filtered.is_empty() {
                    play = Some(filtered[sel]);
                }
                if esc || ctx.input(|i| i.viewport().close_requested()) {
                    close = true;
                }
            });
            self.apply_base_cursor(ctx, "LinAmp, Jump to file");
        });
        if let Some(i) = play {
            self.play_index(i);
            close = true;
        }
        if close {
            self.jump_file = None;
        }
    }

    /// Jump-to-time: enter m:ss (or seconds) and seek there in the current track.
    fn ui_jump_time(&mut self, ctx: &egui::Context) {
        if self.jump_time.is_none() {
            return;
        }
        let screen = self.geom.win_pos[0] + egui::vec2(40.0, 40.0);
        let builder = egui::ViewportBuilder::default()
            .with_title("LinAmp, Jump to time")
            .with_position(screen)
            .with_inner_size(egui::vec2(260.0, 110.0))
            .with_resizable(false)
            .with_taskbar(false)
            .with_window_level(self.window_level());
        let id = egui::ViewportId::from_hash_of("linamp_jumptime");
        let mut close = false;
        let mut seek_to: Option<f32> = None;
        ctx.show_viewport_immediate(id, builder, |ctx, _| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let (input, parsed) = {
                    let buf = self.jump_time.as_mut().unwrap();
                    ui.label("Jump to time (m:ss or seconds):");
                    let resp =
                        ui.add(egui::TextEdit::singleline(buf).hint_text("1:23"));
                    resp.request_focus();
                    (buf.clone(), parse_time(buf))
                };
                let _ = input;
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if ui.add_enabled(parsed.is_some(), egui::Button::new("Jump")).clicked() {
                        seek_to = parsed;
                    }
                    if ui.button("Cancel").clicked() {
                        close = true;
                    }
                });
                if ctx.input(|i| i.key_pressed(egui::Key::Enter)) {
                    if let Some(s) = parsed {
                        seek_to = Some(s);
                    }
                }
                if ctx.input(|i| i.viewport().close_requested() || i.key_pressed(egui::Key::Escape)) {
                    close = true;
                }
            });
            self.apply_base_cursor(ctx, "LinAmp, Jump to time");
        });
        if let Some(s) = seek_to {
            self.player.seek(Duration::from_secs_f32(s));
            self.seek_preview = s;
            close = true;
        }
        if close {
            self.jump_time = None;
        }
    }

    /// "Save EQ preset as…": name the current EQ curve and store it in the library.
    fn ui_eq_save_name(&mut self, ctx: &egui::Context) {
        if self.eq.save_name.is_none() {
            return;
        }
        let screen = self.geom.win_pos[0] + egui::vec2(40.0, 40.0);
        let builder = egui::ViewportBuilder::default()
            .with_title("LinAmp, Save EQ preset")
            .with_position(screen)
            .with_inner_size(egui::vec2(260.0, 110.0))
            .with_resizable(false)
            .with_taskbar(false)
            .with_window_level(self.window_level());
        let id = egui::ViewportId::from_hash_of("linamp_eqsave");
        let mut close = false;
        let mut save: Option<String> = None;
        ctx.show_viewport_immediate(id, builder, |ctx, _| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let name = {
                    let buf = self.eq.save_name.as_mut().unwrap();
                    ui.label("Preset name:");
                    let resp = ui.add(egui::TextEdit::singleline(buf).hint_text("My preset"));
                    resp.request_focus();
                    buf.clone()
                };
                let valid = !name.trim().is_empty();
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if ui.add_enabled(valid, egui::Button::new("Save")).clicked() {
                        save = Some(name.clone());
                    }
                    if ui.button("Cancel").clicked() {
                        close = true;
                    }
                });
                if valid && ctx.input(|i| i.key_pressed(egui::Key::Enter)) {
                    save = Some(name);
                }
                if ctx.input(|i| i.viewport().close_requested() || i.key_pressed(egui::Key::Escape))
                {
                    close = true;
                }
            });
            self.apply_base_cursor(ctx, "LinAmp, Save EQ preset");
        });
        if let Some(name) = save {
            self.commit_eq_preset(name);
            close = true;
        }
        if close {
            self.eq.save_name = None;
        }
    }

    /// Open the "Open location" dialog (classic Ctrl+L) to stream a URL.
    fn open_location(&mut self) {
        self.location_entry = Some(String::new());
    }

    /// Begin streaming the entered URL. The connect/probe happens off the UI
    /// thread; `poll_pending_stream` picks it up once ready.
    fn play_location(&mut self, url: &str) {
        let url = url.trim().to_string();
        if url.is_empty() {
            return;
        }
        match crate::stream::open(&url) {
            Ok(pending) => {
                self.status = format!("Connecting: {}", crate::text::display_url(&url));
                self.pending_stream = Some((pending, url, Instant::now()));
            }
            Err(e) => self.status = format!("Couldn't open location: {e}"),
        }
    }

    /// Poll a stream that's being opened; hand it to the player once ready, or
    /// report a failure/timeout. Runs each frame while a connect is in flight.
    fn poll_pending_stream(&mut self) {
        let Some((mut pending, url, started)) = self.pending_stream.take() else {
            return;
        };
        match pending.poll() {
            Some(Ok(source)) => {
                let title = pending.title();
                match self.player.play_stream(source, title, url.clone()) {
                    Ok(()) => self.status = format!("Streaming: {}", crate::text::display_url(&url)),
                    Err(e) => self.status = format!("Couldn't play stream: {e}"),
                }
            }
            Some(Err(e)) => self.status = format!("Couldn't open location: {e}"),
            None => {
                if started.elapsed() > std::time::Duration::from_secs(20) {
                    self.status = "Timed out opening stream".into();
                } else {
                    self.pending_stream = Some((pending, url, started)); // keep waiting
                }
            }
        }
    }

    /// "Open location": enter a stream URL and play it (the classic player Ctrl+L).
    fn ui_location(&mut self, ctx: &egui::Context) {
        if self.location_entry.is_none() {
            return;
        }
        let screen = self.geom.win_pos[0] + egui::vec2(40.0, 40.0);
        let builder = egui::ViewportBuilder::default()
            .with_title("LinAmp, Open location")
            .with_position(screen)
            .with_inner_size(egui::vec2(320.0, 110.0))
            .with_resizable(false)
            .with_taskbar(false)
            .with_window_level(self.window_level());
        let id = egui::ViewportId::from_hash_of("linamp_location");
        let mut close = false;
        let mut go: Option<String> = None;
        ctx.show_viewport_immediate(id, builder, |ctx, _| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let url = {
                    let buf = self.location_entry.as_mut().unwrap();
                    ui.label("Open location (URL):");
                    let resp = ui.add(
                        egui::TextEdit::singleline(buf)
                            .hint_text("http://stream.example.com/radio"),
                    );
                    resp.request_focus();
                    // Right-click Cut/Copy/Paste/Select all, so pasting a URL doesn't
                    // require Ctrl+V.
                    text_edit_context_menu(&resp, ctx, buf);
                    buf.clone()
                };
                let valid = !url.trim().is_empty();
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if ui.add_enabled(valid, egui::Button::new("Open")).clicked() {
                        go = Some(url.clone());
                    }
                    if ui.button("Cancel").clicked() {
                        close = true;
                    }
                });
                if valid && ctx.input(|i| i.key_pressed(egui::Key::Enter)) {
                    go = Some(url);
                }
                if ctx.input(|i| i.viewport().close_requested() || i.key_pressed(egui::Key::Escape))
                {
                    close = true;
                }
            });
            self.apply_base_cursor(ctx, "LinAmp, Open location");
        });
        if let Some(url) = go {
            self.play_location(&url);
            close = true;
        }
        if close {
            self.location_entry = None;
        }
    }

    /// Open the configured Minibrowser URL in the system default browser.
    fn open_minibrowser_url(&mut self) {
        let url = self.prefs.minibrowser_url.trim().to_string();
        if url.is_empty() {
            return;
        }
        match open::that(&url) {
            Ok(()) => self.status = format!("Opened in browser: {url}"),
            Err(e) => self.status = format!("Couldn't open browser: {e}"),
        }
    }

    /// The Minibrowser window: a themed nod to the classic player's SHOUTcast pane. It shows
    /// the configured address and opens it in the system browser (LinAmp has no
    /// embedded web view), styled to match the active skin.
    fn ui_minibrowser(&mut self, ctx: &egui::Context) {
        if !self.mb_visible || !self.prefs.minibrowser_enabled {
            return;
        }
        let screen = self.geom.win_pos[0] + egui::vec2(60.0, 60.0);
        let builder = egui::ViewportBuilder::default()
            .with_title("LinAmp, Minibrowser")
            .with_position(screen)
            .with_inner_size(egui::vec2(360.0, 220.0))
            .with_resizable(false)
            .with_taskbar(false)
            .with_window_level(self.window_level());
        let id = egui::ViewportId::from_hash_of("linamp_minibrowser");
        let mut close = false;
        let mut open_url = false;
        let url = self.prefs.minibrowser_url.clone();
        ctx.show_viewport_immediate(id, builder, |ctx, _| {
            let style = ctx.style();
            let border = egui::Stroke::new(1.0_f32, style.visuals.window_stroke.color);
            egui::TopBottomPanel::top("mb_bar").show(ctx, |ui| {
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("Address:").strong());
                    // The address is read-only here; it's edited in Preferences.
                    let mut addr = url.clone();
                    ui.add(
                        egui::TextEdit::singleline(&mut addr)
                            .desired_width(f32::INFINITY)
                            .interactive(false),
                    );
                });
                ui.add_space(4.0);
            });
            egui::TopBottomPanel::bottom("mb_actions").show(ctx, |ui| {
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    if ui.button("Open in browser").clicked() {
                        open_url = true;
                    }
                    if ui.button("Close").clicked() {
                        close = true;
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(
                            egui::RichText::new("Set the address in Preferences")
                                .small()
                                .color(egui::Color32::GRAY),
                        );
                    });
                });
                ui.add_space(4.0);
            });
            egui::CentralPanel::default().show(ctx, |ui| {
                // A simulated "page", a nod to the original minibrowser rather
                // than a real web view.
                ui.vertical_centered(|ui| {
                    ui.add_space(24.0);
                    ui.heading("LinAmp Minibrowser");
                    ui.add_space(6.0);
                    ui.label(&url);
                    ui.add_space(12.0);
                    ui.label(
                        egui::RichText::new("Click \"Open in browser\" to visit this address.")
                            .italics(),
                    );
                });
            });
            // On-theme border just inside the window edge, matching the menus.
            let vr = ctx.screen_rect().shrink(1.0);
            ctx.layer_painter(egui::LayerId::new(
                egui::Order::Foreground,
                egui::Id::new("linamp_mb_border"),
            ))
            .rect_stroke(vr, egui::Rounding::ZERO, border);
            self.apply_base_cursor(ctx, "LinAmp, Minibrowser");
            if ctx.input(|i| i.viewport().close_requested() || i.key_pressed(egui::Key::Escape)) {
                close = true;
            }
        });
        if open_url {
            self.open_minibrowser_url();
        }
        if close {
            self.mb_visible = false;
        }
    }

    fn apply_eq_slider(&mut self, region: classic::EqRegion, ey: f32) {
        let v = classic::eq_value_at_y(ey);
        match region {
            classic::EqRegion::Preamp => {
                // Dragging the preamp is a manual override, so it turns Auto off.
                self.eq.auto = false;
                self.eq.preamp_db = v * 12.0;
                self.apply_eq();
            }
            classic::EqRegion::Band(i) if i < 10 => {
                self.eq.bands[i] = v * 12.0;
                self.apply_eq_auto_gain();
                self.apply_eq();
            }
            _ => {}
        }
    }

    /// Reset an EQ preamp/band to 0 dB (flat), as the classic player does on a double-click.
    fn reset_eq_slider(&mut self, region: classic::EqRegion) {
        match region {
            classic::EqRegion::Preamp => {
                self.eq.auto = false; // a manual override, like dragging it
                self.eq.preamp_db = 0.0;
                self.apply_eq();
            }
            classic::EqRegion::Band(i) if i < 10 => {
                self.eq.bands[i] = 0.0;
                self.apply_eq_auto_gain();
                self.apply_eq();
            }
            _ => {}
        }
    }

    /// Apply each classic window's REGION.TXT shape (main/EQ/playlist, per shade
    /// state, scaled). Fingerprinted so the X calls only happen on change. No-op
    /// for skins without regions (unless we previously shaped, then reset to
    /// rectangular). X11 only; a no-op on other platforms.
    fn apply_window_shapes(&mut self) {
        use crate::region::Polygon;
        let (has, main_r, main_sr, eq_r, eq_sr, pl_r) = match self.classic.as_ref() {
            Some(cs) => (
                !cs.regions.is_empty(),
                cs.regions.main.clone(),
                cs.regions.main_shade.clone(),
                cs.regions.eq.clone(),
                cs.regions.eq_shade.clone(),
                cs.regions.playlist.clone(),
            ),
            None => return,
        };
        if !has && !self.shaped_any {
            return; // normal skin, never shaped, leave windows rectangular
        }
        let skin = self.theme.name.clone();
        let main: &[Polygon] = if self.geom.main_shade { &main_sr } else { &main_r };
        self.shape_one(0, "LinAmp", true, main, classic::MAIN_W, self.main_h() as u32, &skin);
        let eq: &[Polygon] = if self.geom.eq_shade { &eq_sr } else { &eq_r };
        let eqv = self.eq_visible;
        self.shape_one(1, "LinAmp Equalizer", eqv, eq, classic::MAIN_W, self.eq_h() as u32, &skin);
        let (plw, plh) = if self.geom.pl_shade {
            (classic::MAIN_W, 14)
        } else {
            (self.plview.px_w as u32, self.plview.px_h as u32)
        };
        let pl: &[Polygon] = if self.geom.pl_shade { &[] } else { &pl_r };
        let plv = self.pl_visible;
        self.shape_one(2, "LinAmp Playlist", plv, pl, plw, plh, &skin);
        self.shaped_any = true;
    }

    fn shape_one(
        &mut self,
        idx: usize,
        title: &str,
        visible: bool,
        polys: &[crate::region::Polygon],
        w: u32,
        h: u32,
        skin: &str,
    ) {
        if !visible {
            // A hidden window's XID is gone; forget its shape so it re-applies when
            // shown again (the new window starts rectangular).
            self.render.shape_sig[idx] = None;
            return;
        }
        use std::hash::{Hash, Hasher};
        let mut hh = std::collections::hash_map::DefaultHasher::new();
        (skin, idx, self.scale.to_bits(), w, h, polys.is_empty()).hash(&mut hh);
        let sig = hh.finish();
        if self.render.shape_sig[idx] == Some(sig) {
            return;
        }
        let s = self.scale;
        let rects: Vec<(i16, i16, u16, u16)> = if polys.is_empty() {
            // Reset to a full rectangle (undoes a prior shape on skin change).
            vec![(0, 0, (w as f32 * s) as u16, (h as f32 * s) as u16)]
        } else {
            crate::region::polygons_to_rects(polys, w, h)
                .into_iter()
                .map(|(x, y, rw, rh)| {
                    (
                        (x as f32 * s) as i16,
                        (y as f32 * s) as i16,
                        (rw as f32 * s).ceil() as u16,
                        (rh as f32 * s).ceil() as u16,
                    )
                })
                .collect()
        };
        set_window_shape(title, &rects);
        self.render.shape_sig[idx] = Some(sig);
    }

    /// The skin cursor name for the region under native coords (nx, ny) of `win`.
    fn cursor_name(&self, win: ClWin, nx: f32, ny: f32) -> &'static str {
        use classic::{Btn, EqRegion, PlRegion};
        match win {
            ClWin::Main => {
                if self.geom.main_shade {
                    return "titlebar";
                }
                match classic::region_at(nx, ny) {
                    Some(Btn::Seek) => "posbar",
                    Some(Btn::Volume) | Some(Btn::Balance) => "volbal",
                    Some(Btn::TitleBar) => "titlebar",
                    Some(Btn::Close) => "close",
                    _ => "normal",
                }
            }
            ClWin::Eq => {
                if self.geom.eq_shade {
                    return "eqtitle";
                }
                match classic::eq_region_at(nx, ny) {
                    Some(EqRegion::Preamp) | Some(EqRegion::Band(_)) => "eqslid",
                    Some(EqRegion::TitleBar) => "eqtitle",
                    Some(EqRegion::Close) => "close",
                    _ => "normal",
                }
            }
            ClWin::Pl => {
                if self.geom.pl_shade {
                    return "ptbar";
                }
                match classic::pl_region_at(nx, ny, self.plview.px_w, self.plview.px_h) {
                    Some(PlRegion::TitleBar) => "ptbar",
                    Some(PlRegion::Resize) => "psize",
                    _ => "pnormal",
                }
            }
        }
    }

    /// Update `win`'s X cursor from the skin's `.cur` set based on the hovered
    /// region, only when it changes. No-op for skins without custom cursors.
    fn update_cursor(&mut self, ctx: &egui::Context, win: ClWin, rect: egui::Rect) {
        if self.prefs.disable_skin_cursors {
            return;
        }
        if !self.classic.as_ref().map(|c| c.has_cursors()).unwrap_or(false) {
            return;
        }
        let Some(hp) = ctx.pointer_hover_pos() else {
            return;
        };
        if !rect.contains(hp) {
            return;
        }
        let idx = win_idx(win);
        let nx = (hp.x - rect.left()) / self.scale;
        let ny = (hp.y - rect.top()) / self.scale;
        let name = self.cursor_name(win, nx, ny);
        if self.render.cur_cursor[idx] == Some(name) {
            return;
        }
        let title = match win {
            ClWin::Main => "LinAmp",
            ClWin::Eq => "LinAmp Equalizer",
            ClWin::Pl => "LinAmp Playlist",
        };
        let fallback = if matches!(win, ClWin::Pl) { "pnormal" } else { "normal" };
        let skin = self.theme.name.clone();
        if let Some(cs) = self.classic.as_ref() {
            if let Some(ci) = cs.cursor(name).or_else(|| cs.cursor(fallback)) {
                set_window_cursor(title, &format!("{skin}:{name}"), ci);
            }
        }
        self.render.cur_cursor[idx] = Some(name);
    }

    /// Apply the skin's base cursor to an auxiliary window (library, editor,
    /// preferences, the jump dialogs) so the custom cursor is uniform across every
    /// window, not just the classic ones. These windows use real egui widgets, so
    /// egui otherwise swaps the cursor per widget; we pin egui's cursor to a
    /// constant (stopping it from fighting us) and reassert the skin cursor each
    /// frame the pointer is inside. Call at the end of the window's viewport
    /// closure, with that viewport's `ctx`.
    fn apply_base_cursor(&self, ctx: &egui::Context, title: &str) {
        if self.prefs.disable_skin_cursors {
            return;
        }
        let Some(cs) = self.classic.as_ref() else {
            return;
        };
        if !cs.has_cursors() || ctx.pointer_hover_pos().is_none() {
            return;
        }
        // Pin egui's cursor to Default so its per-widget cursors (the pointing
        // hand over buttons/links, the text caret in fields) don't show through on
        // these widget-heavy windows, we want the skin cursor everywhere inside
        // them. This runs only when skin cursors are on and the skin has them (the
        // early returns above), so the normal per-widget cursors are untouched
        // when skin cursors are off. On X11 it also stops egui-winit from
        // overwriting the XDefineCursor we set; on Windows the WM_SETCURSOR
        // subclass paints the skin cursor over the Default that winit applies.
        ctx.set_cursor_icon(egui::CursorIcon::Default);
        if let Some(ci) = cs.cursor("normal") {
            set_window_cursor(title, &format!("{}:normal", self.theme.name), ci);
        }
    }

    /// Restore the system cursor on every window, used when the custom-cursor
    /// option is switched off, since our `XDefineCursor` otherwise persists.
    fn clear_skin_cursors(&mut self) {
        for title in [
            "LinAmp",
            "LinAmp Equalizer",
            "LinAmp Playlist",
            "LinAmp Library",
            "LinAmp Preferences",
            "LinAmp, File Info",
            "LinAmp, Jump to file",
            "LinAmp, Jump to time",
            "LinAmp Menu",
            "LinAmp Submenu",
        ] {
            unset_window_cursor(title);
        }
        self.render.cur_cursor = [None; 3];
    }

    fn render_classic(&mut self, ctx: &egui::Context) {
        if self.classic.is_none() {
            return;
        }

        // App-level focus: this frame's root focus OR'd with last frame's child
        // focus (children render later); hidden windows don't hold focus. The
        // shared active/inactive title-bar behaviour is in docs/ARCHITECTURE.md.
        self.geom.foc_root = ctx.input(|i| i.viewport().focused).unwrap_or(self.geom.foc_root);
        if !self.eq_visible {
            self.geom.foc_eq = false;
        }
        if !self.pl_visible {
            self.geom.foc_pl = false;
        }
        if !self.lib_visible {
            self.geom.foc_lib = false;
        }
        // A classic popup menu is part of our app: keep the app considered focused
        // while one is open so the windows stay on their active title bars instead
        // of dimming (the popup is a separate OS window that may take focus).
        self.geom.app_focused = self.geom.foc_root
            || self.geom.foc_eq
            || self.geom.foc_pl
            || self.geom.foc_lib
            || self.popups.cur.is_some();

        // Root viewport = main window.
        self.render_window(ctx, ClWin::Main);
        self.ui_prefs(ctx);
        self.ui_library_window(ctx);
        self.ui_tag_editor(ctx);
        self.ui_jump_file(ctx);
        self.ui_jump_time(ctx);
        self.ui_eq_save_name(ctx);
        self.ui_location(ctx);
        self.ui_minibrowser(ctx);
        self.poll_pending_stream();

        // Initialise window positions, once.
        if !self.geom.positions_init {
            if let Some(r) = ctx.input(|i| i.viewport().outer_rect) {
                if self.geom.has_pos_memory {
                    // Restore the saved layout. If it would sit (partly) off the
                    // current virtual desktop, example: after a resolution drop, nudge
                    // the whole visible cluster back on, prioritising the top-left so
                    // the windows stay grabbable. Shifting every window by the same
                    // delta preserves the docking.
                    if let Some(vr) = virtual_desktop_rect(ctx) {
                        let mut union = self.win_rect_pts(0);
                        if self.eq_visible {
                            union = union.union(self.win_rect_pts(1));
                        }
                        if self.pl_visible {
                            union = union.union(self.win_rect_pts(2));
                        }
                        if self.lib_visible {
                            union = union
                                .union(egui::Rect::from_min_size(self.geom.lib_pos, self.lib_size()));
                        }
                        let delta = egui::vec2(
                            nudge_onto(union.min.x, union.max.x, vr.min.x, vr.max.x),
                            nudge_onto(union.min.y, union.max.y, vr.min.y, vr.max.y),
                        );
                        for p in self.geom.win_pos.iter_mut() {
                            *p += delta;
                        }
                        self.geom.lib_pos += delta;
                    }
                    self.geom.root_last = None; // move the root window to the restored pos
                } else {
                    self.geom.win_pos[0] = r.min;
                    // Dock the EQ/playlist windows under the main one, honouring
                    // which are visible and their collapsed state.
                    self.restack();
                    // Seed the library position (docked right of main) if it was
                    // restored visible but never positioned.
                    if !self.geom.lib_pos_init {
                        self.geom.lib_pos = self.library_default_pos();
                        self.geom.lib_pos_init = true;
                    }
                }
                self.geom.positions_init = true;
            }
        }

        // Apply root position/size only when we changed them (avoids fighting OS).
        let size = self.win_inner_size(ClWin::Main);
        let pos = self.geom.win_pos[0];
        if self.geom.root_last != Some((pos, size)) {
            ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(size));
            if self.geom.positions_init {
                ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(pos));
            }
            self.geom.root_last = Some((pos, size));
        }

        // Keep the root window's always-on-top level in sync (send only on change).
        if self.on_top_last != Some(self.prefs.always_on_top) {
            ctx.send_viewport_cmd(egui::ViewportCommand::WindowLevel(self.window_level()));
            self.on_top_last = Some(self.prefs.always_on_top);
        }

        // Equalizer child window.
        if self.eq_visible {
            let builder = egui::ViewportBuilder::default()
                .with_title("LinAmp Equalizer")
                .with_inner_size(self.win_inner_size(ClWin::Eq))
                .with_position(self.geom.win_pos[1])
                .with_decorations(false)
                .with_resizable(false)
                .with_taskbar(false)
                .with_window_level(self.window_level());
            let id = egui::ViewportId::from_hash_of("linamp_eq");
            ctx.show_viewport_immediate(id, builder, |ctx, _| {
                // Fire global shortcuts while this window has focus (keyboard
                // input goes to the focused viewport, not the root).
                self.handle_shortcuts(ctx, ClWin::Eq);
                self.geom.foc_eq = ctx.input(|i| i.viewport().focused).unwrap_or(self.geom.foc_eq);
                self.render_window(ctx, ClWin::Eq);
            });
        }

        // Playlist child window.
        if self.pl_visible {
            let builder = egui::ViewportBuilder::default()
                .with_title("LinAmp Playlist")
                .with_inner_size(self.win_inner_size(ClWin::Pl))
                .with_position(self.geom.win_pos[2])
                .with_decorations(false)
                .with_resizable(false)
                .with_taskbar(false)
                .with_window_level(self.window_level());
            let id = egui::ViewportId::from_hash_of("linamp_pl");
            ctx.show_viewport_immediate(id, builder, |ctx, _| {
                self.handle_shortcuts(ctx, ClWin::Pl);
                self.geom.foc_pl = ctx.input(|i| i.viewport().focused).unwrap_or(self.geom.foc_pl);
                self.render_window(ctx, ClWin::Pl);
            });
        }

        // The options / playlist menus render in their own (unclipped) window.
        self.draw_popup_viewport(ctx);

        // Keep the EQ/playlist/library windows owned by the main window (z-order
        // group), and feed any running background scan.
        group_child_windows(self.eq_visible, self.pl_visible, self.lib_visible);
        self.apply_window_shapes();
        self.poll_library_scan();
    }

    /// The bottom-bar menu button that should appear pressed: the one held
    /// down, or the one whose fly-out menu is currently open.
    fn pl_pressed_button(&self) -> Option<classic::PlRegion> {
        use classic::PlRegion as P;
        if let Some(r) = self.plview.menu_press {
            return Some(r);
        }
        // The playlist fly-out kinds are only ever opened for the playlist window,
        // so the kind alone identifies a pressed bottom-bar button.
        if let Some((kind, _)) = self.popups.cur {
            return match kind {
                Popup::PlAdd => Some(P::AddMenu),
                Popup::PlRem => Some(P::RemMenu),
                Popup::PlSel => Some(P::SelMenu),
                Popup::PlMisc => Some(P::MiscMenu),
                Popup::PlList => Some(P::ListMenu),
                _ => None,
            };
        }
        None
    }

    // ---- skins / theming ---------------------------------------------------

    fn load_skin_dialog(&mut self, ctx: &egui::Context) {
        hide_popup_window();
        if let Some(path) = self.pick_dialog(|| {
            rfd::FileDialog::new()
                .add_filter("Classic skin", &["wsz", "zip"])
                .pick_file()
        }) {
            self.apply_skin_path(ctx, &path);
        }
    }

    fn ui_skins(&mut self, ui: &mut egui::Ui) {
        egui::CollapsingHeader::new(format!("Skin: {}", self.theme.name))
            .default_open(false)
            .show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    if ui.button("Default").clicked() {
                        self.theme = Theme::default();
                        apply_theme(ui.ctx(), &self.theme);
                        self.push_tray_theme();
                        self.status = "Theme: default".to_string();
                    }
                    if ui.button("Load .wsz...").clicked() {
                        self.load_skin_dialog(ui.ctx());
                    }
                    if ui.button("Rescan").clicked() {
                        self.discover_skins();
                    }
                });
                if self.skins.is_empty() {
                    ui.label(
                        RichText::new("No skins found in ./skins, use \"Load .wsz...\".")
                            .italics()
                            .size(10.0)
                            .color(Color32::GRAY),
                    );
                }
                ui.horizontal_wrapped(|ui| {
                    for path in self.skins.clone() {
                        let name = path
                            .file_stem()
                            .map(|s| s.to_string_lossy().into_owned())
                            .unwrap_or_default();
                        if ui.button(name).clicked() {
                            self.apply_skin_path(ui.ctx(), &path);
                        }
                    }
                });
            });
    }

    // ---- keyboard ----------------------------------------------------------

    // ---- UI sections -------------------------------------------------------

    fn ui_album_art(&mut self, ui: &mut egui::Ui, size: f32) {
        if let Some(tex) = &self.album_art {
            let sized = egui::load::SizedTexture::new(tex.id(), egui::vec2(size, size));
            ui.add(egui::Image::new(sized));
        } else {
            let (resp, painter) =
                ui.allocate_painter(egui::vec2(size, size), egui::Sense::hover());
            painter.rect_filled(resp.rect, 3.0, self.theme.panel);
            painter.text(
                resp.rect.center(),
                egui::Align2::CENTER_CENTER,
                "\u{266B}",
                egui::FontId::proportional(size * 0.5),
                self.theme.lcd_dim,
            );
        }
    }

    fn apply_eq(&mut self) {
        // Push the live settings to the DSP; when disabled the source bypasses
        // all filtering (and the preamp), matching the classic player's behaviour.
        self.player
            .eq()
            .set(self.eq.enabled, self.eq.preamp_db, self.eq.bands);
    }
}

/// Toggle/replace a selection list: ctrl-click toggles, plain click replaces,
/// `None` means the "All" row (clear the selection).
fn apply_sel(sel: &mut Vec<String>, name: Option<String>, ctrl: bool) {
    match name {
        None => sel.clear(),
        Some(n) if ctrl => {
            if let Some(pos) = sel.iter().position(|x| *x == n) {
                sel.remove(pos);
            } else {
                sel.push(n);
            }
        }
        Some(n) => {
            sel.clear();
            sel.push(n);
        }
    }
}

/// An artist/album pane row: a full-width selectable name plus a right-aligned
/// count. Returns the selectable's response.
fn pane_row(
    ui: &mut egui::Ui,
    selected: bool,
    name: &str,
    count: i64,
    text: Color32,
    dim: Color32,
    scale: f32,
) -> egui::Response {
    ui.horizontal(|ui| {
        let w = (ui.available_width() - 34.0 * scale).max(20.0 * scale);
        let r = ui.add_sized(
            [w, 16.0 * scale],
            egui::SelectableLabel::new(selected, RichText::new(name).color(text)),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(RichText::new(count.to_string()).color(dim));
        });
        r
    })
    .inner
}

/// Allocate a full-width row and run `draw(rect, paint)` where `paint(x, width,
/// text, color, right_align)` renders a clipped column. Paints the selection
/// background first. Returns the row's click response.
fn paint_track_row(
    ui: &mut egui::Ui,
    h: f32,
    selected: bool,
    sel_bg: Color32,
    scale: f32,
    draw: impl FnOnce(egui::Rect, &dyn Fn(f32, f32, &str, Color32, bool)),
) -> egui::Response {
    let (rect, resp) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), h), egui::Sense::click());
    if selected {
        ui.painter().rect_filled(rect, 0.0, sel_bg);
    }
    let painter = ui.painter().clone();
    let paint = move |x: f32, width: f32, text: &str, color: Color32, right: bool| {
        use egui::text::{LayoutJob, TextFormat, TextWrapping};
        // Lay the cell out as a single line that truncates with an ellipsis. Inset
        // the text a little on both sides so a long value never runs into, or even
        // touches, the neighbouring column or the resize divider at the boundary.
        let gap = 6.0 * scale; // right gap before the next column
        let lpad = 3.0 * scale; // left gap after the column boundary / divider
        let avail = (width - gap - lpad).max(0.0);
        let mut job = LayoutJob::single_section(
            text.to_owned(),
            TextFormat {
                font_id: egui::FontId::proportional(12.0 * scale),
                color,
                ..Default::default()
            },
        );
        job.wrap = TextWrapping {
            max_width: avail,
            max_rows: 1,
            break_anywhere: true,
            overflow_character: Some('\u{2026}'),
        };
        let galley = painter.layout_job(job);
        let gy = rect.center().y - galley.size().y / 2.0;
        let pos = if right {
            egui::pos2(x + width - 2.0 * scale - galley.size().x, gy)
        } else {
            egui::pos2(x + lpad, gy)
        };
        painter.galley(pos, galley, color);
    };
    draw(rect, &paint);
    resp
}

/// `h:mm:ss` (or `mm:ss`) for a whole-second total.
fn hms(secs: u64) -> String {
    let (h, m, s) = (secs / 3600, (secs % 3600) / 60, secs % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

impl eframe::App for LinAmpApp {
    /// Persist the playlist + playlist-window size for next launch (called
    /// periodically and on exit). The skin is saved separately on change.
    fn save(&mut self, _storage: &mut dyn eframe::Storage) {
        self.build_session().save();
    }

    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let now = Instant::now();
        let dt = (now - self.last_frame).as_secs_f32();
        self.last_frame = now;

        // Tray clicks (#40): act on whatever the notification-area icon reported
        // since the last frame. Collected first so each dispatch can take &mut self.
        let tray_actions: Vec<_> = self.tray_rx.try_iter().collect();
        for action in tray_actions {
            self.handle_tray_action(ctx, action);
        }

        // Global media keys (#165): the keyboard transport keys report the same
        // TrayActions, so route them through the same handler. A media-key
        // PlayPause/Stop/Prev/Next never carries Toggle/Restore/Quit, so this is
        // just the transport subset.
        let media_actions: Vec<_> = self.media_keys_rx.try_iter().collect();
        for action in media_actions {
            self.handle_tray_action(ctx, action);
        }

        // Rip/convert progress: drain the queue worker's messages into the job list.
        self.poll_convert();

        // Primary handover (#193): a secondary polls the instance lock, and when the
        // primary exits and frees it, takes over as the new primary so the open-request
        // queue keeps draining (files handed over meanwhile are still pending, not lost).
        if !self.is_primary && now.duration_since(self.last_lock_try) >= Duration::from_secs(2) {
            self.last_lock_try = now;
            if let Some(lock) = crate::try_lock_instance(&self.lock_path) {
                self.instance_lock = Some(lock);
                self.is_primary = true;
                self.status = "Now the primary instance.".into();
            }
        }

        // Files forwarded from another launch "Open with" (#166): that launch dropped
        // the paths in the open-request queue. Only the primary instance drains it, so
        // handed-over files always land in one deterministic instance (#193). Poll it
        // (throttled) and open them per the "Open with" pref, bringing the window forward.
        if self.is_primary && now.duration_since(self.last_open_check).as_millis() >= 250 {
            self.last_open_check = now;
            let opened = crate::openq::drain();
            if !opened.is_empty() {
                self.open_paths(ctx, opened, true);
            }
        }

        // Startup taskbar suppression (Linux): winit's with_taskbar(false) doesn't
        // set _NET_WM_STATE_SKIP_TASKBAR on X11, so in tray mode send the EWMH hint
        // ourselves. The window isn't in the WM's client list the instant we launch,
        // so re-send (keeping the loop awake) until the deadline, then stop (steady
        // state makes no X calls). Windows handles this at window creation, so this
        // is Linux-only.
        #[cfg(target_os = "linux")]
        if let Some(deadline) = self.tray_taskbar_deadline {
            if self.tray_mode && Instant::now() < deadline {
                set_taskbar_button("LinAmp", false);
                ctx.request_repaint_after(std::time::Duration::from_millis(100));
            } else {
                self.tray_taskbar_deadline = None;
            }
        }

        // Persist the session (playlist, window layout, EQ) whenever it changes,
        // so it survives exit without relying on eframe's save-on-exit.
        self.persist_session_if_changed();

        // Graceful shutdown on a WM close (Alt+F4 / the window "X"): close the
        // child viewports first, then the root a couple of frames later. Closing
        // the root while children still exist crashes winit. Full rationale in
        // docs/ARCHITECTURE.md (Graceful shutdown).
        if self.quit_countdown.is_none() && ctx.input(|i| i.viewport().close_requested()) {
            if self.tray_mode && !self.force_quit {
                // Tray mode: an OS close (Alt+F4, or the taskbar's Close) hides to
                // the tray instead of quitting. Only Exit (which sets force_quit)
                // gets through to the shutdown below.
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                self.hide_to_tray(ctx);
            } else {
                // Persist this instance's prefs and layout as it closes, so with
                // multiple instances the last one to close is the one restored next
                // launch, matching its settings (incl. skin) and window placement
                // (#193). Prefs otherwise only save on change, so without this the
                // persisted config would be "last to change", not "last to close".
                // Save before the teardown below hides the child windows, which must
                // not be recorded as the closed layout.
                self.prefs.save();
                self.build_session().save();
                self.eq_visible = false;
                self.pl_visible = false;
                self.lib_visible = false;
                self.popups.cur = None;
                self.quit_countdown = Some(2);
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            }
        }
        if let Some(n) = self.quit_countdown {
            ctx.request_repaint();
            if n == 0 {
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            } else {
                self.quit_countdown = Some(n - 1);
            }
            return; // don't render (or query) the classic windows mid-teardown
        }

        if self.mode_init {
            let m = self.ui_mode;
            self.set_mode(ctx, m);
            self.mode_init = false;
        }
        if self.player.state() == PlayState::Playing {
            self.marquee_offset = (self.marquee_offset + dt * 4.0) % 100000.0;
        }
        // The classic player flashes the time display while paused (~1 Hz). Toggle the
        // hidden half off the frame clock; is_animating() keeps repaints coming so
        // it blinks, and the per-window fingerprint includes it so the texture
        // actually refreshes on each toggle.
        self.time_blink_hidden = self.player.state() == PlayState::Paused
            && (ctx.input(|i| i.time) / 0.5) as i64 % 2 == 1;

        // Follow the system default output device if it changes mid-session.
        if now.duration_since(self.last_device_check).as_secs_f32() > 1.5 {
            self.player.poll_device();
            self.last_device_check = now;
        }

        self.handle_shortcuts(ctx, ClWin::Main);

        // Advance a running fade-out; stop or pause when it reaches zero, then
        // reset the gain so a later resume or replay is at full volume.
        if let Some((t, end)) = self.fade {
            let t = t - dt;
            if t <= 0.0 {
                match end {
                    FadeEnd::Stop => self.player.stop(),
                    FadeEnd::Pause => self.player.pause(),
                }
                self.player.set_fade(1.0);
                self.fade = None;
            } else {
                self.player.set_fade(t / self.prefs.fade_on_stop_secs.max(0.1));
                self.fade = Some((t, end));
            }
        }

        // Advance any in-progress crossfade, then start one when the current track
        // nears its end (begins the next track overlapping this one's tail).
        self.player.tick_crossfade(dt);
        if self.should_start_crossfade() {
            if let Some(i) = self.playlist.next_index(self.shuffle, self.repeat_all) {
                // A track is playing, so play_index crossfades into it (the old sink
                // keeps fading out). No next track: let it play out and advance below.
                self.play_index(i);
            }
        }

        if self.player.finished() {
            if self.stop_after_current {
                self.player.stop();
                self.stop_after_current = false; // one-shot, like the classic player
            } else if self.repeat_one {
                if let Some(i) = self.playlist.current {
                    self.play_index(i);
                }
            } else {
                self.next();
            }
        }

        // Decode any pending cover art into a GPU texture (needs ctx).
        if self.art_dirty {
            self.album_art = self.art_bytes.as_ref().and_then(|b| load_texture(ctx, b));
            self.art_dirty = false;
        }

        // Classic skin view takes over the whole (borderless) window.
        if self.ui_mode == UiMode::Classic && self.classic.is_some() {
            self.render_classic(ctx);
            // Repaint at ~30fps only when something is moving; otherwise idle and
            // wake slowly (egui still repaints immediately on input).
            let next = if self.is_animating() { 33 } else { 250 };
            ctx.request_repaint_after(Duration::from_millis(next));
            return;
        }

        // Modern fallback window: handle drops here (always appends).
        self.handle_dropped(ctx, None);

        egui::TopBottomPanel::top("player").show(ctx, |ui| {
            ui.add_space(4.0);
            self.ui_display(ui, dt);
            ui.add_space(4.0);
            self.ui_viz(ui);
            ui.add_space(4.0);
            self.ui_seekbar(ui);
            self.ui_transport(ui);
            ui.add_space(2.0);
            self.ui_equalizer(ui);
            self.ui_skins(ui);
        });

        egui::TopBottomPanel::bottom("status").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new(&self.status).size(11.0).color(Color32::GRAY));
            });
        });

        egui::CentralPanel::default().show(ctx, |ui| {
            let prev_view = self.view;
            ui.horizontal(|ui| {
                ui.selectable_value(&mut self.view, View::Playlist, "Playlist");
                ui.selectable_value(&mut self.view, View::Library, "Library");
            });
            if self.view != prev_view && self.view == View::Library {
                self.refresh_library();
            }
            ui.separator();
            match self.view {
                View::Playlist => self.ui_playlist(ui),
                // The library has its own docked window; don't render it a second
                // time in this tab (it would reuse the same panel ids in-frame).
                View::Library if self.lib_visible => {
                    ui.add_space(8.0);
                    ui.label(
                        RichText::new("The Media Library is open in its own window.")
                            .color(Color32::GRAY),
                    );
                }
                View::Library => self.ui_library(ui),
            }
        });

        let interval = if self.is_animating() { 33 } else { 250 };
        ctx.request_repaint_after(Duration::from_millis(interval));
    }
}

/// Upload (or update in place) a composed RGBA frame into a nearest-neighbour
/// texture, returning a handle to draw with.
/// Build a label with the Alt-mnemonic character at char index `pos` underlined
/// (as in the reference dialog). Usable for buttons, checkboxes and tabs.
fn mnem(text: &str, pos: usize) -> egui::text::LayoutJob {
    use egui::text::{LayoutJob, TextFormat};
    // Color32::PLACEHOLDER lets egui substitute the widget's resolved text
    // colour at paint time, so disabled/selected/hover states still apply.
    let mut job = LayoutJob::default();
    for (i, ch) in text.chars().enumerate() {
        let mut fmt = TextFormat::simple(egui::FontId::proportional(14.0), Color32::PLACEHOLDER);
        if i == pos {
            fmt.underline = Stroke::new(1.0_f32, Color32::PLACEHOLDER);
        }
        job.append(&ch.to_string(), 0.0, fmt);
    }
    job
}

fn upload_tex(
    slot: &mut Option<egui::TextureHandle>,
    ctx: &egui::Context,
    img: &image::RgbaImage,
    name: &str,
) -> egui::TextureHandle {
    let color = egui::ColorImage::from_rgba_unmultiplied(
        [img.width() as usize, img.height() as usize],
        img.as_raw(),
    );
    match slot {
        Some(t) => {
            t.set(color, egui::TextureOptions::NEAREST);
            t.clone()
        }
        None => {
            let t = ctx.load_texture(name, color, egui::TextureOptions::NEAREST);
            *slot = Some(t.clone());
            t
        }
    }
}

/// Decode encoded image bytes into an egui texture.
fn load_texture(ctx: &egui::Context, bytes: &[u8]) -> Option<egui::TextureHandle> {
    let img = image::load_from_memory(bytes).ok()?.to_rgba8();
    let size = [img.width() as usize, img.height() as usize];
    let color = egui::ColorImage::from_rgba_unmultiplied(size, img.as_raw());
    Some(ctx.load_texture("cover", color, egui::TextureOptions::LINEAR))
}

// ---- free helpers ----------------------------------------------------------

/// A classic-style on/off toggle rendered as a selectable label.
fn toggle(ui: &mut egui::Ui, value: &mut bool, label: &str) -> egui::Response {
    let mut resp = ui.selectable_label(*value, label);
    if resp.clicked() {
        *value = !*value;
        resp.mark_changed();
    }
    resp
}

#[cfg(test)]
mod tests {
    use super::{crossfade_due, file_name_key, parse_time, reorder_play_next};

    #[test]
    fn crossfade_due_fires_in_the_end_window() {
        // A 100s track with a 5s crossfade: due from 95s on, not before.
        assert!(!crossfade_due(5.0, true, false, false, false, false, Some(100.0), 94.9));
        assert!(crossfade_due(5.0, true, false, false, false, false, Some(100.0), 95.0));
        assert!(crossfade_due(5.0, true, false, false, false, false, Some(100.0), 99.0));
    }

    #[test]
    fn crossfade_due_held_back_by_every_gate() {
        // In the window, but each gate on its own must suppress the start.
        let in_window = |args: (bool, bool, bool, bool, bool)| {
            let (playing, is_cf, stop_after, repeat_one, stop_fading) = args;
            crossfade_due(5.0, playing, is_cf, stop_after, repeat_one, stop_fading, Some(100.0), 99.0)
        };
        assert!(in_window((true, false, false, false, false)), "baseline should fire");
        assert!(!in_window((false, false, false, false, false)), "not playing");
        assert!(!in_window((true, true, false, false, false)), "already crossfading");
        assert!(!in_window((true, false, true, false, false)), "stop after current");
        assert!(!in_window((true, false, false, true, false)), "repeat one");
        assert!(!in_window((true, false, false, false, true)), "stop-fade in progress");
    }

    #[test]
    fn crossfade_due_skips_streams_and_short_tracks() {
        // No known duration (a stream) never crossfades.
        assert!(!crossfade_due(5.0, true, false, false, false, false, None, 999.0));
        // A track shorter than the fade plays in full instead of fading early.
        assert!(!crossfade_due(5.0, true, false, false, false, false, Some(4.0), 3.9));
        // Crossfade disabled (zero length) is always off.
        assert!(!crossfade_due(0.0, true, false, false, false, false, Some(100.0), 99.0));
    }

    #[test]
    fn file_name_key_is_lowercased_basename() {
        use std::path::Path;
        assert_eq!(file_name_key(Path::new("/music/B/Zebra.MP3")), "zebra.mp3");
        assert_eq!(file_name_key(Path::new("/music/A/apple.flac")), "apple.flac");
        // Filename sort ignores the directory, so a file in B/ can precede one in A/.
        let mut names: Vec<String> =
            ["/z/Alpha.mp3", "/a/beta.mp3"].iter().map(|p| file_name_key(Path::new(p))).collect();
        names.sort();
        assert_eq!(names, vec!["alpha.mp3".to_string(), "beta.mp3".to_string()]);
    }

    #[test]
    fn play_next_moves_selection_after_current() {
        let mut v = vec![0, 1, 2, 3, 4];
        // current = 0 (unselected), move rows 3 & 4 to sit right after it.
        let (cur, start, n) = reorder_play_next(&mut v, Some(0), &[3, 4]);
        assert_eq!(v, vec![0, 3, 4, 1, 2]);
        assert_eq!(cur, Some(0));
        assert_eq!(start, 1);
        assert_eq!(n, 2);
    }

    #[test]
    fn play_next_no_current_goes_to_front() {
        let mut v = vec![10, 11, 12];
        let (cur, start, n) = reorder_play_next(&mut v, None, &[2]);
        assert_eq!(v, vec![12, 10, 11]);
        assert_eq!(cur, None);
        assert_eq!(start, 0);
        assert_eq!(n, 1);
    }

    #[test]
    fn play_next_ignores_duplicate_and_out_of_range() {
        let mut v = vec![0, 1, 2, 3];
        // Duplicate 3 and an out-of-range index collapse to a single moved row.
        let (cur, start, n) = reorder_play_next(&mut v, Some(0), &[3, 3, 99]);
        assert_eq!(v, vec![0, 3, 1, 2]);
        assert_eq!(cur, Some(0));
        assert_eq!(start, 1);
        assert_eq!(n, 1);
    }

    #[test]
    fn play_next_current_in_selection_goes_to_front() {
        let mut v = vec![0, 1, 2, 3];
        // current = 2 is itself selected: block [1,2] moves to the front, and
        // current follows to its offset within the block.
        let (cur, start, n) = reorder_play_next(&mut v, Some(2), &[1, 2]);
        assert_eq!(v, vec![1, 2, 0, 3]);
        assert_eq!(start, 0);
        assert_eq!(cur, Some(1));
        assert_eq!(n, 2);
    }

    #[test]
    fn play_next_preserves_order_and_shifts_current() {
        let mut v = vec![0, 1, 2, 3, 4];
        // current = 4, move rows 0 & 2: after removal current shifts to 2, the
        // block is inserted after it in ascending order.
        let (cur, start, n) = reorder_play_next(&mut v, Some(4), &[0, 2]);
        assert_eq!(v, vec![1, 3, 4, 0, 2]);
        assert_eq!(cur, Some(2));
        assert_eq!(start, 3);
        assert_eq!(n, 2);
    }

    #[test]
    fn parse_time_formats() {
        assert_eq!(parse_time("1:23"), Some(83.0));
        assert_eq!(parse_time("0:05"), Some(5.0));
        assert_eq!(parse_time("90"), Some(90.0));
        assert_eq!(parse_time(" 2:00 "), Some(120.0));
        assert_eq!(parse_time(""), None);
        assert_eq!(parse_time("abc"), None);
        assert_eq!(parse_time("-5"), None);
    }

    #[test]
    fn rating_must_sort_numerically_not_by_star_string() {
        // Guard the reason apply_lib_sort sorts the Rating column by the numeric
        // star count: the empty-star glyph (U+2606) sorts AFTER the filled star
        // (U+2605), so sorting the rendered star string would rank a 3-star row
        // above a 5-star one. Numeric sorting is correct; string sorting is not.
        assert!('\u{2606}' > '\u{2605}');
        assert!("\u{2605}\u{2605}\u{2605}\u{2606}\u{2606}" > "\u{2605}\u{2605}\u{2605}\u{2605}\u{2605}");
        assert!(5i64 > 3); // numeric order a descending sort gets right
    }

    #[test]
    fn win_index_roundtrips() {
        use super::{idx_win, win_idx};
        for i in 0..3 {
            assert_eq!(win_idx(idx_win(i)), i);
        }
        assert_eq!(win_idx(idx_win(99)), 2); // out-of-range clamps to the playlist
    }

    #[test]
    fn apply_sel_replace_toggle_clear() {
        use super::apply_sel;
        let mut s: Vec<String> = vec![];
        apply_sel(&mut s, Some("a".into()), false); // plain click replaces
        assert_eq!(s, vec!["a"]);
        apply_sel(&mut s, Some("b".into()), false);
        assert_eq!(s, vec!["b"]);
        apply_sel(&mut s, Some("c".into()), true); // ctrl adds to the selection
        assert_eq!(s, vec!["b", "c"]);
        apply_sel(&mut s, Some("b".into()), true); // ctrl again toggles it off
        assert_eq!(s, vec!["c"]);
        apply_sel(&mut s, None, false); // clicking empty space clears
        assert!(s.is_empty());
    }

    #[test]
    fn hms_formats() {
        use super::hms;
        assert_eq!(hms(0), "0:00");
        assert_eq!(hms(5), "0:05");
        assert_eq!(hms(65), "1:05");
        assert_eq!(hms(3600), "1:00:00");
        assert_eq!(hms(3661), "1:01:01");
    }
}
