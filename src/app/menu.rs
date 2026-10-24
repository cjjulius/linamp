//! Classic right-click / options popup menus (borderless, always-on-top
//! windows composited by egui).

use super::*;

/// Popup menus in classic mode (options + the playlist bottom-bar fly-outs).
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Popup {
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
    /// Right-click on the main window's Stop button.
    StopMenu,
    /// The Alt+I bookmarks list (opened from the Options menu).
    Bookmarks,
    /// Delete-a-saved-EQ-preset list (opened from the EQ Presets menu).
    EqDelete,
}

/// Window size (points) for a popup menu, from its item/separator count.
pub(super) fn popup_dims(kind: Popup) -> (f32, f32) {
    let (buttons, seps, width) = match kind {
        Popup::Options => (11, 3, 182.0),
        Popup::PlAdd => (2, 0, 182.0),
        Popup::PlRem => (4, 0, 182.0),
        Popup::PlSel => (3, 0, 182.0),
        Popup::PlMisc => (6, 0, 182.0),
        Popup::PlList => (3, 0, 182.0),
        // Right-click context menus: wider to fit the shortcut column.
        Popup::PlTrack => (6, 2, 210.0),
        Popup::PlVoid => (12, 4, 210.0),
        Popup::EqPresets => (crate::eq::PRESETS.len() as i32 + 2, 1, 182.0),
        Popup::StopMenu => (3, 0, 200.0),
        // Heights overridden per item count in draw_popup_viewport.
        Popup::Bookmarks => (2, 1, 240.0),
        Popup::EqDelete => (1, 0, 200.0),
    };
    (width, buttons as f32 * 24.0 + seps as f32 * 9.0 + 16.0)
}

/// A context-menu row: `label` on the left, dimmed `shortcut` on the right, full
/// width so shortcuts align to the menu's right edge. Returns true when clicked.
/// Colours match an egui `Button` (and so the Media Library's context menus) —
/// the themed override text colour and the standard hover highlight — so every
/// menu shares one look across skins.
pub(super) fn menu_item(ui: &mut egui::Ui, label: &str, shortcut: &str) -> bool {
    let w = ui.available_width();
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(w, 18.0), egui::Sense::click());
    let vis = ui.visuals().widgets.style(&resp).clone();
    // egui buttons colour their text with `override_text_color` when the skin
    // sets one (apply_theme does); fall back to the widget's own colour.
    let text_color = ui
        .visuals()
        .override_text_color
        .unwrap_or_else(|| vis.text_color());
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
        text_color,
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

