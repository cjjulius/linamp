//! Tray, taskbar and window-mode lifecycle (system integration), split out of
//! app.rs as methods on `LinAmpApp`. Behavior is unchanged.

use super::*;

impl super::LinAmpApp {
    /// Act on a click from the system-tray icon (#40).
    pub(super) fn handle_tray_action(&mut self, ctx: &egui::Context, action: crate::tray::TrayAction) {
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
    pub(super) fn set_window_mode(&mut self, ctx: &egui::Context, enabled: bool) {
        self.prefs.tray_enabled = enabled;
        self.prefs.save();
        // Re-create the tray to match: spawning disabled drops the icon, enabled
        // adds it, and the old tray's Drop removes the previous icon.
        self.respawn_tray(ctx, enabled);
        set_taskbar_button("LinAmp", !self.tray_mode);
        // Leaving tray mode while the window is hidden would strand it; bring it back.
        if !self.tray_mode && self.hidden_to_tray {
            self.show_from_tray(ctx);
        }
    }

    /// Switch Icon Dark Mode live (#223): recolour the window and taskbar icon now,
    /// and respawn the tray (when it is running) so its icon matches. Persists the
    /// choice, so the next launch starts with the chosen look.
    pub(super) fn set_icon_dark_mode(&mut self, ctx: &egui::Context, dark: bool) {
        self.prefs.icon_dark_mode = dark;
        self.prefs.save();
        // Window, taskbar and Alt-Tab icon: set it on the main window directly. We
        // cannot go through egui/winit here: its runtime Icon command only updates the
        // small (title-bar) icon, while the taskbar button and Alt-Tab use the big
        // icon, which otherwise would not change until the next launch. set_app_window_icon
        // sets both on the main window (Windows WM_SETICON, X11 _NET_WM_ICON).
        if let (Some(big), Some(small)) =
            (crate::icon::window_rgba(dark), crate::icon::tray_rgba(dark))
        {
            set_app_window_icon((big.0, big.1, &big.2), (small.0, small.1, &small.2));
        }
        // The per-user theme icon that a GNOME dock and the app launcher render is
        // installed at startup (fileassoc::register, driven by this same pref), so a
        // fresh launch starts in the right colour. We deliberately do NOT rewrite it
        // here on a live toggle: GNOME caches a running app's icon and only re-reads
        // it, laggily, when its window re-associates, which detaches the window into a
        // fresh dock entry (it visibly jumps to the end of the dock). Toggling the
        // theme file live is what triggered that, so the dock/launcher now follows the
        // pref on the next launch instead. The tray (below) and the X11 _NET_WM_ICON
        // (above, for KDE/XFCE/tiling bars) still update live.
        // The running tray baked in the old colour, so respawn it to pick up the new
        // one. Only when tray mode is on; otherwise there is no tray icon to update,
        // and the window mode (taskbar button, hidden state) is unchanged.
        if self.prefs.tray_enabled {
            self.respawn_tray(ctx, true);
        }
    }

    /// Respawn the tray to match the current state: drop the old icon and start a
    /// fresh one for `enabled`, using the current `icon_dark_mode` colour, then push
    /// the skin's menu palette (a fresh tray starts on default colours). Shared by
    /// the window-mode and icon-colour toggles so the respawn stays in one place.
    pub(super) fn respawn_tray(&mut self, ctx: &egui::Context, enabled: bool) {
        let (tray, rx) = crate::tray::Tray::spawn(ctx, enabled, self.prefs.icon_dark_mode);
        self._tray = tray;
        self.tray_rx = rx;
        self.tray_mode = enabled && self._tray.is_active();
        self.push_tray_theme();
    }

    /// Push the current skin's menu palette to the tray so its native menu matches
    /// LinAmp's own menus (#40). Cheap, and a no-op in effect off Windows.
    pub(super) fn push_tray_theme(&self) {
        let (bg, text, hover_bg, hover_text, border) = menu_palette(&self.theme);
        self._tray
            .set_menu_colors(bg, text, hover_bg, hover_text, border);
    }

    /// Genuinely quit the app (main-menu Exit or the tray's Exit). Marks the quit
    /// so the close handler shuts down even in tray mode, then starts the normal
    /// close. The window may be minimized to the tray, but eframe still runs the
    /// teardown while minimized, so there's no need to restore it first.
    pub(super) fn request_exit(&mut self, ctx: &egui::Context) {
        self.force_quit = true;
        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
    }

    /// Hide every LinAmp window and keep running in the tray. The child windows
    /// go with the main one so none are left floating, and their state is saved
    /// so a later restore reopens exactly what was showing.
    pub(super) fn hide_to_tray(&mut self, ctx: &egui::Context) {
        if self.hidden_to_tray {
            return;
        }
        // Capture the on-screen layout before hiding, so exiting from the tray (which
        // skips the session save while hidden) restores exactly these windows and
        // positions (#40).
        self.build_session().save();
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
    pub(super) fn show_from_tray(&mut self, ctx: &egui::Context) {
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

    /// Taskbar-mode minimize (#209). The EQ/PL/library child viewports are their
    /// own X11 top-levels and X11 won't iconify them with the root, so minimizing
    /// the root alone leaves them on screen and the app looks like it never
    /// minimized. Hide them like the tray path and stash what was open, then
    /// minimize the root; `restore_from_taskbar` brings the children back when the
    /// root is restored. Harmless on Windows, where the children follow the root.
    pub(super) fn minimize_to_taskbar(&mut self, ctx: &egui::Context) {
        if !self.minimized_to_taskbar {
            self.min_prev_vis = (self.eq_visible, self.pl_visible, self.lib_visible);
            self.eq_visible = false;
            self.pl_visible = false;
            self.lib_visible = false;
            self.minimized_to_taskbar = true;
            self.min_confirmed = false;
        }
        ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
    }

    /// Reopen the child windows that `minimize_to_taskbar` hid, once the root has
    /// been restored from the taskbar. The WM restores the root itself.
    pub(super) fn restore_from_taskbar(&mut self) {
        if self.minimized_to_taskbar {
            let (eq, pl, lib) = self.min_prev_vis;
            self.eq_visible = eq;
            self.pl_visible = pl;
            self.lib_visible = lib;
            self.minimized_to_taskbar = false;
            self.min_confirmed = false;
        }
    }

    /// Watch the root viewport's minimized flag so a taskbar restore (true->false)
    /// reopens the children hidden by `minimize_to_taskbar`. Gated on `min_confirmed`
    /// so the frame that sends Minimized(true), before the WM applies it, cannot be
    /// read as an immediate restore.
    pub(super) fn poll_taskbar_restore(&mut self, ctx: &egui::Context) {
        if !self.minimized_to_taskbar {
            return;
        }
        let min = ctx.input(|i| i.viewport().minimized).unwrap_or(false);
        let (confirmed, restore) = taskbar_restore_step(min, self.min_confirmed);
        self.min_confirmed = confirmed;
        if restore {
            self.restore_from_taskbar();
            ctx.request_repaint();
        }
    }
}
