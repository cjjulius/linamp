//! The Media Library: 3-pane browser, its themed window frame and the
//! background scan/refresh plumbing.

use egui::{Color32, RichText};

use super::*;

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

impl super::LinAmpApp {
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
    pub(super) fn refresh_library(&mut self) {
        let q = self.lib_query.trim().to_string();
        // Bookmarks are backed by the store, not the DB, so this view works even
        // without a library; the search box filters it in place.
        if self.lib_nav == crate::library::LibView::Bookmarks {
            self.refresh_bookmarks_view(&q);
            return;
        }
        if self.library.is_none() {
            return;
        }
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

    /// Build the Bookmarks view from the bookmark store. Each bookmark shows its
    /// full library metadata when the track is indexed, otherwise just its saved
    /// title and path (so bookmarks to un-scanned files still list and play). An
    /// optional query filters by title or path.
    fn refresh_bookmarks_view(&mut self, query: &str) {
        self.lib_artists.clear();
        self.lib_albums.clear();
        self.lib_ratings.clear();
        let q = query.to_lowercase();
        let lib = self.library.as_ref();
        self.lib_results = self
            .bookmarks
            .items
            .iter()
            .filter(|b| {
                q.is_empty()
                    || b.title.to_lowercase().contains(&q)
                    || b.path.to_lowercase().contains(&q)
            })
            .map(|b| {
                let path = std::path::PathBuf::from(&b.path);
                lib.and_then(|l| l.track_for_path(&path)).unwrap_or_else(|| {
                    let mut t = crate::playlist::Track::new(path);
                    if !b.title.trim().is_empty() {
                        t.title = b.title.clone();
                    }
                    t
                })
            })
            .collect();
        self.lib_sel_tracks.clear();
        self.apply_lib_sort();
    }

    /// Lighter refresh for an artist/album selection change: the full artist list
    /// doesn't depend on the selection, so recompute only the album pane and the
    /// track list, skipping the artists() query (#96). Falls back to the full
    /// refresh outside the Audio nav view or during a search.
    pub(super) fn refresh_library_selection(&mut self) {
        let audio_no_search = self.lib_nav == crate::library::LibView::Audio
            && self.lib_query.trim().is_empty();
        let Some(lib) = self.library.as_ref().filter(|_| audio_no_search) else {
            self.refresh_library();
            return;
        };
        self.lib_albums = lib.albums(&self.lib_sel_artists);
        self.lib_results = lib.tracks_for(&self.lib_sel_artists, &self.lib_sel_albums);
        self.lib_sel_tracks.clear();
        self.apply_lib_sort();
    }

    /// Re-order `lib_results` (and the parallel `lib_ratings`) by the active column
    /// sort. The rating column sorts by the numeric star count, not the rendered
    /// star string (whose empty-star glyph would sort the wrong way).
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
    pub(super) fn poll_library_scan(&mut self) {
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

    /// The media library + search, in its own window (the classic UI has no
    /// built-in library view). Reuses the same list UI as the modern view.
    /// The Media Library: a borderless, themed window that docks to the player
    /// windows (via a custom title bar) but never window-shades.
    pub(super) fn ui_library_window(&mut self, ctx: &egui::Context) {
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
    /// behind the content, draw the caption and inset the browser into the frame.
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

    /// The 3-pane Media Library browser (nav tree, artist/album columns, track
    /// table, bottom Play/Enqueue bar), themed from the active skin. Shared by
    /// the docked window and the modern `View::Library` tab.
    pub(super) fn ui_library(&mut self, ui: &mut egui::Ui) {
        use crate::library::LibView;
        // Run a debounced search once the typing pause has elapsed (#96).
        if self.lib_search_at.is_some_and(|at| std::time::Instant::now() >= at) {
            self.lib_search_at = None;
            self.refresh_library();
        }
        // Mirror the classic popups' click-away for the library's egui menus: a
        // primary press on another window (or a classic popup opening) clears the
        // open flag, so the gate below stops attaching the menu and it closes. The
        // menu never gates on focus, so a right-click always opens it. Modern mode
        // is a single viewport whose menus close on their own.
        if self.classic_clicked {
            self.lib_menu_open = false;
        }
        self.classic_clicked = false;
        // Clicking anywhere in the Library window also dismisses an open classic
        // popup (playlist/options menu), the classic click-away only runs for the
        // classic windows, so it never sees clicks landing here.
        if ui.ctx().input(|i| i.pointer.any_pressed()) {
            self.popup = None;
        }
        let lib_menu_base = self.ui_mode != UiMode::Classic || self.lib_menu_open;
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
        // Cache the rating keyed on the selected path, so the star display doesn't
        // query SQLite on every repaint; recompute only when the selection changes.
        let cur_rating = if let Some(p) = &rating_target {
            let hit = self.lib_rating_cache.as_ref().is_some_and(|(cp, _)| cp == p);
            if !hit {
                let r = self.library.as_ref().map_or(0, |lib| lib.rating(p));
                self.lib_rating_cache = Some((p.clone(), r));
            }
            self.lib_rating_cache.as_ref().map_or(0, |(_, r)| *r)
        } else {
            0
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
                ui.add_space(6.0 * s);
                // A real, clickable view now (the same bookmarks as Options > Bookmarks).
                if ui
                    .selectable_label(
                        self.lib_nav == LibView::Bookmarks,
                        RichText::new("Bookmarks"),
                    )
                    .clicked()
                {
                    nav_click = Some(LibView::Bookmarks);
                }
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
                                            if resp.secondary_clicked() {
                                                self.popup = None;
                                                self.lib_menu_open = true;
                                                if !sel {
                                                    artist_click = Some((Some(name.clone()), false));
                                                }
                                            }
                                            if lib_menu_base || resp.secondary_clicked() {
                                            resp.context_menu(|ui| {
                                                // Keep the menu from squishing to a
                                                // sliver when opened near a screen edge.
                                                ui.set_min_width(170.0);
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
                                            if resp.secondary_clicked() {
                                                self.popup = None;
                                                self.lib_menu_open = true;
                                                if !sel {
                                                    album_click = Some((Some(name.clone()), false));
                                                }
                                            }
                                            if lib_menu_base || resp.secondary_clicked() {
                                            resp.context_menu(|ui| {
                                                ui.set_min_width(170.0);
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
                                    let msg = if self.lib_nav == LibView::Bookmarks {
                                        // Bookmarks live in their own store, so this
                                        // view is empty independently of the DB.
                                        if self.lib_query.trim().is_empty() {
                                            "No bookmarks yet. Press Alt+I while a track plays to add one."
                                        } else {
                                            "No bookmarks match your search."
                                        }
                                    } else if lib_empty {
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
                                            LibView::Bookmarks => "No bookmarks yet.",
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
                                    if resp.secondary_clicked() {
                                        // A library right-click dismisses any open
                                        // classic playlist/options popup, and marks
                                        // the library menu open for the gate.
                                        self.popup = None;
                                        self.lib_menu_open = true;
                                        if !selected {
                                            track_click = Some((i, false));
                                        }
                                    }
                                    if resp.double_clicked() {
                                        dbl_track = Some(i);
                                    }
                                    if lib_menu_base || resp.secondary_clicked() {
                                    resp.context_menu(|ui| {
                                        ui.set_min_width(180.0);
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
                                        let remove_label = if self.lib_nav == LibView::Bookmarks {
                                            "Remove bookmark"
                                        } else {
                                            "Remove from library"
                                        };
                                        if ui.button(remove_label).clicked() {
                                            lib_action = Some(LibAction::Remove(rows.clone()));
                                            ui.close_menu();
                                        }
                                    });
                                    }
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
            // Toggle direction on the same column; otherwise sort the new column, 
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
            self.lib_search_at = None; // cancel any pending debounced search
            self.refresh_library();
        }
        if search_changed {
            // Debounce: wait for a typing pause before the (un-indexable) LIKE
            // scan, so holding/typing fast runs one query, not one per keystroke.
            let delay = std::time::Duration::from_millis(150);
            self.lib_search_at = Some(std::time::Instant::now() + delay);
            ui.ctx().request_repaint_after(delay);
        }
        if let Some(v) = nav_click {
            self.lib_nav = v;
            self.lib_sel_artists.clear();
            self.lib_sel_albums.clear();
            self.lib_query.clear();
            self.lib_search_at = None; // nav refreshes now; drop any pending search
            self.lib_sort = None; // each view starts in its natural order
            self.refresh_library();
        }
        if let Some((name, ctrl)) = artist_click {
            apply_sel(&mut self.lib_sel_artists, name, ctrl);
            self.lib_sel_albums.clear();
            self.refresh_library_selection();
        }
        if let Some((name, ctrl)) = album_click {
            apply_sel(&mut self.lib_sel_albums, name, ctrl);
            self.refresh_library_selection();
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
            self.lib_rating_cache = None; // the selected track's rating changed
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
                    self.lib_rating_cache = None; // ratings changed
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
                    if self.lib_nav == LibView::Bookmarks {
                        // In the Bookmarks view "Remove" drops the bookmark, not the
                        // library row (the file stays indexed and on disk).
                        for p in &paths {
                            self.bookmarks.remove_path(&p.to_string_lossy());
                        }
                        self.bookmarks.save();
                    } else if let Some(lib) = &self.library {
                        lib.remove_paths(&paths);
                    }
                    self.lib_sel_tracks.clear();
                    self.refresh_library();
                }
            }
        }

        // Gather the target tracks for Play/Enqueue (selection, else all shown).
        // Built only when actually acting, so merely having the library open does
        // not clone the whole result set (up to a couple thousand tracks) a frame.
        let targets: Vec<crate::playlist::Track> = if !(do_play || do_enqueue) {
            Vec::new()
        } else if self.lib_sel_tracks.is_empty() {
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
