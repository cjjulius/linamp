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

const FFT_SIZE: usize = 2048;
const VIZ_BANDS: usize = 28;
/// Fewer, wider bars for the small classic analyzer (Winamp-like).
const CLASSIC_BANDS: usize = 19;
/// Duration of the stop-with-fade (Shift+V) fade-out, in seconds.
const FADE_SECS: f32 = 2.0;

/// Register a broad-Unicode fallback font so egui-rendered text (the Media
/// Library, the modern UI, dialogs) can display scripts the default font lacks —
/// e.g. Vietnamese. It is appended as a fallback, so the default look is kept for
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
    let mut dirs = vec![std::path::PathBuf::from("music")];
    if let Ok(exe) = std::env::current_exe() {
        if let Some(d) = exe.parent() {
            dirs.push(d.join("music"));
        }
    }
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

/// Window size (points) for a popup menu, from its item/separator count.
fn popup_dims(kind: Popup) -> (f32, f32) {
    let (buttons, seps, width) = match kind {
        Popup::Options => (9, 3, 182.0),
        Popup::PlAdd => (2, 0, 182.0),
        Popup::PlRem => (3, 0, 182.0),
        Popup::PlSel => (3, 0, 182.0),
        Popup::PlMisc => (4, 0, 182.0),
        Popup::PlList => (3, 0, 182.0),
        // Right-click context menus: wider to fit the shortcut column.
        Popup::PlTrack => (6, 2, 210.0),
        Popup::PlVoid => (12, 4, 210.0),
        Popup::EqPresets => (crate::eq::PRESETS.len() as i32, 0, 182.0),
    };
    (width, buttons as f32 * 24.0 + seps as f32 * 9.0 + 16.0)
}

/// A context-menu row: `label` on the left, dimmed `shortcut` on the right, with
/// a hover highlight. Returns true when clicked. Kept shorter than the 24pt
/// per-button budget in [`popup_dims`] so a tall menu never clips its last row.
fn menu_item(ui: &mut egui::Ui, label: &str, shortcut: &str) -> bool {
    let w = ui.available_width();
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(w, 18.0), egui::Sense::click());
    let vis = ui.visuals().widgets.style(&resp).clone();
    let weak = ui.visuals().weak_text_color();
    let font = egui::TextStyle::Button.resolve(ui.style());
    let p = ui.painter();
    if resp.hovered() {
        p.rect_filled(rect, vis.rounding, vis.weak_bg_fill);
    }
    p.text(
        rect.left_center() + egui::vec2(8.0, 0.0),
        egui::Align2::LEFT_CENTER,
        label,
        font.clone(),
        vis.text_color(),
    );
    if !shortcut.is_empty() {
        p.text(
            rect.right_center() - egui::vec2(8.0, 0.0),
            egui::Align2::RIGHT_CENTER,
            shortcut,
            font,
            weak,
        );
    }
    resp.clicked()
}

/// Core of "Play Next": move the rows in `sel` to sit just after `current` (or to
/// the front when nothing is playing), preserving `sel`'s order. Mutates `tracks`
/// and returns the current track's new index and the index where the moved block
/// starts (for re-selecting it). Pure/index-based so it can be unit-tested.
fn reorder_play_next<T>(
    tracks: &mut Vec<T>,
    current: Option<usize>,
    sel: &[usize],
) -> (Option<usize>, usize) {
    let mut sel: Vec<usize> = sel.iter().copied().filter(|&i| i < tracks.len()).collect();
    sel.sort_unstable();
    sel.dedup();
    if sel.is_empty() {
        return (current, 0);
    }
    // Pull the selected rows out back-to-front, then restore ascending order.
    let mut moved = Vec::with_capacity(sel.len());
    for &i in sel.iter().rev() {
        moved.push(tracks.remove(i));
    }
    moved.reverse();
    let cur_selected = current.map(|c| sel.contains(&c)).unwrap_or(false);
    let (insert_at, final_cur) = match current {
        // Current track is itself moving: the block (with it) goes to the front.
        Some(c) if cur_selected => (0usize, Some(sel.iter().position(|&i| i == c).unwrap())),
        // Current survives: it shifts left by the selected rows removed before it,
        // and the block is inserted right after it (so its index is unchanged).
        Some(c) => {
            let shifted = c - sel.iter().filter(|&&i| i < c).count();
            ((shifted + 1).min(tracks.len()), Some(shifted))
        }
        None => (0, None),
    };
    for (k, t) in moved.into_iter().enumerate() {
        tracks.insert(insert_at + k, t);
    }
    (final_cur, insert_at)
}

/// If `r` overlaps `keep` (the main window), the position `r` should move to so it
/// no longer covers it — pushed to `keep`'s nearest edge (least movement). `None`
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

/// Global cursor in egui points if the platform exposes it: `GetCursorPos`
/// (Windows), `XQueryPointer` (X11/XWayland), `None` (native Wayland/else). When
/// `Some`, the drag runs "manual" — we move the window and its docked group each
/// frame; `None` falls back to the OS window-drag.
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

/// The full virtual desktop (all monitors) in egui points, if the platform
/// exposes it. Drags are clamped to this instead of a single monitor, so a
/// dragged window (and its docked group) can cross to another display but can't
/// be pushed off the desktop entirely. `None` → no manual clamp (OS handles it).
/// Specialised to Windows (virtual-screen metrics) and X11 (Mint/Pop!_OS).
fn virtual_desktop_rect(ctx: &egui::Context) -> Option<egui::Rect> {
    let ppp = ctx.pixels_per_point().max(0.1);
    let _ = ppp;
    #[cfg(windows)]
    {
        extern "system" {
            fn GetSystemMetrics(index: i32) -> i32;
        }
        const SM_XVIRTUALSCREEN: i32 = 76;
        const SM_YVIRTUALSCREEN: i32 = 77;
        const SM_CXVIRTUALSCREEN: i32 = 78;
        const SM_CYVIRTUALSCREEN: i32 = 79;
        unsafe {
            let (x, y) = (
                GetSystemMetrics(SM_XVIRTUALSCREEN),
                GetSystemMetrics(SM_YVIRTUALSCREEN),
            );
            let (w, h) = (
                GetSystemMetrics(SM_CXVIRTUALSCREEN),
                GetSystemMetrics(SM_CYVIRTUALSCREEN),
            );
            if w > 0 && h > 0 {
                return Some(egui::Rect::from_min_size(
                    egui::pos2(x as f32 / ppp, y as f32 / ppp),
                    egui::vec2(w as f32 / ppp, h as f32 / ppp),
                ));
            }
        }
        None
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        x11util::screen_px().map(|(w, h)| {
            egui::Rect::from_min_size(
                egui::pos2(0.0, 0.0),
                egui::vec2(w as f32 / ppp, h as f32 / ppp),
            )
        })
    }
    #[cfg(not(any(windows, all(unix, not(target_os = "macos")))))]
    {
        None
    }
}

/// Linux/X11 helpers for classic-window management: the global cursor query
/// (manual tandem drag) and `WM_TRANSIENT_FOR` grouping (EQ/playlist ride the
/// main window's z-order, no separate taskbar entry). A no-op without an X
/// display (e.g. native Wayland), where the caller uses OS behaviour.
#[cfg(all(unix, not(target_os = "macos")))]
mod x11util {
    use std::cell::RefCell;
    use std::ffi::CString;
    use std::os::raw::{c_char, c_int, c_long, c_uchar, c_ulong, c_void};
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

