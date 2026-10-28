//! Classic right-click / options popup menus (borderless, always-on-top
//! windows composited by egui).

use super::*;
use super::library_ui::{DynRow, LibAction};

/// Popup menus in classic mode (options + the playlist bottom-bar fly-outs).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
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
    /// The Media Library context menu (artist / album / track). Its rows are
    /// built per click and carried in `PopupState.dynamic`; this is the library
    /// half of the menu system, consolidated onto the classic popup window (#116).
    LibMenu,
    /// The Rate flyout opened from the track menu, acting on
    /// `PopupState.rate_rows`. Chained from `LibMenu` like `EqDelete` is from
    /// `EqPresets` (classic popups are single, never nested windows).
    LibRate,
}

/// The action a fixed popup-menu row performs when clicked. Pairing each row
/// with its action in one list (`fixed_menu_entries`) lets the same list drive
/// the drawing, the window height and the test, so they can never drift (#154).
#[derive(Clone, Copy)]
pub(super) enum MenuAct {
    OpenFiles,
    OpenFolder,
    RemoveSelected,
    Crop,
    RemoveMissing,
    RemoveAll,
    SelectAll,
    SelectNone,
    InvertSelection,
    Sort(SortKey),
    Randomize,
    Reverse,
    NewList,
    SaveList,
    LoadList,
    PlayTarget,
    PlayNext,
    EditTagsTarget,
    ExploreTarget,
    Stop,
    ToggleStopAfter,
    StopFade,
}

/// One row of a fixed popup menu: a clickable item (label + dimmed shortcut +
/// its action) or a separator. The single source of truth for a fixed menu's
/// layout.
pub(super) enum MenuEntry {
    Item { label: String, shortcut: &'static str, act: MenuAct },
    Sep,
}

/// The rows of a fixed popup menu, in order, or `None` for the dynamic menus
/// (Options / EqPresets / EqDelete / Bookmarks) which size themselves from
/// runtime state in `draw_popup_viewport`. `stop_after_current` only affects the
/// Stop menu's toggle label, not the row count. This one list drives the draw
/// loop, the height in `popup_dims` and the layout test.
pub(super) fn fixed_menu_entries(kind: Popup, stop_after_current: bool) -> Option<Vec<MenuEntry>> {
    use MenuAct::*;
    let it = |label: &str, shortcut: &'static str, act: MenuAct| MenuEntry::Item {
        label: label.to_string(),
        shortcut,
        act,
    };
    let sorts = || {
        vec![
            it("Sort by title", "", Sort(SortKey::Title)),
            it("Sort by artist", "", Sort(SortKey::Artist)),
            it("Sort by album", "", Sort(SortKey::Album)),
            it("Sort by duration", "", Sort(SortKey::Duration)),
            it("Sort by filename", "", Sort(SortKey::Filename)),
            it("Sort by path", "", Sort(SortKey::Path)),
        ]
    };
    Some(match kind {
        Popup::PlAdd => vec![
            it("Add file(s)…", "", OpenFiles),
            it("Add folder…", "", OpenFolder),
        ],
        Popup::PlRem => vec![
            it("Remove selected", "", RemoveSelected),
            it("Crop (keep selected)", "", Crop),
            it("Remove missing files", "", RemoveMissing),
            it("Remove all", "", RemoveAll),
        ],
        Popup::PlSel => vec![
            it("Select all", "", SelectAll),
            it("Select none", "", SelectNone),
            it("Invert selection", "", InvertSelection),
        ],
        Popup::PlMisc => {
            let mut v = sorts();
            v.push(it("Randomize list", "", Randomize));
            v.push(it("Reverse list", "", Reverse));
            v
        }
        Popup::PlList => vec![
            it("New list", "", NewList),
            it("Save list…", "", SaveList),
            it("Load list…", "", LoadList),
        ],
        Popup::PlTrack => vec![
            it("Play", "Enter", PlayTarget),
            it("Play Next", "", PlayNext),
            MenuEntry::Sep,
            it("Edit file tags…", "Alt+3", EditTagsTarget),
            it("Explore folder", "", ExploreTarget),
            MenuEntry::Sep,
            it("Remove", "Del", RemoveSelected),
            it("Crop (keep selected)", "", Crop),
        ],
        Popup::PlVoid => {
            let mut v = vec![
                it("Add file(s)…", "Ins", OpenFiles),
                it("Add folder…", "Shift+L", OpenFolder),
                MenuEntry::Sep,
                it("Select all", "Ctrl+A", SelectAll),
                it("Select none", "", SelectNone),
                it("Invert selection", "", InvertSelection),
                MenuEntry::Sep,
            ];
            v.extend(sorts());
            v.push(MenuEntry::Sep);
            v.push(it("Remove all", "", RemoveAll));
            v.push(MenuEntry::Sep);
            v.push(it("Save list…", "", SaveList));
            v.push(it("Load list…", "", LoadList));
            v
        }
        Popup::StopMenu => vec![
            it("Stop", "", Stop),
            it(
                &format!("Stop after current: {}", if stop_after_current { "on" } else { "off" }),
                "",
                ToggleStopAfter,
            ),
            it("Stop with fade out", "", StopFade),
        ],
        _ => return None,
    })
}

