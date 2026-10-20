use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use egui::{Color32, Id, Key, RichText, Stroke};
use rustfft::{Fft, FftPlanner};

use crate::classic::{self, Btn, ClassicSkin, MainState, PlayUi};
use crate::config::{Prefs, Session};
use crate::library::Library;
use crate::player::{PlayState, Player};
use crate::playlist::{save_playlist_file, Playlist};
use crate::skin::{self, Theme};
use crate::viz::{self, VizShared};

const FFT_SIZE: usize = 2048;
const VIZ_BANDS: usize = 28;
/// Fewer, wider bars for the small classic analyzer (Winamp-like).
const CLASSIC_BANDS: usize = 19;
/// Duration of the stop-with-fade (Shift+V) fade-out, in seconds.
const FADE_SECS: f32 = 2.0;
/// Magnetic docking snap distance in points.
const SNAP: f32 = 16.0;

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

/// Window size (points) for a popup menu, from its item/separator count.
fn popup_dims(kind: Popup) -> (f32, f32) {
    let (buttons, seps) = match kind {
        Popup::Options => (9, 3),
        Popup::PlAdd => (2, 0),
        Popup::PlRem => (3, 0),
        Popup::PlSel => (3, 0),
        Popup::PlMisc => (4, 0),
        Popup::PlList => (3, 0),
        Popup::EqPresets => (crate::eq::PRESETS.len() as i32, 0),
    };
    (182.0, buttons as f32 * 24.0 + seps as f32 * 9.0 + 16.0)
}

/// If `r` overlaps `keep` (the main window), the position `r` should move to so it
/// no longer covers it — pushed to `keep`'s nearest edge (least movement). `None`
/// when they don't overlap (flush/touching counts as not overlapping).
fn push_off(r: egui::Rect, keep: egui::Rect) -> Option<egui::Pos2> {
    let separated = r.right() <= keep.left()
        || r.left() >= keep.right()
        || r.bottom() <= keep.top()
        || r.top() >= keep.bottom();
    if separated {
        return None;
    }
    use egui::vec2;
    // Resolve the overlap by the least movement, but favour below over above: only
    // go above when the window is *mostly* above `keep` (its centre is above the
    // top edge); otherwise the vertical escape is downward. Left/right compete on
    // least movement (no left/right preference), and we pick whichever axis — the
    // chosen vertical or the nearest horizontal — needs the least movement.
    let vpush = if r.center().y < keep.top() {
        keep.top() - r.bottom() // up: window is mostly above
    } else {
        keep.bottom() - r.top() // down: favour below
    };
    let hpush = if (keep.left() - r.right()).abs() < (keep.right() - r.left()).abs() {
        keep.left() - r.right() // out to the left
    } else {
        keep.right() - r.left() // out to the right
    };
    let push = if vpush.abs() <= hpush.abs() {
        vec2(0.0, vpush)
    } else {
        vec2(hpush, 0.0)
    };
    Some(r.min + push)
}

/// Two windows are docked when an edge is flush (within tolerance) and they
/// overlap on the shared axis.
fn rects_docked(a: egui::Rect, b: egui::Rect) -> bool {
    let tol = 3.0;
    let x_overlap = a.left() < b.right() - 1.0 && a.right() > b.left() + 1.0;
    let y_overlap = a.top() < b.bottom() - 1.0 && a.bottom() > b.top() + 1.0;
    let v_adj =
        x_overlap && ((a.bottom() - b.top()).abs() <= tol || (a.top() - b.bottom()).abs() <= tol);
    let h_adj =
        y_overlap && ((a.right() - b.left()).abs() <= tol || (a.left() - b.right()).abs() <= tol);
    v_adj || h_adj
}

/// Which of the three windows are transitively docked to the main window (0).
fn dock_group_from_main(rects: &[egui::Rect; 3], visible: &[bool; 3]) -> [bool; 3] {
    let mut in_group = [true, false, false];
    let mut changed = true;
    while changed {
        changed = false;
        for j in [1usize, 2usize] {
            if in_group[j] || !visible[j] {
                continue;
            }
            if (0..3).any(|k| in_group[k] && rects_docked(rects[j], rects[k])) {
                in_group[j] = true;
                changed = true;
            }
        }
    }
    in_group
}

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

/// An in-progress title-bar drag of a classic window.
///
/// On Windows (where we can read the global cursor) we drive the move ourselves
/// each frame so docked windows move in tandem, live. Elsewhere we fall back to
/// the OS window-drag (ViewportCommand::StartDrag), which snaps the group on
/// release. `win_pos` stays authoritative and is snapped on release either way.
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
}

/// Global cursor position in egui points, if the platform exposes it.
///
/// - **Windows:** `GetCursorPos`.
/// - **Linux/X11 (incl. XWayland):** `XQueryPointer` on the root window.
/// - **Native Wayland / everything else:** `None` — the compositor doesn't
///   expose a global cursor, so the caller falls back to the OS window-drag.
///
/// When this returns `Some`, the drag runs in "manual" mode: we drive the
/// window position ourselves each frame and move the docked group in tandem.
fn global_cursor_points(ctx: &egui::Context) -> Option<egui::Pos2> {
    #[cfg(windows)]
    {
        #[repr(C)]
        struct Point {
            x: i32,
            y: i32,
        }
        extern "system" {
            fn GetCursorPos(p: *mut Point) -> i32;
        }
        let mut pt = Point { x: 0, y: 0 };
        if unsafe { GetCursorPos(&mut pt) } != 0 {
            let ppp = ctx.pixels_per_point().max(0.1);
            return Some(egui::pos2(pt.x as f32 / ppp, pt.y as f32 / ppp));
        }
        None
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        x11util::cursor_px().map(|(x, y)| {
            let ppp = ctx.pixels_per_point().max(0.1);
            egui::pos2(x as f32 / ppp, y as f32 / ppp)
        })
    }
    #[cfg(not(any(windows, all(unix, not(target_os = "macos")))))]
    {
        let _ = ctx;
        None
    }
}

/// Linux/X11 helpers used for classic-window management: the global cursor query
/// (for manual tandem dragging) and `WM_TRANSIENT_FOR` window grouping (so the
/// EQ/playlist windows ride the main window's z-order, raise with it, and don't
/// get their own taskbar entry — the X11 analogue of the Windows owner-window
/// grouping). Everything degrades to a no-op when there is no X display (e.g.
/// native Wayland), so the caller falls back to OS behaviour there.
#[cfg(all(unix, not(target_os = "macos")))]
mod x11util {
    use std::cell::RefCell;
    use std::ffi::CString;
    use std::os::raw::{c_int, c_long, c_uchar, c_ulong};
    use std::ptr;
    use x11_dl::xlib;

    enum Conn {
        Untried,
        Unavailable,
        Ready(xlib::Xlib, *mut xlib::Display),
    }

    thread_local! {
        static CONN: RefCell<Conn> = RefCell::new(Conn::Untried);
    }

    /// Run `f` with the cached Xlib handle + display, opening the connection on
    /// first use. `None` when there is no X display.
    fn with_conn<R>(f: impl FnOnce(&xlib::Xlib, *mut xlib::Display) -> R) -> Option<R> {
        CONN.with(|cell| {
            if matches!(*cell.borrow(), Conn::Untried) {
                let next = match xlib::Xlib::open() {
                    Ok(xlib) => {
                        let d = unsafe { (xlib.XOpenDisplay)(ptr::null()) };
                        if d.is_null() {
                            Conn::Unavailable
                        } else {
                            Conn::Ready(xlib, d)
                        }
                    }
                    Err(_) => Conn::Unavailable,
                };
                *cell.borrow_mut() = next;
            }
            match &*cell.borrow() {
                Conn::Ready(xlib, d) => Some(f(xlib, *d)),
                _ => None,
            }
        })
    }

    /// Global pointer position in physical pixels, or `None` if unavailable.
    /// Called only during an active title-bar drag.
    pub fn cursor_px() -> Option<(i32, i32)> {
        with_conn(|xlib, d| unsafe {
            let root = (xlib.XDefaultRootWindow)(d);
            let (mut rr, mut cr): (xlib::Window, xlib::Window) = (0, 0);
            let (mut rx, mut ry, mut wx, mut wy) = (0i32, 0i32, 0i32, 0i32);
            let mut mask: u32 = 0;
            let ok = (xlib.XQueryPointer)(
                d, root, &mut rr, &mut cr, &mut rx, &mut ry, &mut wx, &mut wy, &mut mask,
            );
            // False only means the pointer is on another screen; rx/ry stay valid
            // global coords, but we treat it as unknown and fall back to OS drag.
            (ok != 0).then_some((rx, ry))
        })
        .flatten()
    }

    unsafe fn intern(xlib: &xlib::Xlib, d: *mut xlib::Display, name: &str) -> xlib::Atom {
        let c = CString::new(name).unwrap();
        (xlib.XInternAtom)(d, c.as_ptr(), 0)
    }

    /// The root's `_NET_CLIENT_LIST` — the WM's managed top-level windows.
    unsafe fn client_list(xlib: &xlib::Xlib, d: *mut xlib::Display) -> Vec<xlib::Window> {
        let root = (xlib.XDefaultRootWindow)(d);
        let prop = intern(xlib, d, "_NET_CLIENT_LIST");
        const XA_WINDOW: xlib::Atom = 33;
        let (mut ty, mut fmt): (xlib::Atom, c_int) = (0, 0);
        let (mut nitems, mut after): (c_ulong, c_ulong) = (0, 0);
        let mut data: *mut c_uchar = ptr::null_mut();
        let st = (xlib.XGetWindowProperty)(
            d, root, prop, 0, 4096, 0, XA_WINDOW, &mut ty, &mut fmt, &mut nitems, &mut after,
            &mut data,
        );
        let mut out = Vec::new();
        if st == 0 && !data.is_null() && fmt == 32 {
            let p = data as *const c_ulong;
            for i in 0..nitems as isize {
                out.push(*p.offset(i) as xlib::Window);
            }
        }
        if !data.is_null() {
            (xlib.XFree)(data as *mut _);
        }
        out
    }

    /// A window's `_NET_WM_NAME` (UTF-8 title), or empty if unset.
    unsafe fn wm_name(xlib: &xlib::Xlib, d: *mut xlib::Display, w: xlib::Window) -> String {
        let prop = intern(xlib, d, "_NET_WM_NAME");
        let utf8 = intern(xlib, d, "UTF8_STRING");
        let (mut ty, mut fmt): (xlib::Atom, c_int) = (0, 0);
        let (mut nitems, mut after): (c_ulong, c_ulong) = (0, 0);
        let mut data: *mut c_uchar = ptr::null_mut();
        let st = (xlib.XGetWindowProperty)(
            d, w, prop, 0, 1024, 0, utf8, &mut ty, &mut fmt, &mut nitems, &mut after, &mut data,
        );
        let mut s = String::new();
        if st == 0 && !data.is_null() {
            s = String::from_utf8_lossy(std::slice::from_raw_parts(data, nitems as usize))
                .into_owned();
        }
        if !data.is_null() {
            (xlib.XFree)(data as *mut _);
        }
        s
    }

    /// Error handler that swallows async X errors (e.g. a `BadWindow` from a
    /// window destroyed mid-scan) so a stray property query can never abort the
    /// process via Xlib's default handler.
    unsafe extern "C" fn ignore_err(
        _d: *mut xlib::Display,
        _e: *mut xlib::XErrorEvent,
    ) -> c_int {
        0
    }

    /// Ask the WM to drop `win` from the taskbar via `_NET_WM_STATE_SKIP_TASKBAR`.
    /// Sent as an EWMH client message (the correct way to change state on a window
    /// that is already mapped) so the EQ/playlist windows don't each get their own
    /// taskbar entry — only the main window represents the group.
    unsafe fn skip_taskbar(xlib: &xlib::Xlib, d: *mut xlib::Display, win: xlib::Window) {
        let root = (xlib.XDefaultRootWindow)(d);
        let net_wm_state = intern(xlib, d, "_NET_WM_STATE");
        let skip = intern(xlib, d, "_NET_WM_STATE_SKIP_TASKBAR");
        const NET_WM_STATE_ADD: c_long = 1;
        let mut ev: xlib::XEvent = std::mem::zeroed();
        {
            let cm = &mut ev.client_message;
            cm.type_ = xlib::ClientMessage;
            cm.send_event = 1;
            cm.display = d;
            cm.window = win;
            cm.message_type = net_wm_state;
            cm.format = 32;
            cm.data.set_long(0, NET_WM_STATE_ADD);
            cm.data.set_long(1, skip as c_long);
            cm.data.set_long(2, 0);
            cm.data.set_long(3, 1); // source indication: normal application
            cm.data.set_long(4, 0);
        }
        let mask = xlib::SubstructureRedirectMask | xlib::SubstructureNotifyMask;
        (xlib.XSendEvent)(d, root, 0, mask, &mut ev);
    }