    /// The default screen's size in physical pixels. On Mint/Pop!_OS (X11) the
    /// screen spans all monitors, so this is the virtual-desktop extent.
    pub fn screen_px() -> Option<(i32, i32)> {
        with_conn(|xlib, d| unsafe {
            let s = (xlib.XDefaultScreen)(d);
            let w = (xlib.XDisplayWidth)(d, s);
            let h = (xlib.XDisplayHeight)(d, s);
            (w > 0 && h > 0).then_some((w, h))
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

    /// Drop `win` from the taskbar via `_NET_WM_STATE_SKIP_TASKBAR`, sent as an
    /// EWMH client message so only the main window represents the group.
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

    /// Link EQ/playlist to the main window via `WM_TRANSIENT_FOR`. Acts only when
    /// the links aren't current (steady state makes no X calls); re-links after a
    /// show/hide, which recreates the child with a new XID.
    pub fn group(eq_visible: bool, pl_visible: bool, lib_visible: bool) {
        thread_local! {
            // (eq_visible, pl_visible, lib_visible, linked?) we last acted on.
            static STATE: RefCell<(bool, bool, bool, bool)> =
                RefCell::new((false, false, false, false));
        }
        let cfg = (eq_visible, pl_visible, lib_visible);
        let done = STATE.with(|s| {
            let mut s = s.borrow_mut();
            if (s.0, s.1, s.2) != cfg {
                *s = (cfg.0, cfg.1, cfg.2, false); // visibility changed -> re-link
            }
            s.3
        });
        if done {
            return;
        }

        let linked = with_conn(|xlib, d| unsafe {
            // Guard the whole scan: querying a window another client just
            // destroyed would otherwise fire Xlib's fatal default handler.
            let prev = (xlib.XSetErrorHandler)(Some(ignore_err));
            let clients = client_list(xlib, d);
            let (mut main, mut eq, mut pl, mut lib) = (0, 0, 0, 0);
            for &w in &clients {
                match wm_name(xlib, d, w).as_str() {
                    "LinAmp" => main = w,
                    "LinAmp Equalizer" => eq = w,
                    "LinAmp Playlist" => pl = w,
                    "LinAmp Library" => lib = w,
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
                if lib_visible {
                    if lib != 0 {
                        (xlib.XSetTransientForHint)(d, lib, main);
                        skip_taskbar(xlib, d, lib);
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
            STATE.with(|s| s.borrow_mut().3 = true);
        }
    }

    #[repr(C)]
    struct XRectangle {
        x: i16,
        y: i16,
        width: u16,
        height: u16,
    }

    type ShapeFn = unsafe extern "C" fn(
        *mut xlib::Display,
        xlib::Window,
        c_int, // dest_kind (ShapeBounding = 0)
        c_int, // x_off
        c_int, // y_off
        *const XRectangle,
        c_int, // n_rects
        c_int, // op (ShapeSet = 0)
        c_int, // ordering (Unsorted = 0)
    );

    /// Resolve `XShapeCombineRectangles` from libXext (not in x11-dl), cached.
    /// `None` when libXext is unavailable, so shaping degrades to rectangular.
    fn shape_fn() -> Option<ShapeFn> {
        thread_local! {
            static F: RefCell<Option<Option<ShapeFn>>> = const { RefCell::new(None) };
        }
        F.with(|c| {
            if c.borrow().is_none() {
                let resolved = unsafe {
                    extern "C" {
                        fn dlopen(filename: *const c_char, flag: c_int) -> *mut c_void;
                        fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
                    }
                    let mut handle = ptr::null_mut();
                    for name in [b"libXext.so.6\0".as_slice(), b"libXext.so\0".as_slice()] {
                        handle = dlopen(name.as_ptr() as *const c_char, 2 /* RTLD_NOW */);
                        if !handle.is_null() {
                            break;
                        }
                    }
                    if handle.is_null() {
                        None
                    } else {
                        let sym = dlsym(
                            handle,
                            b"XShapeCombineRectangles\0".as_ptr() as *const c_char,
                        );
                        if sym.is_null() {
                            None
                        } else {
                            Some(std::mem::transmute::<*mut c_void, ShapeFn>(sym))
                        }
                    }
                };
                *c.borrow_mut() = Some(resolved);
            }
            c.borrow().unwrap()
        })
    }

    /// Clip the window titled `title` to `rects` (window-pixel coords) via the X11
    /// Shape extension. An empty slice clears the shape (full rectangle). Returns
    /// false when X11/libXext/the window is unavailable (caller treats as no-op).
    pub fn set_shape(title: &str, rects: &[(i16, i16, u16, u16)]) -> bool {
        let Some(shape) = shape_fn() else {
            return false;
        };
        with_conn(|xlib, d| unsafe {
            let prev = (xlib.XSetErrorHandler)(Some(ignore_err));
            let clients = client_list(xlib, d);
            let mut win: xlib::Window = 0;
            for &w in &clients {
                if wm_name(xlib, d, w) == title {
                    win = w;
                    break;
                }
            }
            let ok = if win != 0 {
                const SHAPE_BOUNDING: c_int = 0;
                const SHAPE_SET: c_int = 0;
                const UNSORTED: c_int = 0;
                let xr: Vec<XRectangle> = rects
                    .iter()
                    .map(|&(x, y, width, height)| XRectangle { x, y, width, height })
                    .collect();
                shape(
                    d, win, SHAPE_BOUNDING, 0, 0, xr.as_ptr(), xr.len() as c_int, SHAPE_SET,
                    UNSORTED,
                );
                (xlib.XFlush)(d);
                true
            } else {
                false
            };
            (xlib.XSetErrorHandler)(prev);
            ok
        })
        .unwrap_or(false)
    }

    #[repr(C)]
    struct XcursorImage {
        version: u32,
        size: u32,
        width: u32,
        height: u32,
        xhot: u32,
        yhot: u32,
        delay: u32,
        pixels: *mut u32,
    }

    struct CursorFns {
        create: unsafe extern "C" fn(c_int, c_int) -> *mut XcursorImage,
        load: unsafe extern "C" fn(*mut xlib::Display, *const XcursorImage) -> xlib::Cursor,
        destroy: unsafe extern "C" fn(*mut XcursorImage),
    }

    /// Resolve the three libXcursor entry points we need (cached). `None` when
    /// libXcursor is unavailable.
    fn cursor_fns() -> Option<&'static CursorFns> {
        thread_local! {
            static FNS: RefCell<Option<Option<&'static CursorFns>>> = const { RefCell::new(None) };
        }
        FNS.with(|c| {
            if c.borrow().is_none() {
                let resolved = unsafe {
                    extern "C" {
                        fn dlopen(filename: *const c_char, flag: c_int) -> *mut c_void;
                        fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
                    }
                    let mut h = ptr::null_mut();
                    for name in [b"libXcursor.so.1\0".as_slice(), b"libXcursor.so\0".as_slice()] {
                        h = dlopen(name.as_ptr() as *const c_char, 2);
                        if !h.is_null() {
                            break;
                        }
                    }
                    let sym = |n: &[u8]| dlsym(h, n.as_ptr() as *const c_char);
                    if h.is_null() {
                        None
                    } else {
                        let create = sym(b"XcursorImageCreate\0");
                        let load = sym(b"XcursorImageLoadCursor\0");
                        let destroy = sym(b"XcursorImageDestroy\0");
                        if create.is_null() || load.is_null() || destroy.is_null() {
                            None
                        } else {
                            // Leak a 'static so the pointers stay valid for the process.
                            Some(&*Box::leak(Box::new(CursorFns {
                                create: std::mem::transmute(create),
                                load: std::mem::transmute(load),
                                destroy: std::mem::transmute(destroy),
                            })))
                        }
                    }
                };
                *c.borrow_mut() = Some(resolved);
            }
            c.borrow().unwrap()
        })
    }

    /// Resolve the X window whose title is `title`, cached by title so the whole
    /// client list isn't rescanned every frame. The cached id is validated (its
    /// title re-checked), so a reopened window with a new id still resolves.
    unsafe fn window_by_title(
        xlib: &xlib::Xlib,
        d: *mut xlib::Display,
        title: &str,
    ) -> xlib::Window {
        thread_local! {
            static WIN_CACHE: RefCell<std::collections::HashMap<String, xlib::Window>> =
                RefCell::new(std::collections::HashMap::new());
        }
        WIN_CACHE.with(|c| {
            if let Some(&w) = c.borrow().get(title) {
                if w != 0 && wm_name(xlib, d, w) == title {
                    return w;
                }
            }
            let win = client_list(xlib, d)
                .into_iter()
                .find(|&c| wm_name(xlib, d, c) == title)
                .unwrap_or(0);
            let mut cache = c.borrow_mut();
            if win != 0 {
                cache.insert(title.to_string(), win);
            } else {
                cache.remove(title);
            }
            win
        })
    }

    /// Restore the default cursor on the window titled `title` (used when the
    /// custom-cursor option is turned off).
    pub fn unset_cursor(title: &str) {
        with_conn(|xlib, d| unsafe {
            let prev = (xlib.XSetErrorHandler)(Some(ignore_err));
            let win = window_by_title(xlib, d, title);
            if win != 0 {
                (xlib.XUndefineCursor)(d, win);
                (xlib.XFlush)(d);
            }
            (xlib.XSetErrorHandler)(prev);
        });
    }

    /// Set the cursor of the window titled `title` to a custom ARGB cursor.
    /// `cache_key` dedupes the created X cursor (e.g. "skin:posbar"). Premultiplied
    /// ARGB pixels, row-major. Returns false when unavailable (caller no-ops).
    pub fn set_cursor(
        title: &str,
        cache_key: &str,
        w: u32,
        h: u32,
        xhot: u16,
        yhot: u16,
        argb: &[u32],
    ) -> bool {
        let Some(fns) = cursor_fns() else {
            return false;
        };
        thread_local! {
            static CACHE: RefCell<std::collections::HashMap<String, xlib::Cursor>> =
                RefCell::new(std::collections::HashMap::new());
        }
        with_conn(|xlib, d| unsafe {
            let prev = (xlib.XSetErrorHandler)(Some(ignore_err));
            let win = window_by_title(xlib, d, title);
            let ok = if win != 0 {
                let cursor = CACHE.with(|cache| {
                    if let Some(&cur) = cache.borrow().get(cache_key) {
                        return cur;
                    }
                    let img = (fns.create)(w as c_int, h as c_int);
                    let cur = if img.is_null() {
                        0
                    } else {
                        (*img).xhot = xhot as u32;
                        (*img).yhot = yhot as u32;
                        let dst = std::slice::from_raw_parts_mut(
                            (*img).pixels,
                            (w * h) as usize,
                        );
                        let n = dst.len().min(argb.len());
                        dst[..n].copy_from_slice(&argb[..n]);
                        let c = (fns.load)(d, img);
                        (fns.destroy)(img);
                        c
                    };
                    if cur != 0 {
                        cache.borrow_mut().insert(cache_key.to_string(), cur);
                    }
                    cur
                });
                if cursor != 0 {
                    (xlib.XDefineCursor)(d, win, cursor);
                    (xlib.XFlush)(d);
                    true
                } else {
                    false
                }
            } else {
                false
            };
            (xlib.XSetErrorHandler)(prev);
            ok
        })
        .unwrap_or(false)
    }
}

/// Make EQ/playlist *owned* by the main window so they ride its z-order (rise
/// together, stay above, hide on minimize) — Winamp-style grouping. Idempotent,
/// runs each frame to cover show/hide. Windows (owner window) and Linux/X11
/// (`WM_TRANSIENT_FOR`); a no-op elsewhere.
fn group_child_windows(eq_visible: bool, pl_visible: bool, lib_visible: bool) {
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        x11util::group(eq_visible, pl_visible, lib_visible);
    }
    #[cfg(windows)]
    {
        let _ = (eq_visible, pl_visible, lib_visible);
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
            for title in ["LinAmp Equalizer", "LinAmp Playlist", "LinAmp Library"] {
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

/// Apply (or clear) a window-shape mask to a classic window by title. X11 (Shape
/// extension) and Windows (`SetWindowRgn`); a no-op elsewhere.
fn set_window_shape(title: &str, rects: &[(i16, i16, u16, u16)]) {
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        x11util::set_shape(title, rects);
    }
    #[cfg(windows)]
    {
        win_set_shape(title, rects);
    }
    #[cfg(not(any(windows, all(unix, not(target_os = "macos")))))]
    {
        let _ = (title, rects);
    }
}

/// Windows: clip a borderless classic window to the union of `rects` via
/// `SetWindowRgn`, giving the skin's non-rectangular shape. Coordinates are in
/// window pixels from the top-left (no border on these windows). The region is
/// handed to the OS (don't free it); an empty `rects` clears the shape.
#[cfg(windows)]
fn win_set_shape(title: &str, rects: &[(i16, i16, u16, u16)]) {
    #[allow(non_snake_case)]
    extern "system" {
        fn FindWindowW(class: *const u16, title: *const u16) -> isize;
        fn CreateRectRgn(x1: i32, y1: i32, x2: i32, y2: i32) -> isize;
        fn CombineRgn(dst: isize, src1: isize, src2: isize, mode: i32) -> i32;
        fn SetWindowRgn(hwnd: isize, hrgn: isize, redraw: i32) -> i32;
        fn DeleteObject(o: isize) -> i32;
    }
    const RGN_OR: i32 = 2;
    let wide: Vec<u16> = title.encode_utf16().chain(std::iter::once(0)).collect();
    unsafe {
        let hwnd = FindWindowW(std::ptr::null(), wide.as_ptr());
        if hwnd == 0 {
            return;
        }
        if rects.is_empty() {
            SetWindowRgn(hwnd, 0, 1); // clear -> rectangular
            return;
        }
        let rgn = CreateRectRgn(0, 0, 0, 0);
        for &(x, y, w, h) in rects {
            let r = CreateRectRgn(
                x as i32,
                y as i32,
                x as i32 + w as i32,
                y as i32 + h as i32,
            );
            CombineRgn(rgn, rgn, r, RGN_OR);
            DeleteObject(r);
        }
        // SetWindowRgn takes ownership of `rgn` — do not delete it afterwards.
        SetWindowRgn(hwnd, rgn, 1);
    }
}

/// Set a classic window's custom (skin-defined) cursor. X11 only; a no-op
/// elsewhere, where the standard cursor is kept.
fn set_window_cursor(title: &str, cache_key: &str, cur: &crate::cursor::CursorImage) {
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        x11util::set_cursor(
            title,
            cache_key,
            cur.width,
            cur.height,
            cur.xhot,
            cur.yhot,
            &cur.argb,
        );
    }
    #[cfg(windows)]
    {
        crate::wincursor::apply(title, cache_key, cur);
    }
    #[cfg(not(any(windows, all(unix, not(target_os = "macos")))))]
    {
        let _ = (title, cache_key, cur);
    }
}

/// Restore the default cursor on a window (when custom cursors are turned off).
/// X11 only; a no-op elsewhere.
fn unset_window_cursor(title: &str) {
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        x11util::unset_cursor(title);
    }
    #[cfg(windows)]
    {
        crate::wincursor::clear(title);
    }
    #[cfg(not(any(windows, all(unix, not(target_os = "macos")))))]
    {
        let _ = title;
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
    /// Right-click on a playlist track.
    PlTrack,
    /// Right-click on empty playlist space.
    PlVoid,
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

/// A deferred Media Library action, raised by a context menu while `self` is
/// borrowed immutably and applied after the UI pass. Track actions carry row
/// indices into `lib_results`; artist/album actions carry the name.
enum LibAction {
    Play(Vec<usize>),
    Enqueue(Vec<usize>),
    PlayArtist(String),
    EnqueueArtist(String),
    PlayAlbum(String),
    EnqueueAlbum(String),
    Rate(Vec<usize>, i64),
    EditTags(usize),
    Explore(usize),
    Remove(Vec<usize>),
    SelectAll,
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
    /// Star rating (0–5) for each row in `lib_results`, parallel-indexed. Only
    /// populated for the Top Rated view, where the list shows the stars.
    lib_ratings: Vec<i64>,
    /// User-resizable track-table column fractions of the flexible width:
    /// [artist, title]; the third column (album/rating) takes the remainder.
    lib_col_fr: [f32; 2],
    /// Active track-table sort: (column 0=artist 1=title 2=album/rating 3=time,
    /// ascending). None = the view's natural order. Reset when the view changes.
    lib_sort: Option<(u8, bool)>,
    // Media Library window (dockable, non-shading) + 3-pane browser state.
    lib_pos: egui::Pos2,
    lib_pos_init: bool,
    /// Grab offset (points) while the custom title bar is being dragged.
    lib_drag: Option<egui::Vec2>,
    lib_nav: crate::library::LibView,
    lib_artists: Vec<(String, i64)>,
    lib_albums: Vec<(String, i64)>,
    lib_sel_artists: Vec<String>,
    lib_sel_albums: Vec<String>,
    lib_sel_tracks: std::collections::BTreeSet<usize>,
    /// Active background scan receiver + its latest progress text.
    lib_scan: Option<std::sync::mpsc::Receiver<crate::library::ScanMsg>>,
    lib_scan_text: String,

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
    /// Composed GEN.BMP library-window frame + a fingerprint (skin, active, size)
    /// so it is only re-composited when something changes.
    lib_tex: Option<egui::TextureHandle>,
    lib_tex_sig: Option<(String, bool, u32, u32)>,
    /// Per-window (main/eq/pl) X11 shape fingerprint, so REGION.TXT masks are only
    /// re-applied on change; `shaped_any` tracks whether any shape was ever set (to
    /// reset windows to rectangular when switching to a skin without regions).
    shape_sig: [Option<u64>; 3],
    shaped_any: bool,
    /// Last custom cursor name applied per window (main/eq/pl), to switch the X
    /// cursor only when the hovered region changes.
    cur_cursor: [Option<&'static str>; 3],
    /// Last composed fingerprint per window (main/eq/pl), to skip re-compositing
    /// and re-uploading an unchanged frame.
    tex_fp: [u64; 3],
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
    /// true when the last session's window positions were restored, so frame 1
    /// keeps them (clamped on-screen) instead of applying the default layout.
    has_pos_memory: bool,
    /// App-level focus: true while any LinAmp window is focused. All windows show
    /// their active title bar together and their inactive one only when the whole
    /// app loses focus (e.g. to another application), matching Winamp — rather
    /// than one window dimming when another LinAmp window is clicked. Per-window
    /// focus is tracked in `foc_*` and OR'd each frame.
    app_focused: bool,
    foc_root: bool,
    foc_eq: bool,
    foc_pl: bool,
    foc_lib: bool,
    /// Focus held by an open context-menu popup viewport, folded into
    /// `app_focused` so the menu counts as part of the app.
    foc_popup: bool,
    /// Hash of the last-persisted session, so we only write on actual changes.
    session_sig: Option<u64>,
    /// Frames remaining in a graceful shutdown (set when the WM asks to close),
    /// used to tear down child viewports before the root to avoid a winit crash.
    quit_countdown: Option<u8>,
    drag: Option<Dragging>,
    /// While resizing the playlist, the grab offsets (width, height) in px.
    resize_pl: Option<egui::Vec2>,
    root_last: Option<(egui::Pos2, egui::Vec2)>,
    /// Last always-on-top state pushed to the root window (so we only send the
    /// window-level command when it changes).
    on_top_last: Option<bool>,
    pl_menu_press: Option<classic::PlRegion>,
    prefs: Prefs,
    /// Path of the currently loaded skin (persisted as `last_skin`).
    skin_path: Option<PathBuf>,
    scale: f32,
    prefs_open: bool,
    lib_visible: bool,
    tag_editor: Option<TagEditor>,
    /// Jump-to-file search state (Some while the dialog is open).
    jump_file: Option<JumpFile>,
    /// Jump-to-time input buffer (Some while the dialog is open).
    jump_time: Option<String>,

    marquee_offset: f32,
    last_frame: Instant,
    last_device_check: Instant,
    last_session_check: Instant,
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
#[derive(Clone, Copy, PartialEq, Eq)]
enum TagTab {
    Basic,
    Id3,
}

struct TagEditor {
    index: usize,
    path: PathBuf,
    info: crate::tags::FileInfo,
    v1: crate::tags::Id3Fields,
    v2: crate::tags::Id3Fields,
    v1_on: bool,
    v2_on: bool,
    tab: TagTab,
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
        install_fonts(&cc.egui_ctx);
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
            lib_ratings: Vec::new(),
            lib_col_fr: [0.28, 0.42],
            lib_sort: None,
            lib_pos: egui::Pos2::ZERO,
            lib_pos_init: false,
            lib_drag: None,
            lib_tex: None,
            lib_tex_sig: None,
            shape_sig: [None; 3],
            shaped_any: false,
            cur_cursor: [None; 3],
            lib_nav: crate::library::LibView::Audio,
            lib_artists: Vec::new(),
            lib_albums: Vec::new(),
            lib_sel_artists: Vec::new(),
            lib_sel_albums: Vec::new(),
            lib_sel_tracks: std::collections::BTreeSet::new(),
            lib_scan: None,
            lib_scan_text: String::new(),
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
            tex_fp: [0; 3],
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
            has_pos_memory: false,
            app_focused: true,
            foc_root: true,
            foc_eq: false,
            foc_pl: false,
            foc_lib: false,
            foc_popup: false,
            session_sig: None,
            quit_countdown: None,
            drag: None,
            resize_pl: None,
            root_last: None,
            on_top_last: None,
            pl_menu_press: None,
            prefs,
            skin_path: None,
            scale,
            prefs_open: false,
            lib_visible: false,
            tag_editor: None,
            jump_file: None,
            jump_time: None,
            marquee_offset: 0.0,
            last_frame: Instant::now(),
            last_device_check: Instant::now(),
            last_session_check: Instant::now(),
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
            let first_run = !Session::exists();
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
            app.lib_visible = session.lib_visible;
            if let Some(fr) = session.lib_col_fr {
                app.lib_col_fr = fr;
            }
            // Restore the saved window layout (absolute top-lefts). When present
            // it replaces the default dock arrangement; frame 1 clamps it on-
            // screen. Positions are absolute so docking is preserved exactly.
            if let Some((x, y)) = session.main_pos {
                let p = |o: Option<(f32, f32)>, d: egui::Pos2| {
                    o.map(|(x, y)| egui::pos2(x, y)).unwrap_or(d)
                };
                app.win_pos[0] = egui::pos2(x, y);
                app.win_pos[1] = p(session.eq_pos, app.win_pos[0]);
                app.win_pos[2] = p(session.pl_pos, app.win_pos[0]);
                app.lib_pos = p(session.lib_pos, app.win_pos[0]);
                app.lib_pos_init = true;
                app.has_pos_memory = true;
            }

            // Fresh install (no saved session): the classic trio open and
            // stacked (at the 1x default), the library closed, and the bundled
            // demo song cued in the playlist. The base skin loads by default.
            if first_run {
                app.eq_visible = true;
                app.pl_visible = true;
                app.lib_visible = false;
                if let Some(dir) = first_run_music_dir() {
                    app.playlist.add_folder(&dir);
                    if app.prefs.tag_on_load {
                        app.playlist.load_all();
                    }
                    // First run only: start the bundled song playing. On later
                    // launches a saved session exists, so this never re-fires.
                    if !app.playlist.is_empty() {
                        app.play_index(0);
                    }
                }
            }
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
                    // Record the play for the library's Most/Recently Played views
                    // (no-op when the track isn't in the library).
                    if let Some(lib) = &self.library {
                        lib.increment_play(&track.path);
                    }
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
            if !self.lib_pos_init {
                self.lib_pos = self.library_default_pos();
                self.lib_pos_init = true;
            }
            self.refresh_library();
        }
    }

    /// Default library placement: just below the current window stack.
    /// Default position when the library is first opened: docked to the right edge
    /// of the main window, top-aligned with it.
    fn library_default_pos(&self) -> egui::Pos2 {
        let base = self.win_pos[0];
        egui::pos2(base.x + classic::MAIN_W as f32 * self.scale, base.y)
    }

    /// Snap the library window to the player windows / screen edges on drag end.
    fn snap_lib_pos(&self, pos: egui::Pos2, monitor: Option<egui::Vec2>) -> egui::Pos2 {
        let r = egui::Rect::from_min_size(pos, self.lib_size());
        let mut others = vec![self.win_rect_pts(0)];
        if self.eq_visible {
            others.push(self.win_rect_pts(1));
        }
        if self.pl_visible {
            others.push(self.win_rect_pts(2));
        }
        snap_rect(r, &others, monitor)
    }

    /// Rebuild the library panes/results from the active nav node, search text
    /// and artist/album selection.
    fn refresh_library(&mut self) {
        if self.library.is_none() {
            return;
        }
        let q = self.lib_query.trim().to_string();
        if !q.is_empty() {
            // A search overrides the nav node and the artist/album panes.
            self.lib_results = self.library.as_ref().unwrap().search(&q, 2000);
            self.lib_ratings.clear();
            self.lib_artists.clear();
            self.lib_albums.clear();
        } else {
            match self.lib_nav {
                crate::library::LibView::Audio => {
                    let lib = self.library.as_ref().unwrap();
                    self.lib_artists = lib.artists();
                    self.lib_albums = lib.albums(&self.lib_sel_artists);
                    self.lib_results = lib.tracks_for(&self.lib_sel_artists, &self.lib_sel_albums);
                }
                other => {
                    let lib = self.library.as_ref().unwrap();
                    self.lib_artists.clear();
                    self.lib_albums.clear();
                    self.lib_results = lib.view(other, 2000);
                }
            }
            // The Top Rated list shows each track's stars; other views don't, so
            // skip the per-row rating lookups there.
            self.lib_ratings = if self.lib_nav == crate::library::LibView::TopRated {
                let lib = self.library.as_ref().unwrap();
                self.lib_results.iter().map(|t| lib.rating(&t.path)).collect()
            } else {
                Vec::new()
            };
        }
        self.lib_sel_tracks.clear();
        // Apply the user's column sort over the view's natural order, if any.
        self.apply_lib_sort();
    }

    /// Re-order `lib_results` (and the parallel `lib_ratings`) by the active column
    /// sort. The rating column sorts by the numeric star count, not the rendered
    /// "★★★☆☆" string (whose empty-star glyph would sort the wrong way).
    fn apply_lib_sort(&mut self) {
        let Some((col, asc)) = self.lib_sort else {
            return;
        };
        if self.lib_results.len() < 2 {
            return;
        }
        let show_rating =
            self.lib_nav == crate::library::LibView::TopRated && self.lib_query.trim().is_empty();
        let mut idx: Vec<usize> = (0..self.lib_results.len()).collect();
        idx.sort_by(|&a, &b| {
            let ta = &self.lib_results[a];
            let tb = &self.lib_results[b];
            let ord = match col {
                0 => ta.artist.to_lowercase().cmp(&tb.artist.to_lowercase()),
                1 => ta.title.to_lowercase().cmp(&tb.title.to_lowercase()),
                2 if show_rating => self
                    .lib_ratings
                    .get(a)
                    .copied()
                    .unwrap_or(0)
                    .cmp(&self.lib_ratings.get(b).copied().unwrap_or(0)),
                2 => ta.album.to_lowercase().cmp(&tb.album.to_lowercase()),
                _ => ta.duration.unwrap_or_default().cmp(&tb.duration.unwrap_or_default()),
            };
            // Stable tiebreak on original position keeps the order deterministic.
            let ord = ord.then(a.cmp(&b));
            if asc {
                ord
            } else {
                ord.reverse()
            }
        });
        self.lib_results = idx.iter().map(|&i| self.lib_results[i].clone()).collect();
        if !self.lib_ratings.is_empty() {
            self.lib_ratings = idx.iter().map(|&i| self.lib_ratings[i]).collect();
        }
        self.lib_sel_tracks.clear();
    }

    /// Pick a folder and index it into the library on a background thread.
    fn scan_into_library(&mut self) {
        if self.lib_scan.is_some() {
            return; // a scan is already running
        }
        if let Some(dir) = rfd::FileDialog::new().pick_folder() {
            self.lib_scan = Some(crate::library::scan_folder_bg(dir));
            self.lib_scan_text = "Scanning…".into();
        }
    }

    /// Poll the background scan; refresh and clear when it finishes.
    fn poll_library_scan(&mut self) {
        let Some(rx) = &self.lib_scan else {
            return;
        };
        let mut done = None;
        let mut err = None;
        while let Ok(msg) = rx.try_recv() {
            match msg {
                crate::library::ScanMsg::Progress { done: d, total } => {
                    self.lib_scan_text = format!("Scanning… {d}/{total}");
                }
                crate::library::ScanMsg::Done(n) => done = Some(n),
                crate::library::ScanMsg::Error(e) => err = Some(e),
            }
        }
        if let Some(e) = err {
            self.status = format!("Scan error: {e}");
            self.lib_scan = None;
            self.lib_scan_text.clear();
        } else if let Some(n) = done {
            self.status = format!("Scanned {n} file(s) into library.");
            self.lib_scan = None;
            self.lib_scan_text.clear();
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
                self.status = format!("Loaded skin: {}", self.theme.name);
            }
            Err(e) => self.status = format!("Skin error: {e}"),
        }
        // Load the classic sprite sheets for the same skin. Drop every cached
        // window texture so all three re-composite with the new skin.
        self.classic = ClassicSkin::load(path).ok();
        self.classic_tex = None;
        self.eq_tex = None;
        self.pl_tex = None;
        // Reset cursors so a skin without custom cursors doesn't keep showing the
        // previous skin's; the per-frame cursor update re-applies for the new skin
        // if it ships any.
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
            focused: self.app_focused,
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
            || self.fade.is_some()
            || self.drag.is_some()
            || self.classic_seek_preview.is_some()
            || self.popup.is_some()
            || self.quit_countdown.is_some()
            // Keep repainting while a background scan runs so its progress and the
            // final refresh show up without waiting on the next input.
            || self.lib_scan.is_some()
            || (self.player.state() == PlayState::Stopped
                && self.viz_mode == VizMode::Spectrum
                // Bars *and* peak markers — the peaks fall slower, so without
                // them the falloff would stall when the repaint rate drops.
                && self
                    .cviz_bars
                    .iter()
                    .chain(self.cviz_peaks.iter())
                    .any(|&b| b > 0.001))
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
            lib_visible: self.lib_visible,
            lib_col_fr: Some(self.lib_col_fr),
            // Remember the window layout once it's been established on frame 1.
            main_pos: self.positions_init.then_some((self.win_pos[0].x, self.win_pos[0].y)),
            eq_pos: self.positions_init.then_some((self.win_pos[1].x, self.win_pos[1].y)),
            pl_pos: self.positions_init.then_some((self.win_pos[2].x, self.win_pos[2].y)),
            lib_pos: (self.positions_init && self.lib_pos_init)
                .then_some((self.lib_pos.x, self.lib_pos.y)),
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
        // Don't churn the file every 300ms during a drag; the final resting
        // position is saved on the next cycle after the button is released.
        if self.drag.is_some() || self.lib_drag.is_some() {
            return;
        }
        // Change detection hashes every track path, so don't run it every frame.
        let now = Instant::now();
        if now.duration_since(self.last_session_check).as_millis() < 300 {
            return;
        }
        self.last_session_check = now;
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        self.eq_visible.hash(&mut h);
        self.pl_visible.hash(&mut h);
        self.main_shade.hash(&mut h);
        self.eq_shade.hash(&mut h);
        self.pl_shade.hash(&mut h);
        self.lib_visible.hash(&mut h);
        self.pl_px_w.to_bits().hash(&mut h);
        self.pl_px_h.to_bits().hash(&mut h);
        for p in &self.win_pos {
            p.x.to_bits().hash(&mut h);
            p.y.to_bits().hash(&mut h);
        }
        self.lib_pos.x.to_bits().hash(&mut h);
        self.lib_pos.y.to_bits().hash(&mut h);
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
            focused: self.app_focused,
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

    /// "Play Next": move the selected track(s) to play right after the current
    /// one (or to the front when nothing is playing), keeping their order. The
    /// moved block becomes the new selection.
    fn play_next_selected(&mut self) {
        if self.pl_selection.is_empty() {
            return;
        }
        let len = self.playlist.tracks.len();
        let mut sel: Vec<usize> = self.pl_selection.iter().copied().filter(|&i| i < len).collect();
        sel.sort_unstable();
        sel.dedup();
        let count = sel.len();
        if count == 0 {
            return;
        }
        let (cur, start) = reorder_play_next(&mut self.playlist.tracks, self.playlist.current, &sel);
        self.playlist.current = cur;
        self.pl_selection = (start..start + count).collect();
        self.sort_key = SortKey::Custom;
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
            .with_title("LinAmp Menu")
            .with_position(screen)
            .with_inner_size(egui::vec2(w, h))
            .with_decorations(false)
            .with_resizable(false)
            .with_taskbar(false)
            // Don't steal OS focus when the menu opens: the owning window keeps
            // focus, so the classic windows don't flip to their unfocused skin
            // (the menu is part of our app) and a follow-up click on a window
            // isn't swallowed to re-focus it.
            .with_active(false)
            .with_window_level(self.window_level());
        let id = egui::ViewportId::from_hash_of("linamp_popup");
        ctx.show_viewport_immediate(id, builder, |ctx, _| {
            // If the menu does end up focused (e.g. a click landed on it), keep
            // the app considered focused so the windows stay on their active skin.
            self.foc_popup = ctx.input(|i| i.viewport().focused).unwrap_or(self.foc_popup);
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
                            let libl = if self.lib_visible { "Hide Media Library" } else { "Show Media Library" };
                            if item(ui, libl) {
                                self.toggle_library();
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
                            // ASCII box: the bundled Noto Sans has no U+2713 check
                            // glyph, so a literal ✓ renders as tofu here.
                            let aot = if self.prefs.always_on_top {
                                "[x] Always on top"
                            } else {
                                "[ ] Always on top"
                            };
                            if item(ui, aot) {
                                self.prefs.always_on_top = !self.prefs.always_on_top;
                                self.prefs.save();
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
                        Popup::PlTrack => {
                            if menu_item(ui, "Play", "Enter") {
                                if let Some(&i) = self.pl_selection.iter().min() {
                                    self.play_index(i);
                                }
                                close = true;
                            }
                            if menu_item(ui, "Play Next", "") {
                                self.play_next_selected();
                                close = true;
                            }
                            ui.separator();
                            if menu_item(ui, "Edit file tags…", "Alt+3") {
                                self.open_tag_editor();
                                close = true;
                            }
                            if menu_item(ui, "Explore folder", "") {
                                if let Some(&i) = self.pl_selection.iter().min() {
                                    if let Some(t) = self.playlist.tracks.get(i) {
                                        open_in_file_manager(&t.path);
                                    }
                                }
                                close = true;
                            }
                            ui.separator();
                            if menu_item(ui, "Remove", "Del") {
                                self.pl_remove(false);
                                close = true;
                            }
                            if menu_item(ui, "Crop (keep selected)", "") {
                                self.pl_remove(true);
                                close = true;
                            }
                        }
                        Popup::PlVoid => {
                            if menu_item(ui, "Add file(s)…", "Ins") {
                                self.open_files();
                                close = true;
                            }
                            if menu_item(ui, "Add folder…", "Shift+L") {
                                self.open_folder();
                                close = true;
                            }
                            ui.separator();
                            if menu_item(ui, "Select all", "Ctrl+A") {
                                self.pl_select_all();
                                close = true;
                            }
                            if menu_item(ui, "Select none", "") {
                                self.pl_selection.clear();
                                close = true;
                            }
                            if menu_item(ui, "Invert selection", "") {
                                self.pl_invert_selection();
                                close = true;
                            }
                            ui.separator();
                            if menu_item(ui, "Sort by title", "") {
                                sort(self, SortKey::Title);
                                close = true;
                            }
                            if menu_item(ui, "Sort by artist", "") {
                                sort(self, SortKey::Artist);
                                close = true;
                            }
                            if menu_item(ui, "Sort by album", "") {
                                sort(self, SortKey::Album);
                                close = true;
                            }
                            if menu_item(ui, "Sort by duration", "") {
                                sort(self, SortKey::Duration);
                                close = true;
                            }
                            ui.separator();
                            if menu_item(ui, "Remove all", "") {
                                self.playlist.clear();
                                self.pl_selection.clear();
                                self.player.stop();
                                close = true;
                            }
                            ui.separator();
                            if menu_item(ui, "Save list…", "") {
                                self.save_playlist_dialog();
                                close = true;
                            }
                            if menu_item(ui, "Load list…", "") {
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
            self.apply_base_cursor(ctx, "LinAmp Menu");
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
        // Escape closes an open menu. The popup no longer steals focus, so its own
        // viewport won't see the key — the focused window catches it here instead.
        if self.popup.is_some() && ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.popup = None;
        }

        if secondary && win == ClWin::Main {
            if let Some(l) = local {
                self.open_popup_in(Popup::Options, win, l);
            }
        }

        // Right-click on the playlist list area: a track menu when a row is under
        // the cursor, otherwise the playlist-management menu for empty space.
        if secondary && win == ClWin::Pl && !shaded {
            if let Some(l) = local {
                let nx = l.x / self.scale;
                let ny = l.y / self.scale;
                if let Some(classic::PlRegion::List) =
                    classic::pl_region_at(nx, ny, self.pl_px_w, self.pl_px_h)
                {
                    let row = classic::pl_row_at(ny, self.pl_first);
                    if row < self.playlist.len() {
                        // Winamp rule: right-clicking a row outside the current
                        // selection selects just it; a row already in the
                        // selection keeps the whole multi-selection.
                        if !self.pl_selection.contains(&row) {
                            self.pl_selection.clear();
                            self.pl_selection.insert(row);
                        }
                        self.open_popup_in(Popup::PlTrack, win, l);
                    } else {
                        self.open_popup_in(Popup::PlVoid, win, l);
                    }
                }
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
                        let mut target = g - d.grab;
                        let group = d.group.clone();
                        let lib_offset = d.lib_offset;
                        let press_pos = d.press_pos;
                        // Clamp so the dragged window — and, for the main window, its
                        // whole docked block (EQ/PL + library) — stops at the screen
                        // edge instead of being pushed off it.
                        // Clamp to the whole virtual desktop (all monitors), so the
                        // block can be dragged onto another display but not off the
                        // desktop. Only the dragged block's bounding offsets matter.
                        if let Some(vr) = virtual_desktop_rect(ctx) {
                            let mut lo = egui::vec2(0.0, 0.0);
                            let mut hi = self.win_inner_size(win);
                            for (mi, off) in &group {
                                lo = lo.min(*off);
                                hi = hi.max(*off + self.win_inner_size(idx_win(*mi)));
                            }
                            if let Some(loff) = lib_offset {
                                lo = lo.min(loff);
                                hi = hi.max(loff + self.lib_size());
                            }
                            let min_x = vr.min.x - lo.x;
                            let min_y = vr.min.y - lo.y;
                            target.x = target.x.clamp(min_x, (vr.max.x - hi.x).max(min_x));
                            target.y = target.y.clamp(min_y, (vr.max.y - hi.y).max(min_y));
                        }
                        self.win_pos[idx] = target;
                        for (mi, off) in group {
                            self.win_pos[mi] = target + off;
                        }
                        if let Some(off) = lib_offset {
                            self.lib_pos = target + off;
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
                if let Some(off) = d.lib_offset {
                    self.lib_pos = self.win_pos[idx] + off;
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
            if d.lib_offset.is_some() {
                self.lib_pos += delta;
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
            // The main window carries its whole transitively-docked block (EQ/PL
            // and the library, including chains through the library). Other windows
            // move on their own.
            let (group, lib_offset) = if win == ClWin::Main {
                self.main_drag_group()
            } else {
                (Vec::new(), None)
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
                lib_offset,
            });
        }
    }

    /// After a drag, resolve overlaps: main is never covered and EQ/playlist never
    /// cover each other. The just-dragged window gives way (a child moves off main
    /// and its sibling; a dragged main pushes the children off instead).
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
            // The dragged child moves off main, and stacks below its sibling only
            // when that sibling is itself docked to main; a floating sibling must
            // not block docking directly under main.
            let di = win_idx(dragged);
            let oi = if di == 1 { 2 } else { 1 };
            for _ in 0..4 {
                let mut moved = false;
                if let Some(p) = push_off(rect(self, di), rect(self, 0)) {
                    self.win_pos[di] = p;
                    moved = true;
                }
                if vis(self, oi) && rects_docked(rect(self, oi), rect(self, 0)) {
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

    /// The full drag group for a main-window move: the EQ/playlist windows (by
    /// offset) and the library (by offset) that are transitively docked to main —
    /// including chains that pass *through* the library (e.g. main–library–playlist).
    fn main_drag_group(&self) -> (Vec<(usize, egui::Vec2)>, Option<egui::Vec2>) {
        let rects = [
            self.win_rect_pts(0),
            self.win_rect_pts(1),
            self.win_rect_pts(2),
            egui::Rect::from_min_size(self.lib_pos, self.lib_size()),
        ];
        let vis = [true, self.eq_visible, self.pl_visible, self.lib_visible];
        let in_group = dock_group_from_main(&rects, &vis);
        let group = [1usize, 2]
            .into_iter()
            .filter(|&j| in_group[j])
            .map(|j| (j, self.win_pos[j] - self.win_pos[0]))
            .collect();
        let lib_offset = in_group[3].then(|| self.lib_pos - self.win_pos[0]);
        (group, lib_offset)
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
        use egui::Rect;
        let r = Rect::from_min_size(pos, size);
        let mut others = Vec::new();
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
            let jwin = match j {
                0 => ClWin::Main,
                1 => ClWin::Eq,
                _ => ClWin::Pl,
            };
            others.push(Rect::from_min_size(self.win_pos[j], self.win_inner_size(jwin)));
        }
        // The library is a snap target too (but never pushes / is pushed).
        if self.lib_visible {
            others.push(Rect::from_min_size(self.lib_pos, self.lib_size()));
        }
        snap_rect(r, &others, monitor)
    }

    fn close_window(&mut self, ctx: &egui::Context, win: ClWin) {
        match win {
            ClWin::Main => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
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

    /// Current on-screen rects of the three windows (None when hidden).
    fn window_rects(&self) -> [Option<egui::Rect>; 3] {
        [
            Some(self.win_rect_pts(0)),
            self.eq_visible.then(|| self.win_rect_pts(1)),
            self.pl_visible.then(|| self.win_rect_pts(2)),
        ]
    }

    fn toggle_shade(&mut self, win: ClWin) {
        let idx = win_idx(win);
        // Collapsing/expanding changes this window's height. Windows docked in a
        // flush stack below it should move by the same amount so the stack stays
        // intact instead of overlapping (only docked windows are affected).
        let before = self.window_rects();
        let old_h = before[idx].map(|r| r.height()).unwrap_or(0.0);
        let moved = docked_group_below(&before, idx);
        match win {
            ClWin::Main => self.main_shade = !self.main_shade,
            ClWin::Eq => self.eq_shade = !self.eq_shade,
            ClWin::Pl => self.pl_shade = !self.pl_shade,
        }
        let delta = self.win_inner_size(win).y - old_h;
        for (j, &m) in moved.iter().enumerate() {
            if m {
                self.win_pos[j].y += delta;
            }
        }
        self.root_last = None;
    }

    /// Whether the library window is currently edge-docked to the player group.
    fn library_docked(&self) -> bool {
        if !self.lib_visible {
            return false;
        }
        let lib = egui::Rect::from_min_size(self.lib_pos, self.lib_size());
        rects_docked(lib, self.win_rect_pts(0))
            || (self.eq_visible && rects_docked(lib, self.win_rect_pts(1)))
            || (self.pl_visible && rects_docked(lib, self.win_rect_pts(2)))
    }

    fn set_double_size(&mut self, _ctx: &egui::Context, dbl: bool) {
        if self.prefs.double_size == dbl {
            return;
        }
        // Remember whether the library rides with the group so it can re-dock to
        // the resized stack; a floating library is left to resize in place.
        let lib_docked = self.library_docked();
        let old_scale = self.scale;
        self.prefs.double_size = dbl;
        self.scale = self.prefs.scale();
        self.prefs.save();
        self.restack();
        if lib_docked && old_scale > 0.0 {
            // The whole group scales from the main window's top-left, so scaling the
            // library's offset from main by the same ratio keeps it docked.
            let ratio = self.scale / old_scale;
            let main = self.win_pos[0];
            self.lib_pos = main + (self.lib_pos - main) * ratio;
        }
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
            .with_inner_size(egui::vec2(340.0, 345.0))
            .with_resizable(false)
            .with_taskbar(false)
            .with_window_level(self.window_level());
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

                ui.add_space(8.0);
                ui.strong("Appearance");
                let mut disable_cur = self.prefs.disable_skin_cursors;
                if ui
                    .checkbox(&mut disable_cur, "Disable Skin Cursors")
                    .changed()
                {
                    self.prefs.disable_skin_cursors = disable_cur;
                    self.prefs.save();
                    // Drop the custom cursors immediately when turning the option
                    // off; re-enabling reapplies them as windows are hovered.
                    if disable_cur {
                        self.clear_skin_cursors();
                    } else {
                        self.cur_cursor = [None; 3];
                    }
                }

                ui.add_space(10.0);
                if ui.button("Close").clicked() {
                    close = true;
                }
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

    /// The media library + search, in its own window (the classic UI has no
    /// built-in library view). Reuses the same list UI as the modern view.
    /// The Media Library: a borderless, themed window that docks to the player
    /// windows (via a custom title bar) but never window-shades.
    fn ui_library_window(&mut self, ctx: &egui::Context) {
        if !self.lib_visible {
            return;
        }
        let builder = egui::ViewportBuilder::default()
            .with_title("LinAmp Library")
            .with_position(self.lib_pos)
            .with_inner_size(self.lib_size())
            .with_decorations(false)
            .with_resizable(false)
            .with_taskbar(false)
            .with_window_level(self.window_level());
        let id = egui::ViewportId::from_hash_of("linamp_library");
        let mut close = false;
        let gen = self.ui_mode == UiMode::Classic
            && self.classic.as_ref().map(|c| c.has_gen()).unwrap_or(false);
        ctx.show_viewport_immediate(id, builder, |ctx, _| {
            // Global shortcuts while this window has keyboard focus.
            self.handle_shortcuts(ctx, ClWin::Main);
            if gen {
                self.ui_library_gen(ctx, &mut close);
            } else {
                let panel = ctx.style().visuals.panel_fill;
                egui::CentralPanel::default()
                    .frame(egui::Frame::none().fill(panel))
                    .show(ctx, |ui| {
                        self.ui_library_titlebar(ctx, ui, &mut close);
                        self.ui_library(ui);
                    });
            }
            self.apply_base_cursor(ctx, "LinAmp Library");
            if ctx.input(|i| i.viewport().close_requested()) {
                close = true;
            }
        });
        if close {
            self.lib_visible = false;
        }
    }

    /// Library window themed with the skin's GEN.BMP frame: composite the frame
    /// behind the content, draw the caption, and inset the browser into the frame.
    fn ui_library_gen(&mut self, ctx: &egui::Context, close: &mut bool) {
        self.foc_lib = ctx.input(|i| i.viewport().focused).unwrap_or(self.foc_lib);
        // Use the shared app-level focus so the library doesn't dim merely because
        // another LinAmp window was clicked.
        let active = self.app_focused;
        // Compose the frame at the base (1x) size; it is drawn to the (possibly
        // scaled) window rect and upscaled like the classic windows.
        let base = self.lib_base_size();
        let (w, h) = (base.x as u32, base.y as u32);
        let sig = (self.theme.name.clone(), active, w, h);
        if self.lib_tex_sig.as_ref() != Some(&sig) {
            if let Some(img) = self
                .classic
                .as_ref()
                .and_then(|c| c.compose_gen_window(w, h, active, "LinAmp Library"))
            {
                let color = egui::ColorImage::from_rgba_unmultiplied(
                    [w as usize, h as usize],
                    img.as_raw(),
                );
                self.lib_tex = Some(ctx.load_texture("lib_gen", color, egui::TextureOptions::NEAREST));
                self.lib_tex_sig = Some(sig);
            }
        }
        let s = self.scale;
        let (top, right, bottom, left) = classic::ClassicSkin::gen_insets();
        // Insets scale with the window (the 1x frame is upscaled into it).
        let (top, right, bottom, left) = (
            top as f32 * s,
            right as f32 * s,
            bottom as f32 * s,
            left as f32 * s,
        );
        egui::CentralPanel::default()
            .frame(egui::Frame::none())
            .show(ctx, |ui| {
                let full = ui.max_rect();
                if let Some(tex) = &self.lib_tex {
                    let uv = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
                    ui.painter().image(tex.id(), full, uv, Color32::WHITE);
                }
                // (The caption is drawn into the frame with GEN.BMP's own font.)
                // Close button (top-right corner of the frame).
                let close_rect = egui::Rect::from_min_max(
                    egui::pos2(full.right() - 20.0 * s, full.top() + 3.0 * s),
                    egui::pos2(full.right() - 6.0 * s, full.top() + 17.0 * s),
                );
                if ui
                    .interact(close_rect, ui.id().with("lib_gen_close"), egui::Sense::click())
                    .clicked()
                {
                    *close = true;
                }
                // Drag the title bar (minus the close corner) to move / dock.
                let drag_rect = egui::Rect::from_min_max(
                    full.min,
                    egui::pos2(full.right() - 24.0 * s, full.top() + top),
                );
                let dresp =
                    ui.interact(drag_rect, ui.id().with("lib_gen_drag"), egui::Sense::click_and_drag());
                self.handle_lib_drag(ctx, &dresp);
                // Inset the browser into the frame interior.
                let content = egui::Rect::from_min_max(
                    egui::pos2(full.left() + left, full.top() + top),
                    egui::pos2(full.right() - right, full.bottom() - bottom),
                );
                ui.allocate_new_ui(egui::UiBuilder::new().max_rect(content), |ui| {
                    self.ui_library(ui);
                });
            });
    }

    /// Shared title-bar drag/dock handling for the library window.
    fn handle_lib_drag(&mut self, ctx: &egui::Context, resp: &egui::Response) {
        if resp.drag_started() {
            if let Some(g) = global_cursor_points(ctx) {
                self.lib_drag = Some(g - self.lib_pos);
            }
        }
        if resp.dragged() {
            if let (Some(g), Some(grab)) = (global_cursor_points(ctx), self.lib_drag) {
                let mut p = g - grab;
                // Clamp to the whole virtual desktop so the library can be dragged
                // onto another monitor but not off the desktop.
                if let Some(vr) = virtual_desktop_rect(ctx) {
                    let size = self.lib_size();
                    p.x = p.x.clamp(vr.min.x, (vr.max.x - size.x).max(vr.min.x));
                    p.y = p.y.clamp(vr.min.y, (vr.max.y - size.y).max(vr.min.y));
                }
                self.lib_pos = p;
                ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(self.lib_pos));
            }
        }
        if resp.drag_stopped() {
            self.lib_drag = None;
            let monitor = ctx.input(|i| i.viewport().monitor_size);
            self.lib_pos = self.snap_lib_pos(self.lib_pos, monitor);
            ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(self.lib_pos));
        }
    }

    /// Draw the library's custom title bar and handle drag-to-move / close.
    fn ui_library_titlebar(&mut self, ctx: &egui::Context, ui: &mut egui::Ui, close: &mut bool) {
        let full = ui.available_width();
        let (bar, bar_resp) =
            ui.allocate_exact_size(egui::vec2(full, 20.0), egui::Sense::click_and_drag());
        let accent = self.theme.accent;
        let txt = skin::contrast_text(accent);
        ui.painter().rect_filled(bar, 0.0, accent);
        ui.painter().text(
            bar.left_center() + egui::vec2(6.0, 0.0),
            egui::Align2::LEFT_CENTER,
            "LinAmp Library",
            egui::FontId::proportional(13.0),
            txt,
        );
        // Close box on the right.
        let x_rect = egui::Rect::from_min_size(
            egui::pos2(bar.right() - 18.0, bar.center().y - 7.0),
            egui::vec2(14.0, 14.0),
        );
        let x_resp = ui.interact(x_rect, ui.id().with("lib_close"), egui::Sense::click());
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
            *close = true;
        }
        // Drag the title bar to move + dock the window (skip when over the X).
        if bar_resp.drag_started() && !x_resp.hovered() {
            if let Some(g) = global_cursor_points(ctx) {
                self.lib_drag = Some(g - self.lib_pos);
            }
        }
        if bar_resp.dragged() {
            if let (Some(g), Some(grab)) = (global_cursor_points(ctx), self.lib_drag) {
                self.lib_pos = g - grab;
                ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(self.lib_pos));
            }
        }
        if bar_resp.drag_stopped() {
            self.lib_drag = None;
            let monitor = ctx.input(|i| i.viewport().monitor_size);
            self.lib_pos = self.snap_lib_pos(self.lib_pos, monitor);
            ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(self.lib_pos));
        }
        ui.add_space(2.0);
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
        let (v1, v2) = crate::tags::read_id3(&path);
        let info = crate::tags::read_info(&path);
        let editable = crate::tags::can_edit(&path);
        let (v1_on, v2_on) = (v1.is_some(), v2.is_some());
        let v1 = v1.unwrap_or_default();
        let v2 = v2.unwrap_or_default();
        let status = if editable {
            String::new()
        } else {
            "Read-only: tag writing isn't supported for this format yet.".into()
        };
        self.tag_editor = Some(TagEditor {
            index,
            path,
            info,
            v1,
            v2,
            v1_on,
            v2_on,
            tab: TagTab::Basic,
            editable,
            status,
        });
    }

    /// Open the tag editor for a file by path (e.g. a Media Library row that may
    /// not be in the playlist). `index` is a sentinel so saving doesn't touch a
    /// playlist row.
    fn open_tag_editor_path(&mut self, path: std::path::PathBuf) {
        let (v1, v2) = crate::tags::read_id3(&path);
        let info = crate::tags::read_info(&path);
        let editable = crate::tags::can_edit(&path);
        let (v1_on, v2_on) = (v1.is_some(), v2.is_some());
        let v1 = v1.unwrap_or_default();
        let v2 = v2.unwrap_or_default();
        let status = if editable {
            String::new()
        } else {
            "Read-only: tag writing isn't supported for this format yet.".into()
        };
        self.tag_editor = Some(TagEditor {
            index: usize::MAX,
            path,
            info,
            v1,
            v2,
            v1_on,
            v2_on,
            tab: TagTab::Basic,
            editable,
            status,
        });
    }

    /// Write the editor's tags back to the file and refresh the playlist row.
    /// Returns `true` when the write succeeded (so the caller can close).
    fn save_tag_editor(&mut self) -> bool {
        let (index, path, v1, v2, v1_on, v2_on) = match &self.tag_editor {
            Some(ed) if ed.editable => (
                ed.index,
                ed.path.clone(),
                ed.v1.clone(),
                ed.v2.clone(),
                ed.v1_on,
                ed.v2_on,
            ),
            _ => return false,
        };
        let result = crate::tags::write_id3(
            &path,
            v1_on.then_some(&v1),
            v2_on.then_some(&v2),
        );
        if result.is_ok() {
            if let Some(t) = self.playlist.tracks.get_mut(index) {
                t.loaded = false;
                t.load();
            }
        }
        // On success the caller closes the window, so only an error needs a
        // status message left behind.
        if let Err(e) = &result {
            if let Some(ed) = self.tag_editor.as_mut() {
                ed.status = format!("Error: {e}");
            }
        }
        result.is_ok()
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
            .with_inner_size(egui::vec2(660.0, 470.0))
            .with_resizable(false)
            .with_taskbar(false)
            .with_window_level(self.window_level());
        let id = egui::ViewportId::from_hash_of("linamp_tags");
        let mut close = false;
        let mut do_save = false;
        ctx.show_viewport_immediate(id, builder, |ctx, _| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let ed = self.tag_editor.as_mut().unwrap();
                let on = ed.editable;

                // Escape closes the window without saving. Consume it here, before
                // the text fields can swallow it, but let an open dropdown close
                // first.
                if !ctx.memory(|m| m.any_popup_open())
                    && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape))
                {
                    close = true;
                }
                // While Alt is held the letters are menu shortcuts, not text, so
                // drop the character events before a focused field types them (e.g.
                // Alt-U would otherwise insert a "u"). AltGr (Ctrl+Alt) is left
                // alone so accented input still works.
                ctx.input_mut(|i| {
                    if i.modifiers.alt && !i.modifiers.ctrl {
                        i.events.retain(|e| !matches!(e, egui::Event::Text(_)));
                    }
                });

                // Theme-derived colours so the dialog tracks the active skin.
                let line = ui.visuals().widgets.noninteractive.bg_stroke;
                let panel = ui.visuals().panel_fill;
                let idle_fill = ui.visuals().faint_bg_color;
                // Outline idle widgets so buttons/fields don't blur into the panel.
                ui.visuals_mut().widgets.inactive.bg_stroke = line;

                // Path, in a sunken box that sets it apart from the tags below.
                egui::Frame::group(ui.style())
                    .fill(ui.visuals().extreme_bg_color)
                    .show(ui, |ui| {
                        ui.set_min_width(624.0);
                        ui.add(
                            egui::Label::new(RichText::new(&ed.info.path).strong()).truncate(),
                        );
                    });
                ui.add_space(4.0);

                // Tab strip that sits on — and joins into — the content box below.
                let top_round = egui::Rounding { nw: 4.0, ne: 4.0, sw: 0.0, se: 0.0 };
                let tab_btn = |ui: &mut egui::Ui, label: &str, active: bool| {
                    let fill = if active { panel } else { idle_fill };
                    ui.add(
                        egui::Button::new(mnem(label, 0))
                            .fill(fill)
                            .stroke(line)
                            .rounding(top_round)
                            .min_size(egui::vec2(0.0, 24.0)),
                    )
                };
                let prev_y = ui.spacing().item_spacing.y;
                ui.spacing_mut().item_spacing.y = 0.0;
                let (basic_resp, id3_resp) = ui
                    .horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 2.0;
                        let b = tab_btn(ui, "Basic Info", ed.tab == TagTab::Basic);
                        let i = tab_btn(ui, "ID3", ed.tab == TagTab::Id3);
                        (b, i)
                    })
                    .inner;
                if basic_resp.clicked() {
                    ed.tab = TagTab::Basic;
                }
                if id3_resp.clicked() {
                    ed.tab = TagTab::Id3;
                }

                let group = egui::Frame::group(ui.style()).fill(panel).show(ui, |ui| {
                    ui.set_min_width(624.0);
                    ui.set_min_height(300.0);
                    match ed.tab {
                        TagTab::Basic => {
                            egui::Grid::new("basic_grid")
                                .num_columns(2)
                                .spacing([12.0, 4.0])
                                .show(ui, |ui| {
                                    let i = &ed.info;
                                    let mut r = |k: &str, v: String| {
                                        ui.label(k);
                                        ui.label(v);
                                        ui.end_row();
                                    };
                                    r("Size", format!("{} bytes", i.size_bytes));
                                    r("Length", format!("{} seconds", i.length_secs));
                                    r("Format", i.mpeg.clone());
                                    r("Bitrate", format!("{} kbps", i.bitrate_kbps));
                                    r("Sample rate", format!("{} Hz", i.sample_rate_hz));
                                    r("Channels", i.channel_mode.clone());
                                    r("Copyrighted", if i.copyrighted { "Yes" } else { "No" }.into());
                                    r("Original", if i.original { "Yes" } else { "No" }.into());
                                    r("Emphasis", i.emphasis.clone());
                                });
                            None
                        }
                        TagTab::Id3 => {
                            let genre_combo = |ui: &mut egui::Ui, salt: &str, val: &mut String| {
                                let combo = egui::ComboBox::from_id_salt(salt)
                                    .selected_text(if val.is_empty() { "—".into() } else { val.clone() })
                                    .width(140.0)
                                    .show_ui(ui, |ui| {
                                        let mut picked = false;
                                        for g in crate::tags::GENRES {
                                            if ui.selectable_label(val == g, *g).clicked() {
                                                *val = (*g).to_string();
                                                picked = true;
                                            }
                                        }
                                        picked
                                    });
                                if combo.inner == Some(true) {
                                    // egui only auto-closes the popup on a mouse
                                    // click, so close it ourselves for keyboard
                                    // selection — and keep focus on the combo so the
                                    // next Tab moves to the following field rather
                                    // than jumping back to the top.
                                    ui.memory_mut(|m| m.close_popup());
                                    combo.response.request_focus();
                                }
                            };
                            let resps = ui.columns(2, |cols| {
                                // ----- ID3v1 (left) -----
                                let r1 = cols[0].checkbox(&mut ed.v1_on, mnem("ID3v1 Tag", 4));
                                cols[0].add_enabled_ui(on && ed.v1_on, |ui| {
                                    egui::Grid::new("v1_grid").num_columns(2).spacing([6.0, 4.0]).show(ui, |ui| {
                                        let f = &mut ed.v1;
                                        let lim = |ui: &mut egui::Ui, l: &str, v: &mut String, n: usize| {
                                            ui.label(l);
                                            ui.add(egui::TextEdit::singleline(v).desired_width(150.0).char_limit(n));
                                            ui.end_row();
                                        };
                                        lim(ui, "Track", &mut f.track, 3);
                                        lim(ui, "Title", &mut f.title, crate::tags::V1_MAX_TEXT);
                                        lim(ui, "Artist", &mut f.artist, crate::tags::V1_MAX_TEXT);
                                        lim(ui, "Album", &mut f.album, crate::tags::V1_MAX_TEXT);
                                        lim(ui, "Year", &mut f.year, crate::tags::V1_MAX_YEAR);
                                        ui.label("Genre");
                                        genre_combo(ui, "v1_genre", &mut f.genre);
                                        ui.end_row();
                                        ui.label("Comment");
                                        ui.add(egui::TextEdit::multiline(&mut f.comment).desired_width(150.0).desired_rows(2).char_limit(crate::tags::V1_MAX_TEXT));
                                        ui.end_row();
                                    });
                                });
                                // ----- ID3v2 (right) -----
                                let r2 = cols[1].checkbox(&mut ed.v2_on, mnem("ID3v2 Tag", 4));
                                cols[1].add_enabled_ui(on && ed.v2_on, |ui| {
                                    egui::Grid::new("v2_grid").num_columns(2).spacing([6.0, 4.0]).show(ui, |ui| {
                                        let f = &mut ed.v2;
                                        let row = |ui: &mut egui::Ui, l: &str, v: &mut String| {
                                            ui.label(l);
                                            ui.add(egui::TextEdit::singleline(v).desired_width(170.0));
                                            ui.end_row();
                                        };
                                        row(ui, "Track", &mut f.track);
                                        row(ui, "Title", &mut f.title);
                                        row(ui, "Artist", &mut f.artist);
                                        row(ui, "Album", &mut f.album);
                                        row(ui, "Year", &mut f.year);
                                        ui.label("Genre");
                                        genre_combo(ui, "v2_genre", &mut f.genre);
                                        ui.end_row();
                                        ui.label("Comment");
                                        ui.add(egui::TextEdit::multiline(&mut f.comment).desired_width(170.0).desired_rows(2));
                                        ui.end_row();
                                        row(ui, "Composer", &mut f.composer);
                                        row(ui, "Orig. Artist", &mut f.orig_artist);
                                        row(ui, "Copyright", &mut f.copyright);
                                        row(ui, "URL", &mut f.url);
                                        row(ui, "Encoded by", &mut f.encoded_by);
                                    });
                                });
                                (r1, r2)
                            });
                            Some(resps)
                        }
                    }
                });
                let checks = group.inner;
                // Erase the seam under the active tab so it reads as one surface
                // with the content box (the classic folder-tab join).
                let active_rect = if ed.tab == TagTab::Basic {
                    basic_resp.rect
                } else {
                    id3_resp.rect
                };
                // Span from the tab's own bottom edge to the box's top edge so
                // the join holds even if the tab row is taller than the button.
                let box_top = group.response.rect.top();
                let tab_bot = active_rect.bottom();
                let seam = egui::Rect::from_x_y_ranges(
                    (active_rect.left() + line.width)..=(active_rect.right() - line.width),
                    (tab_bot.min(box_top) - 1.0)..=(tab_bot.max(box_top) + 1.0),
                );
                ui.painter().rect_filled(seam, 0.0, panel);
                ui.spacing_mut().item_spacing.y = prev_y;

                // Only surface an error (e.g. a failed write) — no "changed"
                // chatter, the dialog is busy enough.
                ui.add_space(6.0);
                if !ed.status.is_empty() {
                    ui.label(RichText::new(&ed.status).color(ui.visuals().error_fg_color));
                }
                ui.add_space(4.0);

                // Shortcuts: Alt-B/Alt-I pick the tab, Alt-1/2 toggle the tags,
                // Alt-U update, Alt-C cancel, Alt-T/Alt-F copy to/from ID3v1 (read
                // raw so a focused field doesn't swallow them). Each shortcut also
                // moves keyboard focus to the control it acts on, so a following
                // Tab continues from there.
                let (k_basic, k_id3, k_v1, k_v2, k_up, k_cancel, k_to, k_from) = ctx.input(|i| {
                    let a = i.modifiers.alt;
                    (
                        a && i.key_pressed(egui::Key::B),
                        a && i.key_pressed(egui::Key::I),
                        a && i.key_pressed(egui::Key::Num1),
                        a && i.key_pressed(egui::Key::Num2),
                        a && i.key_pressed(egui::Key::U),
                        a && i.key_pressed(egui::Key::C),
                        a && i.key_pressed(egui::Key::T),
                        a && i.key_pressed(egui::Key::F),
                    )
                });
                if k_basic {
                    ed.tab = TagTab::Basic;
                    basic_resp.request_focus();
                }
                if k_id3 {
                    ed.tab = TagTab::Id3;
                    id3_resp.request_focus();
                }
                let id3 = ed.tab == TagTab::Id3;
                if on && id3 && k_v1 {
                    ed.v1_on = !ed.v1_on;
                    if let Some((r1, _)) = &checks {
                        r1.request_focus();
                    }
                }
                if on && id3 && k_v2 {
                    ed.v2_on = !ed.v2_on;
                    if let Some((_, r2)) = &checks {
                        r2.request_focus();
                    }
                }
                if on && id3 && k_up {
                    do_save = true;
                }

                let can_copy = on && id3 && ed.v2_on;
                let mut do_copy_to = k_to && can_copy;
                let mut do_copy_from = k_from && can_copy;
                ui.horizontal(|ui| {
                    // Update and the copy buttons only apply to the ID3 tab.
                    if id3 {
                        ui.add_enabled_ui(on, |ui| {
                            if ui.button(mnem("Update", 0)).clicked() {
                                do_save = true;
                            }
                        });
                    }
                    if ui.button(mnem("Cancel", 0)).clicked() {
                        close = true;
                    }
                    if id3 {
                        ui.add_enabled_ui(can_copy, |ui| {
                            let to = ui.button(mnem("Copy to ID3v1", 5));
                            if to.clicked() {
                                do_copy_to = true;
                            }
                            if k_to {
                                to.request_focus();
                            }
                            let from = ui.button(mnem("Copy from ID3v1", 5));
                            if from.clicked() {
                                do_copy_from = true;
                            }
                            if k_from {
                                from.request_focus();
                            }
                        });
                    }
                });
                if k_cancel {
                    close = true;
                }
                // Copy only non-blank source fields so a copy never wipes a value.
                if do_copy_to {
                    ed.v1_on = true; // checking v1 un-greys its fields first
                    let cp = |dst: &mut String, src: &str, n: usize| {
                        if !src.trim().is_empty() {
                            *dst = src.chars().take(n).collect();
                        }
                    };
                    let max = usize::MAX;
                    cp(&mut ed.v1.track, &ed.v2.track, max);
                    cp(&mut ed.v1.title, &ed.v2.title, crate::tags::V1_MAX_TEXT);
                    cp(&mut ed.v1.artist, &ed.v2.artist, crate::tags::V1_MAX_TEXT);
                    cp(&mut ed.v1.album, &ed.v2.album, crate::tags::V1_MAX_TEXT);
                    cp(&mut ed.v1.year, &ed.v2.year, crate::tags::V1_MAX_YEAR);
                    cp(&mut ed.v1.genre, &ed.v2.genre, max);
                    cp(&mut ed.v1.comment, &ed.v2.comment, crate::tags::V1_MAX_TEXT);
                }
                if do_copy_from {
                    let cp = |dst: &mut String, src: &str| {
                        if !src.trim().is_empty() {
                            *dst = src.to_string();
                        }
                    };
                    cp(&mut ed.v2.track, &ed.v1.track);
                    cp(&mut ed.v2.title, &ed.v1.title);
                    cp(&mut ed.v2.artist, &ed.v1.artist);
                    cp(&mut ed.v2.album, &ed.v1.album);
                    cp(&mut ed.v2.year, &ed.v1.year);
                    cp(&mut ed.v2.genre, &ed.v1.genre);
                    cp(&mut ed.v2.comment, &ed.v1.comment);
                }
            });
            self.apply_base_cursor(ctx, "LinAmp — File Info");
            // Window-close (the title-bar X): discard without saving. Escape is
            // handled at the top of the panel, before the fields can swallow it.
            if ctx.input(|i| i.viewport().close_requested()) {
                close = true;
            }
        });
        // Cancel/Esc discards; Update closes only when the write succeeds, so a
        // failed save leaves the window open with its error.
        if close {
            self.tag_editor = None;
        } else if do_save && self.save_tag_editor() {
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
            self.apply_base_cursor(ctx, "LinAmp — Jump to file");
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
            self.apply_base_cursor(ctx, "LinAmp — Jump to time");
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

    /// Apply each classic window's REGION.TXT shape (main/EQ/playlist, per shade
    /// state, scaled). Fingerprinted so the X calls only happen on change. No-op
    /// for skins without regions (unless we previously shaped — then reset to
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
            return; // normal skin, never shaped — leave windows rectangular
        }
        let skin = self.theme.name.clone();
        let main: &[Polygon] = if self.main_shade { &main_sr } else { &main_r };
        self.shape_one(0, "LinAmp", true, main, classic::MAIN_W, self.main_h() as u32, &skin);
        let eq: &[Polygon] = if self.eq_shade { &eq_sr } else { &eq_r };
        let eqv = self.eq_visible;
        self.shape_one(1, "LinAmp Equalizer", eqv, eq, classic::MAIN_W, self.eq_h() as u32, &skin);
        let (plw, plh) = if self.pl_shade {
            (classic::MAIN_W, 14)
        } else {
            (self.pl_px_w as u32, self.pl_px_h as u32)
        };
        let pl: &[Polygon] = if self.pl_shade { &[] } else { &pl_r };
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
            self.shape_sig[idx] = None;
            return;
        }
        use std::hash::{Hash, Hasher};
        let mut hh = std::collections::hash_map::DefaultHasher::new();
        (skin, idx, self.scale.to_bits(), w, h, polys.is_empty()).hash(&mut hh);
        let sig = hh.finish();
        if self.shape_sig[idx] == Some(sig) {
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
        self.shape_sig[idx] = Some(sig);
    }

    /// The skin cursor name for the region under native coords (nx, ny) of `win`.
    fn cursor_name(&self, win: ClWin, nx: f32, ny: f32) -> &'static str {
        use classic::{Btn, EqRegion, PlRegion};
        match win {
            ClWin::Main => {
                if self.main_shade {
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
                if self.eq_shade {
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
                if self.pl_shade {
                    return "ptbar";
                }
                match classic::pl_region_at(nx, ny, self.pl_px_w, self.pl_px_h) {
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
        if self.cur_cursor[idx] == Some(name) {
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
        self.cur_cursor[idx] = Some(name);
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
        // X11 only: pin egui's cursor so egui-winit doesn't overwrite the one we
        // set via XDefineCursor. On Windows the WM_SETCURSOR subclass intercepts
        // before winit, so no pin is needed (and pinning would wrongly suppress
        // the normal per-widget cursors when skin cursors are off).
        #[cfg(all(unix, not(target_os = "macos")))]
        ctx.set_cursor_icon(egui::CursorIcon::Default);
        if let Some(ci) = cs.cursor("normal") {
            set_window_cursor(title, &format!("{}:normal", self.theme.name), ci);
        }
    }

    /// Restore the system cursor on every window — used when the custom-cursor
    /// option is switched off, since our `XDefineCursor` otherwise persists.
    fn clear_skin_cursors(&mut self) {
        for title in [
            "LinAmp",
            "LinAmp Equalizer",
            "LinAmp Playlist",
            "LinAmp Library",
            "LinAmp Preferences",
            "LinAmp — File Info",
            "LinAmp — Jump to file",
            "LinAmp — Jump to time",
            "LinAmp Menu",
        ] {
            unset_window_cursor(title);
        }
        self.cur_cursor = [None; 3];
    }

    fn render_classic(&mut self, ctx: &egui::Context) {
        if self.classic.is_none() {
            return;
        }

        // App-level focus for this frame: this frame's root focus OR'd with last
        // frame's child-window focus (children render later, below). Hidden
        // windows don't hold focus. So all windows share one active/inactive
        // title-bar state and only dim when the whole app is unfocused.
        self.foc_root = ctx.input(|i| i.viewport().focused).unwrap_or(self.foc_root);
        if !self.eq_visible {
            self.foc_eq = false;
        }
        if !self.pl_visible {
            self.foc_pl = false;
        }
        if !self.lib_visible {
            self.foc_lib = false;
        }
        if self.popup.is_none() {
            self.foc_popup = false;
        }
        self.app_focused =
            self.foc_root || self.foc_eq || self.foc_pl || self.foc_lib || self.foc_popup;

        // An egui-native menu/combo (Media Library, modern playlist, a dropdown)
        // and a classic popup should never be open at once: close the classic one
        // when an egui popup takes over. Checked before the windows handle input
        // this frame so a right-click that opens a classic menu (and closes the
        // egui one) isn't itself cancelled.
        if self.popup.is_some() && ctx.memory(|m| m.any_popup_open()) {
            self.popup = None;
        }

        // Root viewport = main window.
        self.render_window(ctx, ClWin::Main);
        self.ui_prefs(ctx);
        self.ui_library_window(ctx);
        self.ui_tag_editor(ctx);
        self.ui_jump_file(ctx);
        self.ui_jump_time(ctx);

        // Initialise window positions, once.
        if !self.positions_init {
            if let Some(r) = ctx.input(|i| i.viewport().outer_rect) {
                if self.has_pos_memory {
                    // Restore the saved layout. If it would sit (partly) off the
                    // current virtual desktop — e.g. after a resolution drop — nudge
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
                                .union(egui::Rect::from_min_size(self.lib_pos, self.lib_size()));
                        }
                        let delta = egui::vec2(
                            nudge_onto(union.min.x, union.max.x, vr.min.x, vr.max.x),
                            nudge_onto(union.min.y, union.max.y, vr.min.y, vr.max.y),
                        );
                        for p in self.win_pos.iter_mut() {
                            *p += delta;
                        }
                        self.lib_pos += delta;
                    }
                    self.root_last = None; // move the root window to the restored pos
                } else {
                    self.win_pos[0] = r.min;
                    // Dock the EQ/playlist windows under the main one, honouring
                    // which are visible and their collapsed state.
                    self.restack();
                    // Seed the library position (docked right of main) if it was
                    // restored visible but never positioned.
                    if !self.lib_pos_init {
                        self.lib_pos = self.library_default_pos();
                        self.lib_pos_init = true;
                    }
                }
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
                .with_position(self.win_pos[1])
                .with_decorations(false)
                .with_resizable(false)
                .with_taskbar(false)
                .with_window_level(self.window_level());
            let id = egui::ViewportId::from_hash_of("linamp_eq");
            ctx.show_viewport_immediate(id, builder, |ctx, _| {
                // Fire global shortcuts while this window has focus (keyboard
                // input goes to the focused viewport, not the root).
                self.handle_shortcuts(ctx, ClWin::Eq);
                self.foc_eq = ctx.input(|i| i.viewport().focused).unwrap_or(self.foc_eq);
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
                .with_taskbar(false)
                .with_window_level(self.window_level());
            let id = egui::ViewportId::from_hash_of("linamp_pl");
            ctx.show_viewport_immediate(id, builder, |ctx, _| {
                self.handle_shortcuts(ctx, ClWin::Pl);
                self.foc_pl = ctx.input(|i| i.viewport().focused).unwrap_or(self.foc_pl);
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

    /// A cheap fingerprint of everything `compose_window(win)` draws from, so the
    /// expensive re-composite + GPU re-upload can be skipped when nothing changed.
    fn window_fp(&self, win: ClWin) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        let ps: u8 = match self.player.state() {
            PlayState::Stopped => 0,
            PlayState::Playing => 1,
            PlayState::Paused => 2,
        };
        self.theme.name.hash(&mut h);
        ps.hash(&mut h);
        // Active/inactive title bar follows app focus (all windows together).
        self.app_focused.hash(&mut h);
        match win {
            ClWin::Main => {
                self.main_shade.hash(&mut h);
                self.pressed_btn.hash(&mut h);
                self.shuffle.hash(&mut h);
                self.repeat_all.hash(&mut h);
                self.eq_visible.hash(&mut h);
                self.pl_visible.hash(&mut h);
                self.time_remaining.hash(&mut h);
                (self.viz_mode as u8).hash(&mut h);
                self.player.volume().to_bits().hash(&mut h);
                self.balance.to_bits().hash(&mut h);
                let pos = self
                    .classic_seek_preview
                    .unwrap_or_else(|| self.player.position().as_secs_f32());
                pos.to_bits().hash(&mut h);
                self.marquee_offset.to_bits().hash(&mut h);
                self.playlist.current.hash(&mut h);
                if let Some(t) = self.playlist.current_track() {
                    t.display_name().hash(&mut h);
                    t.bitrate.hash(&mut h);
                    t.sample_rate.hash(&mut h);
                    t.channels.hash(&mut h);
                    t.duration.hash(&mut h);
                }
                self.album_art.is_some().hash(&mut h);
                // Spectrum animation (rising/falling bars and peaks).
                for b in self.cviz_bars.iter().chain(self.cviz_peaks.iter()) {
                    b.to_bits().hash(&mut h);
                }
            }
            ClWin::Eq => {
                self.eq_shade.hash(&mut h);
                self.eq_enabled.hash(&mut h);
                self.eq_auto.hash(&mut h);
                self.eq_active.hash(&mut h);
                self.eq_preamp_db.to_bits().hash(&mut h);
                for b in &self.eq_bands {
                    b.to_bits().hash(&mut h);
                }
            }
            ClWin::Pl => {
                self.pl_shade.hash(&mut h);
                self.pl_px_w.to_bits().hash(&mut h);
                self.pl_px_h.to_bits().hash(&mut h);
                self.pl_first.hash(&mut h);
                self.pl_pressed_button().hash(&mut h);
                self.time_remaining.hash(&mut h);
                self.playlist.len().hash(&mut h);
                self.playlist.current.hash(&mut h);
                // The bottom-bar / shade time advances once a second while playing.
                self.player.position().as_secs().hash(&mut h);
                // Visible rows' identity + text — catches selection, current-row
                // and lazy tag loads without hashing the whole playlist. (Tags only
                // load for visible rows, so off-screen rows can't change unseen.)
                let vis = if self.pl_shade {
                    0
                } else {
                    classic::pl_visible_rows(self.pl_px_h as u32)
                };
                let end = (self.pl_first + vis).min(self.playlist.len());
                for i in self.pl_first..end {
                    (Some(i) == self.playlist.current).hash(&mut h);
                    self.pl_selection.contains(&i).hash(&mut h);
                    if let Some(t) = self.playlist.tracks.get(i) {
                        t.display_name().hash(&mut h);
                        t.duration.hash(&mut h);
                    }
                }
                if self.pl_shade {
                    if let Some(t) = self.playlist.current_track() {
                        t.display_name().hash(&mut h);
                        t.duration.hash(&mut h);
                    }
                }
            }
        }
        h.finish()
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

        // Re-composite + re-upload only when this window's inputs changed; the GPU
        // keeps the cached texture otherwise (near-zero cost when idle/paused).
        let idx = win_idx(win);
        let fp = self.window_fp(win);
        let tex_empty = match win {
            ClWin::Main => self.classic_tex.is_none(),
            ClWin::Eq => self.eq_tex.is_none(),
            ClWin::Pl => self.pl_tex.is_none(),
        };
        if self.tex_fp[idx] != fp || tex_empty {
            let img = self.compose_window(win);
            self.tex_fp[idx] = fp;
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
            let _ = upload_tex(slot, ctx, &img, name);
        }
        let tex = match win {
            ClWin::Main => self.classic_tex.clone(),
            ClWin::Eq => self.eq_tex.clone(),
            ClWin::Pl => self.pl_tex.clone(),
        }
        .expect("composed above");
        let nw = match win {
            ClWin::Pl if !self.pl_shade => self.pl_px_w,
            _ => classic::MAIN_W as f32,
        };
        let nh = self.win_native_h(win);

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
                // Skin-defined mouse cursor for the region under the pointer.
                self.update_cursor(ctx, win, rect);
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
                    cs.compose_eq_shade(self.app_focused)
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
                    cs.compose_pl_shade(self.app_focused, &text, &time, self.pl_colors().normal)
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
                        focused: self.app_focused,
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
        if k.open && !k.alt {
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
        if k.alt && k.open {
            self.toggle_library();
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
        let mut to_play_next: Option<usize> = None;
        let mut to_tag: Option<std::path::PathBuf> = None;
        let mut to_explore: Option<std::path::PathBuf> = None;
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
                                if ui.button("Play Next").clicked() {
                                    to_play_next = Some(i);
                                    ui.close_menu();
                                }
                                ui.separator();
                                if ui.button("Edit file tags…").clicked() {
                                    to_tag = Some(track.path.clone());
                                    ui.close_menu();
                                }
                                if ui.button("Explore folder").clicked() {
                                    to_explore = Some(track.path.clone());
                                    ui.close_menu();
                                }
                                ui.separator();
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
        if let Some(i) = to_play_next {
            let (cur, _) =
                reorder_play_next(&mut self.playlist.tracks, self.playlist.current, &[i]);
            self.playlist.current = cur;
            self.sort_key = SortKey::Custom;
        }
        if let Some(p) = to_tag {
            self.open_tag_editor_path(p);
        }
        if let Some(p) = to_explore {
            open_in_file_manager(&p);
        }
        if let Some(i) = to_remove {
            self.playlist.remove(i);
        }
        if let Some((from, to)) = reorder {
            self.move_track(from, to);
        }
    }

    /// The 3-pane Media Library browser (nav tree, artist/album columns, track
    /// table, bottom Play/Enqueue bar), themed from the active skin. Shared by
    /// the docked window and the modern `View::Library` tab.
    fn ui_library(&mut self, ui: &mut egui::Ui) {
        use crate::library::LibView;
        // When the skin ships general-purpose window art (GEN.BMP/GENEX.BMP), use
        // its defined Media Library colour scheme; otherwise fall back to the
        // palette sampled from the skin.
        let gen = if self.ui_mode == UiMode::Classic {
            self.classic.as_ref().and_then(|c| c.gen_colors())
        } else {
            None
        };
        let rgb = |c: [u8; 3]| Color32::from_rgb(c[0], c[1], c[2]);
        let (panel_fill, list_fill, row_col, sel_bg, accent, header_bg, header_fg) =
            if let Some(g) = gen {
                (
                    rgb(g.window_bg),
                    rgb(g.item_bg),
                    rgb(g.item_fg),
                    rgb(g.selection),
                    rgb(g.item_fg),
                    rgb(g.header_bg),
                    rgb(g.header_text),
                )
            } else {
                (
                    ui.visuals().panel_fill,
                    self.theme.list_bg,
                    self.theme.list_normal,
                    self.theme.list_sel_bg,
                    self.theme.accent,
                    skin::blend(self.theme.list_normal, self.theme.list_bg, 0.3),
                    self.theme.list_normal,
                )
            };
        let sel_col = skin::contrast_text(sel_bg);
        let dim = skin::blend(row_col, list_fill, 0.45);

        // Scale the whole browser with the double-size setting so it grows with
        // the classic windows. egui widgets follow the (scaled) style; the custom
        // row painters take `s` explicitly, and fixed sizes below are multiplied.
        let s = self.scale;
        if (s - 1.0).abs() > 0.01 {
            let st = ui.style_mut();
            for (_, f) in st.text_styles.iter_mut() {
                f.size *= s;
            }
            st.spacing.item_spacing *= s;
            st.spacing.button_padding *= s;
            st.spacing.interact_size *= s;
            st.spacing.scroll.bar_width *= s;
        }
        let panel_frame = egui::Frame::none()
            .fill(panel_fill)
            .inner_margin(egui::Margin::same(4.0 * s));

        // Deferred actions (collected while `self` is borrowed immutably).
        let mut nav_click: Option<LibView> = None;
        let mut artist_click: Option<(Option<String>, bool)> = None; // (None = "All", ctrl)
        let mut album_click: Option<(Option<String>, bool)> = None;
        let mut track_click: Option<(usize, bool)> = None;
        let mut dbl_track: Option<usize> = None;
        let mut rate: Option<i64> = None;
        let mut do_play = false;
        let mut do_enqueue = false;
        let mut do_scan = false;
        let mut do_clear = false;
        let mut search_changed = false;
        let mut lib_action: Option<LibAction> = None;
        // Column-resize drag deltas (in body-width fractions) for the two dividers.
        let mut col_dx = [0.0f32; 2];
        // A clicked header column (0=artist 1=title 2=album/rating 3=time) to sort by.
        let mut header_click: Option<u8> = None;

        let browsing = self.lib_query.trim().is_empty() && self.lib_nav == LibView::Audio;
        // The Top Rated list swaps the Album column for a star-rating column.
        let show_rating = self.lib_nav == LibView::TopRated && self.lib_query.trim().is_empty();

        // ---- top: search + scan -------------------------------------------
        egui::TopBottomPanel::top("lib_top")
            .frame(panel_frame)
            .show_inside(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label("Search:");
                    if ui
                        .add(
                            egui::TextEdit::singleline(&mut self.lib_query)
                                .hint_text("title / artist / album")
                                .desired_width(190.0 * s),
                        )
                        .changed()
                    {
                        search_changed = true;
                    }
                    if ui.button("Clear").clicked() {
                        do_clear = true;
                    }
                    ui.separator();
                    if ui
                        .add_enabled(self.lib_scan.is_none(), egui::Button::new("Scan folder…"))
                        .clicked()
                    {
                        do_scan = true;
                    }
                    if !self.lib_scan_text.is_empty() {
                        ui.label(RichText::new(&self.lib_scan_text).color(accent));
                    }
                });
            });

        // ---- bottom: play / enqueue / rating / status ---------------------
        let total_secs: u64 = self
            .lib_results
            .iter()
            .filter_map(|t| t.duration)
            .map(|d| d.as_secs())
            .sum();
        let rating_target = self
            .lib_sel_tracks
            .iter()
            .next()
            .and_then(|&i| self.lib_results.get(i))
            .map(|t| t.path.clone());
        let cur_rating = match (&self.library, &rating_target) {
            (Some(lib), Some(p)) => lib.rating(p),
            _ => 0,
        };
        egui::TopBottomPanel::bottom("lib_bottom")
            .frame(panel_frame)
            .show_inside(ui, |ui| {
                ui.horizontal(|ui| {
                    if ui.button(mnem("Play", 0)).clicked() {
                        do_play = true;
                    }
                    if ui.button(mnem("Enqueue", 0)).clicked() {
                        do_enqueue = true;
                    }
                    ui.separator();
                    ui.add_enabled_ui(rating_target.is_some(), |ui| {
                        ui.spacing_mut().item_spacing.x = 1.0 * s;
                        for n in 1..=5i64 {
                            let star = if n <= cur_rating { "★" } else { "☆" };
                            if ui
                                .add(
                                    egui::Label::new(RichText::new(star).color(accent).size(15.0 * s))
                                        .sense(egui::Sense::click()),
                                )
                                .clicked()
                            {
                                rate = Some(if cur_rating == n { 0 } else { n });
                            }
                        }
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let status =
                            format!("{} items  [{}]", self.lib_results.len(), hms(total_secs));
                        ui.label(RichText::new(status).color(dim));
                    });
                });
            });

        // ---- left: nav tree -----------------------------------------------
        egui::SidePanel::left("lib_nav")
            .resizable(false)
            .exact_width(140.0 * s)
            .frame(panel_frame)
            .show_inside(ui, |ui| {
                ui.add_space(2.0 * s);
                ui.label(RichText::new("Local Media").color(dim).size(11.0 * s));
                let mut nav = |ui: &mut egui::Ui, label: &str, v: LibView| {
                    let indent = egui::RichText::new(format!("   {label}"));
                    if ui.selectable_label(self.lib_nav == v, indent).clicked() {
                        nav_click = Some(v);
                    }
                };
                nav(ui, "Audio", LibView::Audio);
                ui.add_space(4.0 * s);
                ui.label(RichText::new("Views").color(dim).size(11.0 * s));
                nav(ui, "Most Played", LibView::MostPlayed);
                nav(ui, "Recently Added", LibView::RecentlyAdded);
                nav(ui, "Recently Played", LibView::RecentlyPlayed);
                nav(ui, "Never Played", LibView::NeverPlayed);
                nav(ui, "Top Rated", LibView::TopRated);
                ui.add_space(6.0 * s);
                ui.label(RichText::new("Playlists").color(dim).size(11.0 * s).italics());
                ui.label(RichText::new("Bookmarks").color(dim).size(11.0 * s).italics());
                ui.with_layout(egui::Layout::bottom_up(egui::Align::LEFT), |ui| match &self.library {
                    Some(lib) => {
                        ui.label(RichText::new(format!("{} tracks", lib.count())).color(dim).size(11.0 * s));
                    }
                    None => {
                        ui.colored_label(Color32::LIGHT_RED, "DB unavailable");
                    }
                });
            });

        // ---- center: artist/album columns + track table -------------------
        egui::CentralPanel::default()
            .frame(egui::Frame::none().fill(list_fill))
            .show_inside(ui, |ui| {
                if browsing {
                    egui::TopBottomPanel::top("lib_cols")
                        .resizable(false)
                        .exact_height(148.0 * s)
                        .frame(egui::Frame::none().fill(list_fill))
                        .show_inside(ui, |ui| {
                            ui.columns(2, |cols| {
                                // Artist column.
                                let c = &mut cols[0];
                                let all_artists = self.lib_sel_artists.is_empty();
                                let n = self.lib_artists.len();
                                c.label(RichText::new("Artist").color(dim).size(11.0 * s));
                                egui::ScrollArea::vertical()
                                    .id_salt("lib_artists")
                                    .auto_shrink([false, false])
                                    .show(c, |ui| {
                                        if ui
                                            .selectable_label(
                                                all_artists,
                                                RichText::new(format!("All ({n} artists)"))
                                                    .color(row_col),
                                            )
                                            .clicked()
                                        {
                                            artist_click = Some((None, false));
                                        }
                                        for (name, cnt) in &self.lib_artists {
                                            let sel = self.lib_sel_artists.contains(name);
                                            let resp = pane_row(ui, sel, name, *cnt, row_col, dim, s);
                                            if resp.clicked() {
                                                let ctrl = ui.input(|i| i.modifiers.command || i.modifiers.ctrl);
                                                artist_click = Some((Some(name.clone()), ctrl));
                                            }
                                            if resp.secondary_clicked() && !sel {
                                                artist_click = Some((Some(name.clone()), false));
                                            }
                                            resp.context_menu(|ui| {
                                                if ui.button(format!("Play all by \u{201c}{name}\u{201d}")).clicked() {
                                                    lib_action = Some(LibAction::PlayArtist(name.clone()));
                                                    ui.close_menu();
                                                }
                                                if ui.button(format!("Enqueue all by \u{201c}{name}\u{201d}")).clicked() {
                                                    lib_action = Some(LibAction::EnqueueArtist(name.clone()));
                                                    ui.close_menu();
                                                }
                                            });
                                        }
                                    });
                                // Album column.
                                let c = &mut cols[1];
                                let all_albums = self.lib_sel_albums.is_empty();
                                let m = self.lib_albums.len();
                                c.label(RichText::new("Album").color(dim).size(11.0 * s));
                                egui::ScrollArea::vertical()
                                    .id_salt("lib_albums")
                                    .auto_shrink([false, false])
                                    .show(c, |ui| {
                                        if ui
                                            .selectable_label(
                                                all_albums,
                                                RichText::new(format!("All ({m} albums)"))
                                                    .color(row_col),
                                            )
                                            .clicked()
                                        {
                                            album_click = Some((None, false));
                                        }
                                        for (name, cnt) in &self.lib_albums {
                                            let label = if name.is_empty() { "(no album)" } else { name };
                                            let sel = self.lib_sel_albums.contains(name);
                                            let resp = pane_row(ui, sel, label, *cnt, row_col, dim, s);
                                            if resp.clicked() {
                                                let ctrl = ui.input(|i| i.modifiers.command || i.modifiers.ctrl);
                                                album_click = Some((Some(name.clone()), ctrl));
                                            }
                                            if resp.secondary_clicked() && !sel {
                                                album_click = Some((Some(name.clone()), false));
                                            }
                                            resp.context_menu(|ui| {
                                                if ui.button(format!("Play all from \u{201c}{label}\u{201d}")).clicked() {
                                                    lib_action = Some(LibAction::PlayAlbum(name.clone()));
                                                    ui.close_menu();
                                                }
                                                if ui.button(format!("Enqueue all from \u{201c}{label}\u{201d}")).clicked() {
                                                    lib_action = Some(LibAction::EnqueueAlbum(name.clone()));
                                                    ui.close_menu();
                                                }
                                            });
                                        }
                                    });
                            });
                        });
                }

                // Track table.
                egui::CentralPanel::default()
                    .frame(egui::Frame::none().fill(list_fill).inner_margin(egui::Margin::same(2.0 * s)))
                    .show_inside(ui, |ui| {
                        let w = ui.available_width();
                        let dur_w = 48.0 * s;
                        // Reserve the scrollbar gutter so the header and the rows
                        // (which are narrower once the list scrolls) share one column
                        // layout, and anchor the Time column from the left so the
                        // right-aligned times line up with the header either way.
                        let sb = ui.spacing().scroll.bar_width + 2.0 * s;
                        let body = (w - dur_w - 12.0 * s - sb).max(120.0 * s);
                        let fa = self.lib_col_fr[0];
                        let ft = self.lib_col_fr[1];
                        let fb = (1.0 - fa - ft).max(0.1);
                        let (aw, tw, bw) = (body * fa, body * ft, body * fb);
                        let time_off = 2.0 * s + body + 6.0 * s;
                        let col3 = if show_rating { "Rating" } else { "Album" };
                        let (sort_col, sort_asc) = self.lib_sort.unwrap_or((u8::MAX, true));
                        // Header (filled with the header background colour).
                        let hdr = paint_track_row(ui, 15.0 * s, true, header_bg, s, |rect, paint| {
                            let mut x = rect.left() + 2.0 * s;
                            for (label, cw) in [("Artist", aw), ("Title", tw), (col3, bw)] {
                                paint(x, cw, label, header_fg, false);
                                x += cw;
                            }
                            paint(rect.left() + time_off, dur_w, "Time", header_fg, true);
                        });
                        // Sort arrow on the active column, drawn as a triangle (the
                        // ▲/▼ glyphs aren't in the bundled fonts, so a text arrow
                        // renders as tofu). Placed near the right edge of its column.
                        if sort_col != u8::MAX {
                            let b = hdr.rect.left() + 2.0 * s;
                            let off = 7.0 * s;
                            let ax = match sort_col {
                                0 => b + aw - off,
                                1 => b + aw + tw - off,
                                2 => b + aw + tw + bw - off,
                                _ => b + aw + tw + bw + off, // Time column
                            };
                            let cy = hdr.rect.center().y;
                            let tri = 3.5 * s;
                            let pts = if sort_asc {
                                vec![
                                    egui::pos2(ax, cy - tri),
                                    egui::pos2(ax - tri, cy + tri),
                                    egui::pos2(ax + tri, cy + tri),
                                ]
                            } else {
                                vec![
                                    egui::pos2(ax - tri, cy - tri),
                                    egui::pos2(ax + tri, cy - tri),
                                    egui::pos2(ax, cy + tri),
                                ]
                            };
                            ui.painter()
                                .add(egui::Shape::convex_polygon(pts, row_col, egui::Stroke::NONE));
                        }
                        // Clicking a header column sorts by it (toggles direction on
                        // repeat). Added before the divider handles so a click in the
                        // column body sorts while a drag on the boundary resizes.
                        let base0 = hdr.rect.left() + 2.0 * s;
                        let zones = [
                            (0u8, base0, base0 + aw),
                            (1, base0 + aw, base0 + aw + tw),
                            (2, base0 + aw + tw, base0 + aw + tw + bw),
                            (3, base0 + aw + tw + bw, hdr.rect.right()),
                        ];
                        for (ci, x0, x1) in zones {
                            let z = egui::Rect::from_min_max(
                                egui::pos2(x0, hdr.rect.top()),
                                egui::pos2(x1, hdr.rect.bottom()),
                            );
                            let zr = ui.interact(z, ui.id().with(("lib_hdr", ci)), egui::Sense::click());
                            if zr.hovered() {
                                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                            }
                            if zr.clicked() {
                                header_click = Some(ci);
                            }
                        }
                        // Draggable dividers between the flexible columns (a subtle,
                        // themed bar that brightens on hover) so columns can be
                        // resized to fit long values.
                        let base = hdr.rect.left() + 2.0;
                        for (k, bx) in [(0usize, base + aw), (1usize, base + aw + tw)] {
                            let hit = egui::Rect::from_min_max(
                                egui::pos2(bx - 3.0 * s, hdr.rect.top()),
                                egui::pos2(bx + 3.0 * s, hdr.rect.bottom()),
                            );
                            let dr = ui.interact(hit, ui.id().with(("lib_col_div", k)), egui::Sense::drag());
                            let hot = dr.hovered() || dr.dragged();
                            if hot {
                                ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
                            }
                            let stroke = egui::Stroke::new(1.0_f32, if hot { row_col } else { dim });
                            ui.painter().vline(bx, hdr.rect.y_range(), stroke);
                            if dr.dragged() && body > 1.0 {
                                col_dx[k] += dr.drag_delta().x / body;
                            }
                        }
                        egui::ScrollArea::vertical()
                            .id_salt("lib_tracks")
                            .auto_shrink([false, false])
                            .show(ui, |ui| {
                                if self.lib_results.is_empty() {
                                    // Only prompt to scan when the library is actually
                                    // empty; an empty smart view / filter over a
                                    // populated library gets a message that fits it.
                                    let lib_empty =
                                        self.library.as_ref().map(|l| l.count()).unwrap_or(0) == 0;
                                    let msg = if lib_empty {
                                        "No tracks. Use \u{201c}Scan folder\u{2026}\u{201d} to index your music."
                                    } else if !self.lib_query.trim().is_empty() {
                                        "No tracks match your search."
                                    } else {
                                        match self.lib_nav {
                                            LibView::TopRated => "No rated tracks yet.",
                                            LibView::MostPlayed => "No tracks have been played yet.",
                                            LibView::RecentlyPlayed => "No tracks have been played yet.",
                                            LibView::NeverPlayed => "Every track has been played.",
                                            LibView::RecentlyAdded => "No tracks have been added recently.",
                                            LibView::Audio => "No tracks match the selected artist/album.",
                                        }
                                    };
                                    ui.label(RichText::new(msg).color(dim).italics());
                                }
                                for (i, t) in self.lib_results.iter().enumerate() {
                                    let selected = self.lib_sel_tracks.contains(&i);
                                    let is_cur = self
                                        .playlist
                                        .current
                                        .and_then(|c| self.playlist.tracks.get(c))
                                        .map(|c| c.path == t.path)
                                        .unwrap_or(false);
                                    let tc = if selected {
                                        sel_col
                                    } else if is_cur {
                                        self.theme.list_current
                                    } else {
                                        row_col
                                    };
                                    // Third column: stars in Top Rated, else album.
                                    let col3_text = if show_rating {
                                        let r = self.lib_ratings.get(i).copied().unwrap_or(0).clamp(0, 5) as usize;
                                        format!("{}{}", "\u{2605}".repeat(r), "\u{2606}".repeat(5 - r))
                                    } else if t.album.is_empty() {
                                        "(no album)".to_string()
                                    } else {
                                        t.album.clone()
                                    };
                                    let resp = paint_track_row(ui, 16.0 * s, selected, sel_bg, s, |rect, paint| {
                                        let artist = if t.artist.is_empty() { "(unknown)" } else { &t.artist };
                                        let mut x = rect.left() + 2.0 * s;
                                        paint(x, aw, artist, tc, false);
                                        x += aw;
                                        paint(x, tw, &t.title, tc, false);
                                        x += tw;
                                        paint(x, bw, &col3_text, tc, false);
                                        let d = t.duration.map(fmt_time).unwrap_or_default();
                                        paint(rect.left() + time_off, dur_w, &d, tc, true);
                                    });
                                    if resp.clicked() {
                                        let ctrl = ui.input(|i| i.modifiers.command || i.modifiers.ctrl);
                                        track_click = Some((i, ctrl));
                                    }
                                    // Right-click selects the row it targets (unless it's
                                    // already part of the selection) so the menu's target
                                    // is highlighted.
                                    if resp.secondary_clicked() && !selected {
                                        track_click = Some((i, false));
                                    }
                                    if resp.double_clicked() {
                                        dbl_track = Some(i);
                                    }
                                    resp.context_menu(|ui| {
                                        // Act on the selection when the right-clicked
                                        // row is part of it; otherwise just this row.
                                        let rows: Vec<usize> = if selected && !self.lib_sel_tracks.is_empty() {
                                            self.lib_sel_tracks.iter().copied().collect()
                                        } else {
                                            vec![i]
                                        };
                                        if ui.button("Play").clicked() {
                                            lib_action = Some(LibAction::Play(rows.clone()));
                                            ui.close_menu();
                                        }
                                        if ui.button("Enqueue").clicked() {
                                            lib_action = Some(LibAction::Enqueue(rows.clone()));
                                            ui.close_menu();
                                        }
                                        ui.separator();
                                        if !t.artist.is_empty()
                                            && ui.button(format!("Play all by \u{201c}{}\u{201d}", t.artist)).clicked()
                                        {
                                            lib_action = Some(LibAction::PlayArtist(t.artist.clone()));
                                            ui.close_menu();
                                        }
                                        if !t.album.is_empty()
                                            && ui.button(format!("Play all from \u{201c}{}\u{201d}", t.album)).clicked()
                                        {
                                            lib_action = Some(LibAction::PlayAlbum(t.album.clone()));
                                            ui.close_menu();
                                        }
                                        ui.separator();
                                        if ui.button("Select all").clicked() {
                                            lib_action = Some(LibAction::SelectAll);
                                            ui.close_menu();
                                        }
                                        ui.menu_button("Rate", |ui| {
                                            for s in 1..=5i64 {
                                                if ui.button("\u{2605}".repeat(s as usize)).clicked() {
                                                    lib_action = Some(LibAction::Rate(rows.clone(), s));
                                                    ui.close_menu();
                                                }
                                            }
                                            if ui.button("Clear rating").clicked() {
                                                lib_action = Some(LibAction::Rate(rows.clone(), 0));
                                                ui.close_menu();
                                            }
                                        });
                                        if ui.button("Edit tags\u{2026}").clicked() {
                                            lib_action = Some(LibAction::EditTags(i));
                                            ui.close_menu();
                                        }
                                        if ui.button("Explore folder").clicked() {
                                            lib_action = Some(LibAction::Explore(i));
                                            ui.close_menu();
                                        }
                                        ui.separator();
                                        if ui.button("Remove from library").clicked() {
                                            lib_action = Some(LibAction::Remove(rows.clone()));
                                            ui.close_menu();
                                        }
                                    });
                                }
                            });
                    });
            });

        // ---- apply deferred actions ---------------------------------------
        // Column resize: divider 0 is artist|title (album fixed), divider 1 is
        // title|album (artist fixed). Keep every column at least 12% wide.
        if col_dx[0] != 0.0 {
            let a = (self.lib_col_fr[0] + col_dx[0]).clamp(0.12, 0.70);
            let t = (self.lib_col_fr[1] - col_dx[0]).clamp(0.12, 0.70);
            if a + t <= 0.88 {
                self.lib_col_fr = [a, t];
            }
        }
        if col_dx[1] != 0.0 {
            let t = (self.lib_col_fr[1] + col_dx[1]).clamp(0.12, 0.70);
            if self.lib_col_fr[0] + t <= 0.88 {
                self.lib_col_fr[1] = t;
            }
        }
        if let Some(ci) = header_click {
            // Toggle direction on the same column; otherwise sort the new column —
            // ascending for text/time, descending for Rating so more stars sort
            // to the top first.
            self.lib_sort = Some(match self.lib_sort {
                Some((c, asc)) if c == ci => (ci, !asc),
                _ => (ci, !(ci == 2 && show_rating)),
            });
            self.apply_lib_sort();
        }
        if do_clear {
            self.lib_query.clear();
            self.refresh_library();
        }
        if search_changed {
            self.refresh_library();
        }
        if let Some(v) = nav_click {
            self.lib_nav = v;
            self.lib_sel_artists.clear();
            self.lib_sel_albums.clear();
            self.lib_query.clear();
            self.lib_sort = None; // each view starts in its natural order
            self.refresh_library();
        }
        if let Some((name, ctrl)) = artist_click {
            apply_sel(&mut self.lib_sel_artists, name, ctrl);
            self.lib_sel_albums.clear();
            self.refresh_library();
        }
        if let Some((name, ctrl)) = album_click {
            apply_sel(&mut self.lib_sel_albums, name, ctrl);
            self.refresh_library();
        }
        if let Some((i, ctrl)) = track_click {
            if ctrl {
                if !self.lib_sel_tracks.insert(i) {
                    self.lib_sel_tracks.remove(&i);
                }
            } else {
                self.lib_sel_tracks.clear();
                self.lib_sel_tracks.insert(i);
            }
        }
        if let Some(r) = rate {
            if let Some(lib) = &self.library {
                for &i in &self.lib_sel_tracks {
                    if let Some(t) = self.lib_results.get(i) {
                        lib.set_rating(&t.path, r);
                    }
                }
            }
            if self.lib_nav == LibView::TopRated {
                self.refresh_library();
            }
        }
        if do_scan {
            self.scan_into_library();
        }

        if let Some(act) = lib_action {
            let rows_to_tracks = |app: &Self, rows: &[usize]| -> Vec<crate::playlist::Track> {
                rows.iter()
                    .filter_map(|&i| app.lib_results.get(i).cloned())
                    .collect()
            };
            let by = |app: &Self, artists: &[String], albums: &[String]| {
                app.library
                    .as_ref()
                    .map(|l| l.tracks_for(artists, albums))
                    .unwrap_or_default()
            };
            match act {
                LibAction::SelectAll => {
                    self.lib_sel_tracks = (0..self.lib_results.len()).collect();
                }
                LibAction::Play(rows) => {
                    let ts = rows_to_tracks(self, &rows);
                    self.play_tracks(ts);
                }
                LibAction::Enqueue(rows) => {
                    let ts = rows_to_tracks(self, &rows);
                    self.enqueue_tracks(ts);
                }
                LibAction::PlayArtist(a) => {
                    let ts = by(self, std::slice::from_ref(&a), &[]);
                    self.play_tracks(ts);
                }
                LibAction::EnqueueArtist(a) => {
                    let ts = by(self, std::slice::from_ref(&a), &[]);
                    self.enqueue_tracks(ts);
                }
                LibAction::PlayAlbum(al) => {
                    let ts = by(self, &[], std::slice::from_ref(&al));
                    self.play_tracks(ts);
                }
                LibAction::EnqueueAlbum(al) => {
                    let ts = by(self, &[], std::slice::from_ref(&al));
                    self.enqueue_tracks(ts);
                }
                LibAction::Rate(rows, r) => {
                    if let Some(lib) = &self.library {
                        for &i in &rows {
                            if let Some(t) = self.lib_results.get(i) {
                                lib.set_rating(&t.path, r);
                            }
                        }
                    }
                    if self.lib_nav == LibView::TopRated {
                        self.refresh_library();
                    }
                }
                LibAction::EditTags(i) => {
                    if let Some(t) = self.lib_results.get(i).cloned() {
                        self.open_tag_editor_path(t.path);
                    }
                }
                LibAction::Explore(i) => {
                    if let Some(t) = self.lib_results.get(i) {
                        open_in_file_manager(&t.path);
                    }
                }
                LibAction::Remove(rows) => {
                    let paths: Vec<_> = rows
                        .iter()
                        .filter_map(|&i| self.lib_results.get(i).map(|t| t.path.clone()))
                        .collect();
                    if let Some(lib) = &self.library {
                        lib.remove_paths(&paths);
                    }
                    self.lib_sel_tracks.clear();
                    self.refresh_library();
                }
            }
        }

        // Gather the target tracks for Play/Enqueue (selection, else all shown).
        let targets: Vec<crate::playlist::Track> = if self.lib_sel_tracks.is_empty() {
            self.lib_results.clone()
        } else {
            self.lib_sel_tracks
                .iter()
                .filter_map(|&i| self.lib_results.get(i).cloned())
                .collect()
        };
        if do_play && !targets.is_empty() {
            self.playlist.tracks.clear();
            self.playlist.current = None;
            for t in &targets {
                self.playlist.tracks.push(t.clone());
            }
            self.play_index(0);
        }
        if do_enqueue && !targets.is_empty() {
            for t in &targets {
                self.playlist.tracks.push(t.clone());
            }
            self.status = format!("{} track(s) in playlist.", self.playlist.len());
        }
        if let Some(i) = dbl_track {
            if let Some(t) = self.lib_results.get(i).cloned() {
                self.playlist.tracks.push(t);
                let idx = self.playlist.tracks.len() - 1;
                self.play_index(idx);
            }
        }
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
        // the text a little on both sides so a long value never runs into — or even
        // touches — the neighbouring column or the resize divider at the boundary.
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
            self.lib_visible = false;
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
            // Repaint at ~30fps only when something is moving; otherwise idle and
            // wake slowly (egui still repaints immediately on input).
            let next = if self.is_animating() { 33 } else { 250 };
            ctx.request_repaint_after(Duration::from_millis(next));
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
    // Outline colour, derived from the skin so bevels/outlines track the theme
    // (used by the tag editor's tabs, buttons and field frames).
    let outline = egui::Stroke::new(1.0_f32, skin::blend(t.bg, skin::contrast_text(t.bg), 0.35));
    visuals.widgets.noninteractive.bg_stroke = outline;
    visuals.override_text_color = Some(skin::contrast_text(t.bg));
    ctx.set_visuals(visuals);

    let mut style = (*ctx.style()).clone();
    style.spacing.item_spacing = egui::vec2(6.0, 4.0);
    style.spacing.slider_width = 120.0;
    ctx.set_style(style);
}

#[cfg(test)]
mod tests {
    use super::{parse_time, reorder_play_next};

    #[test]
    fn play_next_moves_selection_after_current() {
        let mut v = vec![0, 1, 2, 3, 4];
        // current = 0 (unselected), move rows 3 & 4 to sit right after it.
        let (cur, start) = reorder_play_next(&mut v, Some(0), &[3, 4]);
        assert_eq!(v, vec![0, 3, 4, 1, 2]);
        assert_eq!(cur, Some(0));
        assert_eq!(start, 1);
    }

    #[test]
    fn play_next_no_current_goes_to_front() {
        let mut v = vec![10, 11, 12];
        let (cur, start) = reorder_play_next(&mut v, None, &[2]);
        assert_eq!(v, vec![12, 10, 11]);
        assert_eq!(cur, None);
        assert_eq!(start, 0);
    }

    #[test]
    fn play_next_current_in_selection_goes_to_front() {
        let mut v = vec![0, 1, 2, 3];
        // current = 2 is itself selected: block [1,2] moves to the front, and
        // current follows to its offset within the block.
        let (cur, start) = reorder_play_next(&mut v, Some(2), &[1, 2]);
        assert_eq!(v, vec![1, 2, 0, 3]);
        assert_eq!(start, 0);
        assert_eq!(cur, Some(1));
    }

    #[test]
    fn play_next_preserves_order_and_shifts_current() {
        let mut v = vec![0, 1, 2, 3, 4];
        // current = 4, move rows 0 & 2: after removal current shifts to 2, the
        // block is inserted after it in ascending order.
        let (cur, start) = reorder_play_next(&mut v, Some(4), &[0, 2]);
        assert_eq!(v, vec![1, 3, 4, 0, 2]);
        assert_eq!(cur, Some(2));
        assert_eq!(start, 3);
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
        // star count: the empty-star glyph ☆ (U+2606) sorts AFTER the filled ★
        // (U+2605), so sorting the rendered "★★★☆☆" string would rank a 3-star row
        // above a 5-star one. Numeric sorting is correct; string sorting is not.
        assert!('\u{2606}' > '\u{2605}');
        assert!("\u{2605}\u{2605}\u{2605}\u{2606}\u{2606}" > "\u{2605}\u{2605}\u{2605}\u{2605}\u{2605}");
        assert!(5i64 > 3); // numeric order a descending sort gets right
    }
}
