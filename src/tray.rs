//! System-tray (notification-area) icon with a right-click transport menu and
//! left-click show/hide, matching classic Winamp's tray behaviour (#40).
//!
//! The native tray runs on its own thread and reports what the user clicked
//! back over a channel the app drains once per frame. The app resolves the
//! left-click `Toggle` against its own window state, so the platform code never
//! has to track whether the window is currently hidden.
//!
//! Windows uses `Shell_NotifyIcon` on a hidden top-level window. Linux will use a
//! StatusNotifierItem (the `ksni` crate) in a follow-up commit. Every other
//! target gets a no-op tray so the app still builds and runs without one.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc::Receiver;
use std::sync::Arc;

/// What the user asked for from the tray icon. Drained by the app each frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrayAction {
    PlayPause,
    Stop,
    Prev,
    Next,
    /// Left-click: show the window if it is hidden, hide it if it is shown.
    Toggle,
    /// Menu "Restore": always bring the window back and focus it.
    Restore,
    /// Menu "Exit LinAmp": quit the application.
    Quit,
}

/// Menu-command ids shared between the platform code and its tests. They double
/// as the Win32 `TrackPopupMenu` command ids. Used by the Windows backend and the
/// tests; a plain Linux build (ksni names its own actions) doesn't touch them.
#[allow(dead_code)]
mod cmd {
    pub const PLAY_PAUSE: u32 = 1;
    pub const STOP: u32 = 2;
    pub const PREV: u32 = 3;
    pub const NEXT: u32 = 4;
    pub const RESTORE: u32 = 5;
    pub const EXIT: u32 = 6;
}

/// Map a tray menu-command id to its action. Returns `None` for ids that are
/// not menu commands (separators never report, and 0 means "dismissed"). Used by
/// the Windows backend and the tests; unused on a plain Linux build.
#[allow(dead_code)]
fn action_for_command(id: u32) -> Option<TrayAction> {
    match id {
        cmd::PLAY_PAUSE => Some(TrayAction::PlayPause),
        cmd::STOP => Some(TrayAction::Stop),
        cmd::PREV => Some(TrayAction::Prev),
        cmd::NEXT => Some(TrayAction::Next),
        cmd::RESTORE => Some(TrayAction::Restore),
        cmd::EXIT => Some(TrayAction::Quit),
        _ => None,
    }
}

/// The tray menu's rows, in order. A `0` id marks a separator. The owner-draw
/// handlers index this by position (passed as each item's data), so the builder,
/// the measure pass and the draw pass all read the same labels. Windows-only
/// (ksni builds its own menu), so unused on a plain Linux build.
#[allow(dead_code)]
const MENU_ITEMS: &[(u32, &str)] = &[
    (cmd::PLAY_PAUSE, "Play/Pause"),
    (cmd::STOP, "Stop"),
    (0, ""),
    (cmd::PREV, "Previous"),
    (cmd::NEXT, "Next"),
    (0, ""),
    (cmd::RESTORE, "Restore LinAmp"),
    (cmd::EXIT, "Exit LinAmp"),
];

/// Skin-derived colours for the owner-drawn Windows tray menu, held as COLORREFs
/// so the tray thread can read them without egui. The app updates these from the
/// current skin (`Tray::set_menu_colors`), so the menu matches LinAmp's own menus:
/// the same background, text and border, with the skin's selection colour on hover.
pub struct MenuColors {
    bg: AtomicU32,
    text: AtomicU32,
    hover_bg: AtomicU32,
    hover_text: AtomicU32,
    border: AtomicU32,
}

/// Pack an egui colour into a Win32 COLORREF (0x00BBGGRR).
fn colorref(c: egui::Color32) -> u32 {
    (c.r() as u32) | ((c.g() as u32) << 8) | ((c.b() as u32) << 16)
}

impl MenuColors {
    fn new() -> Self {
        // Dark defaults until the app pushes the skin's palette.
        Self {
            bg: AtomicU32::new(colorref(egui::Color32::from_rgb(20, 22, 28))),
            text: AtomicU32::new(colorref(egui::Color32::from_rgb(210, 210, 210))),
            hover_bg: AtomicU32::new(colorref(egui::Color32::from_rgb(40, 70, 120))),
            hover_text: AtomicU32::new(colorref(egui::Color32::WHITE)),
            border: AtomicU32::new(colorref(egui::Color32::from_rgb(70, 74, 82))),
        }
    }

    fn set(
        &self,
        bg: egui::Color32,
        text: egui::Color32,
        hover_bg: egui::Color32,
        hover_text: egui::Color32,
        border: egui::Color32,
    ) {
        self.bg.store(colorref(bg), Ordering::Relaxed);
        self.text.store(colorref(text), Ordering::Relaxed);
        self.hover_bg.store(colorref(hover_bg), Ordering::Relaxed);
        self.hover_text.store(colorref(hover_text), Ordering::Relaxed);
        self.border.store(colorref(border), Ordering::Relaxed);
    }
}