    /// Link the EQ/playlist windows to the main window via `WM_TRANSIENT_FOR`.
    /// Scans and applies only while the links aren't yet in place for the current
    /// set of visible windows; steady state makes no X calls at all. Re-links
    /// after a show/hide (which recreates the child window with a new XID).
    pub fn group(eq_visible: bool, pl_visible: bool) {
        thread_local! {
            // (eq_visible, pl_visible, linked?) we last acted on.
            static STATE: RefCell<(bool, bool, bool)> = RefCell::new((false, false, false));
        }
        let cfg = (eq_visible, pl_visible);
        let done = STATE.with(|s| {
            let mut s = s.borrow_mut();
            if (s.0, s.1) != cfg {
                *s = (cfg.0, cfg.1, false); // visibility changed -> re-link
            }
            s.2
        });
        if done {
            return;
        }

        let linked = with_conn(|xlib, d| unsafe {
            // Guard the whole scan: querying a window another client just
            // destroyed would otherwise fire Xlib's fatal default handler.
            let prev = (xlib.XSetErrorHandler)(Some(ignore_err));
            let clients = client_list(xlib, d);
            let (mut main, mut eq, mut pl) = (0, 0, 0);
            for &w in &clients {
                match wm_name(xlib, d, w).as_str() {
                    "LinAmp" => main = w,
                    "LinAmp Equalizer" => eq = w,
                    "LinAmp Playlist" => pl = w,
                    _ => {}
                }
            }
            let mut ok = main != 0;
            if main != 0 {
                if eq_visible {
                    if eq != 0 {
                        (xlib.XSetTransientForHint)(d, eq, main);
                        skip_taskbar(xlib, d, eq);
                    } else {
                        ok = false;
                    }
                }
                if pl_visible {
                    if pl != 0 {
                        (xlib.XSetTransientForHint)(d, pl, main);
                        skip_taskbar(xlib, d, pl);
                    } else {
                        ok = false;
                    }
                }
            }
            (xlib.XSync)(d, 0); // deliver any errors to ignore_err before restoring
            (xlib.XSetErrorHandler)(prev);
            ok
        })
        .unwrap_or(false);

        if linked {
            STATE.with(|s| s.borrow_mut().2 = true);
        }
    }
}

/// Make the EQ/playlist windows *owned* by the main window so they ride its
/// z-order as a group (rise together, stay above it, hide when it minimizes) —
/// Winamp-style window grouping. Idempotent and cheap, so it runs each frame to
/// cover windows being shown/hidden (which recreates them). Implemented on
/// Windows (owner window) and Linux/X11 (`WM_TRANSIENT_FOR`); a no-op elsewhere.
fn group_child_windows(eq_visible: bool, pl_visible: bool) {
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        x11util::group(eq_visible, pl_visible);
    }
    #[cfg(windows)]
    {
        let _ = (eq_visible, pl_visible);
        #[allow(non_snake_case)]
        extern "system" {
            fn FindWindowW(class: *const u16, title: *const u16) -> isize;
            fn SetWindowLongPtrW(hwnd: isize, index: i32, new: isize) -> isize;
            fn GetWindowLongPtrW(hwnd: isize, index: i32) -> isize;
        }
        const GWLP_HWNDPARENT: i32 = -8;
        let wide = |s: &str| -> Vec<u16> { s.encode_utf16().chain(std::iter::once(0)).collect() };
        unsafe {
            let main = FindWindowW(std::ptr::null(), wide("LinAmp").as_ptr());
            if main == 0 {
                return;
            }
            for title in ["LinAmp Equalizer", "LinAmp Playlist"] {
                let child = FindWindowW(std::ptr::null(), wide(title).as_ptr());
                if child != 0 && GetWindowLongPtrW(child, GWLP_HWNDPARENT) != main {
                    SetWindowLongPtrW(child, GWLP_HWNDPARENT, main);
                }
            }
        }
    }
    #[cfg(not(any(windows, all(unix, not(target_os = "macos")))))]
    {
        let _ = (eq_visible, pl_visible);
    }
}

/// Popup menus in classic mode (options + the playlist bottom-bar fly-outs).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Popup {
    Options,
    PlAdd,
    PlRem,
    PlSel,
    PlMisc,
    PlList,
    EqPresets,
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
}

impl SortKey {
    fn label(self) -> &'static str {
        match self {
            SortKey::Custom => "Manual order",
            SortKey::Title => "Title",
            SortKey::Artist => "Artist",
            SortKey::Album => "Album",
            SortKey::Duration => "Duration",
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
    eq_enabled: bool,
    eq_preamp_db: f32,
    eq_bands: [f32; 10],

    // playlist / library views
    view: View,
    pl_search: String,
    sort_key: SortKey,
    lib_query: String,
    lib_results: Vec<crate::playlist::Track>,

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
    classic_tex: Option<egui::TextureHandle>,
    eq_tex: Option<egui::TextureHandle>,
    pl_tex: Option<egui::TextureHandle>,
    active_region: Option<Btn>,
    eq_active: Option<classic::EqRegion>,
    pressed_btn: Option<Btn>,
    classic_seek_preview: Option<f32>,
    balance: f32,
    time_remaining: bool,
    /// Stop when the current track ends instead of advancing (Ctrl+V).
    stop_after_current: bool,
    /// Remaining fade-out time in seconds while stopping with fade (Shift+V).
    fade: Option<f32>,
    mode_init: bool,
    eq_visible: bool,
    pl_visible: bool,
    eq_auto: bool,
    pl_first: usize,
    pl_selection: HashSet<usize>,
    pl_last_click: Option<(usize, Instant)>,
    popup: Option<(Popup, ClWin, egui::Pos2)>,
    popup_fresh: bool,
    main_shade: bool,
    eq_shade: bool,
    pl_shade: bool,
    tb_last_click: Option<(ClWin, Instant)>,
    /// Last click on the main time display, for double-click elapsed/remaining.
    time_last_click: Option<Instant>,
    // multi-window geometry (points)
    win_pos: [egui::Pos2; 3],
    pl_px_h: f32,
    pl_px_w: f32,
    positions_init: bool,
    /// Hash of the last-persisted session, so we only write on actual changes.
    session_sig: Option<u64>,
    /// Frames remaining in a graceful shutdown (set when the WM asks to close),
    /// used to tear down child viewports before the root to avoid a winit crash.
    quit_countdown: Option<u8>,
    drag: Option<Dragging>,
    /// While resizing the playlist, the grab offsets (width, height) in px.
    resize_pl: Option<egui::Vec2>,
    root_last: Option<(egui::Pos2, egui::Vec2)>,
    pl_menu_press: Option<classic::PlRegion>,
    prefs: Prefs,
    /// Path of the currently loaded skin (persisted as `last_skin`).
    skin_path: Option<PathBuf>,
    scale: f32,
    prefs_open: bool,
    library_open: bool,
    tag_editor: Option<TagEditor>,
    /// Jump-to-file search state (Some while the dialog is open).
    jump_file: Option<JumpFile>,
    /// Jump-to-time input buffer (Some while the dialog is open).
    jump_time: Option<String>,

    marquee_offset: f32,
    last_frame: Instant,
    last_device_check: Instant,
    status: String,
}

/// Keys pressed this frame, read out of the input lock before we touch `self`.
#[derive(Default)]
struct KeyState {
    play: bool,
    pause: bool,
    stop: bool,
    next: bool,
    prev: bool,
    toggle: bool,
    open: bool,
    repeat: bool,
    shuffle: bool,
    vol_up: bool,
    vol_down: bool,
    seek_fwd: bool,
    seek_back: bool,
    ctrl: bool,
    alt: bool,
    shift: bool,
    key_d: bool,
    key_p: bool,
    key_t: bool,
    key_3: bool,
    key_e: bool,
    key_g: bool,
    key_a: bool,
    key_j: bool,
    del: bool,
    insert: bool,
    enter: bool,
}

/// State for the file-info / tag editor window.
struct TagEditor {
    index: usize,
    path: PathBuf,
    fields: crate::tags::TagFields,
    orig: crate::tags::TagFields,
    editable: bool,
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

impl LinAmpApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let player = Player::new().expect("failed to open audio output device");
        let viz = player.viz();
        let library = Library::open().ok();
        let fft = FftPlanner::<f32>::new().plan_fft_forward(FFT_SIZE);
        let prefs = Prefs::load();
        let scale = prefs.scale();

        let mut app = Self {
            player,
            playlist: Playlist::default(),
            library,
            shuffle: false,
            repeat_all: true,
            repeat_one: false,
            seeking: false,
            seek_preview: 0.0,
            eq_enabled: false,
            eq_preamp_db: 0.0,
            eq_bands: [0.0; 10],
            view: View::Playlist,
            pl_search: String::new(),
            sort_key: SortKey::Custom,
            lib_query: String::new(),
            lib_results: Vec::new(),
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
            classic_tex: None,
            eq_tex: None,
            pl_tex: None,
            active_region: None,
            eq_active: None,
            pressed_btn: None,
            classic_seek_preview: None,
            balance: 0.0,
            time_remaining: false,
            stop_after_current: false,
            fade: None,
            mode_init: true,
            eq_visible: false,
            pl_visible: false,
            eq_auto: false,
            pl_first: 0,
            pl_selection: HashSet::new(),
            pl_last_click: None,
            popup: None,
            popup_fresh: false,
            main_shade: false,
            eq_shade: false,
            pl_shade: false,
            tb_last_click: None,
            time_last_click: None,
            win_pos: [egui::pos2(100.0, 100.0); 3],
            pl_px_h: classic::PL_H as f32,
            pl_px_w: classic::MAIN_W as f32,
            positions_init: false,
            session_sig: None,
            quit_countdown: None,
            drag: None,
            resize_pl: None,
            root_last: None,
            pl_menu_press: None,
            prefs,
            skin_path: None,
            scale,
            prefs_open: false,
            library_open: false,
            tag_editor: None,
            jump_file: None,
            jump_time: None,
            marquee_offset: 0.0,
            last_frame: Instant::now(),
            last_device_check: Instant::now(),
            status: "Ready. Press L to open files.".to_string(),
        };

        // Accept files/folders/playlists passed on the command line.
        let mut had_cli = false;
        for arg in std::env::args().skip(1) {
            let p = PathBuf::from(arg);
            had_cli = true;
            if p.is_dir() {
                app.playlist.add_folder(&p);
            } else if matches!(
                p.extension().and_then(|e| e.to_str()).map(|e| e.to_lowercase()).as_deref(),
                Some("m3u") | Some("m3u8") | Some("pls")
            ) {
                app.playlist.load_file(&p);
            } else {
                app.playlist.add_file(p);
            }
        }
        if had_cli {
            if !app.playlist.is_empty() {
                app.play_index(0);
            }
        } else {
            // Restore last session's playlist/window size (but don't auto-play).
            let session = Session::load();
            for t in &session.tracks {
                app.playlist.add_file(PathBuf::from(t));
            }
            app.playlist.current = session.current.filter(|&i| i < app.playlist.len());
            if app.prefs.tag_on_load {
                app.playlist.load_all();
            }
            if let Some(w) = session.pl_width {
                app.pl_px_w = w.max(classic::MAIN_W as f32);
            }
            if let Some(h) = session.pl_height {
                app.pl_px_h = h.max(87.0);
            }
            // Restore which windows were open and their collapsed/full state.
            // Positions aren't remembered; positions_init docks them on frame 1.
            app.eq_visible = session.eq_visible;
            app.pl_visible = session.pl_visible;
            app.main_shade = session.main_shade;
            app.eq_shade = session.eq_shade;
            app.pl_shade = session.pl_shade;
        }
        app.refresh_library();
        app.discover_skins();

