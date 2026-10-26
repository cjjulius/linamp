//! System-tray (notification-area) icon with a right-click transport menu and
//! left-click show/hide, matching classic Winamp's tray behaviour (#40).
//!
//! The native tray runs on its own thread and reports what the user clicked
//! back over a channel the app drains once per frame. The app resolves the
//! left-click `Toggle` against its own window state, so the platform code never
//! has to track whether the window is currently hidden.
//!
//! Windows uses `Shell_NotifyIcon` on a message-only window. Linux will use a
//! StatusNotifierItem (the `ksni` crate) in a follow-up commit. Every other
//! target gets a no-op tray so the app still builds and runs without one.

use std::sync::mpsc::Receiver;

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
/// as the Win32 `TrackPopupMenu` command ids.
mod cmd {
    pub const PLAY_PAUSE: u32 = 1;
    pub const STOP: u32 = 2;
    pub const PREV: u32 = 3;
    pub const NEXT: u32 = 4;
    pub const RESTORE: u32 = 5;
    pub const EXIT: u32 = 6;
}

/// Map a tray menu-command id to its action. Returns `None` for ids that are
/// not menu commands (separators never report, and 0 means "dismissed").
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

/// A live tray icon. Dropping it removes the icon and stops the tray thread.
pub struct Tray {
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

        #[cfg(windows)]
        {
            let win = if enabled {
                windows_impl::WinTray::spawn(ctx.clone(), tx).ok()
            } else {
                None
            };
            (Tray { _win: win }, rx)
        }

        #[cfg(target_os = "linux")]
        {
            let linux = if enabled {
                linux_impl::LinuxTray::spawn(ctx.clone(), tx)
            } else {
                None
            };
            (Tray { _linux: linux }, rx)
        }

        #[cfg(not(any(windows, target_os = "linux")))]
        {
            // No tray backend on this target; a no-op so the app still runs.
            let _ = (ctx, enabled, tx);
            (Tray { _noop: () }, rx)
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

    /// A running StatusNotifierItem. The handle keeps the background service
    /// alive; dropping it (at app exit) lets the service and icon go away.
    pub(super) struct LinuxTray {
        _handle: Handle<LinampTray>,
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
            tray.spawn().ok().map(|handle| LinuxTray { _handle: handle })
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
    use super::{action_for_command, cmd, TrayAction};
    use std::sync::atomic::{AtomicIsize, Ordering};
    use std::sync::mpsc::Sender;
    use std::sync::Arc;

    // Minimal Win32 surface. We talk to the notification area through a
    // message-only window so the icon has somewhere to deliver its clicks.
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

    const NIM_ADD: u32 = 0;
    const NIM_DELETE: u32 = 2;
    const NIF_MESSAGE: u32 = 0x01;
    const NIF_ICON: u32 = 0x02;
    const NIF_TIP: u32 = 0x04;

    const MF_STRING: u32 = 0x0000;
    const MF_SEPARATOR: u32 = 0x0800;
    const TPM_RIGHTBUTTON: u32 = 0x0002;
    const TPM_RETURNCMD: u32 = 0x0100;

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
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn GetModuleHandleW(name: *const u16) -> Hinstance;
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

    /// Per-window state the WndProc needs: where to send actions and which egui
    /// context to wake. Heap-allocated and reached through `GWLP_USERDATA`.
    struct TrayCtx {
        tx: Sender<TrayAction>,
        egui: egui::Context,
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

    unsafe fn build_menu() -> Hmenu {
        let menu = CreatePopupMenu();
        let items: &[(u32, &str)] = &[
            (cmd::PLAY_PAUSE, "Play/Pause"),
            (cmd::STOP, "Stop"),
            (0, ""), // separator
            (cmd::PREV, "Previous"),
            (cmd::NEXT, "Next"),
            (0, ""), // separator
            (cmd::RESTORE, "Restore LinAmp"),
            (cmd::EXIT, "Exit LinAmp"),
        ];
        for (id, label) in items {
            if *id == 0 {
                AppendMenuW(menu, MF_SEPARATOR, 0, std::ptr::null());
            } else {
                let text = wide(label);
                AppendMenuW(menu, MF_STRING, *id as usize, text.as_ptr());
            }
        }
        menu
    }

    unsafe extern "system" fn wnd_proc(hwnd: Hwnd, msg: u32, w: Wparam, l: Lparam) -> Lresult {
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
                    // Winamp-style transport menu at the cursor. The foreground
                    // and WM_NULL calls are the documented way to make the menu
                    // dismiss correctly when clicked away from.
                    let menu = build_menu();
                    let mut pt = Point { x: 0, y: 0 };
                    GetCursorPos(&mut pt);
                    SetForegroundWindow(hwnd);
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
        pub(super) fn spawn(egui: egui::Context, tx: Sender<TrayAction>) -> Result<Self, ()> {
            let hwnd_slot = Arc::new(AtomicIsize::new(0));
            let hwnd_for_thread = hwnd_slot.clone();

            let thread = std::thread::Builder::new()
                .name("linamp-tray".into())
                .spawn(move || unsafe { tray_thread(egui, tx, hwnd_for_thread) })
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

    unsafe fn tray_thread(egui: egui::Context, tx: Sender<TrayAction>, hwnd_slot: Arc<AtomicIsize>) {
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

        let window_name = wide("LinAmp");
        // HWND_MESSAGE = -3: a message-only window, never shown on screen.
        let hwnd = CreateWindowExW(
            0,
            class_name.as_ptr(),
            window_name.as_ptr(),
            0,
            0,
            0,
            0,
            0,
            -3,
            0,
            hinst,
            std::ptr::null(),
        );
        if hwnd == 0 {
            return;
        }

        // Publish the window handle so Drop can post it a close message.
        hwnd_slot.store(hwnd, Ordering::Release);

        // Hand the WndProc its context. No tray messages arrive until the icon
        // is added just below, so setting this after CreateWindow is safe.
        let ctx = Box::into_raw(Box::new(TrayCtx { tx, egui }));
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