/// A live tray icon. Dropping it removes the icon and stops the tray thread.
pub struct Tray {
    /// Shared skin colours for the owner-drawn Windows menu. Kept on every target
    /// so `set_menu_colors` needs no platform check; only the Windows menu reads it.
    colors: Arc<MenuColors>,
    #[cfg(windows)]
    _win: Option<windows_impl::WinTray>,
    #[cfg(target_os = "linux")]
    _linux: Option<linux_impl::LinuxTray>,
    #[cfg(not(any(windows, target_os = "linux")))]
    _noop: (),
}

impl Tray {
    /// Build the tray icon (when `enabled`) and return it together with the
    /// receiver the app drains. A disabled or unsupported tray still returns a
    /// valid receiver, it just never yields, so callers need no platform checks.
    pub fn spawn(ctx: &egui::Context, enabled: bool) -> (Tray, Receiver<TrayAction>) {
        let (tx, rx) = std::sync::mpsc::channel();
        let colors = Arc::new(MenuColors::new());

        #[cfg(windows)]
        {
            let win = if enabled {
                windows_impl::WinTray::spawn(ctx.clone(), tx, colors.clone()).ok()
            } else {
                None
            };
            (Tray { colors, _win: win }, rx)
        }

        #[cfg(target_os = "linux")]
        {
            let linux = if enabled {
                linux_impl::LinuxTray::spawn(ctx.clone(), tx)
            } else {
                None
            };
            (Tray { colors, _linux: linux }, rx)
        }

        #[cfg(not(any(windows, target_os = "linux")))]
        {
            // No tray backend on this target; a no-op so the app still runs.
            let _ = (ctx, enabled, tx);
            (Tray { colors, _noop: () }, rx)
        }
    }

    /// Update the colours the Windows tray menu draws with, from the current
    /// skin. A no-op in effect off Windows (the Linux menu follows the desktop
    /// theme), but always safe to call.
    pub fn set_menu_colors(
        &self,
        bg: egui::Color32,
        text: egui::Color32,
        hover_bg: egui::Color32,
        hover_text: egui::Color32,
        border: egui::Color32,
    ) {
        self.colors.set(bg, text, hover_bg, hover_text, border);
    }

    /// Whether a tray icon is actually running. False when the tray is disabled
    /// or its backend failed to start (for example a Linux desktop with no
    /// StatusNotifierItem host). The app uses this to avoid hiding the window to
    /// a tray that isn't there, so a close always has somewhere to go.
    pub fn is_active(&self) -> bool {
        #[cfg(windows)]
        {
            self._win.is_some()
        }
        #[cfg(target_os = "linux")]
        {
            self._linux.is_some()
        }
        #[cfg(not(any(windows, target_os = "linux")))]
        {
            false
        }
    }
}

#[cfg(target_os = "linux")]
mod linux_impl {
    use super::TrayAction;
    use ksni::blocking::{Handle, TrayMethods};
    use ksni::menu::StandardItem;
    use ksni::{Icon, MenuItem, Tray as SniTray};
    use std::sync::mpsc::Sender;

    /// The StatusNotifierItem LinAmp registers on the session bus. Holds the
    /// channel back to the app and the egui context to wake it on a click.
    struct LinampTray {
        tx: Sender<TrayAction>,
        egui: egui::Context,
        icon: Vec<Icon>,
    }

    impl LinampTray {
        fn emit(&self, action: TrayAction) {
            let _ = self.tx.send(action);
            self.egui.request_repaint();
        }

        /// Build one `StandardItem` that emits `action` when clicked.
        fn item(label: &str, action: TrayAction) -> MenuItem<Self> {
            StandardItem {
                label: label.to_string(),
                activate: Box::new(move |t: &mut Self| t.emit(action)),
                ..Default::default()
            }
            .into()
        }
    }

    impl SniTray for LinampTray {
        fn id(&self) -> String {
            "linamp".into()
        }

        fn title(&self) -> String {
            "LinAmp".into()
        }

        // We ship our own pixmap rather than an icon-theme name, so the tray
        // looks right regardless of the desktop's installed icon set.
        fn icon_pixmap(&self) -> Vec<Icon> {
            self.icon.clone()
        }

        // Primary (left) click toggles the window, matching the Windows backend.
        fn activate(&mut self, _x: i32, _y: i32) {
            self.emit(TrayAction::Toggle);
        }

        fn menu(&self) -> Vec<MenuItem<Self>> {
            vec![
                Self::item("Play/Pause", TrayAction::PlayPause),
                Self::item("Stop", TrayAction::Stop),
                MenuItem::Separator,
                Self::item("Previous", TrayAction::Prev),
                Self::item("Next", TrayAction::Next),
                MenuItem::Separator,
                Self::item("Restore LinAmp", TrayAction::Restore),
                Self::item("Exit LinAmp", TrayAction::Quit),
            ]
        }
    }

    /// A running StatusNotifierItem. Dropping it shuts the service down and removes
    /// the icon (see `Drop`), so leaving tray mode or exiting clears the tray.
    pub(super) struct LinuxTray {
        handle: Handle<LinampTray>,
    }