        // Restore equalizer state from the last session and push it to the DSP.
        let eq_session = Session::load();
        if !eq_session.eq_bands.is_empty() {
            app.eq_enabled = eq_session.eq_enabled;
            app.eq_auto = eq_session.eq_auto;
            app.eq_preamp_db = eq_session.eq_preamp;
            for (i, b) in eq_session.eq_bands.iter().take(10).enumerate() {
                app.eq_bands[i] = *b;
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
        app
    }

    // ---- transport helpers -------------------------------------------------

    fn play_index(&mut self, idx: usize) {
        self.playlist.ensure_loaded(idx);
        if let Some(track) = self.playlist.tracks.get(idx).cloned() {
            match self.player.play_file(&track.path) {
                Ok(()) => {
                    self.playlist.current = Some(idx);
                    self.seek_preview = 0.0;
                    self.marquee_offset = 0.0;
                    self.apply_eq();
                    self.art_bytes = crate::playlist::read_cover(&track.path);
                    self.art_dirty = true;
                    self.status = format!("Playing: {}", track.display_name());
                }
                Err(e) => self.status = format!("Error: {e}"),
            }
        }
    }

    fn play_pause(&mut self) {
        match self.player.state() {
            PlayState::Stopped => {
                if !self.playlist.is_empty() {
                    let start = self.playlist.current.unwrap_or(0);
                    self.play_index(start);
                }
            }
            _ => self.player.toggle_pause(),
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
        }
        self.restore_current(cur);
    }

    // ---- file dialogs ------------------------------------------------------

    fn open_files(&mut self) {
        if let Some(paths) = rfd::FileDialog::new()
            .add_filter(
                "Audio",
                &["mp3", "flac", "ogg", "oga", "wav", "m4a", "aac", "opus", "wv", "aiff", "aif"],
            )
            .pick_files()
        {
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

    fn maybe_scan_tags(&mut self) {
        if self.prefs.tag_on_load {
            self.playlist.load_all();
        }
    }

    /// Append any files/folders/playlists dropped onto this viewport's window.
    fn handle_dropped(&mut self, ctx: &egui::Context) {
        let dropped: Vec<PathBuf> = ctx.input(|i| {
            i.raw.dropped_files.iter().filter_map(|f| f.path.clone()).collect()
        });
        if dropped.is_empty() {
            return;
        }
        let was_empty = self.playlist.is_empty();
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
        self.maybe_scan_tags();
        if was_empty && !self.playlist.is_empty() {
            self.play_index(0);
        }
        self.status = format!("{} track(s) in playlist.", self.playlist.len());
    }

    fn open_folder(&mut self) {
        if let Some(dir) = rfd::FileDialog::new().pick_folder() {
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
        if let Some(path) = rfd::FileDialog::new()
            .add_filter("M3U playlist", &["m3u"])
            .add_filter("PLS playlist", &["pls"])
            .set_file_name("playlist.m3u")
            .save_file()
        {
            match save_playlist_file(&path, &self.playlist.tracks) {
                Ok(()) => self.status = format!("Saved {}", path.display()),
                Err(e) => self.status = format!("Save error: {e}"),
            }
        }
    }

    fn load_playlist_dialog(&mut self) {
        if let Some(path) = rfd::FileDialog::new()
            .add_filter("Playlists", &["m3u", "m3u8", "pls"])
            .pick_file()
        {
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

    fn refresh_library(&mut self) {
        if let Some(lib) = &self.library {
            let q = self.lib_query.trim();
            self.lib_results = if q.is_empty() {
                lib.all(1000)
            } else {
                lib.search(q, 1000)
            };
        }
    }

    fn scan_into_library(&mut self) {
        if let Some(dir) = rfd::FileDialog::new().pick_folder() {
            if let Some(lib) = &self.library {
                match lib.scan_folder(&dir) {
                    Ok(n) => self.status = format!("Scanned {n} file(s) into library."),
                    Err(e) => self.status = format!("Scan error: {e}"),
                }
            }
            self.refresh_library();
        }
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
                    // De-duplicate by file name so the same skin found in a
                    // second location (e.g. next to the exe) isn't listed twice;
                    // the first directory searched (./skins) wins.
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
                self.status = format!("Loaded skin: {}", self.theme.name);
            }
            Err(e) => self.status = format!("Skin error: {e}"),
        }
        // Load the classic sprite sheets for the same skin.
        self.classic = ClassicSkin::load(path).ok();
        self.classic_tex = None;
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
                self.root_last = None;
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
        if let Some(t) = self.playlist.current_track() {
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
            focused: true,
            play,
            pressed: self.pressed_btn,
            shuffle: self.shuffle,
            repeat: self.repeat_all,
            eq_on: self.eq_visible,
            pl_on: self.pl_visible,
            pos,
            duration: dur,
            time_remaining: self.time_remaining,
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
            VizMode::Spectrum => {
                self.viz.snapshot(&mut self.viz_samples, FFT_SIZE);
                let rate = self.viz.rate();
                let bars =
                    viz::compute_bars(&self.viz_samples, &*self.fft, FFT_SIZE, CLASSIC_BANDS, rate);
                for i in 0..CLASSIC_BANDS {
                    let target = bars.get(i).copied().unwrap_or(0.0);
                    let cur = self.cviz_bars[i];
                    // Smoothed rise (not instant) and a steady fall toward the
                    // current level, so it isn't so jumpy or sticky.
                    let b = if target > cur {
                        cur + (target - cur) * 0.55
                    } else {
                        (cur - 0.045).max(target)
                    };
                    self.cviz_bars[i] = b;
                    // Peak marker: holds the max, then falls slowly.
                    self.cviz_peaks[i] = if b > self.cviz_peaks[i] {
                        b
                    } else {
                        (self.cviz_peaks[i] - 0.012).max(0.0)
                    };
                }
            }
            VizMode::Oscilloscope => {
                self.viz.snapshot(&mut self.viz_samples, 76);
            }
            VizMode::Off => {}
        }
    }

    /// Height in native px of each classic window given shade state.
    fn main_h(&self) -> f32 {
        if self.main_shade { 14.0 } else { classic::MAIN_H as f32 }
    }
    fn eq_h(&self) -> f32 {
        if self.eq_shade { 14.0 } else { classic::EQ_H as f32 }
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
                    let frac = ((lx - 16.0) / (248.0 - 29.0)).clamp(0.0, 1.0);
                    self.classic_seek_preview = Some(frac * dur);
                }
            }
            Btn::Volume => {
                let frac = ((lx - 107.0) / (68.0 - 14.0)).clamp(0.0, 1.0);
                self.player.set_volume(frac);
            }
            Btn::Balance => {
                let t = ((lx - 177.0) / (38.0 - 14.0)).clamp(0.0, 1.0);
                let b = t * 2.0 - 1.0;
                // Snap to dead-centre, as Winamp does.
                self.balance = if b.abs() < 0.06 { 0.0 } else { b };
                self.player.set_balance(self.balance);
            }
            _ => {}
        }
    }

    /// Stop playback — fading out first when the "fade out when stopping" pref is
    /// on (otherwise an immediate cut). Shift+V always forces a fade regardless.
    fn stop_playback(&mut self) {
        if self.prefs.fade_on_stop && self.player.state() != PlayState::Stopped {
            self.fade = Some(FADE_SECS);
            self.status = "Fading out…".into();
        } else {
            self.player.stop();
            self.fade = None;
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
            pl_width: Some(self.pl_px_w),
            pl_height: Some(self.pl_px_h),
            eq_enabled: self.eq_enabled,
            eq_auto: self.eq_auto,
            eq_preamp: self.eq_preamp_db,
            eq_bands: self.eq_bands.to_vec(),
            eq_visible: self.eq_visible,
            pl_visible: self.pl_visible,
            main_shade: self.main_shade,
            eq_shade: self.eq_shade,
            pl_shade: self.pl_shade,
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
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        self.eq_visible.hash(&mut h);
        self.pl_visible.hash(&mut h);
        self.main_shade.hash(&mut h);
        self.eq_shade.hash(&mut h);
        self.pl_shade.hash(&mut h);
        self.pl_px_w.to_bits().hash(&mut h);
        self.pl_px_h.to_bits().hash(&mut h);
        self.playlist.current.hash(&mut h);
        self.eq_enabled.hash(&mut h);
        self.eq_auto.hash(&mut h);
        self.eq_preamp_db.to_bits().hash(&mut h);
        for b in &self.eq_bands {
            b.to_bits().hash(&mut h);
        }
        for t in &self.playlist.tracks {
            t.path.hash(&mut h);
        }
        let sig = h.finish();
        if self.session_sig != Some(sig) {
            self.session_sig = Some(sig);
            self.build_session().save();
        }
    }

    fn trigger_classic(&mut self, ctx: &egui::Context, region: Btn) {
        match region {
            Btn::Prev => self.prev(),
            Btn::Play => self.play_pause(),
            Btn::Pause => self.player.pause(),
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
            Btn::Close => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
            Btn::Minimize => ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true)),
            Btn::Time => self.time_remaining = !self.time_remaining,
            Btn::Viz => self.viz_mode = self.viz_mode.next(),
            Btn::Seek | Btn::Volume | Btn::Balance | Btn::TitleBar | Btn::Options => {}
        }
    }

    fn make_eq_state(&self) -> classic::EqState {
        classic::EqState {
            focused: true,
            on: self.eq_enabled,
            auto: self.eq_auto,
            pressed: self.eq_active,
            preamp: (self.eq_preamp_db / 12.0).clamp(-1.0, 1.0),
            bands: self.eq_bands.map(|d| (d / 12.0).clamp(-1.0, 1.0)),
        }
    }

    fn build_pl_rows(&self) -> Vec<classic::PlRow> {
        let cur = self.playlist.current;
        self.playlist
            .tracks
            .iter()
            .enumerate()
            .map(|(i, t)| {
                (
                    format!("{}. {}", i + 1, t.display_name()),
                    Some(i) == cur,
                    self.pl_selection.contains(&i),
                    t.duration.map(|d| d.as_secs_f32()),
                )
            })
            .collect()
    }

    fn pl_select_all(&mut self) {
        self.pl_selection = (0..self.playlist.len()).collect();
    }

    /// Move the single-row selection by `delta` (keyboard navigation), clamped,
    /// and scroll it into view.
    fn pl_move_selection(&mut self, delta: i32) {
        if self.playlist.is_empty() {
            return;
        }
        let len = self.playlist.len() as i32;
        let anchor = if delta < 0 {
            self.pl_selection.iter().min().copied()
        } else {
            self.pl_selection.iter().max().copied()
        };
        let next = match anchor {
            Some(a) => (a as i32 + delta).clamp(0, len - 1),
            // Nothing selected yet: enter from the near end.
            None if delta < 0 => len - 1,
            None => 0,
        } as usize;
        self.pl_selection.clear();
        self.pl_selection.insert(next);
        self.pl_last_click = Some((next, Instant::now()));
        self.pl_scroll_to(next);
    }

    /// Adjust the playlist scroll offset so row `idx` is visible.
    fn pl_scroll_to(&mut self, idx: usize) {
        let visible = classic::pl_visible_rows(self.pl_px_h as u32).max(1);
        if idx < self.pl_first {
            self.pl_first = idx;
        } else if idx >= self.pl_first + visible {
            self.pl_first = idx + 1 - visible;
        }
    }

    fn pl_invert_selection(&mut self) {
        self.pl_selection = (0..self.playlist.len())
            .filter(|i| !self.pl_selection.contains(i))
            .collect();
    }