/// `(items, separators)` in an entry list.
fn count_entries(entries: &[MenuEntry]) -> (i32, i32) {
    let items = entries.iter().filter(|e| matches!(e, MenuEntry::Item { .. })).count() as i32;
    let seps = entries.iter().filter(|e| matches!(e, MenuEntry::Sep)).count() as i32;
    (items, seps)
}

/// Height (points) one item row costs in a popup menu: the 18px item (see
/// `menu_item`) plus the 2px inter-row spacing set on the popup.
const MENU_ROW_H: f32 = 20.0;
/// Height one separator row costs: egui's 6px separator plus the 2px spacing.
const MENU_SEP_H: f32 = 8.0;
/// Inner padding (points) on each edge of a popup menu's content frame.
const MENU_MARGIN: f32 = 6.0;

/// Window height for a popup with `buttons` item rows and `seps` separators. The
/// rows/seps already include one inter-row gap each, so the fixed term is the
/// two margins (2 × MENU_MARGIN) minus the one gap that the last row doesn't have.
fn popup_height(buttons: i32, seps: i32) -> f32 {
    buttons as f32 * MENU_ROW_H + seps as f32 * MENU_SEP_H + (2.0 * MENU_MARGIN - 2.0)
}

/// Window size (points) for a popup menu, from its item/separator count. The
/// counts must match what the matching arm in `draw_popup_viewport` draws, or the
/// window leaves an empty band (too many) or clips the last rows (too few) (#154).
pub(super) fn popup_dims(kind: Popup) -> (f32, f32) {
    // Width per kind; the right-click context menus are wider for the shortcut
    // column, the Stop/EqDelete menus a little wider, Bookmarks widest.
    let width = match kind {
        Popup::PlTrack | Popup::PlVoid => 210.0,
        Popup::StopMenu | Popup::EqDelete => 200.0,
        Popup::Bookmarks => 240.0,
        _ => 182.0,
    };
    // Fixed menus take their height from the same entry list the draw loop and
    // the test use, so the three can't drift (#154). The dynamic menus
    // (Options / EqPresets / Bookmarks / EqDelete) recompute their height from
    // runtime item counts in draw_popup_viewport; these are only fallbacks.
    let height = if let Some(entries) = fixed_menu_entries(kind, false) {
        let (items, seps) = count_entries(&entries);
        popup_height(items, seps)
    } else {
        let (buttons, seps) = match kind {
            Popup::Options => (13, 3),
            Popup::EqPresets => (crate::eq::PRESETS.len() as i32 + 2, 1),
            Popup::Bookmarks => (2, 1),
            Popup::EqDelete => (1, 0),
            _ => (0, 0),
        };
        popup_height(buttons, seps)
    };
    (width, height)
}