    impl LinuxTray {
        pub(super) fn spawn(egui: egui::Context, tx: Sender<TrayAction>) -> Option<Self> {
            let tray = LinampTray {
                tx,
                egui,
                icon: load_icon_pixmap(),
            };
            // A failure here means no StatusNotifierItem host is running (e.g. a
            // bare window manager with no tray). Run on without a tray.
            tray.spawn().ok().map(|handle| LinuxTray { handle })
        }
    }

    impl Drop for LinuxTray {
        fn drop(&mut self) {
            // ksni's Handle does not stop the service when dropped, so the icon would
            // linger after leaving tray mode (or exiting). Shut it down explicitly and
            // wait, so the icon is gone before a replacement tray registers.
            self.handle.shutdown().wait();
        }
    }

    /// Decode the embedded logo into the ARGB32 pixmap the SNI spec expects
    /// (one icon; bytes ordered A, R, G, B per pixel).
    fn load_icon_pixmap() -> Vec<Icon> {
        let bytes = include_bytes!("../assets/linamp_logo_32.png");
        let Ok(img) = image::load_from_memory(bytes) else {
            return Vec::new();
        };
        let img = img.to_rgba8();
        let (w, h) = img.dimensions();
        let mut data = Vec::with_capacity((w * h * 4) as usize);
        for px in img.pixels() {
            let [r, g, b, a] = px.0;
            data.extend_from_slice(&[a, r, g, b]);
        }
        vec![Icon {
            width: w as i32,
            height: h as i32,
            data,
        }]
    }
}

#[cfg(windows)]
mod windows_impl {
    use super::{action_for_command, MenuColors, TrayAction, MENU_ITEMS};
    use std::sync::atomic::{AtomicIsize, Ordering};
    use std::sync::mpsc::Sender;
    use std::sync::Arc;

    // Minimal Win32 surface. We talk to the notification area through a hidden
    // top-level window so the icon has somewhere to deliver its clicks.
    type Hwnd = isize;
    type Hicon = isize;
    type Hinstance = isize;
    type Hmenu = isize;
    type Wparam = usize;
    type Lparam = isize;
    type Lresult = isize;

    const WM_APP_TRAY: u32 = 0x8000 + 1; // WM_APP + 1: our icon callback message.
    const WM_CLOSE: u32 = 0x0010;
    const WM_LBUTTONUP: u32 = 0x0202;
    const WM_LBUTTONDBLCLK: u32 = 0x0203;
    const WM_RBUTTONUP: u32 = 0x0205;
    const WM_NULL: u32 = 0x0000;
    // A tool window never appears in the taskbar or Alt+Tab, so the tray's helper
    // window stays invisible even while it owns the foreground for the popup menu.
    const WS_EX_TOOLWINDOW: u32 = 0x0000_0080;
    // A borderless popup, created zero-sized and off-screen, then shown without
    // activation (SW_SHOWNA). It must be shown (not merely created) for
    // SetForegroundWindow to take, which the tray menu needs to highlight and
    // dismiss. At zero size and off-screen it draws nothing.
    const WS_POPUP: u32 = 0x8000_0000;
    const SW_SHOWNA: i32 = 8;
    const OFF_SCREEN: i32 = -32000;

    const NIM_ADD: u32 = 0;
    const NIM_DELETE: u32 = 2;
    const NIF_MESSAGE: u32 = 0x01;
    const NIF_ICON: u32 = 0x02;
    const NIF_TIP: u32 = 0x04;

    const MF_OWNERDRAW: u32 = 0x0100;
    const MF_DISABLED: u32 = 0x0002;
    const TPM_RIGHTBUTTON: u32 = 0x0002;
    const TPM_RETURNCMD: u32 = 0x0100;

    // Owner-draw: we paint each item ourselves so the menu matches the skin.
    const WM_MEASUREITEM: u32 = 0x002C;
    const WM_DRAWITEM: u32 = 0x002B;
    const ODS_SELECTED: u32 = 0x0001;
    // SetMenuInfo: skin the menu background and drop the left check gutter.
    const MIM_BACKGROUND: u32 = 0x0000_0002;
    const MIM_STYLE: u32 = 0x0000_0010;
    const MNS_NOCHECK: u32 = 0x8000_0000;
    // DrawTextW flags: one left-aligned, vertically centred line, '&' literal.
    const DT_LEFT: u32 = 0x0000;
    const DT_SINGLELINE: u32 = 0x0020;
    const DT_VCENTER: u32 = 0x0004;
    const DT_NOPREFIX: u32 = 0x0800;
    const TRANSPARENT: i32 = 1;
    const DEFAULT_GUI_FONT: i32 = 17;

    const IMAGE_ICON: u32 = 1;
    const LR_DEFAULTCOLOR: u32 = 0;
    const SM_CXSMICON: i32 = 49;
    const SM_CYSMICON: i32 = 50;
    const GWLP_USERDATA: i32 = -21;
    const IDI_APPLICATION: usize = 32512;

    #[repr(C)]
    struct Guid {
        data1: u32,
        data2: u16,
        data3: u16,
        data4: [u8; 8],
    }

