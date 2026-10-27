//! Global multimedia-key hotkeys (#165), so Play/Pause, Stop, Previous and Next
//! work from the keyboard while LinAmp is unfocused or hidden to the tray.
//!
//! The keys feed the same transport actions as the tray (`TrayAction`), so the
//! app drains one more channel each frame and routes it through the existing
//! `handle_tray_action`. The backend runs on its own thread and reports what was
//! pressed over that channel, waking the egui loop so the action is handled even
//! when the window is idle.
//!
//! Windows registers the `VK_MEDIA_*` keys with `RegisterHotKey`. Linux will
//! announce LinAmp over MPRIS (the standard route desktops use to deliver media
//! keys to players) in a follow-up; see `linux_impl`. Every other target gets a
//! no-op so the app still builds and runs without global keys.

use crate::tray::TrayAction;
use std::sync::mpsc::Receiver;

/// Map a media-hotkey id to the transport action it triggers. Shared between the
/// Windows backend and the tests so both read the same table. Ids are our own,
/// assigned in registration order; `None` guards an id we never registered.
#[cfg_attr(not(windows), allow(dead_code))]
fn action_for_hotkey(id: u32) -> Option<TrayAction> {
    match id {
        hotkey::PLAY_PAUSE => Some(TrayAction::PlayPause),
        hotkey::STOP => Some(TrayAction::Stop),
        hotkey::PREV => Some(TrayAction::Prev),
        hotkey::NEXT => Some(TrayAction::Next),
        _ => None,
    }
}

/// Hotkey ids, assigned in registration order. Shared with the tests; the
/// Windows backend passes them to `RegisterHotKey` as the WM_HOTKEY `wParam`.
#[cfg_attr(not(windows), allow(dead_code))]
mod hotkey {
    pub const PLAY_PAUSE: u32 = 1;
    pub const STOP: u32 = 2;
    pub const PREV: u32 = 3;
    pub const NEXT: u32 = 4;
}

/// Live global-media-key registration. Dropping it unregisters the keys and
/// stops the backend thread.
pub struct MediaKeys {
    #[cfg(windows)]
    _win: Option<windows_impl::WinMediaKeys>,
    #[cfg(not(windows))]
    _noop: (),
}

impl MediaKeys {
    /// Register the global media keys and return the receiver the app drains.
    /// A backend that can't start (or an unsupported target) still returns a
    /// valid receiver that simply never yields, so callers need no platform
    /// checks. Media keys are global, so they apply in both window modes.
    pub fn spawn(ctx: &egui::Context) -> (MediaKeys, Receiver<TrayAction>) {
        let (tx, rx) = std::sync::mpsc::channel();

        #[cfg(windows)]
        {
            let win = windows_impl::WinMediaKeys::spawn(ctx.clone(), tx).ok();
            (MediaKeys { _win: win }, rx)
        }

        #[cfg(not(windows))]
        {
            // No global-key backend on this target yet (Linux MPRIS is a
            // follow-up). A no-op so the app still runs.
            let _ = (ctx, tx);
            (MediaKeys { _noop: () }, rx)
        }
    }
}

#[cfg(windows)]
mod windows_impl {
    use super::{action_for_hotkey, hotkey};
    use crate::tray::TrayAction;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::mpsc::Sender;
    use std::sync::Arc;

    // Virtual-key codes for the keyboard transport keys.
    const VK_MEDIA_NEXT_TRACK: u32 = 0xB0;
    const VK_MEDIA_PREV_TRACK: u32 = 0xB1;
    const VK_MEDIA_STOP: u32 = 0xB2;
    const VK_MEDIA_PLAY_PAUSE: u32 = 0xB3;

    // MOD_NOREPEAT: one WM_HOTKEY per physical press, so holding a key doesn't
    // repeat the action. No modifier keys are required for the media keys.
    const MOD_NOREPEAT: u32 = 0x4000;

    const WM_HOTKEY: u32 = 0x0312;
    const WM_QUIT: u32 = 0x0012;

    type Hwnd = isize;
    type Wparam = usize;
    type Lparam = isize;

    #[repr(C)]
    struct Point {
        x: i32,
        y: i32,
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

    #[link(name = "user32")]
    extern "system" {
        fn RegisterHotKey(hwnd: Hwnd, id: i32, modifiers: u32, vk: u32) -> i32;
        fn UnregisterHotKey(hwnd: Hwnd, id: i32) -> i32;
        fn GetMessageW(msg: *mut Msg, hwnd: Hwnd, min: u32, max: u32) -> i32;
        fn PostThreadMessageW(thread_id: u32, msg: u32, w: Wparam, l: Lparam) -> i32;
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn GetCurrentThreadId() -> u32;
    }

    /// The keys to register, each with the id the WM_HOTKEY reports back.
    const KEYS: &[(u32, u32)] = &[
        (hotkey::PLAY_PAUSE, VK_MEDIA_PLAY_PAUSE),
        (hotkey::STOP, VK_MEDIA_STOP),
        (hotkey::PREV, VK_MEDIA_PREV_TRACK),
        (hotkey::NEXT, VK_MEDIA_NEXT_TRACK),
    ];