    /// Remove tracks: when `keep` is true keep only selected (crop), else remove
    /// the selected ones. Preserves the current track by path where possible.
    fn pl_remove(&mut self, keep: bool) {
        if self.pl_selection.is_empty() {
            return;
        }
        let cur_path = self.playlist.current_track().map(|t| t.path.clone());
        let sel = self.pl_selection.clone();
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
        self.pl_selection.clear();
        self.playlist.current =
            cur_path.and_then(|p| self.playlist.tracks.iter().position(|t| t.path == p));
        self.pl_first = self.pl_first.min(self.playlist.len().saturating_sub(1));
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
                self.eq_enabled = !self.eq_enabled;
                self.apply_eq();
            }
            R::Auto => {
                self.eq_auto = !self.eq_auto;
                self.apply_eq_auto_gain();
                self.apply_eq();
            }
            R::Close => {
                self.eq_visible = false;
            }
            R::Presets | R::TitleBar | R::Preamp | R::Band(_) => {}
        }
    }

    /// Apply a built-in preset (by index into [`eq::PRESETS`]).
    fn apply_eq_preset(&mut self, idx: usize) {
        if let Some((_, preamp, bands)) = crate::eq::PRESETS.get(idx) {
            self.eq_bands = *bands;
            self.eq_preamp_db = *preamp;
            if !self.eq_enabled {
                self.eq_enabled = true;
            }
            self.apply_eq_auto_gain();
            self.apply_eq();
        }
    }

    /// When Auto is on, set the preamp to offset the loudest boosted band so the
    /// EQ is less likely to clip.
    fn apply_eq_auto_gain(&mut self) {
        if self.eq_auto {
            self.eq_preamp_db = crate::eq::auto_preamp(&self.eq_bands);
        }
    }

    /// Render the open popup menu in its own borderless window (so it is never
    /// clipped by the small main/playlist windows). Positioned at the click.
    fn draw_popup_viewport(&mut self, ctx: &egui::Context) {
        let Some((kind, owner, pos)) = self.popup else {
            return;
        };
        let screen = self.win_pos[win_idx(owner)] + pos.to_vec2();
        let (w, h) = popup_dims(kind);
        let mut close = false;
        let sort = |app: &mut Self, k: SortKey| {
            app.sort_key = k;
            app.sort_playlist();
        };
        let builder = egui::ViewportBuilder::default()
            .with_position(screen)
            .with_inner_size(egui::vec2(w, h))
            .with_decorations(false)
            .with_resizable(false)
            .with_taskbar(false);
        let id = egui::ViewportId::from_hash_of("linamp_popup");
        ctx.show_viewport_immediate(id, builder, |ctx, _| {
            let style = ctx.style();
            egui::CentralPanel::default()
                .frame(egui::Frame::popup(&style))
                .show(ctx, |ui| {
                    let item = |ui: &mut egui::Ui, label: &str| ui.button(label).clicked();
                    match kind {
                        Popup::Options => {
                            if item(ui, "Open file(s)…") {
                                self.open_files();
                                close = true;
                            }
                            if item(ui, "Open folder…") {
                                self.open_folder();
                                close = true;
                            }
                            ui.separator();
                            let eql = if self.eq_visible { "Hide Equalizer" } else { "Show Equalizer" };
                            if item(ui, eql) {
                                self.eq_visible = !self.eq_visible;
                                if self.eq_visible {
                                    self.restack();
                                }
                                close = true;
                            }
                            let pll = if self.pl_visible { "Hide Playlist" } else { "Show Playlist" };
                            if item(ui, pll) {
                                self.pl_visible = !self.pl_visible;
                                if self.pl_visible {
                                    self.restack();
                                }
                                close = true;
                            }
                            ui.separator();
                            if item(ui, "Media Library…") {
                                self.library_open = true;
                                self.refresh_library();
                                close = true;
                            }
                            if item(ui, "Edit file tags… (Alt+3)") {
                                self.open_tag_editor();
                                close = true;
                            }
                            if item(ui, "Load skin…") {
                                self.load_skin_dialog(ctx);
                                close = true;
                            }
                            if item(ui, "Preferences…") {
                                self.prefs_open = true;
                                close = true;
                            }
                            ui.separator();
                            if item(ui, "Exit") {
                                // This runs inside the popup's own viewport, so a
                                // plain Close would only dismiss the menu. Target
                                // the root viewport to actually quit the app.
                                ctx.send_viewport_cmd_to(
                                    egui::ViewportId::ROOT,
                                    egui::ViewportCommand::Close,
                                );
                                close = true;
                            }
                        }
                        Popup::PlAdd => {
                            if item(ui, "Add file(s)…") {
                                self.open_files();
                                close = true;
                            }
                            if item(ui, "Add folder…") {
                                self.open_folder();
                                close = true;
                            }
                        }
                        Popup::PlRem => {
                            if item(ui, "Remove selected") {
                                self.pl_remove(false);
                                close = true;
                            }
                            if item(ui, "Crop (keep selected)") {
                                self.pl_remove(true);
                                close = true;
                            }
                            if item(ui, "Remove all") {
                                self.playlist.clear();
                                self.pl_selection.clear();
                                self.player.stop();
                                close = true;
                            }
                        }
                        Popup::PlSel => {
                            if item(ui, "Select all") {
                                self.pl_select_all();
                                close = true;
                            }
                            if item(ui, "Select none") {
                                self.pl_selection.clear();
                                close = true;
                            }
                            if item(ui, "Invert selection") {
                                self.pl_invert_selection();
                                close = true;
                            }
                        }
                        Popup::PlMisc => {
                            if item(ui, "Sort by title") {
                                sort(self, SortKey::Title);
                                close = true;
                            }
                            if item(ui, "Sort by artist") {
                                sort(self, SortKey::Artist);
                                close = true;
                            }
                            if item(ui, "Sort by album") {
                                sort(self, SortKey::Album);
                                close = true;
                            }
                            if item(ui, "Sort by duration") {
                                sort(self, SortKey::Duration);
                                close = true;
                            }
                        }
                        Popup::PlList => {
                            if item(ui, "New list") {
                                self.playlist.clear();
                                self.pl_selection.clear();
                                self.player.stop();
                                close = true;
                            }
                            if item(ui, "Save list…") {
                                self.save_playlist_dialog();
                                close = true;
                            }
                            if item(ui, "Load list…") {
                                self.load_playlist_dialog();
                                close = true;
                            }
                        }
                        Popup::EqPresets => {
                            for (i, (name, _, _)) in crate::eq::PRESETS.iter().enumerate() {
                                if item(ui, name) {
                                    self.apply_eq_preset(i);
                                    close = true;
                                }
                            }
                        }
                    }
                });
            if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
                close = true;
            }
        });

        self.popup_fresh = false;
        if close {
            self.popup = None;
        }
    }

    fn open_popup_in(&mut self, kind: Popup, win: ClWin, local: egui::Pos2) {
        self.popup = Some((kind, win, local));
        self.popup_fresh = true;
    }

    /// Per-window input in local (viewport) coordinates.
    fn handle_window_input(&mut self, ctx: &egui::Context, win: ClWin) {
        let outer = ctx.input(|i| i.viewport().outer_rect);
        let monitor = ctx.input(|i| i.viewport().monitor_size);
        let local = ctx.pointer_latest_pos();
        let screen_cursor = match (outer, local) {
            (Some(o), Some(l)) => Some(o.min + l.to_vec2()),
            _ => None,
        };
        let (pressed, released, down, secondary, scroll) = ctx.input(|i| {
            (
                i.pointer.primary_pressed(),
                i.pointer.primary_released(),
                i.pointer.primary_down(),
                i.pointer.secondary_clicked(),
                i.raw_scroll_delta.y,
            )
        });
        let shaded = match win {
            ClWin::Main => self.main_shade,
            ClWin::Eq => self.eq_shade,
            ClWin::Pl => self.pl_shade,
        };

        // Clicking any main window dismisses an open menu (click-away).
        if pressed && self.popup.is_some() && !self.popup_fresh {
            self.popup = None;
        }

        if secondary && win == ClWin::Main {
            if let Some(l) = local {
                self.open_popup_in(Popup::Options, win, l);
            }
        }

        if win == ClWin::Pl && !shaded && scroll != 0.0 {
            let max = self.playlist.len().saturating_sub(1) as i32;
            let step = if scroll > 0.0 { -3 } else { 3 };
            self.pl_first = (self.pl_first as i32 + step).clamp(0, max.max(0)) as usize;
        }

        if pressed {
            self.active_region = None;
            self.eq_active = None;
            if let Some(l) = local {
                let nx = l.x / self.scale;
                let ny = l.y / self.scale;
                if shaded {
                    use classic::ShadeHit as H;
                    // The main shade bar carries extra controls; EQ/PL shades only
                    // have close + title bar.
                    let hit = if win == ClWin::Main {
                        classic::main_shade_region_at(nx, ny)
                    } else {
                        classic::shade_region_at(nx, ny)
                    };
                    match hit {
                        Some(H::Close) => self.close_window(ctx, win),
                        Some(H::Restore) => self.toggle_shade(win),
                        Some(H::TitleBar) => self.start_drag(ctx, win, screen_cursor),
                        Some(H::Options) => self.open_popup_in(Popup::Options, win, l),
                        Some(H::Prev) => self.trigger_classic(ctx, Btn::Prev),
                        Some(H::Play) => self.trigger_classic(ctx, Btn::Play),
                        Some(H::Pause) => self.trigger_classic(ctx, Btn::Pause),
                        Some(H::Stop) => self.trigger_classic(ctx, Btn::Stop),
                        Some(H::Next) => self.trigger_classic(ctx, Btn::Next),
                        Some(H::Eject) => self.trigger_classic(ctx, Btn::Eject),
                        None => {}
                    }
                } else {
                    match win {
                        ClWin::Main => {
                            let r = classic::region_at(nx, ny);
                            self.active_region = r;
                            match r {
                                Some(Btn::TitleBar) => {
                                    self.active_region = None;
                                    self.start_drag(ctx, win, screen_cursor);
                                }
                                Some(b @ (Btn::Seek | Btn::Volume | Btn::Balance)) => {
                                    self.apply_classic_slider(b, nx)
                                }
                                _ => {}
                            }
                        }
                        ClWin::Eq => {
                            let r = classic::eq_region_at(nx, ny);
                            self.eq_active = r;
                            match r {
                                Some(classic::EqRegion::TitleBar) => {
                                    self.eq_active = None;
                                    self.start_drag(ctx, win, screen_cursor);
                                }
                                Some(er @ (classic::EqRegion::Preamp | classic::EqRegion::Band(_))) => {
                                    self.apply_eq_slider(er, ny)
                                }
                                _ => {}
                            }
                        }
                        ClWin::Pl => {
                            use classic::PlRegion as P;
                            match classic::pl_region_at(nx, ny, self.pl_px_w, self.pl_px_h) {
                                Some(P::Close) => self.close_window(ctx, ClWin::Pl),
                                Some(P::TitleBar) => self.start_drag(ctx, win, screen_cursor),
                                Some(P::Resize) => {
                                    self.resize_pl =
                                        Some(egui::vec2(self.pl_px_w - nx, self.pl_px_h - ny))
                                }
                                Some(P::List) => {
                                    let row = classic::pl_row_at(ny, self.pl_first);
                                    if row < self.playlist.len() {
                                        let now = Instant::now();
                                        let dbl = self
                                            .pl_last_click
                                            .map(|(r, t)| r == row && now.duration_since(t).as_millis() < 350)
                                            .unwrap_or(false);
                                        if dbl {
                                            self.play_index(row);
                                        } else {
                                            self.pl_selection.clear();
                                            self.pl_selection.insert(row);
                                        }
                                        self.pl_last_click = Some((row, now));
                                    }
                                }
                                Some(menu @ (P::AddMenu | P::RemMenu | P::SelMenu | P::MiscMenu | P::ListMenu)) => {
                                    // Open on release (below) so this click can't dismiss it.
                                    self.pl_menu_press = Some(menu);
                                }
                                // Mini-player transport buttons mirror the main window's.
                                Some(P::MiniPrev) => self.trigger_classic(ctx, Btn::Prev),
                                Some(P::MiniPlay) => self.trigger_classic(ctx, Btn::Play),
                                Some(P::MiniPause) => self.trigger_classic(ctx, Btn::Pause),
                                Some(P::MiniStop) => self.trigger_classic(ctx, Btn::Stop),
                                Some(P::MiniNext) => self.trigger_classic(ctx, Btn::Next),
                                Some(P::MiniEject) => self.trigger_classic(ctx, Btn::Eject),
                                None => {}
                            }
                        }
                    }
                }
            }
        }

        if down {
            let idx = win_idx(win);
            let is_my_drag = self.drag.as_ref().map(|d| d.win == win).unwrap_or(false);
            if is_my_drag {
                let manual = self.drag.as_ref().unwrap().manual;
                if manual {
                    // Live move: drive position from the global cursor (feedback-
                    // free), moving the docked group in tandem each frame.
                    if let Some(g) = global_cursor_points(ctx) {
                        let d = self.drag.as_ref().unwrap();
                        let target = g - d.grab;
                        let group = d.group.clone();
                        let press_pos = d.press_pos;
                        self.win_pos[idx] = target;
                        for (mi, off) in group {
                            self.win_pos[mi] = target + off;
                        }
                        if (target - press_pos).length() > 3.0 {
                            if let Some(dm) = self.drag.as_mut() {
                                dm.moved = true;
                            }
                        }
                        if idx == 0 {
                            self.root_last = None; // keep root in sync while moving
                        }
                    }
                    ctx.request_repaint();
                } else if !self.drag.as_ref().unwrap().started_os {
                    // OS fallback: hand off to the OS window-drag past a threshold.
                    if let Some(sc) = screen_cursor {
                        if (sc - self.drag.as_ref().unwrap().press_screen).length() > 4.0 {
                            ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
                            if let Some(dm) = self.drag.as_mut() {
                                dm.started_os = true;
                            }
                        }
                    }
                }
            }
            if let Some(l) = local {
                let nx = l.x / self.scale;
                let ny = l.y / self.scale;
                if let Some(r) = self.active_region {
                    if matches!(r, Btn::Seek | Btn::Volume | Btn::Balance) {
                        self.apply_classic_slider(r, nx);
                    }
                }
                if let Some(er) = self.eq_active {
                    self.apply_eq_slider(er, ny);
                }
                if let Some(grab) = self.resize_pl {
                    let hstep = 29.0;
                    let raw_h = (ny + grab.y).max(87.0);
                    self.pl_px_h = ((raw_h / hstep).round() * hstep).max(87.0);
                    let wstep = 25.0;
                    let min_w = classic::MAIN_W as f32;
                    let raw_w = (nx + grab.x).max(min_w);
                    self.pl_px_w = (((raw_w - min_w) / wstep).round() * wstep + min_w).max(min_w);
                }
            }
        }

        // Finalize a drag once the button is up (covers release after the modal
        // OS drag, which may swallow the release event).
        if !down {
            if let Some(d) = self.drag.take() {
                if d.win == win {
                    self.finalize_drag(d, outer, monitor);
                } else {
                    self.drag = Some(d);
                }
            }
        }

        if released {
            self.resize_pl = None;
            if let Some(l) = local {
                let nx = l.x / self.scale;
                let ny = l.y / self.scale;
                if win == ClWin::Main {
                    if let Some(r) = self.active_region {
                        if classic::region_at(nx, ny) == Some(r) {
                            if r == Btn::Options {
                                self.open_popup_in(Popup::Options, win, l);
                            } else if r == Btn::Time {
                                // Double-click the time display toggles elapsed vs
                                // remaining (shown as a negative) everywhere it's shown.
                                let now = Instant::now();
                                let dbl = self
                                    .time_last_click
                                    .map(|t| now.duration_since(t).as_millis() < 350)
                                    .unwrap_or(false);
                                if dbl {
                                    self.time_remaining = !self.time_remaining;
                                    self.time_last_click = None;
                                } else {
                                    self.time_last_click = Some(now);
                                }
                            } else {
                                self.trigger_classic(ctx, r);
                            }
                        }
                    }
                    if self.active_region == Some(Btn::Seek) {
                        if let Some(p) = self.classic_seek_preview.take() {
                            self.player.seek(Duration::from_secs_f32(p));
                        }
                    }
                }
                if win == ClWin::Eq {
                    if let Some(er) = self.eq_active {
                        if classic::eq_region_at(nx, ny) == Some(er) {
                            if er == classic::EqRegion::Presets {
                                self.open_popup_in(Popup::EqPresets, ClWin::Eq, l);
                            } else {
                                self.trigger_eq(ctx, er);
                            }
                        }
                    }
                }
                if win == ClWin::Pl {
                    if let Some(r) = self.pl_menu_press.take() {
                        if classic::pl_region_at(nx, ny, self.pl_px_w, self.pl_px_h) == Some(r) {
                            use classic::PlRegion as P;
                            let kind = match r {
                                P::AddMenu => Popup::PlAdd,
                                P::RemMenu => Popup::PlRem,
                                P::SelMenu => Popup::PlSel,
                                P::MiscMenu => Popup::PlMisc,
                                _ => Popup::PlList,
                            };
                            self.open_popup_in(kind, ClWin::Pl, l);
                        }
                    }
                }
            }
            self.active_region = None;
            self.eq_active = None;
            self.pl_menu_press = None;
        }

        if win == ClWin::Main {
            self.pressed_btn = if down {
                match self.active_region {
                    Some(b @ (Btn::Prev | Btn::Play | Btn::Pause | Btn::Stop | Btn::Next | Btn::Eject
                    | Btn::Shuffle | Btn::Repeat | Btn::Seek | Btn::Volume | Btn::Balance)) => Some(b),
                    _ => None,
                }
            } else {
                None
            };
        }
    }

    fn finalize_drag(&mut self, d: Dragging, outer: Option<egui::Rect>, monitor: Option<egui::Vec2>) {
        let idx = win_idx(d.win);
        let did_move = if d.manual { d.moved } else { d.started_os };
        if did_move {
            // OS fallback moved the window out from under win_pos; re-sync it.
            if !d.manual {
                if let Some(r) = outer {
                    self.win_pos[idx] = r.min;
                }
                for (mi, off) in &d.group {
                    self.win_pos[*mi] = self.win_pos[idx] + *off;
                }
            }
            // Magnetic snap to screen edges / other windows; move the group together.
            let before = self.win_pos[idx];
            let size = self.win_inner_size(d.win);
            let snapped = self.snap_pos(idx, before, size, monitor);
            let delta = snapped - before;
            self.win_pos[idx] = snapped;
            for (mi, _) in &d.group {
                self.win_pos[*mi] += delta;
            }
            if d.win == ClWin::Main {
                self.root_last = None;
            }
            // Main never covered; EQ/playlist never cover each other (they stack).
            self.resolve_overlaps(d.win);
        } else {
            // A click with no move: double-click toggles the window shade.
            let now = Instant::now();
            let dbl = self
                .tb_last_click
                .map(|(w, t)| w == d.win && now.duration_since(t).as_millis() < 350)
                .unwrap_or(false);
            if dbl {
                self.toggle_shade(d.win);
                self.tb_last_click = None;
            } else {
                self.tb_last_click = Some((d.win, now));
            }
        }
    }

    fn win_native_h(&self, win: ClWin) -> f32 {
        match win {
            ClWin::Main => self.main_h(),
            ClWin::Eq => self.eq_h(),
            ClWin::Pl => {
                if self.pl_shade {
                    14.0
                } else {
                    self.pl_px_h
                }
            }
        }
    }

    fn win_inner_size(&self, win: ClWin) -> egui::Vec2 {
        let w = match win {
            ClWin::Pl if !self.pl_shade => self.pl_px_w,
            _ => classic::MAIN_W as f32,
        };
        egui::vec2(w * self.scale, self.win_native_h(win) * self.scale)
    }

    fn start_drag(&mut self, ctx: &egui::Context, win: ClWin, screen_cursor: Option<egui::Pos2>) {
        if let Some(sc) = screen_cursor {
            // Only the main window carries a group: the set of windows actually
            // docked (edge-flush) to it. EQ/playlist always move on their own,
            // even when docked to each other.
            let group = if win == ClWin::Main {
                self.main_dock_group()
            } else {
                Vec::new()
            };
            let idx = win_idx(win);
            let (manual, grab) = match global_cursor_points(ctx) {
                Some(g) => (true, g - self.win_pos[idx]),
                None => (false, egui::Vec2::ZERO),
            };
            self.drag = Some(Dragging {
                win,
                manual,
                grab,
                press_pos: self.win_pos[idx],
                press_screen: sc,
                started_os: false,
                moved: false,
                group,
            });
        }
    }

    /// After a drag, resolve window overlaps: the main window is never covered, and
    /// the EQ/playlist windows never cover each other (they stack). The window that
    /// gives way is the one just dragged (a child moves off main and off its
    /// sibling; when main was dragged, the children move off it instead).
    fn resolve_overlaps(&mut self, dragged: ClWin) {
        let rect = |app: &Self, i: usize| {
            egui::Rect::from_min_size(app.win_pos[i], app.win_inner_size(idx_win(i)))
        };
        let vis = |app: &Self, i: usize| match i {
            0 => true,
            1 => app.eq_visible,
            2 => app.pl_visible,
            _ => false,
        };
        if dragged == ClWin::Main {
            // Children give way: push each off main, then separate them if needed.
            for i in [1usize, 2usize] {
                if vis(self, i) {
                    if let Some(p) = push_off(rect(self, i), rect(self, 0)) {
                        self.win_pos[i] = p;
                    }
                }
            }
            if vis(self, 1) && vis(self, 2) {
                // Push the lower one below the higher so they don't overlap.
                let (hi, lo) = if self.win_pos[1].y <= self.win_pos[2].y {
                    (1, 2)
                } else {
                    (2, 1)
                };
                if let Some(p) = push_off(rect(self, lo), rect(self, hi)) {
                    self.win_pos[lo] = p;
                }
            }
        } else {
            // The dragged child moves out of the main window and its sibling.
            let di = win_idx(dragged);
            let oi = if di == 1 { 2 } else { 1 };
            for _ in 0..4 {
                let mut moved = false;
                if let Some(p) = push_off(rect(self, di), rect(self, 0)) {
                    self.win_pos[di] = p;
                    moved = true;
                }
                if vis(self, oi) {
                    if let Some(p) = push_off(rect(self, di), rect(self, oi)) {
                        self.win_pos[di] = p;
                        moved = true;
                    }
                }
                if !moved {
                    break;
                }
            }
        }
        self.root_last = None;
    }

    fn win_rect_pts(&self, idx: usize) -> egui::Rect {
        let w = match idx {
            0 => ClWin::Main,
            1 => ClWin::Eq,
            _ => ClWin::Pl,
        };
        egui::Rect::from_min_size(self.win_pos[idx], self.win_inner_size(w))
    }

    /// Windows transitively docked to the main window (by offset from main).
    fn main_dock_group(&self) -> Vec<(usize, egui::Vec2)> {
        let rects = [
            self.win_rect_pts(0),
            self.win_rect_pts(1),
            self.win_rect_pts(2),
        ];
        let vis = [true, self.eq_visible, self.pl_visible];
        let in_group = dock_group_from_main(&rects, &vis);
        [1usize, 2usize]
            .into_iter()
            .filter(|&j| in_group[j])
            .map(|j| (j, self.win_pos[j] - self.win_pos[0]))
            .collect()
    }

    /// Canonical stacking: EQ then playlist dock directly below the main window,
    /// in order, regardless of which was opened first.
    fn restack(&mut self) {
        let base = self.win_pos[0];
        let mut y = base.y + self.main_h() * self.scale;
        if self.eq_visible {
            self.win_pos[1] = egui::pos2(base.x, y);
            y += self.eq_h() * self.scale;
        }
        if self.pl_visible {
            self.win_pos[2] = egui::pos2(base.x, y);
        }
    }

    /// Magnetic snap of `pos` (rect of `size`) to screen edges and other windows.
    fn snap_pos(
        &self,
        idx: usize,
        pos: egui::Pos2,
        size: egui::Vec2,
        monitor: Option<egui::Vec2>,
    ) -> egui::Pos2 {
        use egui::{vec2, Rect};
        let s = SNAP;
        let mut r = Rect::from_min_size(pos, size);
        if let Some(m) = monitor {
            if r.left().abs() < s {
                r = r.translate(vec2(-r.left(), 0.0));
            }
            if (m.x - r.right()).abs() < s {
                r = r.translate(vec2(m.x - r.right(), 0.0));
            }
            if r.top().abs() < s {
                r = r.translate(vec2(0.0, -r.top()));
            }
            if (m.y - r.bottom()).abs() < s {
                r = r.translate(vec2(0.0, m.y - r.bottom()));
            }
        }
        for j in 0..3 {
            if j == idx {
                continue;
            }
            let vis = match j {
                0 => true,
                1 => self.eq_visible,
                2 => self.pl_visible,
                _ => false,
            };
            if !vis {
                continue;
            }
            {
                let jwin = match j {
                    0 => ClWin::Main,
                    1 => ClWin::Eq,
                    _ => ClWin::Pl,
                };
                let o = Rect::from_min_size(self.win_pos[j], self.win_inner_size(jwin));
                let xover = r.left() < o.right() + s && r.right() > o.left() - s;
                if xover {
                    if (r.top() - o.bottom()).abs() < s {
                        r = r.translate(vec2(0.0, o.bottom() - r.top()));
                    }
                    if (r.bottom() - o.top()).abs() < s {
                        r = r.translate(vec2(0.0, o.top() - r.bottom()));
                    }
                    if (r.left() - o.left()).abs() < s {
                        r = r.translate(vec2(o.left() - r.left(), 0.0));
                    }
                }
                let yover = r.top() < o.bottom() + s && r.bottom() > o.top() - s;
                if yover {
                    if (r.left() - o.right()).abs() < s {
                        r = r.translate(vec2(o.right() - r.left(), 0.0));
                    }
                    if (r.right() - o.left()).abs() < s {
                        r = r.translate(vec2(o.left() - r.right(), 0.0));
                    }
                    if (r.top() - o.top()).abs() < s {
                        r = r.translate(vec2(0.0, o.top() - r.top()));
                    }
                }
            }
        }
        r.min
    }

    fn close_window(&mut self, ctx: &egui::Context, win: ClWin) {
        match win {
            ClWin::Main => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
            ClWin::Eq => self.eq_visible = false,
            ClWin::Pl => self.pl_visible = false,
        }
    }

    fn toggle_shade(&mut self, win: ClWin) {
        match win {
            ClWin::Main => self.main_shade = !self.main_shade,
            ClWin::Eq => self.eq_shade = !self.eq_shade,
            ClWin::Pl => self.pl_shade = !self.pl_shade,
        }
        // Each window keeps its position across collapse/restore (don't restack);
        // just let the changed window resize next frame.
        self.root_last = None;
    }

    fn set_double_size(&mut self, _ctx: &egui::Context, dbl: bool) {
        if self.prefs.double_size == dbl {
            return;
        }
        self.prefs.double_size = dbl;
        self.scale = self.prefs.scale();
        self.prefs.save();
        self.restack();
        self.root_last = None; // force the root window to resize next frame
    }

    fn ui_prefs(&mut self, ctx: &egui::Context) {
        if !self.prefs_open {
            return;
        }
        // Render in its own real window so it is never clipped by the (small)
        // borderless main window.
        let screen = self.win_pos[0] + egui::vec2(24.0, 24.0);
        let builder = egui::ViewportBuilder::default()
            .with_title("LinAmp Preferences")
            .with_position(screen)
            .with_inner_size(egui::vec2(340.0, 300.0))
            .with_resizable(false)
            .with_taskbar(false);
        let id = egui::ViewportId::from_hash_of("linamp_prefs");
        let mut close = false;
        ctx.show_viewport_immediate(id, builder, |ctx, _| {
            egui::CentralPanel::default().show(ctx, |ui| {
                ui.strong("Display size");
                let mut dbl = self.prefs.double_size;
                ui.radio_value(&mut dbl, false, "1x — pixel-for-pixel");
                ui.radio_value(&mut dbl, true, "2x — scaled for modern screens (Ctrl+D)");
                if dbl != self.prefs.double_size {
                    self.set_double_size(ctx, dbl);
                }
                ui.add_space(8.0);
                ui.strong("ID3 tag loading");
                let mut onload = self.prefs.tag_on_load;
                ui.radio_value(&mut onload, true, "On load — scan when files are added");
                ui.radio_value(&mut onload, false, "On view — lazily as rows scroll in");
                if onload != self.prefs.tag_on_load {
                    self.prefs.tag_on_load = onload;
                    self.prefs.save();
                    if onload {
                        self.playlist.load_all();
                    }
                }

                ui.add_space(8.0);
                ui.strong("Playback");
                // Behaviours that otherwise only have keyboard shortcuts, so they
                // can be clicked instead of memorised. Shuffle/repeat are omitted
                // here as they already have buttons on the classic main window.
                ui.checkbox(&mut self.stop_after_current, "Stop after current track (Ctrl+V)");
                ui.checkbox(&mut self.time_remaining, "Show time remaining (Ctrl+T)");
                let mut fade = self.prefs.fade_on_stop;
                if ui
                    .checkbox(&mut fade, "Fade out when stopping (Shift+V)")
                    .changed()
                {
                    self.prefs.fade_on_stop = fade;
                    self.prefs.save();
                }

                ui.add_space(10.0);
                if ui.button("Close").clicked() {
                    close = true;
                }
            });
            if ctx.input(|i| i.viewport().close_requested() || i.key_pressed(egui::Key::Escape)) {
                close = true;
            }
        });
        if close {
            self.prefs_open = false;
        }
    }

    /// The media library + search, in its own window (the classic UI has no
    /// built-in library view). Reuses the same list UI as the modern view.
    fn ui_library_window(&mut self, ctx: &egui::Context) {
        if !self.library_open {
            return;
        }
        let screen = self.win_pos[0] + egui::vec2(40.0, 40.0);
        let builder = egui::ViewportBuilder::default()
            .with_title("LinAmp Media Library")
            .with_position(screen)
            .with_inner_size(egui::vec2(460.0, 360.0))
            .with_min_inner_size(egui::vec2(320.0, 200.0))
            .with_taskbar(false);
        let id = egui::ViewportId::from_hash_of("linamp_library");
        let mut close = false;
        ctx.show_viewport_immediate(id, builder, |ctx, _| {
            egui::CentralPanel::default().show(ctx, |ui| {
                self.ui_library(ui);
            });
            if ctx.input(|i| i.viewport().close_requested() || i.key_pressed(egui::Key::Escape)) {
                close = true;
            }
        });
        if close {
            self.library_open = false;
        }
    }

    /// Open the file-info / tag editor for the selected row (or current track).
    fn open_tag_editor(&mut self) {
        let idx = if self.pl_selection.len() == 1 {
            self.pl_selection.iter().next().copied()
        } else {
            self.playlist.current
        }
        .or(if self.playlist.is_empty() { None } else { Some(0) });

        let Some(index) = idx else {
            self.status = "No track to edit.".into();
            return;
        };
        let Some(track) = self.playlist.tracks.get(index) else {
            return;
        };
        let path = track.path.clone();
        let fields = crate::tags::read(&path);
        let editable = crate::tags::can_edit(&path);
        let status = if editable {
            String::new()
        } else {
            "Read-only: tag writing isn't supported for this format yet.".into()
        };
        self.tag_editor = Some(TagEditor {
            index,
            path,
            orig: fields.clone(),
            fields,
            editable,
            status,
        });
    }

    /// Write the editor's fields back to the file and refresh the playlist row.
    fn save_tag_editor(&mut self) {
        let (index, path, fields) = match &self.tag_editor {
            Some(ed) if ed.editable => (ed.index, ed.path.clone(), ed.fields.clone()),
            _ => return,
        };
        let result = crate::tags::write(&path, &fields);
        if result.is_ok() {
            if let Some(t) = self.playlist.tracks.get_mut(index) {
                t.loaded = false;
                t.load();
            }
        }
        if let Some(ed) = self.tag_editor.as_mut() {
            match result {
                Ok(()) => {
                    ed.orig = ed.fields.clone();
                    ed.status = "Saved.".into();
                }
                Err(e) => ed.status = format!("Error: {e}"),
            }
        }
    }

    /// The tag editor window (own viewport, like preferences / library).
    fn ui_tag_editor(&mut self, ctx: &egui::Context) {
        if self.tag_editor.is_none() {
            return;
        }
        let screen = self.win_pos[0] + egui::vec2(32.0, 32.0);
        let builder = egui::ViewportBuilder::default()
            .with_title("LinAmp — File Info")
            .with_position(screen)
            .with_inner_size(egui::vec2(400.0, 380.0))
            .with_resizable(false)
            .with_taskbar(false);
        let id = egui::ViewportId::from_hash_of("linamp_tags");
        let mut close = false;
        let mut do_save = false;
        ctx.show_viewport_immediate(id, builder, |ctx, _| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let ed = self.tag_editor.as_mut().unwrap();
                let fname = ed
                    .path
                    .file_name()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default();
                ui.label(RichText::new(fname).strong());
                ui.separator();
                egui::Grid::new("tag_grid")
                    .num_columns(2)
                    .spacing([8.0, 6.0])
                    .show(ui, |ui| {
                        let row = |ui: &mut egui::Ui, label: &str, val: &mut String| {
                            ui.label(label);
                            ui.add(
                                egui::TextEdit::singleline(val).desired_width(260.0),
                            );
                            ui.end_row();
                        };
                        row(ui, "Title", &mut ed.fields.title);
                        row(ui, "Artist", &mut ed.fields.artist);
                        row(ui, "Album", &mut ed.fields.album);
                        row(ui, "Album artist", &mut ed.fields.album_artist);
                        row(ui, "Year", &mut ed.fields.year);
                        row(ui, "Track", &mut ed.fields.track);
                        row(ui, "Genre", &mut ed.fields.genre);
                        ui.label("Comment");
                        ui.add(
                            egui::TextEdit::multiline(&mut ed.fields.comment)
                                .desired_width(260.0)
                                .desired_rows(2),
                        );
                        ui.end_row();
                    });
                ui.add_space(6.0);
                if !ed.status.is_empty() {
                    ui.label(RichText::new(&ed.status).color(Color32::GRAY));
                }
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.add_enabled_ui(ed.editable, |ui| {
                        if ui.button("Save").clicked() {
                            do_save = true;
                        }
                    });
                    if ui.button("Revert").clicked() {
                        ed.fields = ed.orig.clone();
                    }
                    if ui.button("Close").clicked() {
                        close = true;
                    }
                });
            });
            if ctx.input(|i| i.viewport().close_requested() || i.key_pressed(egui::Key::Escape)) {
                close = true;
            }
        });
        if do_save {
            self.save_tag_editor();
        }
        if close {
            self.tag_editor = None;
        }
    }

    /// Jump-to-file: type to filter the playlist, Enter/click to play a match.
    fn ui_jump_file(&mut self, ctx: &egui::Context) {
        if self.jump_file.is_none() {
            return;
        }
        let screen = self.win_pos[0] + egui::vec2(28.0, 28.0);
        let builder = egui::ViewportBuilder::default()
            .with_title("LinAmp — Jump to file")
            .with_position(screen)
            .with_inner_size(egui::vec2(360.0, 300.0))
            .with_min_inner_size(egui::vec2(260.0, 160.0))
            .with_taskbar(false);
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
                    ui.label("Jump to file (type to filter · ↑/↓ + Enter):");
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
        let screen = self.win_pos[0] + egui::vec2(40.0, 40.0);
        let builder = egui::ViewportBuilder::default()
            .with_title("LinAmp — Jump to time")
            .with_position(screen)
            .with_inner_size(egui::vec2(260.0, 110.0))
            .with_resizable(false)
            .with_taskbar(false);
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

    fn apply_eq_slider(&mut self, region: classic::EqRegion, ey: f32) {
        let v = classic::eq_value_at_y(ey);
        match region {
            classic::EqRegion::Preamp => {
                // Dragging the preamp is a manual override, so it turns Auto off.
                self.eq_auto = false;
                self.eq_preamp_db = v * 12.0;
                self.apply_eq();
            }
            classic::EqRegion::Band(i) if i < 10 => {
                self.eq_bands[i] = v * 12.0;
                self.apply_eq_auto_gain();
                self.apply_eq();
            }
            _ => {}
        }
    }

    fn render_classic(&mut self, ctx: &egui::Context) {
        if self.classic.is_none() {
            return;
        }

        // Root viewport = main window.
        self.render_window(ctx, ClWin::Main);
        self.ui_prefs(ctx);
        self.ui_library_window(ctx);
        self.ui_tag_editor(ctx);
        self.ui_jump_file(ctx);
        self.ui_jump_time(ctx);

        // Initialise window positions from the real main window, once.
        if !self.positions_init {
            if let Some(r) = ctx.input(|i| i.viewport().outer_rect) {
                self.win_pos[0] = r.min;
                // Dock the (possibly restored) EQ/playlist windows under the main
                // one, honouring which are visible and their collapsed state.
                self.restack();
                self.positions_init = true;
            }
        }

        // Apply root position/size only when we changed them (avoids fighting OS).
        let size = self.win_inner_size(ClWin::Main);
        let pos = self.win_pos[0];
        if self.root_last != Some((pos, size)) {
            ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(size));
            if self.positions_init {
                ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(pos));
            }
            self.root_last = Some((pos, size));
        }

        // Equalizer child window.
        if self.eq_visible {
            let builder = egui::ViewportBuilder::default()
                .with_title("LinAmp Equalizer")
                .with_inner_size(self.win_inner_size(ClWin::Eq))
                .with_position(self.win_pos[1])
                .with_decorations(false)
                .with_resizable(false)
                .with_taskbar(false);
            let id = egui::ViewportId::from_hash_of("linamp_eq");
            ctx.show_viewport_immediate(id, builder, |ctx, _| {
                // Fire global shortcuts while this window has focus (keyboard
                // input goes to the focused viewport, not the root).
                self.handle_shortcuts(ctx, ClWin::Eq);
                self.render_window(ctx, ClWin::Eq);
            });
        }

        // Playlist child window.
        if self.pl_visible {
            let builder = egui::ViewportBuilder::default()
                .with_title("LinAmp Playlist")
                .with_inner_size(self.win_inner_size(ClWin::Pl))
                .with_position(self.win_pos[2])
                .with_decorations(false)
                .with_resizable(false)
                .with_taskbar(false);
            let id = egui::ViewportId::from_hash_of("linamp_pl");
            ctx.show_viewport_immediate(id, builder, |ctx, _| {
                self.handle_shortcuts(ctx, ClWin::Pl);
                self.render_window(ctx, ClWin::Pl);
            });
        }

        // The options / playlist menus render in their own (unclipped) window.
        self.draw_popup_viewport(ctx);

        // Keep the EQ/playlist windows owned by the main window (z-order group).
        group_child_windows(self.eq_visible, self.pl_visible);
    }

    fn render_window(&mut self, ctx: &egui::Context, win: ClWin) {
        self.handle_window_input(ctx, win);
        self.handle_dropped(ctx);

        // Refresh the analyzer data for the main window.
        if win == ClWin::Main && !self.main_shade {
            self.step_viz();
        }

        // Lazily read tags for the playlist rows about to be shown (on-view mode).
        if win == ClWin::Pl && !self.pl_shade {
            let vis = classic::pl_visible_rows(self.pl_px_h as u32);
            let end = (self.pl_first + vis).min(self.playlist.len());
            for i in self.pl_first..end {
                self.playlist.ensure_loaded(i);
            }
        }

        let img = self.compose_window(win);
        let name = match win {
            ClWin::Main => "cl_main",
            ClWin::Eq => "cl_eq",
            ClWin::Pl => "cl_pl",
        };
        let slot = match win {
            ClWin::Main => &mut self.classic_tex,
            ClWin::Eq => &mut self.eq_tex,
            ClWin::Pl => &mut self.pl_tex,
        };
        let (nw, nh) = (img.width() as f32, img.height() as f32);
        let tex = upload_tex(slot, ctx, &img, name);

        egui::CentralPanel::default()
            .frame(egui::Frame::none().fill(Color32::BLACK))
            .show(ctx, |ui| {
                let rect = ui.max_rect();
                let uv = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
                ui.painter().image(tex.id(), rect, uv, Color32::WHITE);

                // Hover tooltips over the classic buttons. These are hover-only
                // widgets, so they don't intercept the manual click/drag handling;
                // skipped while a window is shaded (its buttons aren't shown).
                let tips = match win {
                    ClWin::Main if !self.main_shade => classic::main_tooltips(),
                    ClWin::Eq if !self.eq_shade => classic::eq_tooltips(),
                    ClWin::Pl if !self.pl_shade => {
                        classic::pl_tooltips(self.pl_px_w as i32, self.pl_px_h as i32)
                    }
                    _ => Vec::new(),
                };
                if nw > 0.0 && nh > 0.0 {
                    let (sx, sy) = (rect.width() / nw, rect.height() / nh);
                    for (label, x, y, w, h) in tips {
                        let min = rect.min + egui::vec2(x as f32 * sx, y as f32 * sy);
                        let r = egui::Rect::from_min_size(min, egui::vec2(w as f32 * sx, h as f32 * sy));
                        ui.interact(r, ui.id().with(("tip", label, x, y)), egui::Sense::hover())
                            .on_hover_text(label);
                    }
                }
            });
    }

    /// The bottom-bar menu button that should appear pressed: the one held
    /// down, or the one whose fly-out menu is currently open.
    fn pl_pressed_button(&self) -> Option<classic::PlRegion> {
        use classic::PlRegion as P;
        if let Some(r) = self.pl_menu_press {
            return Some(r);
        }
        if let Some((kind, ClWin::Pl, _)) = self.popup {
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

    fn compose_window(&self, win: ClWin) -> image::RgbaImage {
        let cs = self.classic.as_ref().unwrap();
        match win {
            ClWin::Main => {
                let st = self.make_main_state();
                if self.main_shade {
                    cs.compose_main_shade(&st)
                } else {
                    cs.compose_main(&st)
                }
            }
            ClWin::Eq => {
                if self.eq_shade {
                    cs.compose_eq_shade(true)
                } else {
                    cs.compose_eq(&self.make_eq_state())
                }
            }
            ClWin::Pl => {
                if self.pl_shade {
                    let (text, time) = match self.playlist.current_track() {
                        Some(t) => {
                            let n = self.playlist.current.map(|i| i + 1).unwrap_or(0);
                            let dur = t.duration.map(|d| d.as_secs_f32()).unwrap_or(0.0);
                            // Playing/paused: the live position, toggling
                            // elapsed/remaining like the main LCD. Stopped: the
                            // track length.
                            let time = if self.player.state() != PlayState::Stopped {
                                classic::fmt_pl_time_signed(
                                    self.player.position().as_secs_f32(),
                                    dur,
                                    self.time_remaining,
                                )
                            } else if dur > 0.0 {
                                classic::fmt_pl_time(dur)
                            } else {
                                String::new()
                            };
                            (format!("{}. {}", n, t.display_name()), time)
                        }
                        None => (String::new(), String::new()),
                    };
                    cs.compose_pl_shade(true, &text, &time, self.pl_colors().normal)
                } else {
                    let rows = self.build_pl_rows();
                    let colors = self.pl_colors();
                    let elapsed_secs = match self.player.state() {
                        PlayState::Stopped => 0.0,
                        _ => self.player.position().as_secs_f32(),
                    };
                    let cur_duration = self
                        .playlist
                        .current_track()
                        .and_then(|t| t.duration)
                        .map(|d| d.as_secs_f32())
                        .unwrap_or(0.0);
                    let mut total_secs = 0.0;
                    let mut total_partial = false;
                    for t in &self.playlist.tracks {
                        match t.duration {
                            Some(d) => total_secs += d.as_secs_f32(),
                            None => total_partial = true,
                        }
                    }
                    let st = classic::PlState {
                        focused: true,
                        colors,
                        rows: &rows,
                        first: self.pl_first,
                        pressed: self.pl_pressed_button(),
                        elapsed_secs,
                        cur_duration,
                        time_remaining: self.time_remaining,
                        total_secs,
                        total_partial,
                    };
                    cs.compose_playlist(&st, self.pl_px_w as u32, self.pl_px_h as u32)
                }
            }
        }
    }

    // ---- skins / theming ---------------------------------------------------

    fn load_skin_dialog(&mut self, ctx: &egui::Context) {
        if let Some(path) = rfd::FileDialog::new()
            .add_filter("Winamp skin", &["wsz", "zip"])
            .pick_file()
        {
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
                        RichText::new("No skins found in ./skins — use \"Load .wsz...\".")
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

    fn handle_shortcuts(&mut self, ctx: &egui::Context, win: ClWin) {
        if ctx.wants_keyboard_input() {
            return; // a text field has focus
        }
        let mut k = KeyState::default();
        ctx.input(|i| {
            k.play = i.key_pressed(Key::X);
            k.pause = i.key_pressed(Key::C);
            k.stop = i.key_pressed(Key::V);
            k.next = i.key_pressed(Key::B);
            k.prev = i.key_pressed(Key::Z);
            k.toggle = i.key_pressed(Key::Space);
            k.open = i.key_pressed(Key::L);
            k.repeat = i.key_pressed(Key::R);
            k.shuffle = i.key_pressed(Key::S);
            k.vol_up = i.key_pressed(Key::ArrowUp);
            k.vol_down = i.key_pressed(Key::ArrowDown);
            k.seek_fwd = i.key_pressed(Key::ArrowRight);
            k.seek_back = i.key_pressed(Key::ArrowLeft);
            k.ctrl = i.modifiers.ctrl || i.modifiers.command;
            k.alt = i.modifiers.alt;
            k.shift = i.modifiers.shift;
            k.key_d = i.key_pressed(Key::D);
            k.key_p = i.key_pressed(Key::P);
            k.key_t = i.key_pressed(Key::T);
            k.key_3 = i.key_pressed(Key::Num3);
            k.key_e = i.key_pressed(Key::E);
            k.key_g = i.key_pressed(Key::G);
            k.key_a = i.key_pressed(Key::A);
            k.key_j = i.key_pressed(Key::J);
            k.del = i.key_pressed(Key::Delete);
            k.insert = i.key_pressed(Key::Insert);
            k.enter = i.key_pressed(Key::Enter);
        });

        if k.play {
            self.play_pause();
        }
        if k.pause {
            self.player.pause();
        }
        if k.stop {
            if k.shift {
                // Stop with a fade-out.
                if self.player.state() != PlayState::Stopped {
                    self.fade = Some(FADE_SECS);
                    self.status = "Fading out…".into();
                }
            } else if k.ctrl {
                // Toggle stop-after-current.
                self.stop_after_current = !self.stop_after_current;
                self.status = format!(
                    "Stop after current track: {}",
                    if self.stop_after_current { "ON" } else { "off" }
                );
            } else {
                self.stop_playback();
            }
        }
        if k.next {
            self.next();
        }
        if k.prev {
            self.prev();
        }
        if k.toggle {
            self.play_pause();
        }
        if k.open {
            if k.shift {
                self.open_folder();
            } else {
                self.open_files();
            }
        }
        if k.repeat {
            self.repeat_all = !self.repeat_all;
        }
        if k.shuffle {
            self.shuffle = !self.shuffle;
        }
        if k.vol_up {
            // Up/Down move the selection in the playlist window, else volume.
            if win == ClWin::Pl {
                self.pl_move_selection(-1);
            } else {
                self.adjust_volume(0.05);
            }
        }
        if k.vol_down {
            if win == ClWin::Pl {
                self.pl_move_selection(1);
            } else {
                self.adjust_volume(-0.05);
            }
        }
        if k.seek_fwd {
            self.seek_relative(5.0);
        }
        if k.seek_back {
            self.seek_relative(-5.0);
        }
        if k.ctrl && k.key_d {
            let dbl = !self.prefs.double_size;
            self.set_double_size(ctx, dbl);
        }
        if k.ctrl && k.key_p {
            self.prefs_open = !self.prefs_open;
        }
        if k.ctrl && k.key_t {
            self.time_remaining = !self.time_remaining;
        }
        if k.alt && k.key_3 {
            self.open_tag_editor();
        }
        if k.key_j {
            if k.ctrl {
                self.jump_time = Some(String::new());
            } else {
                self.jump_file = Some(JumpFile::default());
            }
        }
        // Window toggles.
        if k.alt && k.key_g {
            self.eq_visible = !self.eq_visible;
            if self.eq_visible {
                self.restack();
            }
        }
        if k.alt && k.key_e {
            self.pl_visible = !self.pl_visible;
            if self.pl_visible {
                self.restack();
            }
        }
        // Playlist edit.
        if k.ctrl && k.key_a {
            self.pl_select_all();
        }
        if k.del {
            self.pl_remove(false);
        }
        if k.insert {
            self.open_files();
        }
        if k.enter {
            if let Some(&idx) = self.pl_selection.iter().min() {
                self.play_index(idx);
            }
        }
    }

    // ---- UI sections -------------------------------------------------------

    fn ui_display(&mut self, ui: &mut egui::Ui, dt: f32) {
        let (title, dur) = match self.playlist.current_track() {
            Some(t) => (t.display_name(), t.duration),
            None => ("LinAmp - no track loaded".to_string(), None),
        };

        let pos = if self.seeking {
            Duration::from_secs_f32(self.seek_preview)
        } else {
            self.player.position()
        };

        let lcd = self.theme.lcd;
        let lcd_dim = self.theme.lcd_dim;
        egui::Frame::none()
            .fill(self.theme.list_bg)
            .inner_margin(egui::Margin::symmetric(10.0, 8.0))
            .stroke(Stroke::new(1.0_f32, lcd_dim))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    self.ui_album_art(ui, 64.0);
                    ui.add_space(8.0);
                    ui.vertical(|ui| {
                        ui.horizontal(|ui| {
                            let state_str = match self.player.state() {
                                PlayState::Playing => "\u{25B6}",
                                PlayState::Paused => "II",
                                PlayState::Stopped => "\u{25A0}",
                            };
                            ui.label(RichText::new(state_str).color(lcd).monospace().size(16.0));
                            ui.add_space(6.0);
                            ui.label(
                                RichText::new(fmt_time(pos)).color(lcd).monospace().size(22.0),
                            );
                            if let Some(d) = dur {
                                ui.label(
                                    RichText::new(format!("/ {}", fmt_time(d)))
                                        .color(lcd_dim)
                                        .monospace()
                                        .size(14.0),
                                );
                            }
                        });

                        let shown = marquee(&title, self.marquee_offset);
                        ui.label(RichText::new(shown).color(lcd).monospace().size(14.0));

                        if let Some(t) = self.playlist.current_track() {
                            let khz = t
                                .sample_rate
                                .map(|s| format!("{} kHz", s / 1000))
                                .unwrap_or_default();
                            let kbps =
                                t.bitrate.map(|b| format!("{b} kbps")).unwrap_or_default();
                            let ch = match t.channels {
                                Some(1) => "mono",
                                Some(_) => "stereo",
                                None => "",
                            };
                            ui.label(
                                RichText::new(format!("{kbps}   {khz}   {ch}"))
                                    .color(lcd_dim)
                                    .monospace()
                                    .size(11.0),
                            );
                        }
                    });
                });
            });

        let _ = dt;
    }

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

    fn ui_viz(&mut self, ui: &mut egui::Ui) {
        let (resp, painter) =
            ui.allocate_painter(egui::vec2(ui.available_width(), 60.0), egui::Sense::click());
        if resp.clicked() {
            self.viz_mode = self.viz_mode.next();
        }
        let rect = resp.rect;
        painter.rect_filled(rect, 2.0, self.theme.viz_bg);
        painter.rect_stroke(rect, 2.0, Stroke::new(1.0_f32, self.theme.lcd_dim));

        match self.viz_mode {
            VizMode::Off => {
                painter.text(
                    rect.center(),
                    egui::Align2::CENTER_CENTER,
                    "visualization off (click to cycle)",
                    egui::FontId::proportional(11.0),
                    self.theme.lcd_dim,
                );
            }
            VizMode::Spectrum => {
                self.viz.snapshot(&mut self.viz_samples, FFT_SIZE);
                let rate = self.viz.rate();
                let bars =
                    viz::compute_bars(&self.viz_samples, &*self.fft, FFT_SIZE, VIZ_BANDS, rate);
                for (i, b) in self.viz_bars.iter_mut().enumerate() {
                    let target = bars.get(i).copied().unwrap_or(0.0);
                    if target > *b {
                        *b = target;
                    } else {
                        *b = (*b * 0.82).max(target); // peak decay
                    }
                }
                let n = self.viz_bars.len();
                let gap = 2.0;
                let bw = ((rect.width() - gap * (n as f32 + 1.0)) / n as f32).max(1.0);
                for (i, &b) in self.viz_bars.iter().enumerate() {
                    let x0 = rect.left() + gap + i as f32 * (bw + gap);
                    let h = b * (rect.height() - 4.0);
                    let bar = egui::Rect::from_min_max(
                        egui::pos2(x0, rect.bottom() - 2.0 - h),
                        egui::pos2(x0 + bw, rect.bottom() - 2.0),
                    );
                    let spec = &self.theme.viz_spectrum;
                    let col = if spec.is_empty() {
                        self.theme.lcd
                    } else {
                        let idx = (((1.0 - b) * (spec.len() - 1) as f32).round() as usize)
                            .min(spec.len() - 1);
                        spec[idx]
                    };
                    painter.rect_filled(bar, 0.0, col);
                }
            }
            VizMode::Oscilloscope => {
                let n = (rect.width() as usize).clamp(16, 2048);
                self.viz.snapshot(&mut self.viz_samples, n);
                let mid = rect.center().y;
                let amp = (rect.height() / 2.0) - 3.0;
                let denom = (self.viz_samples.len().max(2) - 1) as f32;
                let step = rect.width() / denom;
                let pts: Vec<egui::Pos2> = self
                    .viz_samples
                    .iter()
                    .enumerate()
                    .map(|(i, &s)| {
                        egui::pos2(rect.left() + i as f32 * step, mid - s.clamp(-1.0, 1.0) * amp)
                    })
                    .collect();
                if pts.len() >= 2 {
                    painter.add(egui::Shape::line(pts, Stroke::new(1.5_f32, self.theme.viz_osc)));
                }
            }
        }
    }

    fn ui_seekbar(&mut self, ui: &mut egui::Ui) {
        let dur = self
            .playlist
            .current_track()
            .and_then(|t| t.duration)
            .map(|d| d.as_secs_f32())
            .unwrap_or(0.0);

        if !self.seeking {
            self.seek_preview = self.player.position().as_secs_f32();
        }

        let max = if dur > 0.0 { dur } else { 1.0 };
        self.seek_preview = self.seek_preview.clamp(0.0, max);
        let resp = ui.add_enabled(
            dur > 0.0,
            egui::Slider::new(&mut self.seek_preview, 0.0..=max)
                .show_value(false)
                .trailing_fill(true),
        );
        if resp.dragged() {
            self.seeking = true;
        }
        if resp.drag_stopped() {
            self.player.seek(Duration::from_secs_f32(self.seek_preview));
            self.seeking = false;
        }
    }

    fn ui_transport(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            if ui.button(RichText::new("|<").monospace()).on_hover_text("Previous (Z)").clicked() {
                self.prev();
            }
            if ui.button(RichText::new(" > ").monospace()).on_hover_text("Play (X)").clicked() {
                self.play_pause();
            }
            if ui.button(RichText::new("II").monospace()).on_hover_text("Pause (C)").clicked() {
                self.player.pause();
            }
            if ui.button(RichText::new("[]").monospace()).on_hover_text("Stop (V)").clicked() {
                self.player.stop();
            }
            if ui.button(RichText::new(">|").monospace()).on_hover_text("Next (B)").clicked() {
                self.next();
            }

            ui.separator();

            let mut vol = self.player.volume();
            ui.label("Vol");
            if ui
                .add(egui::Slider::new(&mut vol, 0.0..=1.0).show_value(false))
                .changed()
            {
                self.player.set_volume(vol);
            }
        });

        ui.horizontal(|ui| {
            toggle(ui, &mut self.shuffle, "Shuffle (S)");
            toggle(ui, &mut self.repeat_all, "Repeat (R)");
            toggle(ui, &mut self.repeat_one, "Repeat 1");
            ui.separator();
            if ui.button("Open files (L)").clicked() {
                self.open_files();
            }
            if ui.button("Open folder").clicked() {
                self.open_folder();
            }
        });
    }

    fn ui_equalizer(&mut self, ui: &mut egui::Ui) {
        egui::CollapsingHeader::new("Equalizer")
            .default_open(false)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    if toggle(ui, &mut self.eq_enabled, "EQ On").changed() {
                        self.apply_eq();
                    }
                    ui.separator();
                    ui.label("Preamp");
                    if ui
                        .add(egui::Slider::new(&mut self.eq_preamp_db, -12.0..=12.0).suffix(" dB"))
                        .changed()
                    {
                        self.apply_eq();
                    }
                    if ui.button("Reset").clicked() {
                        self.eq_bands = [0.0; 10];
                        self.eq_preamp_db = 0.0;
                        self.apply_eq();
                    }
                    if toggle(ui, &mut self.eq_auto, "Auto").changed() {
                        self.apply_eq_auto_gain();
                        self.apply_eq();
                    }
                    egui::ComboBox::from_id_salt("eq_preset")
                        .selected_text("Presets")
                        .show_ui(ui, |ui| {
                            for (i, (name, _, _)) in crate::eq::PRESETS.iter().enumerate() {
                                if ui.selectable_label(false, *name).clicked() {
                                    self.apply_eq_preset(i);
                                }
                            }
                        });
                });
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    for (i, label) in EQ_LABELS.iter().enumerate() {
                        ui.vertical(|ui| {
                            if ui
                                .add(
                                    egui::Slider::new(&mut self.eq_bands[i], -12.0..=12.0)
                                        .vertical()
                                        .show_value(false),
                                )
                                .changed()
                            {
                                self.apply_eq_auto_gain();
                                self.apply_eq();
                            }
                            ui.label(RichText::new(*label).size(9.0).color(self.theme.lcd_dim));
                        });
                    }
                });
            });
    }

    fn apply_eq(&mut self) {
        // Push the live settings to the DSP; when disabled the source bypasses
        // all filtering (and the preamp), matching Winamp's behaviour.
        self.player
            .eq()
            .set(self.eq_enabled, self.eq_preamp_db, self.eq_bands);
    }

    fn ui_playlist(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label("Find:");
            ui.add(
                egui::TextEdit::singleline(&mut self.pl_search)
                    .hint_text("filter playlist")
                    .desired_width(130.0),
            );
            if !self.pl_search.is_empty() && ui.button("x").clicked() {
                self.pl_search.clear();
            }
            ui.separator();
            ui.label("Sort:");
            let prev = self.sort_key;
            egui::ComboBox::from_id_salt("sort_combo")
                .selected_text(self.sort_key.label())
                .show_ui(ui, |ui| {
                    for k in [
                        SortKey::Custom,
                        SortKey::Title,
                        SortKey::Artist,
                        SortKey::Album,
                        SortKey::Duration,
                    ] {
                        ui.selectable_value(&mut self.sort_key, k, k.label());
                    }
                });
            if self.sort_key != prev {
                self.sort_playlist();
            }
        });

        ui.horizontal(|ui| {
            ui.label(
                RichText::new(format!("{} tracks", self.playlist.len())).color(Color32::GRAY),
            );
            if ui.button("Save").clicked() {
                self.save_playlist_dialog();
            }
            if ui.button("Load").clicked() {
                self.load_playlist_dialog();
            }
            if ui.button("Clear").clicked() {
                self.playlist.clear();
                self.player.stop();
            }
        });
        ui.separator();

        let filtering = !self.pl_search.trim().is_empty();
        let needle = self.pl_search.to_lowercase();
        let current = self.playlist.current;

        let mut to_play: Option<usize> = None;
        let mut to_remove: Option<usize> = None;
        let mut reorder: Option<(usize, usize)> = None;

        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for (i, track) in self.playlist.tracks.iter().enumerate() {
                    if filtering && !track.display_name().to_lowercase().contains(&needle) {
                        continue;
                    }
                    let is_cur = Some(i) == current;

                    let row = ui
                        .horizontal(|ui| {
                            if !filtering {
                                ui.dnd_drag_source(Id::new(("pl_grip", i)), i, |ui| {
                                    ui.label(RichText::new("\u{2630}").color(Color32::GRAY));
                                });
                            }

                            ui.label(
                                RichText::new(format!("{:>3}.", i + 1))
                                    .monospace()
                                    .color(Color32::GRAY),
                            );

                            let name = track.display_name();
                            let text = if is_cur {
                                RichText::new(name).color(self.theme.list_current).strong()
                            } else {
                                RichText::new(name).color(self.theme.list_normal)
                            };
                            let resp = ui.add(
                                egui::Label::new(text).sense(egui::Sense::click()).truncate(),
                            );
                            if resp.double_clicked() {
                                to_play = Some(i);
                            }
                            resp.context_menu(|ui| {
                                if ui.button("Play").clicked() {
                                    to_play = Some(i);
                                    ui.close_menu();
                                }
                                if ui.button("Remove").clicked() {
                                    to_remove = Some(i);
                                    ui.close_menu();
                                }
                            });

                            if let Some(d) = track.duration {
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        ui.label(
                                            RichText::new(fmt_time(d))
                                                .monospace()
                                                .color(Color32::GRAY),
                                        );
                                    },
                                );
                            }
                        })
                        .response;

                    // Drag-and-drop reordering (only in unfiltered view).
                    if !filtering {
                        if row.dnd_hover_payload::<usize>().is_some() {
                            let rect = row.rect;
                            let above = ui
                                .input(|inp| inp.pointer.interact_pos())
                                .map(|p| p.y < rect.center().y)
                                .unwrap_or(false);
                            let y = if above { rect.top() } else { rect.bottom() };
                            ui.painter().hline(
                                rect.x_range(),
                                y,
                                Stroke::new(2.0_f32, self.theme.accent),
                            );
                        }
                        if let Some(payload) = row.dnd_release_payload::<usize>() {
                            let rect = row.rect;
                            let above = ui
                                .input(|inp| inp.pointer.interact_pos())
                                .map(|p| p.y < rect.center().y)
                                .unwrap_or(false);
                            let target = if above { i } else { i + 1 };
                            reorder = Some((*payload, target));
                        }
                    }
                }
            });

        if let Some(i) = to_play {
            self.play_index(i);
        }
        if let Some(i) = to_remove {
            self.playlist.remove(i);
        }
        if let Some((from, to)) = reorder {
            self.move_track(from, to);
        }
    }

    fn ui_library(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label("Search:");
            if ui
                .add(
                    egui::TextEdit::singleline(&mut self.lib_query)
                        .hint_text("title / artist / album")
                        .desired_width(160.0),
                )
                .changed()
            {
                self.refresh_library();
            }
            if ui.button("Scan folder...").clicked() {
                self.scan_into_library();
            }
        });

        ui.horizontal(|ui| {
            match &self.library {
                Some(lib) => ui.label(
                    RichText::new(format!("{} tracks in library", lib.count()))
                        .color(Color32::GRAY),
                ),
                None => ui.colored_label(Color32::LIGHT_RED, "Library database unavailable"),
            };
            if !self.lib_results.is_empty() && ui.button("Add all shown to playlist").clicked() {
                let was_empty = self.playlist.is_empty();
                for t in self.lib_results.clone() {
                    self.playlist.tracks.push(t);
                }
                if was_empty && !self.playlist.is_empty() {
                    self.play_index(0);
                }
                self.status = format!("{} track(s) in playlist.", self.playlist.len());
            }
        });
        ui.separator();

        let mut add_play: Option<usize> = None;
        let mut add_only: Option<usize> = None;

        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                if self.lib_results.is_empty() {
                    ui.label(
                        RichText::new("No results. Use \"Scan folder...\" to index your music.")
                            .color(Color32::GRAY)
                            .italics(),
                    );
                }
                for (i, track) in self.lib_results.iter().enumerate() {
                    ui.horizontal(|ui| {
                        let resp = ui.add(
                            egui::Label::new(
                                RichText::new(track.display_name()).color(Color32::LIGHT_GRAY),
                            )
                            .sense(egui::Sense::click())
                            .truncate(),
                        );
                        if resp.double_clicked() {
                            add_play = Some(i);
                        }
                        resp.context_menu(|ui| {
                            if ui.button("Add to playlist").clicked() {
                                add_only = Some(i);
                                ui.close_menu();
                            }
                            if ui.button("Add & play").clicked() {
                                add_play = Some(i);
                                ui.close_menu();
                            }
                        });
                        if let Some(d) = track.duration {
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    ui.label(
                                        RichText::new(fmt_time(d)).monospace().color(Color32::GRAY),
                                    );
                                },
                            );
                        }
                    });
                }
            });

        if let Some(i) = add_only {
            if let Some(t) = self.lib_results.get(i).cloned() {
                self.playlist.tracks.push(t);
                self.status = format!("{} track(s) in playlist.", self.playlist.len());
            }
        }
        if let Some(i) = add_play {
            if let Some(t) = self.lib_results.get(i).cloned() {
                self.playlist.tracks.push(t);
                let idx = self.playlist.tracks.len() - 1;
                self.play_index(idx);
            }
        }
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

        // Persist the session (playlist, window layout, EQ) whenever it changes,
        // so it survives exit without relying on eframe's save-on-exit.
        self.persist_session_if_changed();

        // Graceful shutdown on a window-manager close (Alt+F4 / WM "X"). Tearing
        // down the root window while the EQ/playlist/popup viewports still exist
        // crashes winit (an unwrap on a TranslateCoordinates query of the dying
        // window). Close the children first, stop drawing the classic windows,
        // and close the root a couple of frames later once they're gone.
        if self.quit_countdown.is_none() && ctx.input(|i| i.viewport().close_requested()) {
            self.eq_visible = false;
            self.pl_visible = false;
            self.popup = None;
            self.quit_countdown = Some(2);
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
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

        // Follow the system default output device if it changes mid-session.
        if now.duration_since(self.last_device_check).as_secs_f32() > 1.5 {
            self.player.poll_device();
            self.last_device_check = now;
        }

        self.handle_shortcuts(ctx, ClWin::Main);

        // Advance a running fade-out; stop when it reaches zero.
        if let Some(t) = self.fade {
            let t = t - dt;
            if t <= 0.0 {
                self.player.stop();
                self.player.set_fade(1.0);
                self.fade = None;
            } else {
                self.player.set_fade(t / FADE_SECS);
                self.fade = Some(t);
            }
        }

        if self.player.finished() {
            if self.stop_after_current {
                self.player.stop();
                self.stop_after_current = false; // one-shot, like Winamp
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
            ctx.request_repaint_after(Duration::from_millis(33));
            return;
        }

        // Modern fallback window: handle drops here.
        self.handle_dropped(ctx);

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
                View::Library => self.ui_library(ui),
            }
        });

        let interval =
            if self.player.state() == PlayState::Playing && self.viz_mode != VizMode::Off {
                33
            } else {
                100
            };
        ctx.request_repaint_after(Duration::from_millis(interval));
    }
}