    // NOTIFYICONDATAW. cbSize is set to the full struct size, which selects the
    // modern layout on every supported Windows.
    #[repr(C)]
    struct NotifyIconDataW {
        cb_size: u32,
        h_wnd: Hwnd,
        u_id: u32,
        u_flags: u32,
        u_callback_message: u32,
        h_icon: Hicon,
        sz_tip: [u16; 128],
        dw_state: u32,
        dw_state_mask: u32,
        sz_info: [u16; 256],
        u_version: u32,
        sz_info_title: [u16; 64],
        dw_info_flags: u32,
        guid_item: Guid,
        h_balloon_icon: Hicon,
    }

    #[repr(C)]
    struct Point {
        x: i32,
        y: i32,
    }

    #[repr(C)]
    struct Rect {
        left: i32,
        top: i32,
        right: i32,
        bottom: i32,
    }

    #[repr(C)]
    struct SizeW {
        cx: i32,
        cy: i32,
    }

    // DRAWITEMSTRUCT. The 4-byte gap before the pointer fields is inserted by
    // repr(C) so the layout matches the Win32 struct on 64-bit.
    #[repr(C)]
    struct DrawItemStruct {
        ctl_type: u32,
        ctl_id: u32,
        item_id: u32,
        item_action: u32,
        item_state: u32,
        hwnd_item: isize,
        h_dc: isize,
        rc_item: Rect,
        item_data: usize,
    }

    #[repr(C)]
    struct MeasureItemStruct {
        ctl_type: u32,
        ctl_id: u32,
        item_id: u32,
        item_width: u32,
        item_height: u32,
        item_data: usize,
    }

    #[repr(C)]
    struct MenuInfo {
        cb_size: u32,
        f_mask: u32,
        dw_style: u32,
        cy_max: u32,
        hbr_back: isize,
        dw_context_help_id: u32,
        dw_menu_data: usize,
    }

    #[repr(C)]
    struct WndClassW {
        style: u32,
        wnd_proc: Option<unsafe extern "system" fn(Hwnd, u32, Wparam, Lparam) -> Lresult>,
        cb_cls_extra: i32,
        cb_wnd_extra: i32,
        h_instance: Hinstance,
        h_icon: Hicon,
        h_cursor: isize,
        h_br_background: isize,
        menu_name: *const u16,
        class_name: *const u16,
    }

    #[link(name = "user32")]
    extern "system" {
        fn RegisterClassW(c: *const WndClassW) -> u16;
        fn CreateWindowExW(
            ex_style: u32,
            class_name: *const u16,
            window_name: *const u16,
            style: u32,
            x: i32,
            y: i32,
            w: i32,
            h: i32,
            parent: Hwnd,
            menu: Hmenu,
            instance: Hinstance,
            param: *const core::ffi::c_void,
        ) -> Hwnd;
        fn DefWindowProcW(hwnd: Hwnd, msg: u32, w: Wparam, l: Lparam) -> Lresult;
        fn DestroyWindow(hwnd: Hwnd) -> i32;
        fn ShowWindow(hwnd: Hwnd, cmd: i32) -> i32;
        fn GetMessageW(msg: *mut Msg, hwnd: Hwnd, min: u32, max: u32) -> i32;
        fn TranslateMessage(msg: *const Msg) -> i32;
        fn DispatchMessageW(msg: *const Msg) -> Lresult;
        fn PostMessageW(hwnd: Hwnd, msg: u32, w: Wparam, l: Lparam) -> i32;
        fn PostQuitMessage(code: i32);
        fn SetWindowLongPtrW(hwnd: Hwnd, index: i32, val: isize) -> isize;
        fn GetWindowLongPtrW(hwnd: Hwnd, index: i32) -> isize;
        fn CreatePopupMenu() -> Hmenu;
        fn AppendMenuW(menu: Hmenu, flags: u32, id: usize, item: *const u16) -> i32;
        fn DestroyMenu(menu: Hmenu) -> i32;
        fn TrackPopupMenu(
            menu: Hmenu,
            flags: u32,
            x: i32,
            y: i32,
            reserved: i32,
            hwnd: Hwnd,
            rect: *const core::ffi::c_void,
        ) -> i32;
        fn SetForegroundWindow(hwnd: Hwnd) -> i32;
        fn GetCursorPos(p: *mut Point) -> i32;
        fn LoadImageW(inst: Hinstance, name: *const u16, ty: u32, cx: i32, cy: i32, flags: u32)
            -> isize;
        fn LoadIconW(inst: Hinstance, name: *const u16) -> Hicon;
        fn GetSystemMetrics(index: i32) -> i32;
        fn SetMenuInfo(menu: Hmenu, info: *const MenuInfo) -> i32;
        fn FillRect(h_dc: isize, rc: *const Rect, brush: isize) -> i32;
        fn DrawTextW(h_dc: isize, text: *const u16, count: i32, rc: *mut Rect, format: u32) -> i32;
        fn GetDC(hwnd: Hwnd) -> isize;
        fn ReleaseDC(hwnd: Hwnd, h_dc: isize) -> i32;
        fn GetForegroundWindow() -> Hwnd;
        fn GetWindowThreadProcessId(hwnd: Hwnd, pid: *mut u32) -> u32;
        fn AttachThreadInput(attach: u32, attach_to: u32, attach_flag: i32) -> i32;
        fn GetDpiForSystem() -> u32;
    }