    /// A running media-key listener. Dropping it tells the thread to quit (which
    /// unregisters the keys) and joins it.
    pub(super) struct WinMediaKeys {
        thread_id: Arc<AtomicU32>,
        thread: Option<std::thread::JoinHandle<()>>,
    }

    impl WinMediaKeys {
        pub(super) fn spawn(egui: egui::Context, tx: Sender<TrayAction>) -> Result<Self, ()> {
            let tid_slot = Arc::new(AtomicU32::new(0));
            let tid_for_thread = tid_slot.clone();

            let thread = std::thread::Builder::new()
                .name("linamp-mediakeys".into())
                .spawn(move || unsafe { keys_thread(egui, tx, tid_for_thread) })
                .map_err(|_| ())?;

            // Wait briefly for the thread to publish its id so Drop can post it a
            // quit message. The thread stores it as its first act.
            for _ in 0..200 {
                if tid_slot.load(Ordering::Acquire) != 0 {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(1));
            }

            Ok(WinMediaKeys {
                thread_id: tid_slot,
                thread: Some(thread),
            })
        }
    }

    impl Drop for WinMediaKeys {
        fn drop(&mut self) {
            let tid = self.thread_id.load(Ordering::Acquire);
            if tid != 0 {
                // WM_QUIT makes GetMessageW return 0, so the loop exits and the
                // thread unregisters the keys on its way out.
                unsafe {
                    PostThreadMessageW(tid, WM_QUIT, 0, 0);
                }
            }
            if let Some(t) = self.thread.take() {
                let _ = t.join();
            }
        }
    }

    unsafe fn keys_thread(egui: egui::Context, tx: Sender<TrayAction>, tid_slot: Arc<AtomicU32>) {
        // Publish the thread id so Drop can post WM_QUIT to this message queue.
        tid_slot.store(GetCurrentThreadId(), Ordering::Release);

        // Register each media key to this thread (hwnd null -> WM_HOTKEY goes to
        // the thread's message queue). A key already held by another app fails
        // here; we skip it and keep the rest, so LinAmp still gets the others.
        let mut registered = Vec::new();
        for &(id, vk) in KEYS {
            if RegisterHotKey(0, id as i32, MOD_NOREPEAT, vk) != 0 {
                registered.push(id);
            }
        }

        // Pump thread messages until Drop posts WM_QUIT. GetMessageW with a null
        // hwnd retrieves the thread-queue WM_HOTKEY messages the keys deliver.
        let mut msg = std::mem::zeroed::<Msg>();
        while GetMessageW(&mut msg, 0, 0, 0) > 0 {
            if msg.message == WM_HOTKEY {
                if let Some(action) = action_for_hotkey(msg.w_param as u32) {
                    let _ = tx.send(action);
                    // Wake the egui loop so the app drains the channel even while
                    // unfocused or hidden to the tray.
                    egui.request_repaint();
                }
            }
        }

        for id in registered {
            UnregisterHotKey(0, id as i32);
        }
    }
}

// Linux (MPRIS) backend: to be implemented and verified on the Linux box.
//
// The standard way desktops deliver media keys to a player is MPRIS over D-Bus:
// the app serves org.mpris.MediaPlayer2 and org.mpris.MediaPlayer2.Player under
// the well-known name org.mpris.MediaPlayer2.linamp, and the desktop routes the
// keyboard transport keys (and the system media controls) to the Player
// interface's PlayPause / Stop / Next / Previous methods. Those map onto the
// same `TrayAction`s and the app's play_pause / stop_playback / prev / next.
//
// zbus 5 is already in the tree (via ksni), so the backend can share it rather
// than pulling a second zbus. Implement `linux_impl::spawn` to return a handle
// like the Windows one, emitting `TrayAction` over the channel and calling
// `egui.request_repaint()` on each press. See docs/REVIEW-NOTES.md for the full
// handoff.

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hotkey_ids_map_to_transport_actions() {
        assert_eq!(action_for_hotkey(hotkey::PLAY_PAUSE), Some(TrayAction::PlayPause));
        assert_eq!(action_for_hotkey(hotkey::STOP), Some(TrayAction::Stop));
        assert_eq!(action_for_hotkey(hotkey::PREV), Some(TrayAction::Prev));
        assert_eq!(action_for_hotkey(hotkey::NEXT), Some(TrayAction::Next));
    }

    #[test]
    fn unknown_hotkey_ids_report_nothing() {
        assert_eq!(action_for_hotkey(0), None);
        assert_eq!(action_for_hotkey(99), None);
    }

    #[test]
    fn spawn_hands_back_a_usable_receiver() {
        let ctx = egui::Context::default();
        let (_keys, rx) = MediaKeys::spawn(&ctx);
        // Nothing is queued and the channel is open, so the app's per-frame drain
        // sees an empty, non-erroring receiver.
        assert!(rx.try_recv().is_err());
    }
}
