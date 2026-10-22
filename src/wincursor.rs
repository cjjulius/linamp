//! Windows: apply a classic skin's `.cur` cursors to our windows.
//!
//! egui 0.29 only exposes the fixed `CursorIcon` enum (no custom-image cursor),
//! and winit sets the arrow on every `WM_SETCURSOR`. So we create a native
//! `HCURSOR` from each decoded cursor and **subclass** the window to answer
//! `WM_SETCURSOR` ourselves — the subclass runs before winit and returns TRUE,
//! so winit never overrides it. The X11 side does the equivalent with
//! `XDefineCursor` (see `x11util::set_cursor`). All state is thread-local; every
//! entry point and the subclass proc run on the UI thread that pumps messages.

#![allow(non_snake_case)]

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::os::raw::c_void;

use crate::cursor::CursorImage;

type Hwnd = isize;
type Handle = isize;

const WM_SETCURSOR: u32 = 0x0020;
const HTCLIENT: isize = 1;
const BI_RGB: u32 = 0;
const DIB_RGB_COLORS: u32 = 0;
const SUBCLASS_ID: usize = 0xA10C; // arbitrary, unique to us

type SubclassProc =
    extern "system" fn(Hwnd, u32, usize, isize, usize, usize) -> isize;

#[link(name = "user32")]
extern "system" {
    fn FindWindowW(class: *const u16, title: *const u16) -> Hwnd;
    fn SetCursor(h: Handle) -> Handle;
    fn CreateIconIndirect(info: *const ICONINFO) -> Handle;
}

#[link(name = "gdi32")]
extern "system" {
    fn CreateDIBSection(
        hdc: Handle,
        pbmi: *const BITMAPINFOHEADER,
        usage: u32,
        bits: *mut *mut c_void,
        section: Handle,
        offset: u32,
    ) -> Handle;
    fn CreateBitmap(w: i32, h: i32, planes: u32, bitcount: u32, bits: *const c_void) -> Handle;
    fn DeleteObject(o: Handle) -> i32;
}

#[link(name = "comctl32")]
extern "system" {
    fn SetWindowSubclass(hwnd: Hwnd, proc: SubclassProc, id: usize, refdata: usize) -> i32;
    fn DefSubclassProc(hwnd: Hwnd, msg: u32, w: usize, l: isize) -> isize;
}

#[repr(C)]
struct BITMAPINFOHEADER {
    biSize: u32,
    biWidth: i32,
    biHeight: i32,
    biPlanes: u16,
    biBitCount: u16,
    biCompression: u32,
    biSizeImage: u32,
    biXPelsPerMeter: i32,
    biYPelsPerMeter: i32,
    biClrUsed: u32,
    biClrImportant: u32,
}

#[repr(C)]
struct ICONINFO {
    fIcon: i32,
    xHotspot: u32,
    yHotspot: u32,
    hbmMask: Handle,
    hbmColor: Handle,
}