    #[link(name = "gdi32")]
    extern "system" {
        fn CreateSolidBrush(color: u32) -> isize;
        fn DeleteObject(obj: isize) -> i32;
        fn SelectObject(h_dc: isize, obj: isize) -> isize;
        fn GetStockObject(obj: i32) -> isize;
        fn SetTextColor(h_dc: isize, color: u32) -> u32;
        fn SetBkMode(h_dc: isize, mode: i32) -> i32;
        fn GetTextExtentPoint32W(h_dc: isize, text: *const u16, count: i32, sz: *mut SizeW) -> i32;
        fn AddFontMemResourceEx(
            data: *const core::ffi::c_void,
            size: u32,
            reserved: *const core::ffi::c_void,
            count: *mut u32,
        ) -> isize;
        #[allow(clippy::too_many_arguments)]
        fn CreateFontW(
            height: i32,
            width: i32,
            escapement: i32,
            orientation: i32,
            weight: i32,
            italic: u32,
            underline: u32,
            strike_out: u32,
            char_set: u32,
            out_precision: u32,
            clip_precision: u32,
            quality: u32,
            pitch_and_family: u32,
            face: *const u16,
        ) -> isize;
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn GetModuleHandleW(name: *const u16) -> Hinstance;
        fn GetCurrentThreadId() -> u32;
    }

    #[link(name = "shell32")]
    extern "system" {
        fn Shell_NotifyIconW(message: u32, data: *mut NotifyIconDataW) -> i32;
    }

    #[repr(C)]
    struct Msg {
        hwnd: Hwnd,
        message: u32,
        w_param: Wparam,
        l_param: Lparam,
        time: u32,
        pt: Point,
        private: u32,
    }

    /// Per-window state the WndProc needs: where to send actions, which egui
    /// context to wake, and the skin colours to draw the menu with. Heap-allocated
    /// and reached through `GWLP_USERDATA`.
    struct TrayCtx {
        tx: Sender<TrayAction>,
        egui: egui::Context,
        colors: Arc<MenuColors>,
        /// The menu font (Noto Sans, LinAmp's UI font), or 0 to fall back to the
        /// system menu font.
        font: isize,
    }

    impl TrayCtx {
        /// Send an action and wake the app so it drains the channel even when the
        /// window is hidden and otherwise idle.
        fn emit(&self, action: TrayAction) {
            let _ = self.tx.send(action);
            self.egui.request_repaint();
        }
    }

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    /// Load LinAmp's bundled UI font (Noto Sans) as a GDI font so the tray menu
    /// matches the app's own menus, sized to the system DPI. Returns 0 if the font
    /// can't be created, so the caller falls back to the system menu font.
    unsafe fn load_menu_font() -> isize {
        const FW_NORMAL: i32 = 400;
        const DEFAULT_CHARSET: u32 = 1;
        const CLEARTYPE_QUALITY: u32 = 5;
        let bytes = include_bytes!("../assets/fonts/NotoSans-Regular.ttf");
        let mut count = 0u32;
        AddFontMemResourceEx(
            bytes.as_ptr() as *const core::ffi::c_void,
            bytes.len() as u32,
            std::ptr::null(),
            &mut count,
        );
        // The shared menu point size, scaled to the system DPI (96 = 100%), so the
        // tray menu renders at the same size as LinAmp's own menus.
        let dpi = GetDpiForSystem().max(96) as f32;
        let height = -((crate::app::MENU_FONT_PT * dpi / 96.0).round() as i32);
        let face = wide("Noto Sans");
        CreateFontW(
            height,
            0,
            0,
            0,
            FW_NORMAL,
            0,
            0,
            0,
            DEFAULT_CHARSET,
            0,
            0,
            CLEARTYPE_QUALITY,
            0,
            face.as_ptr(),
        )
    }

    /// The menu font to draw with: the loaded Noto Sans, or the system menu font.
    unsafe fn menu_font(font: isize) -> isize {
        if font != 0 {
            font
        } else {
            GetStockObject(DEFAULT_GUI_FONT)
        }
    }

    /// Load the executable's embedded application icon at tray size, falling
    /// back to the generic system icon if the resource can't be found.
    unsafe fn load_tray_icon(hinst: Hinstance) -> Hicon {
        let cx = GetSystemMetrics(SM_CXSMICON).max(16);
        let cy = GetSystemMetrics(SM_CYSMICON).max(16);
        // winresource embeds assets/linamp.ico as the first icon (resource id 1).
        let icon = LoadImageW(hinst, 1 as *const u16, IMAGE_ICON, cx, cy, LR_DEFAULTCOLOR);
        if icon != 0 {
            return icon;
        }
        LoadIconW(0, IDI_APPLICATION as *const u16)
    }