/// Widest menu label in points at the shared menu font, plus the row padding
/// (`menu_item` insets the label 8px from each edge); `arrow_col` reserves room
/// for a submenu indicator on the right. Used to size the dynamic library menus,
/// whose labels vary per click, before their window is built.
fn menu_text_width<'a>(
    ctx: &egui::Context,
    labels: impl Iterator<Item = &'a str>,
    arrow_col: bool,
) -> f32 {
    let font = egui::FontId::proportional(crate::app::MENU_FONT_PT);
    let mut wmax = 0.0_f32;
    for l in labels {
        let w = ctx.fonts(|f| {
            f.layout_no_wrap(l.to_string(), font.clone(), egui::Color32::WHITE).size().x
        });
        wmax = wmax.max(w);
    }
    wmax + 8.0 + 8.0 + MENU_MARGIN * 2.0 + if arrow_col { 16.0 } else { 0.0 }
}

/// A context-menu row: `label` on the left, dimmed `shortcut` on the right, full
/// width so shortcuts align to the menu's right edge. Returns true when clicked.
/// Colours match an egui `Button` (and so the Media Library's context menus), 
/// the themed override text colour and the standard hover highlight, so every
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
    // One shared size for every menu, including the native tray menu.
    let font = egui::FontId::proportional(crate::app::MENU_FONT_PT);
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
    /// Run a fixed popup-menu action. Returns true (every fixed-menu click closes
    /// its menu). The right-click actions resolve their target the same way the
    /// old inline arms did: the right-clicked row, else the selection's first.
    fn menu_dispatch(&mut self, act: MenuAct) -> bool {
        use MenuAct::*;
        let target = || {
            self.plview.ctx_row.or_else(|| self.plview.selection.iter().min().copied())
        };
        match act {
            OpenFiles => self.open_files(),
            OpenFolder => self.open_folder(),
            RemoveSelected => self.pl_remove(false),
            Crop => self.pl_remove(true),
            RemoveMissing => {
                let n = self.playlist.remove_missing();
                self.plview.selection.clear();
                self.status = format!("Removed {n} missing file(s).");
            }
            RemoveAll => {
                self.playlist.clear();
                self.plview.selection.clear();
                self.player.stop();
            }
            SelectAll => self.pl_select_all(),
            SelectNone => self.plview.selection.clear(),
            InvertSelection => self.pl_invert_selection(),
            Sort(k) => {
                self.sort_key = k;
                self.sort_playlist();
            }
            Randomize => {
                self.playlist.randomize();
                self.plview.selection.clear();
            }
            Reverse => {
                self.playlist.reverse();
                self.plview.selection.clear();
            }
            NewList => {
                self.playlist.clear();
                self.plview.selection.clear();
                self.player.stop();
            }
            SaveList => self.save_playlist_dialog(),
            LoadList => self.load_playlist_dialog(),
            PlayTarget => {
                if let Some(i) = target() {
                    self.play_index(i);
                }
            }
            PlayNext => self.play_next_selected(),
            EditTagsTarget => match target() {
                Some(i) => self.open_tag_editor_at(i),
                None => self.open_tag_editor(),
            },
            ExploreTarget => {
                if let Some(t) = target().and_then(|i| self.playlist.tracks.get(i)) {
                    open_in_file_manager(&t.path);
                }
            }
            Stop => self.stop_playback(),
            ToggleStopAfter => self.stop_after_current = !self.stop_after_current,
            StopFade => {
                if self.player.state() != PlayState::Stopped {
                    self.fade = Some((self.prefs.fade_on_stop_secs, FadeEnd::Stop));
                    self.status = "Fading out…".into();
                }
            }
        }
        true
    }

    /// Render the open popup menu in its own borderless window (so it is never
    /// clipped by the small main/playlist windows). Positioned at the click.
    pub(super) fn draw_popup_viewport(&mut self, ctx: &egui::Context) {
        let Some((kind, anchor)) = self.popups.cur else {
            return;
        };
        let (w, h) = match kind {
            Popup::LibMenu => {
                // Built per click: size from the stored rows, and widen to fit the
                // longest (per-track) label so an artist/album name is never clipped.
                let rows = self.popups.dynamic.as_deref().unwrap_or(&[]);
                let items = rows
                    .iter()
                    .filter(|r| !matches!(r, DynRow::Sep))
                    .count() as i32;
                let seps = rows.iter().filter(|r| matches!(r, DynRow::Sep)).count() as i32;
                let labels = rows.iter().filter_map(|r| match r {
                    DynRow::Item { label, .. } | DynRow::RateSub { label } => Some(label.as_str()),
                    DynRow::Sep => None,
                });
                let w = menu_text_width(ctx, labels, true).clamp(180.0, 360.0);
                (w, popup_height(items, seps))
            }
            Popup::LibRate => {
                // Five star rows plus "Clear rating".
                (160.0, popup_height(6, 0))
            }
            Popup::Options => {
                // 11 fixed items + the optional Minibrowser toggle + the "Open Audio
                // CD" item (Linux + Windows) (#160), 3 separators. Computed here (not
                // in popup_dims) so the height tracks the conditional items instead of
                // clipping the last row when one shows.
                let buttons = 11
                    + i32::from(self.prefs.minibrowser_enabled)
                    + i32::from(cfg!(any(target_os = "linux", target_os = "windows")));
                (182.0, popup_height(buttons, 3))
            }
            Popup::Bookmarks => {
                // Grows with the bookmark count (capped so a long list scrolls rather
                // than running off-screen): the list, a separator, then Add + Clear.
                let rows = self.bookmarks.items.len().clamp(1, 12) as i32;
                (240.0, popup_height(rows + 2, 1))
            }
            Popup::EqPresets => {
                // Built-in presets + the saved library + up to 4 actions (Save,
                // optional Delete, Import, Export). Capped height; the arm scrolls.
                let actions = if self.eq.presets.items.is_empty() { 3 } else { 4 };
                let rows = (crate::eq::PRESETS.len() + self.eq.presets.items.len()) as i32 + actions;
                let seps = if self.eq.presets.items.is_empty() { 1 } else { 2 };
                (182.0, popup_height(rows, seps).min(420.0))
            }
            Popup::EqDelete => {
                let rows = self.eq.presets.items.len().clamp(1, 12) as i32;
                (200.0, popup_height(rows, 0))
            }
            _ => popup_dims(kind),
        };
        let mut screen = anchor;
        // Keep the menu fully on the desktop: a tall menu (example: the empty-area
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
        // Media Library menu's smooth 6px round, 8 makes them match visually.
        let corner = 8.0_f32;
        // Border that matches the Media Library menu: the same themed window
        // stroke colour (set in apply_theme), a crisp 1px line painted just inside
        // the window edge (see below).
        let border = egui::Stroke::new(1.0_f32, ctx.style().visuals.window_stroke.color);
        let builder = egui::ViewportBuilder::default()
            .with_title("LinAmp Menu")
            .with_position(screen)
            .with_inner_size(egui::vec2(w, h))
            .with_decorations(false)
            .with_resizable(false)
            .with_taskbar(false)
            // The OS window is created at a default spot and only then moved to
            // `screen`, which flashes a frame at the wrong place. Keep it hidden on
            // its opening frame (while it gets positioned/sized) and reveal it next
            // frame, so the menu only ever appears where it was clicked.
            .with_visible(!self.popups.fresh)
            // Always on top: the EQ/playlist/library windows are *owned* by the main
            // window (group_child_windows), which keeps them above sibling windows,
            // and clicking one re-raises it, so only a system-topmost popup stays
            // reliably above them. A modal dialog opened from a menu item would then
            // sit under the menu, so dialog actions hide the menu first (see
            // `hide_popup_window`).
            .with_window_level(egui::WindowLevel::AlwaysOnTop);
        let id = egui::ViewportId::from_hash_of(("linamp_popup", self.popups.serial));
        ctx.show_viewport_immediate(id, builder, |ctx, _| {
            let style = ctx.style();
            egui::CentralPanel::default()
                .frame(
                    // Rounded fill lining up with the window's rounded region (see
                    // `round_popup_window`). No stroke here, a frame stroke sits on
                    // the window edge and gets clipped by the region, so the border
                    // is painted just inside the edge below instead.
                    egui::Frame::popup(&style)
                        .stroke(egui::Stroke::NONE)
                        .rounding(corner)
                        .shadow(egui::epaint::Shadow::NONE)
                        .inner_margin(egui::Margin::same(MENU_MARGIN)),
                )
                .show(ctx, |ui| {
                    // Tighten the gap between rows so the menu hugs its items like the
                    // Media Library's menus, instead of reading a little long (#154).
                    // Keep this in step with MENU_ROW_H (18px item + this spacing).
                    ui.spacing_mut().item_spacing.y = 2.0;
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
                            if item(ui, "Open location… (Ctrl+L)") {
                                self.open_location();
                                close = true;
                            }
                            // Audio-CD reading: Linux (verified) and Windows
                            // (best-effort, pending hardware verification) (#160).
                            #[cfg(any(target_os = "linux", target_os = "windows"))]
                            if item(ui, "Open Audio CD") {
                                self.open_audio_cd();
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
                            // Always on top lives on the clutterbar "A" button, the
                            // Ctrl+A shortcut and Preferences, so it isn't repeated
                            // in this menu.
                            if item(ui, "Bookmarks…") {
                                // Classic popups aren't nested; open the list once
                                // this menu closes.
                                self.popups.pending = Some((Popup::Bookmarks, anchor));
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
                                // force_quit makes the close handler quit even in
                                // tray mode (where an X would otherwise hide).
                                self.force_quit = true;
                                ctx.send_viewport_cmd_to(
                                    egui::ViewportId::ROOT,
                                    egui::ViewportCommand::Close,
                                );
                                close = true;
                            }
                        }
                        Popup::PlAdd
                        | Popup::PlRem
                        | Popup::PlSel
                        | Popup::PlMisc
                        | Popup::PlList
                        | Popup::PlTrack
                        | Popup::PlVoid
                        | Popup::StopMenu => {
                            // Fixed menus: one entry list (fixed_menu_entries)
                            // drives the rows, the window height and the layout
                            // test, so they can't drift (#154).
                            if let Some(entries) =
                                fixed_menu_entries(kind, self.stop_after_current)
                            {
                                for e in entries {
                                    match e {
                                        MenuEntry::Sep => {
                                            ui.separator();
                                        }
                                        MenuEntry::Item { label, shortcut, act } => {
                                            if menu_item(ui, &label, shortcut) {
                                                close = self.menu_dispatch(act);
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        Popup::EqPresets => {
                            // Snapshot the saved library so clicks can borrow self
                            // mutably without aliasing the list.
                            let saved: Vec<(usize, String)> = self
                                .eq.presets
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
                                    self.eq.save_name = Some(String::new());
                                    close = true;
                                }
                                if !saved.is_empty() && item(ui, "Delete a saved preset…") {
                                    // Classic popups aren't nested; open the delete
                                    // list once this menu closes.
                                    self.popups.pending = Some((Popup::EqDelete, anchor));
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
                                .eq.presets
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
                            // Pin the actions to the bottom so a long list can never
                            // push Add/Remove/Clear off the window; the list scrolls in
                            // the space above them.
                            egui::TopBottomPanel::bottom("bm_actions")
                                .frame(egui::Frame::none())
                                .show_inside(ui, |ui| {
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
                                });
                            egui::CentralPanel::default()
                                .frame(egui::Frame::none())
                                .show_inside(ui, |ui| {
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
                                });
                        }
                        Popup::LibMenu => {
                            // The Media Library context menu: data-driven rows built
                            // at the click (per-track names, the live selection). Take
                            // the chosen action out first, then apply it after the
                            // borrow on the row list ends.
                            let rows = self.popups.dynamic.take().unwrap_or_default();
                            let mut chosen: Option<LibAction> = None;
                            let mut open_rate = false;
                            for r in &rows {
                                match r {
                                    DynRow::Sep => {
                                        ui.separator();
                                    }
                                    DynRow::Item { label, act } => {
                                        if menu_item(ui, label, "") {
                                            chosen = Some(act.clone());
                                            close = true;
                                        }
                                    }
                                    DynRow::RateSub { label } => {
                                        // Right-pointing triangle marks the one nested
                                        // menu; clicking chains to the Rate flyout.
                                        if menu_item(ui, label, "\u{25B8}") {
                                            open_rate = true;
                                            close = true;
                                        }
                                    }
                                }
                            }
                            // Put the rows back so a non-closing frame keeps drawing them.
                            if !close {
                                self.popups.dynamic = Some(rows);
                            }
                            if let Some(a) = chosen {
                                self.apply_lib_action(a);
                            }
                            if open_rate {
                                self.popups.pending = Some((Popup::LibRate, anchor));
                            }
                        }
                        Popup::LibRate => {
                            let rows = self.popups.rate_rows.clone();
                            for s in 1..=5i64 {
                                if menu_item(ui, &"\u{2605}".repeat(s as usize), "") {
                                    self.apply_lib_action(LibAction::Rate(rows.clone(), s));
                                    close = true;
                                }
                            }
                            if menu_item(ui, "Clear rating", "") {
                                self.apply_lib_action(LibAction::Rate(rows.clone(), 0));
                                close = true;
                            }
                        }
                    }
                });
            // Outline just inside the window edge, a stroke centred on the edge
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
            // (which only runs for the classic windows) never sees the key, so
            // the popup viewport must handle Escape itself.
            if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
                close = true;
            }
        });

        // Round the window's corners to match the frame's rounding (scaled to
        // physical pixels for the current DPI). Applied once per popup, retried
        // each frame only until it succeeds, rather than rebuilt every frame.
        let radius = (corner * ctx.pixels_per_point()).round() as i32;
        // Rebuild the region when first opened, and if a different-sized menu opens
        // into the same serial, so the rounded corners track the window size.
        let size_changed = self.popups.rounded_h != Some(h);
        if (!self.popups.rounded || size_changed) && round_popup_window(radius, self.popups.fresh || size_changed) {
            self.popups.rounded = true;
            self.popups.rounded_h = Some(h);
        }
        // On X11 the popup window is reused and resized between menus (and, when the
        // menu is switched rapidly, destroyed and recreated), while XGetGeometry can
        // briefly report the previous size. Re-apply every frame (size/id-cached, so
        // it's a no-op once stable) and force a reshape on the opening frame, so a
        // recreated window never keeps a square corner.
        #[cfg(all(unix, not(target_os = "macos")))]
        {
            let _ = round_popup_window(radius, self.popups.fresh);
            // Always-on-top isn't enough on X11: the main window's transient-for
            // children (EQ/playlist/library) can stack over the popup, so it opens
            // behind them. Raise it each frame while it's open to keep it visible.
            raise_popup_window();
        }

        if self.popups.fresh {
            // The opening frame drew the menu hidden (see `with_visible` above);
            // request the next frame now so it's revealed immediately rather than
            // waiting on the next input event.
            ctx.request_repaint();
        }
        self.popups.fresh = false;
        if close {
            self.popups.cur = None;
            self.popups.dynamic = None;
            // Chain to a queued popup (example: Options then Bookmarks, or the
            // track menu then its Rate flyout) now this one is gone.
            if let Some((k, p)) = self.popups.pending.take() {
                self.open_popup_at(k, p);
            }
        }
    }

    /// Open a classic popup anchored at a click inside a classic window, given in
    /// that window's local (viewport) coordinates.
    pub(super) fn open_popup_in(&mut self, kind: Popup, win: ClWin, local: egui::Pos2) {
        let anchor = self.geom.win_pos[win_idx(win)] + local.to_vec2();
        self.open_popup_at(kind, anchor);
    }

    /// Open a popup at an absolute screen position. The library menus anchor here
    /// directly (their window is outside the classic `win_pos` array).
    pub(super) fn open_popup_at(&mut self, kind: Popup, anchor: egui::Pos2) {
        self.popups.cur = Some((kind, anchor));
        self.popups.fresh = true;
        // New popup window: re-apply the rounded region on the next frame.
        self.popups.rounded = false;
        self.popups.rounded_h = None;
        // Fresh viewport id per open, so the menu never inherits a reused window's
        // size on a re-right-click (#154).
        self.popups.serial = self.popups.serial.wrapping_add(1);
    }

    /// Open the data-driven Media Library context menu at an absolute screen
    /// position, with the rows to draw and the track rows the Rate flyout targets.
    pub(super) fn open_lib_menu(
        &mut self,
        anchor: egui::Pos2,
        rows: Vec<DynRow>,
        rate_rows: Vec<usize>,
    ) {
        self.popups.dynamic = Some(rows);
        self.popups.rate_rows = rate_rows;
        self.open_popup_at(Popup::LibMenu, anchor);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn popup_height_formula() {
        // Row = 18px item + 2px gap; separator = 6px + 2px gap; plus the two 6px
        // margins minus the one trailing gap the last row doesn't have.
        assert_eq!(popup_height(1, 0), 20.0 + 10.0);
        assert_eq!(popup_height(0, 1), 8.0 + 10.0);
        assert_eq!(popup_height(14, 5), 14.0 * 20.0 + 5.0 * 8.0 + 10.0);
    }

    #[test]
    fn fixed_menu_sizes_match_their_draw_arms() {
        // The fixed menus are drawn, sized and tested from one entry list
        // (fixed_menu_entries), so the window can't clip the last rows (too few)
        // or leave an empty band (too many), the #154 regression. These are the
        // known-good layouts; a menu change updates fixed_menu_entries and this
        // assertion together, and the draw loop and height follow automatically.
        for (kind, items, seps) in [
            (Popup::PlAdd, 2, 0),
            (Popup::PlRem, 4, 0),
            (Popup::PlSel, 3, 0),
            (Popup::PlMisc, 8, 0),
            (Popup::PlList, 3, 0),
            (Popup::PlTrack, 6, 2),
            // 14 items and 4 separators. popup_dims used to claim 5 separators
            // while the draw arm drew 4, so this menu rendered ~8px too tall with
            // an empty band; driving both from one list fixes that (the #154 class).
            (Popup::PlVoid, 14, 4),
            (Popup::StopMenu, 3, 0),
        ] {
            let entries = fixed_menu_entries(kind, false).expect("fixed menu has entries");
            assert_eq!(count_entries(&entries), (items, seps), "{kind:?} layout changed");
            // popup_dims derives height from the same list the draw loop consumes.
            let (_, h) = popup_dims(kind);
            assert_eq!(h, popup_height(items, seps), "{kind:?} height mismatch");
        }
        // The dynamic menus size themselves from runtime state, not a fixed list.
        for kind in [Popup::Options, Popup::EqPresets, Popup::EqDelete, Popup::Bookmarks] {
            assert!(fixed_menu_entries(kind, false).is_none(), "{kind:?} should be dynamic");
        }
    }
}