/// Upload (or update in place) a composed RGBA frame into a nearest-neighbour
/// texture, returning a handle to draw with.
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

fn fmt_time(d: Duration) -> String {
    let total = d.as_secs();
    format!("{:02}:{:02}", total / 60, total % 60)
}

/// Produce a horizontally scrolled view of `text` for the LCD marquee.
fn marquee(text: &str, offset: f32) -> String {
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= 40 {
        return text.to_string();
    }
    let padded: Vec<char> = text.chars().chain("      ".chars()).collect();
    let start = (offset as usize) % padded.len();
    padded.iter().cycle().skip(start).take(40).collect()
}

/// Scroll `text` within a fixed character `width` (Winamp-style marquee).
fn scroll_text(text: &str, offset: f32, width: usize) -> String {
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= width {
        return text.to_string();
    }
    let padded: Vec<char> = text.chars().chain("   *** ".chars()).collect();
    let start = (offset as usize) % padded.len();
    padded.iter().cycle().skip(start).take(width).collect()
}

/// A Winamp-style on/off toggle rendered as a selectable label.
fn toggle(ui: &mut egui::Ui, value: &mut bool, label: &str) -> egui::Response {
    let mut resp = ui.selectable_label(*value, label);
    if resp.clicked() {
        *value = !*value;
        resp.mark_changed();
    }
    resp
}

