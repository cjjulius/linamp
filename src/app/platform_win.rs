//! Platform windowing glue: Win32 (owner windows, regions, cursors) and
//! X11 (transient-for, shape, cursors) FFI. A no-op fallback elsewhere.

/// Global cursor in egui points if the platform exposes it: `GetCursorPos`
/// (Windows), `XQueryPointer` (X11/XWayland), `None` (native Wayland/else). When
/// `Some`, the drag runs "manual" — we move the window and its docked group each
/// frame; `None` falls back to the OS window-drag.
pub(crate) fn global_cursor_points(ctx: &egui::Context) -> Option<egui::Pos2> {
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
pub(crate) fn virtual_desktop_rect(ctx: &egui::Context) -> Option<egui::Rect> {
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

    /// Clip the window titled `title` to a rounded rectangle of its current size
    /// (corner radius `radius` px) via the Shape extension — the X11 counterpart
    /// of Windows' `SetWindowRgn`, so the popup's square corners don't show
    /// through the rounded fill. Returns true once applied; false while the window
    /// isn't mapped/sized yet, so the caller retries next frame.
    pub fn round_window(title: &str, radius: i32) -> bool {
        let Some(shape) = shape_fn() else {
            return false;
        };
        with_conn(|xlib, d| unsafe {
            let prev = (xlib.XSetErrorHandler)(Some(ignore_err));
            let win = window_by_title(xlib, d, title);
            let ok = if win != 0 {
                let (mut root, mut x, mut y, mut w, mut h, mut bw, mut depth) =
                    (0u64, 0i32, 0i32, 0u32, 0u32, 0u32, 0u32);
                let got = (xlib.XGetGeometry)(
                    d, win, &mut root, &mut x, &mut y, &mut w, &mut h, &mut bw, &mut depth,
                );
                if got != 0 && w > 0 && h > 0 {
                    let rects = rounded_rect_rects(w as i32, h as i32, radius);
                    const SHAPE_BOUNDING: c_int = 0;
                    const SHAPE_SET: c_int = 0;
                    const UNSORTED: c_int = 0;
                    let xr: Vec<XRectangle> = rects
                        .iter()
                        .map(|&(rx, ry, rw, rh)| XRectangle {
                            x: rx,
                            y: ry,
                            width: rw,
                            height: rh,
                        })
                        .collect();
                    shape(
                        d, win, SHAPE_BOUNDING, 0, 0, xr.as_ptr(), xr.len() as c_int, SHAPE_SET,
                        UNSORTED,
                    );
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

    /// Horizontal run rectangles filling a `w`×`h` rounded rectangle with corner
    /// radius `r`, one per scanline — the shape the Shape extension clips to.
    fn rounded_rect_rects(w: i32, h: i32, r: i32) -> Vec<(i16, i16, u16, u16)> {
        let r = r.clamp(0, w.min(h) / 2);
        if r == 0 {
            return vec![(0, 0, w as u16, h as u16)];
        }
        let mut rects = Vec::with_capacity(h as usize);
        for yy in 0..h {
            // Inset from each side: 0 in the straight middle rows, growing to `r`
            // at the very top/bottom following the corner arc.
            let inset = if yy < r {
                let dy = (r - yy) as f64;
                (r as f64 - ((r * r) as f64 - dy * dy).max(0.0).sqrt()).ceil() as i32
            } else if yy >= h - r {
                let dy = (yy - (h - 1 - r)) as f64;
                (r as f64 - ((r * r) as f64 - dy * dy).max(0.0).sqrt()).ceil() as i32
            } else {
                0
            };
            let ww = w - 2 * inset;
            if ww > 0 {
                rects.push((inset as i16, yy as i16, ww as u16, 1));
            }
        }
        rects
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

    /// Unmap (hide) the window titled `title` immediately. Used to drop the
    /// always-on-top context-menu popup before a modal file dialog opens, since
    /// the blocking dialog freezes the egui loop before it can close the viewport.
    /// egui tears the window down on the next frame once `self.popup` is cleared.
    pub fn unmap_window(title: &str) {
        with_conn(|xlib, d| unsafe {
            let prev = (xlib.XSetErrorHandler)(Some(ignore_err));
            let win = window_by_title(xlib, d, title);
            if win != 0 {
                (xlib.XUnmapWindow)(d, win);
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
pub(crate) fn group_child_windows(eq_visible: bool, pl_visible: bool, lib_visible: bool) {
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        x11util::group(eq_visible, pl_visible, lib_visible);
    }
    #[cfg(windows)]
    {
        let _ = (eq_visible, pl_visible, lib_visible);
        #[allow(non_snake_case)]
        extern "system" {
            fn SetWindowLongPtrW(hwnd: isize, index: i32, new: isize) -> isize;
            fn GetWindowLongPtrW(hwnd: isize, index: i32) -> isize;
        }
        const GWLP_HWNDPARENT: i32 = -8;
        let main = find_window("LinAmp");
        if main == 0 {
            return;
        }
        unsafe {
            for title in ["LinAmp Equalizer", "LinAmp Playlist", "LinAmp Library"] {
                let child = find_window(title);
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

/// Hide the borderless context-menu popup window immediately, so a modal file
/// dialog opened from a menu item (which blocks the egui loop before egui can
/// close the viewport) appears above it rather than under the always-on-top menu.
/// The window is torn down by egui on the next frame once `self.popup` is cleared.
pub(crate) fn hide_popup_window() {
    #[cfg(windows)]
    {
        #[allow(non_snake_case)]
        extern "system" {
            fn ShowWindow(hwnd: isize, cmd: i32) -> i32;
        }
        const SW_HIDE: i32 = 0;
        let h = find_window("LinAmp Menu");
        if h != 0 {
            unsafe {
                ShowWindow(h, SW_HIDE);
            }
        }
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        x11util::unmap_window("LinAmp Menu");
    }
}

/// A NUL-terminated UTF-16 string for the Win32 `*W` APIs.
#[cfg(windows)]
pub(crate) fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Find a top-level window by exact title, or 0 when it isn't present. The single
/// place that declares `FindWindowW`. Windows only.
#[cfg(windows)]
pub(crate) fn find_window(title: &str) -> isize {
    #[allow(non_snake_case)]
    extern "system" {
        fn FindWindowW(class: *const u16, title: *const u16) -> isize;
    }
    unsafe { FindWindowW(std::ptr::null(), wide(title).as_ptr()) }
}

/// Clip the borderless context-menu window to a rounded rectangle (corner radius
/// `radius` physical px) so its corners are cut away at the OS level — otherwise
/// the square opaque window shows its (dark) clear colour outside the frame's
/// rounded fill. Returns true once the region is applied, so the caller stops
/// retrying. Windows only; a no-op (and `true`) elsewhere.
pub(crate) fn round_popup_window(radius: i32) -> bool {
    #[cfg(windows)]
    {
        #[allow(non_snake_case)]
        extern "system" {
            fn GetClientRect(hwnd: isize, rect: *mut [i32; 4]) -> i32;
            fn CreateRoundRectRgn(x1: i32, y1: i32, x2: i32, y2: i32, w: i32, h: i32) -> isize;
            fn SetWindowRgn(hwnd: isize, rgn: isize, redraw: i32) -> i32;
            fn DeleteObject(obj: isize) -> i32;
        }
        unsafe {
            let h = find_window("LinAmp Menu");
            if h == 0 {
                return false;
            }
            let mut r = [0i32; 4]; // left, top, right, bottom (client is 0,0..w,h)
            if GetClientRect(h, &mut r) == 0 {
                return false;
            }
            let (w, ht) = (r[2] - r[0], r[3] - r[1]);
            if w <= 0 || ht <= 0 {
                return false;
            }
            let rgn = CreateRoundRectRgn(0, 0, w + 1, ht + 1, radius * 2, radius * 2);
            // On success the window takes ownership of `rgn`; on failure it does
            // not, so we must free it or leak a GDI region on every retry.
            if SetWindowRgn(h, rgn, 1) == 0 {
                DeleteObject(rgn);
                return false;
            }
            true
        }
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        x11util::round_window("LinAmp Menu", radius)
    }
    #[cfg(not(any(windows, all(unix, not(target_os = "macos")))))]
    {
        let _ = radius;
        true
    }
}

/// Apply (or clear) a window-shape mask to a classic window by title. X11 (Shape
/// extension) and Windows (`SetWindowRgn`); a no-op elsewhere.
pub(crate) fn set_window_shape(title: &str, rects: &[(i16, i16, u16, u16)]) {
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
pub(crate) fn win_set_shape(title: &str, rects: &[(i16, i16, u16, u16)]) {
    #[allow(non_snake_case)]
    extern "system" {
        fn CreateRectRgn(x1: i32, y1: i32, x2: i32, y2: i32) -> isize;
        fn CombineRgn(dst: isize, src1: isize, src2: isize, mode: i32) -> i32;
        fn SetWindowRgn(hwnd: isize, hrgn: isize, redraw: i32) -> i32;
        fn DeleteObject(o: isize) -> i32;
    }
    const RGN_OR: i32 = 2;
    let hwnd = find_window(title);
    unsafe {
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
pub(crate) fn set_window_cursor(title: &str, cache_key: &str, cur: &crate::cursor::CursorImage) {
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
pub(crate) fn unset_window_cursor(title: &str) {
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

// `wide` is a Win32-only helper, so its test only builds on Windows.
#[cfg(all(test, windows))]
mod tests {
    #[test]
    fn wide_is_nul_terminated_utf16() {
        let w = super::wide("Hi");
        assert_eq!(w, vec![0x48, 0x69, 0]);
        assert_eq!(*w.last().unwrap(), 0);
    }
}