    /// Build the owner-drawn tray menu and the background brush it uses. Every row
    /// is MF_OWNERDRAW (separators also disabled, so they can't be selected) and
    /// carries its index into `MENU_ITEMS` as item data, which the measure and draw
    /// passes read back. Returns the menu and the brush, which the caller frees
    /// after the menu closes.
    unsafe fn build_menu(bg: u32) -> (Hmenu, isize) {
        let menu = CreatePopupMenu();
        let brush = CreateSolidBrush(bg);
        // Skin the menu background and drop the left check gutter so our items
        // fill the full width.
        let mi = MenuInfo {
            cb_size: std::mem::size_of::<MenuInfo>() as u32,
            f_mask: MIM_BACKGROUND | MIM_STYLE,
            dw_style: MNS_NOCHECK,
            cy_max: 0,
            hbr_back: brush,
            dw_context_help_id: 0,
            dw_menu_data: 0,
        };
        SetMenuInfo(menu, &mi);
        for (index, (id, label)) in MENU_ITEMS.iter().enumerate() {
            let data = index as *const u16; // item data = row index (not a string)
            if label.is_empty() {
                AppendMenuW(menu, MF_OWNERDRAW | MF_DISABLED, 0, data);
            } else {
                AppendMenuW(menu, MF_OWNERDRAW, *id as usize, data);
            }
        }
        (menu, brush)
    }

    /// WM_MEASUREITEM: size one row from its label, using the menu font.
    unsafe fn measure_item(mis: &mut MeasureItemStruct, font: isize) {
        let (_, label) = MENU_ITEMS[mis.item_data.min(MENU_ITEMS.len() - 1)];
        if label.is_empty() {
            mis.item_width = 0;
            mis.item_height = 7; // separator row
            return;
        }
        let text = wide(label);
        let dc = GetDC(0);
        let prev = SelectObject(dc, menu_font(font));
        let mut sz = SizeW { cx: 0, cy: 0 };
        GetTextExtentPoint32W(dc, text.as_ptr(), label.chars().count() as i32, &mut sz);
        SelectObject(dc, prev);
        ReleaseDC(0, dc);
        // Room for the text plus left/right padding; a row height close to the
        // egui menus' rows so the two read the same.
        mis.item_width = (sz.cx + 34) as u32;
        mis.item_height = (sz.cy + 6).max(20) as u32;
    }

    /// WM_DRAWITEM: paint one row in the skin's colours, highlighting the hovered
    /// one and drawing a thin themed border down the sides (and across the ends).
    unsafe fn draw_item(dis: &DrawItemStruct, colors: &MenuColors, font: isize) {
        let (_, label) = MENU_ITEMS[dis.item_data.min(MENU_ITEMS.len() - 1)];
        let selected = dis.item_state & ODS_SELECTED != 0 && !label.is_empty();
        let bg = if selected {
            colors.hover_bg.load(Ordering::Relaxed)
        } else {
            colors.bg.load(Ordering::Relaxed)
        };
        let border = colors.border.load(Ordering::Relaxed);
        let rc = &dis.rc_item;

        // Fill the row background.
        let bg_brush = CreateSolidBrush(bg);
        FillRect(dis.h_dc, rc, bg_brush);
        DeleteObject(bg_brush);

        let border_brush = CreateSolidBrush(border);
        // Side borders on every row, so the column reads as one bordered block.
        let left = Rect {
            left: rc.left,
            top: rc.top,
            right: rc.left + 1,
            bottom: rc.bottom,
        };
        let right = Rect {
            left: rc.right - 1,
            top: rc.top,
            right: rc.right,
            bottom: rc.bottom,
        };
        FillRect(dis.h_dc, &left, border_brush);
        FillRect(dis.h_dc, &right, border_brush);
        // Top border on the first row, bottom border on the last.
        if dis.item_data == 0 {
            let top = Rect {
                left: rc.left,
                top: rc.top,
                right: rc.right,
                bottom: rc.top + 1,
            };
            FillRect(dis.h_dc, &top, border_brush);
        }
        if dis.item_data == MENU_ITEMS.len() - 1 {
            let bottom = Rect {
                left: rc.left,
                top: rc.bottom - 1,
                right: rc.right,
                bottom: rc.bottom,
            };
            FillRect(dis.h_dc, &bottom, border_brush);
        }

        if label.is_empty() {
            // Separator: a thin line across the middle in the border colour.
            let mid = (rc.top + rc.bottom) / 2;
            let line = Rect {
                left: rc.left + 3,
                top: mid,
                right: rc.right - 3,
                bottom: mid + 1,
            };
            FillRect(dis.h_dc, &line, border_brush);
            DeleteObject(border_brush);
            return;
        }
        DeleteObject(border_brush);

        // Draw the label.
        let text_color = if selected {
            colors.hover_text.load(Ordering::Relaxed)
        } else {
            colors.text.load(Ordering::Relaxed)
        };
        SetBkMode(dis.h_dc, TRANSPARENT);
        SetTextColor(dis.h_dc, text_color);
        let prev = SelectObject(dis.h_dc, menu_font(font));
        let text = wide(label);
        let mut tr = Rect {
            left: rc.left + 12,
            top: rc.top,
            right: rc.right - 8,
            bottom: rc.bottom,
        };
        DrawTextW(
            dis.h_dc,
            text.as_ptr(),
            label.chars().count() as i32,
            &mut tr,
            DT_LEFT | DT_SINGLELINE | DT_VCENTER | DT_NOPREFIX,
        );
        SelectObject(dis.h_dc, prev);
    }