fn apply_theme(ctx: &egui::Context, t: &Theme) {
    let mut visuals = egui::Visuals::dark();
    visuals.panel_fill = t.bg;
    visuals.window_fill = t.bg;
    visuals.extreme_bg_color = t.list_bg;
    visuals.faint_bg_color = t.panel;
    visuals.widgets.inactive.bg_fill = t.panel;
    visuals.widgets.hovered.bg_fill = skin::blend(t.panel, Color32::WHITE, 0.12);
    visuals.widgets.active.bg_fill = t.accent;
    visuals.selection.bg_fill = t.list_sel_bg;
    visuals.override_text_color = Some(skin::contrast_text(t.bg));
    ctx.set_visuals(visuals);

    let mut style = (*ctx.style()).clone();
    style.spacing.item_spacing = egui::vec2(6.0, 4.0);
    style.spacing.slider_width = 120.0;
    ctx.set_style(style);
}

#[cfg(test)]
mod tests {
    use super::{dock_group_from_main, parse_time, push_off, rects_docked};

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
    use egui::{pos2, vec2, Rect};

    fn r(x: f32, y: f32, w: f32, h: f32) -> Rect {
        Rect::from_min_size(pos2(x, y), vec2(w, h))
    }

    #[test]
    fn stacked_windows_are_docked() {
        assert!(rects_docked(r(0.0, 0.0, 275.0, 116.0), r(0.0, 116.0, 275.0, 116.0)));
    }