thread_local! {
    /// HCURSOR by cache key (e.g. "skin:posbar").
    static CACHE: RefCell<HashMap<String, Handle>> = RefCell::new(HashMap::new());
    /// The cursor each window should currently show.
    static DESIRED: RefCell<HashMap<Hwnd, Handle>> = RefCell::new(HashMap::new());
    /// Windows we've already subclassed.
    static HOOKED: RefCell<HashSet<Hwnd>> = RefCell::new(HashSet::new());
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn find_window(title: &str) -> Hwnd {
    unsafe { FindWindowW(std::ptr::null(), wide(title).as_ptr()) }
}

/// Build (or fetch from cache) an `HCURSOR` for `ci`.
fn hcursor(cache_key: &str, ci: &CursorImage) -> Handle {
    if let Some(h) = CACHE.with(|c| c.borrow().get(cache_key).copied()) {
        return h;
    }
    let (w, h) = (ci.width as i32, ci.height as i32);
    if w <= 0 || h <= 0 || ci.argb.len() < (w * h) as usize {
        return 0;
    }
    let bi = BITMAPINFOHEADER {
        biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
        biWidth: w,
        biHeight: -h, // top-down, matching our row-major pixels
        biPlanes: 1,
        biBitCount: 32,
        biCompression: BI_RGB,
        biSizeImage: 0,
        biXPelsPerMeter: 0,
        biYPelsPerMeter: 0,
        biClrUsed: 0,
        biClrImportant: 0,
    };
    unsafe {
        let mut bits: *mut c_void = std::ptr::null_mut();
        let color = CreateDIBSection(0, &bi, DIB_RGB_COLORS, &mut bits, 0, 0);
        if color == 0 || bits.is_null() {
            if color != 0 {
                DeleteObject(color);
            }
            return 0;
        }
        // Premultiplied 0xAARRGGBB little-endian == BGRA bytes, exactly a 32bpp DIB.
        let dst = std::slice::from_raw_parts_mut(bits as *mut u32, (w * h) as usize);
        dst.copy_from_slice(&ci.argb[..(w * h) as usize]);

        // All-zero 1bpp AND mask (per-pixel alpha from the color bitmap is used).
        let stride = ((w as usize + 15) / 16) * 2; // bytes per row, word-aligned
        let mask_bits = vec![0u8; stride * h as usize];
        let mask = CreateBitmap(w, h, 1, 1, mask_bits.as_ptr() as *const c_void);

        let info = ICONINFO {
            fIcon: 0, // cursor, not icon
            xHotspot: ci.xhot as u32,
            yHotspot: ci.yhot as u32,
            hbmMask: mask,
            hbmColor: color,
        };
        let cur = CreateIconIndirect(&info);
        // CreateIconIndirect copies the bitmaps; free ours.
        DeleteObject(color);
        if mask != 0 {
            DeleteObject(mask);
        }
        if cur != 0 {
            CACHE.with(|c| c.borrow_mut().insert(cache_key.to_string(), cur));
        }
        cur
    }
}

fn ensure_hooked(hwnd: Hwnd) {
    let already = HOOKED.with(|h| h.borrow().contains(&hwnd));
    if already {
        return;
    }
    unsafe {
        if SetWindowSubclass(hwnd, subclass_proc, SUBCLASS_ID, 0) != 0 {
            HOOKED.with(|h| {
                h.borrow_mut().insert(hwnd);
            });
        }
    }
}

/// Show `ci` on the window titled `title` (cached under `cache_key`).
pub fn apply(title: &str, cache_key: &str, ci: &CursorImage) {
    let hwnd = find_window(title);
    if hwnd == 0 {
        return;
    }
    let cur = hcursor(cache_key, ci);
    if cur == 0 {
        return;
    }
    ensure_hooked(hwnd);
    DESIRED.with(|d| {
        d.borrow_mut().insert(hwnd, cur);
    });
}

/// Stop overriding the cursor on the window titled `title` (fall back to normal).
pub fn clear(title: &str) {
    let hwnd = find_window(title);
    if hwnd == 0 {
        return;
    }
    DESIRED.with(|d| {
        d.borrow_mut().remove(&hwnd);
    });
}

extern "system" fn subclass_proc(
    hwnd: Hwnd,
    msg: u32,
    w: usize,
    l: isize,
    _id: usize,
    _refdata: usize,
) -> isize {
    if msg == WM_SETCURSOR && (l & 0xFFFF) == HTCLIENT {
        let cur = DESIRED
            .try_with(|d| d.try_borrow().ok().and_then(|m| m.get(&hwnd).copied()))
            .ok()
            .flatten();
        if let Some(cur) = cur {
            unsafe {
                SetCursor(cur);
            }
            return 1; // TRUE: handled, stop further processing
        }
    }
    unsafe { DefSubclassProc(hwnd, msg, w, l) }
}