impl super::LinAmpApp {
    /// Render the open popup menu in its own borderless window (so it is never
    /// clipped by the small main/playlist windows). Positioned at the click.
    pub(super) fn draw_popup_viewport(&mut self, ctx: &egui::Context) {
        let Some((kind, owner, pos)) = self.popup else {
            return;
        };
        let (w, h) = match kind {
            Popup::Bookmarks => {
                // Grows with the bookmark count (capped so a long list scrolls rather
                // than running off-screen): the list, a separator, then Add + Clear.
                let rows = self.bookmarks.items.len().clamp(1, 12) as f32;
                (240.0, (rows + 2.0) * 24.0 + 9.0 + 16.0)
            }
            Popup::EqPresets => {
                // Built-in presets + the saved library + up to 4 actions (Save,
                // optional Delete, Import, Export). Capped height; the arm scrolls.
                let actions = if self.eq_presets.items.is_empty() { 3 } else { 4 };
                let rows = (crate::eq::PRESETS.len() + self.eq_presets.items.len() + actions) as f32;
                let seps = if self.eq_presets.items.is_empty() { 1.0 } else { 2.0 };
                (182.0, (rows * 24.0 + seps * 9.0 + 16.0).min(420.0))
            }
            Popup::EqDelete => {
                let rows = self.eq_presets.items.len().clamp(1, 12) as f32;
                (200.0, rows * 24.0 + 16.0)
            }
            _ => popup_dims(kind),
        };
        let mut screen = self.win_pos[win_idx(owner)] + pos.to_vec2();
        // Keep the menu fully on the desktop: a tall menu (e.g. the empty-area
        // menu) opened near the bottom/right edge would otherwise spill off-screen
        // with its lower entries unreachable.
        if let Some(vr) = virtual_desktop_rect(ctx) {
            screen.x = screen.x.min(vr.max.x - w).max(vr.min.x);
            screen.y = screen.y.min(vr.max.y - h).max(vr.min.y);
        }
        let mut close = false;
        // Corner radius (logical px) for the fill, border and window region. A hair
        // larger than egui's default 6 because the window's corners are cut by a
        // hard-edged (non-antialiased) GDI region, which reads tighter than the
        // Media Library menu's smooth 6px round — 8 makes them match visually.
        let corner = 8.0_f32;
        // Border that matches the Media Library menu: the same themed window
        // stroke colour (set in apply_theme), a crisp 1px line painted just inside
        // the window edge (see below).
        let border = egui::Stroke::new(1.0_f32, ctx.style().visuals.window_stroke.color);
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
            // Always on top: the EQ/playlist/library windows are *owned* by the main
            // window (group_child_windows), which keeps them above sibling windows,
            // and clicking one re-raises it — so only a system-topmost popup stays
            // reliably above them. A modal dialog opened from a menu item would then
            // sit under the menu, so dialog actions hide the menu first (see
            // `hide_popup_window`).
            .with_window_level(egui::WindowLevel::AlwaysOnTop);
        let id = egui::ViewportId::from_hash_of("linamp_popup");
        ctx.show_viewport_immediate(id, builder, |ctx, _| {
            let style = ctx.style();
            egui::CentralPanel::default()
                .frame(
                    // Rounded fill lining up with the window's rounded region (see
                    // `round_popup_window`). No stroke here — a frame stroke sits on
                    // the window edge and gets clipped by the region, so the border
                    // is painted just inside the edge below instead.
                    egui::Frame::popup(&style)
                        .stroke(egui::Stroke::NONE)
                        .rounding(corner)
                        .shadow(egui::epaint::Shadow::NONE),
                )
                .show(ctx, |ui| {
                    // Route every classic menu through the shared `menu_item` look
                    // (frameless, themed) so the options/fly-out menus match the
                    // playlist and library context menus.
                    let item = |ui: &mut egui::Ui, label: &str| menu_item(ui, label, "");
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
                            if item(ui, "Bookmarks…") {
                                // Classic popups aren't nested; open the list once
                                // this menu closes.
                                self.pending_popup = Some((Popup::Bookmarks, owner, pos));
                                close = true;
                            }
                            // The Minibrowser is an opt-in "Optional Features" extra.
                            if self.prefs.minibrowser_enabled {
                                let mbl = if self.mb_visible {
                                    "Hide Minibrowser"
                                } else {
                                    "Show Minibrowser"
                                };
                                if item(ui, mbl) {
                                    self.mb_visible = !self.mb_visible;
                                    close = true;
                                }
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
                            if item(ui, "Remove missing files") {
                                let n = self.playlist.remove_missing();
                                self.pl_selection.clear();
                                self.status = format!("Removed {n} missing file(s).");
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
                            if item(ui, "Sort by filename") {
                                sort(self, SortKey::Filename);
                                close = true;
                            }
                            if item(ui, "Sort by path") {
                                sort(self, SortKey::Path);
                                close = true;
                            }
                            if item(ui, "Randomize list") {
                                self.playlist.randomize();
                                self.pl_selection.clear();
                                close = true;
                            }
                            if item(ui, "Reverse list") {
                                self.playlist.reverse();
                                self.pl_selection.clear();
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
                            // Target the right-clicked row (falling back to the
                            // selection's first) so Play / Edit act on what the user
                            // actually clicked, even within a multi-selection.
                            let target = self
                                .pl_ctx_row
                                .or_else(|| self.pl_selection.iter().min().copied());
                            if menu_item(ui, "Play", "Enter") {
                                if let Some(i) = target {
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
                                match target {
                                    Some(i) => self.open_tag_editor_at(i),
                                    None => self.open_tag_editor(),
                                }
                                close = true;
                            }
                            if menu_item(ui, "Explore folder", "") {
                                if let Some(t) = target.and_then(|i| self.playlist.tracks.get(i)) {
                                    open_in_file_manager(&t.path);
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
                            if menu_item(ui, "Sort by filename", "") {
                                sort(self, SortKey::Filename);
                                close = true;
                            }
                            if menu_item(ui, "Sort by path", "") {
                                sort(self, SortKey::Path);
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
                            // Snapshot the saved library so clicks can borrow self
                            // mutably without aliasing the list.
                            let saved: Vec<(usize, String)> = self
                                .eq_presets
                                .items
                                .iter()
                                .enumerate()
                                .map(|(i, p)| (i, p.name.clone()))
                                .collect();
                            egui::ScrollArea::vertical().show(ui, |ui| {
                                for (i, (name, _, _)) in crate::eq::PRESETS.iter().enumerate() {
                                    if item(ui, name) {
                                        self.apply_eq_preset(i);
                                        close = true;
                                    }
                                }
                                if !saved.is_empty() {
                                    ui.separator();
                                    for (i, name) in &saved {
                                        if item(ui, name) {
                                            self.load_named_eq_preset(*i);
                                            close = true;
                                        }
                                    }
                                }
                                ui.separator();
                                if item(ui, "Save current as…") {
                                    self.eq_save_name = Some(String::new());
                                    close = true;
                                }
                                if !saved.is_empty() && item(ui, "Delete a saved preset…") {
                                    // Classic popups aren't nested; open the delete
                                    // list once this menu closes.
                                    self.pending_popup = Some((Popup::EqDelete, owner, pos));
                                    close = true;
                                }
                                if item(ui, "Import preset (.eqf)…") {
                                    self.import_eqf();
                                    close = true;
                                }
                                if item(ui, "Export preset (.eqf)…") {
                                    self.export_eqf();
                                    close = true;
                                }
                            });
                        }
                        Popup::EqDelete => {
                            let saved: Vec<(usize, String)> = self
                                .eq_presets
                                .items
                                .iter()
                                .enumerate()
                                .map(|(i, p)| (i, p.name.clone()))
                                .collect();
                            if saved.is_empty() {
                                menu_item(ui, "(no saved presets)", "");
                            } else {
                                egui::ScrollArea::vertical().show(ui, |ui| {
                                    // Delete highest index first so earlier indices
                                    // stay valid; one click deletes one preset.
                                    for (i, name) in &saved {
                                        if item(ui, &format!("Delete: {name}")) {
                                            self.delete_named_eq_preset(*i);
                                            close = true;
                                        }
                                    }
                                });
                            }
                        }
                        Popup::StopMenu => {
                            if item(ui, "Stop") {
                                self.stop_playback();
                                close = true;
                            }
                            let after = if self.stop_after_current { "on" } else { "off" };
                            if item(ui, &format!("Stop after current: {after}")) {
                                self.stop_after_current = !self.stop_after_current;
                                close = true;
                            }
                            if item(ui, "Stop with fade out") {
                                if self.player.state() != PlayState::Stopped {
                                    self.fade = Some(FADE_SECS);
                                    self.status = "Fading out…".into();
                                }
                                close = true;
                            }
                        }
                        Popup::Bookmarks => {
                            // Snapshot (index, title) so the click handler can borrow
                            // self mutably to play without aliasing the list.
                            let list: Vec<(usize, String)> = self
                                .bookmarks
                                .items
                                .iter()
                                .enumerate()
                                .map(|(i, b)| (i, b.title.clone()))
                                .collect();
                            if list.is_empty() {
                                menu_item(ui, "(no bookmarks)", "");
                            } else {
                                egui::ScrollArea::vertical().show(ui, |ui| {
                                    for (i, title) in &list {
                                        if item(ui, title) {
                                            self.play_bookmark(*i);
                                            close = true;
                                        }
                                    }
                                });
                            }
                            ui.separator();
                            // Toggle: offer to remove the current track's bookmark
                            // when it's already bookmarked, else to add it.
                            if self.current_is_bookmarked() {
                                if item(ui, "Remove current track from bookmarks") {
                                    self.remove_current_bookmark();
                                    close = true;
                                }
                            } else if item(ui, "Add current track (Alt+I)") {
                                self.add_bookmark();
                                close = true;
                            }
                            if item(ui, "Clear bookmarks") {
                                self.clear_bookmarks();
                                close = true;
                            }
                        }
                    }
                });
            // Outline just inside the window edge — a stroke centred on the edge
            // has its inner half render invisibly on low-contrast skins once the
            // rounded region clips the outer half, so inset the whole 1px line one
            // point in (rounding one less than the region's) where it can't be
            // clipped, giving the same distinct border the Media Library's menu has.
            let vr = ctx.screen_rect().shrink(1.0);
            ctx.layer_painter(egui::LayerId::new(
                egui::Order::Foreground,
                egui::Id::new("linamp_menu_border"),
            ))
            .rect_stroke(vr, egui::Rounding::same(corner - 1.0), border);
            self.apply_base_cursor(ctx, "LinAmp Menu");
            // Escape closes the menu. The popup is a separate always-on-top OS
            // window that can hold focus, in which case `handle_window_input`
            // (which only runs for the classic windows) never sees the key — so
            // the popup viewport must handle Escape itself.
            if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
                close = true;
            }
        });

        // Round the window's corners to match the frame's rounding (scaled to
        // physical pixels for the current DPI). Applied once per popup — retried
        // each frame only until it succeeds — rather than rebuilt every frame.
        let radius = (corner * ctx.pixels_per_point()).round() as i32;
        if !self.popup_rounded && round_popup_window(radius, self.popup_fresh) {
            self.popup_rounded = true;
        }
        // On X11 the popup window is reused and resized between menus (and, when the
        // menu is switched rapidly, destroyed and recreated), while XGetGeometry can
        // briefly report the previous size. Re-apply every frame (size/id-cached, so
        // it's a no-op once stable) and force a reshape on the opening frame, so a
        // recreated window never keeps a square corner.
        #[cfg(all(unix, not(target_os = "macos")))]
        {
            let _ = round_popup_window(radius, self.popup_fresh);
        }

        self.popup_fresh = false;
        if close {
            self.popup = None;
            // Chain to a queued popup (e.g. Options → Bookmarks) now this one is gone.
            if let Some((k, w, p)) = self.pending_popup.take() {
                self.open_popup_in(k, w, p);
            }
        }
    }

    pub(super) fn open_popup_in(&mut self, kind: Popup, win: ClWin, local: egui::Pos2) {
        self.popup = Some((kind, win, local));
        self.popup_fresh = true;
        // New popup window (or a resize to a different menu): re-apply the rounded
        // region on the next frame.
        self.popup_rounded = false;
        // Opening a classic popup dismisses an open Media Library menu, so the two
        // menu systems are never up at once.
        self.classic_clicked = true;
    }
}