    #[test]
    fn push_off_main_keeps_it_uncovered() {
        let main = r(100.0, 100.0, 200.0, 100.0); // x 100..300, y 100..200
        // Flush below -> not overlapping, no push.
        assert_eq!(push_off(r(100.0, 200.0, 200.0, 100.0), main), None);
        // Overlapping mainly on main's left edge -> pushed out to the left.
        let w = r(60.0, 120.0, 60.0, 40.0);
        let p = push_off(w, main).unwrap();
        assert!((p.x + 60.0 - main.left()).abs() < 0.01, "pushed flush to the left edge");
        // Fully inside -> pushed to the nearest edge; result no longer overlaps.
        let inside = r(150.0, 130.0, 50.0, 40.0);
        let p = push_off(inside, main).unwrap();
        let moved = Rect::from_min_size(p, vec2(50.0, 40.0));
        assert!(push_off(moved, main).is_none(), "result still overlaps main");
        // Dropped mostly ABOVE main (only a sliver overlaps its top edge) -> it is
        // allowed above: pushed up so its bottom meets main's top.
        let above = r(150.0, 70.0, 50.0, 40.0); // y 70..110, overlaps main's top 100..110
        let p = push_off(above, main).unwrap();
        assert!((p.y + 40.0 - main.top()).abs() < 0.01, "clearly-above window snaps above");
        // Centred vertically -> favours below over above despite similar distance.
        let centred = r(150.0, 150.0, 50.0, 100.0); // y150..250 around main 100..200
        let p = push_off(centred, main).unwrap();
        assert!(p.y >= main.top(), "centred window favours below, not above");
    }