    /// Force `hwnd` to the foreground so the popup menu tracks the mouse (hover
    /// highlight) and dismisses on an outside click. Attaching to the current
    /// foreground thread's input bypasses the foreground-change lock, which
    /// otherwise makes SetForegroundWindow a no-op from a background process.
    unsafe fn force_foreground(hwnd: Hwnd) {
        let fg = GetForegroundWindow();
        let fg_thread = GetWindowThreadProcessId(fg, std::ptr::null_mut());
        let this_thread = GetCurrentThreadId();
        if fg_thread != 0 && fg_thread != this_thread {
            AttachThreadInput(this_thread, fg_thread, 1);
            SetForegroundWindow(hwnd);
            AttachThreadInput(this_thread, fg_thread, 0);
        } else {
            SetForegroundWindow(hwnd);
        }
    }

    unsafe extern "system" fn wnd_proc(hwnd: Hwnd, msg: u32, w: Wparam, l: Lparam) -> Lresult {
        if msg == WM_MEASUREITEM {
            let ctx_ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const TrayCtx;
            let font = if ctx_ptr.is_null() { 0 } else { (*ctx_ptr).font };
            measure_item(&mut *(l as *mut MeasureItemStruct), font);
            return 1;
        }
        if msg == WM_DRAWITEM {
            let ctx_ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const TrayCtx;
            if !ctx_ptr.is_null() {
                let ctx = &*ctx_ptr;
                draw_item(&*(l as *const DrawItemStruct), &ctx.colors, ctx.font);
            }
            return 1;
        }
        if msg == WM_APP_TRAY {
            let ctx_ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const TrayCtx;
            if ctx_ptr.is_null() {
                return 0;
            }
            let ctx = &*ctx_ptr;
            // Classic callback protocol: the low word of lParam is the mouse message.
            let mouse = (l as u32) & 0xffff;
            match mouse {
                WM_LBUTTONUP | WM_LBUTTONDBLCLK => ctx.emit(TrayAction::Toggle),
                WM_RBUTTONUP => {
                    // Winamp-style transport menu at the cursor, owner-drawn in the
                    // skin's colours. Forcing the foreground (and the WM_NULL after)
                    // is what makes the menu highlight on hover and dismiss when
                    // clicked away from.
                    let bg = ctx.colors.bg.load(Ordering::Relaxed);
                    let (menu, brush) = build_menu(bg);
                    let mut pt = Point { x: 0, y: 0 };
                    GetCursorPos(&mut pt);
                    force_foreground(hwnd);
                    let chosen = TrackPopupMenu(
                        menu,
                        TPM_RIGHTBUTTON | TPM_RETURNCMD,
                        pt.x,
                        pt.y,
                        0,
                        hwnd,
                        std::ptr::null(),
                    );
                    PostMessageW(hwnd, WM_NULL, 0, 0);
                    DestroyMenu(menu);
                    DeleteObject(brush);
                    if let Some(action) = action_for_command(chosen as u32) {
                        ctx.emit(action);
                    }
                }
                _ => {}
            }
            return 0;
        }
        if msg == WM_CLOSE {
            PostQuitMessage(0);
            return 0;
        }
        DefWindowProcW(hwnd, msg, w, l)
    }

    /// A running Windows tray. Dropping it closes the message loop and removes
    /// the icon from the notification area.
    pub(super) struct WinTray {
        hwnd: Arc<AtomicIsize>,
        thread: Option<std::thread::JoinHandle<()>>,
    }

    impl WinTray {
        pub(super) fn spawn(
            egui: egui::Context,
            tx: Sender<TrayAction>,
            colors: Arc<MenuColors>,
        ) -> Result<Self, ()> {
            let hwnd_slot = Arc::new(AtomicIsize::new(0));
            let hwnd_for_thread = hwnd_slot.clone();

            let thread = std::thread::Builder::new()
                .name("linamp-tray".into())
                .spawn(move || unsafe { tray_thread(egui, tx, colors, hwnd_for_thread) })
                .map_err(|_| ())?;

            // Wait briefly for the thread to publish its HWND so Drop can signal
            // it. The thread stores the handle as its first act.
            for _ in 0..200 {
                if hwnd_slot.load(Ordering::Acquire) != 0 {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(1));
            }

            Ok(WinTray {
                hwnd: hwnd_slot,
                thread: Some(thread),
            })
        }
    }

    impl Drop for WinTray {
        fn drop(&mut self) {
            let hwnd = self.hwnd.load(Ordering::Acquire);
            if hwnd != 0 {
                unsafe {
                    PostMessageW(hwnd, WM_CLOSE, 0, 0);
                }
            }
            if let Some(t) = self.thread.take() {
                let _ = t.join();
            }
        }
    }