    #[test]
    fn gap_is_not_docked() {
        assert!(!rects_docked(r(0.0, 0.0, 275.0, 116.0), r(0.0, 140.0, 275.0, 116.0)));
    }

    #[test]
    fn full_stack_all_follow_main() {
        let rects = [
            r(0.0, 0.0, 275.0, 116.0),
            r(0.0, 116.0, 275.0, 116.0),
            r(0.0, 232.0, 275.0, 180.0),
        ];
        assert_eq!(dock_group_from_main(&rects, &[true, true, true]), [true, true, true]);
    }

    #[test]
    fn eq_pl_docked_only_to_each_other_dont_follow_main() {
        // main alone; eq+pl docked to each other but far from main.
        let rects = [
            r(0.0, 0.0, 275.0, 116.0),
            r(600.0, 0.0, 275.0, 116.0),
            r(600.0, 116.0, 275.0, 180.0),
        ];
        assert_eq!(dock_group_from_main(&rects, &[true, true, true]), [true, false, false]);
    }

    #[test]
    fn detached_eq_breaks_the_chain_to_pl() {
        // eq floated away; pl is flush under eq but neither connects to main.
        let rects = [
            r(0.0, 0.0, 275.0, 116.0),
            r(0.0, 300.0, 275.0, 116.0),
            r(0.0, 416.0, 275.0, 180.0),
        ];
        assert_eq!(dock_group_from_main(&rects, &[true, true, true]), [true, false, false]);
    }

    #[test]
    fn hidden_windows_never_follow() {
        let rects = [
            r(0.0, 0.0, 275.0, 116.0),
            r(0.0, 116.0, 275.0, 116.0),
            r(0.0, 232.0, 275.0, 180.0),
        ];
        // eq hidden -> it can't be in the group, and can't bridge to pl.
        assert_eq!(dock_group_from_main(&rects, &[true, false, true]), [true, false, false]);
    }
}