    unsafe fn tray_thread(
        egui: egui::Context,
        tx: Sender<TrayAction>,
        colors: Arc<MenuColors>,
        hwnd_slot: Arc<AtomicIsize>,
    ) {
        let hinst = GetModuleHandleW(std::ptr::null());
        let class_name = wide("LinAmpTrayWindow");

        let wc = WndClassW {
            style: 0,
            wnd_proc: Some(wnd_proc),
            cb_cls_extra: 0,
            cb_wnd_extra: 0,
            h_instance: hinst,
            h_icon: 0,
            h_cursor: 0,
            h_br_background: 0,
            menu_name: std::ptr::null(),
            class_name: class_name.as_ptr(),
        };
        // Registering an already-registered class fails harmlessly; we only ever
        // create one tray window, so a zero return still lets CreateWindow work.
        RegisterClassW(&wc);

        // A distinct title from the main window ("LinAmp"): FindWindowW("LinAmp")
        // elsewhere (the live taskbar toggle, window grouping) must resolve to the
        // real player window, not this helper, or it would act on the wrong one.
        let window_name = wide("LinAmp Tray");
        // A real top-level window (not message-only), zero-sized and off-screen so
        // it draws nothing. It must be a real, shown window so SetForegroundWindow
        // takes, which TrackPopupMenu needs for the menu to track the mouse (hover
        // highlight) and dismiss on an outside click. WS_EX_TOOLWINDOW keeps it out
        // of the taskbar and Alt+Tab.
        let hwnd = CreateWindowExW(
            WS_EX_TOOLWINDOW,
            class_name.as_ptr(),
            window_name.as_ptr(),
            WS_POPUP,
            OFF_SCREEN,
            OFF_SCREEN,
            0,
            0,
            0,
            0,
            hinst,
            std::ptr::null(),
        );
        if hwnd == 0 {
            return;
        }

        // Publish the window handle so Drop can post it a close message.
        hwnd_slot.store(hwnd, Ordering::Release);

        // Show it without activating. It is zero-sized and off-screen, so nothing
        // appears, but being shown lets SetForegroundWindow take so the popup menu
        // highlights and dismisses correctly.
        ShowWindow(hwnd, SW_SHOWNA);

        // Hand the WndProc its context. No tray messages arrive until the icon
        // is added just below, so setting this after CreateWindow is safe.
        let font = load_menu_font();
        let ctx = Box::into_raw(Box::new(TrayCtx {
            tx,
            egui,
            colors,
            font,
        }));
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, ctx as isize);

        let mut tip = [0u16; 128];
        for (dst, src) in tip.iter_mut().zip(wide("LinAmp")) {
            *dst = src;
        }
        let mut nid = NotifyIconDataW {
            cb_size: std::mem::size_of::<NotifyIconDataW>() as u32,
            h_wnd: hwnd,
            u_id: 1,
            u_flags: NIF_MESSAGE | NIF_ICON | NIF_TIP,
            u_callback_message: WM_APP_TRAY,
            h_icon: load_tray_icon(hinst),
            sz_tip: tip,
            dw_state: 0,
            dw_state_mask: 0,
            sz_info: [0; 256],
            u_version: 0,
            sz_info_title: [0; 64],
            dw_info_flags: 0,
            guid_item: Guid {
                data1: 0,
                data2: 0,
                data3: 0,
                data4: [0; 8],
            },
            h_balloon_icon: 0,
        };
        Shell_NotifyIconW(NIM_ADD, &mut nid);

        // Pump messages until Drop posts WM_CLOSE (which calls PostQuitMessage).
        let mut msg = std::mem::zeroed::<Msg>();
        while GetMessageW(&mut msg, 0, 0, 0) > 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }

        // Tear down: remove the icon, drop the context, destroy the window.
        Shell_NotifyIconW(NIM_DELETE, &mut nid);
        if font != 0 {
            DeleteObject(font);
        }
        let ctx_ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut TrayCtx;
        if !ctx_ptr.is_null() {
            drop(Box::from_raw(ctx_ptr));
        }
        DestroyWindow(hwnd);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_ids_map_to_their_actions() {
        assert_eq!(action_for_command(cmd::PLAY_PAUSE), Some(TrayAction::PlayPause));
        assert_eq!(action_for_command(cmd::STOP), Some(TrayAction::Stop));
        assert_eq!(action_for_command(cmd::PREV), Some(TrayAction::Prev));
        assert_eq!(action_for_command(cmd::NEXT), Some(TrayAction::Next));
        assert_eq!(action_for_command(cmd::RESTORE), Some(TrayAction::Restore));
        assert_eq!(action_for_command(cmd::EXIT), Some(TrayAction::Quit));
    }

    #[test]
    fn non_command_ids_report_nothing() {
        // 0 is the "menu dismissed" return, and ids past the menu are not ours.
        assert_eq!(action_for_command(0), None);
        assert_eq!(action_for_command(99), None);
    }

    #[test]
    fn a_disabled_tray_still_hands_back_a_usable_receiver() {
        let ctx = egui::Context::default();
        let (_tray, rx) = Tray::spawn(&ctx, false);
        // Nothing is queued, and the receiver is open (not hung up), so the
        // app's per-frame drain sees an empty, non-erroring channel.
        assert!(rx.try_recv().is_err());
    }
}
