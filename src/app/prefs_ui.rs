//! The Preferences window UI (the `Ctrl+P` window): tabs, drag, and each page.
//! Split out of app.rs as methods on `LinAmpApp`; behavior is unchanged.

use super::*;

impl super::LinAmpApp {
    pub(super) fn ui_prefs(&mut self, ctx: &egui::Context) {
        if !self.prefs_open {
            // Re-anchor to the main window the next time it opens, and re-apply the
            // rounded corner region then (the OS window is destroyed on close).
            self.prefs_pos = None;
            self.prefs_rounded = None;
            self.prefs_raise = false;
            return;
        }
        // Render in its own real window so it is never clipped by the (small)
        // borderless main window. Borderless with a skin-drawn title bar (like the
        // classic windows and the Library) so the skin cursor covers the whole
        // window, including the title bar, with no OS chrome to break it.
        // On first open, anchor to the main window (top-left + 24) but keep the whole
        // window on the virtual desktop, the same bump the classic windows get, so it is
        // never off-screen when the player sits near the right or bottom edge. A position
        // the user has since dragged (prefs_pos is Some) is left alone: off-screen is
        // allowed once they move it there themselves.
        let (pos, first_open) = match self.prefs_pos {
            Some(p) => (p, false),
            None => {
                let size = self.prefs_size.unwrap_or(PREFS_DEFAULT_SIZE);
                let anchored = self.geom.win_pos[0] + egui::vec2(24.0, 24.0);
                let p = clamp_pos_onscreen(ctx, anchored, size);
                self.prefs_pos = Some(p);
                (p, true)
            }
        };
        let mut builder = egui::ViewportBuilder::default()
            .with_title("LinAmp Preferences")
            // A bit larger than before so the longest page (Conversion) fits without
            // scrolling and the grouped category tree (whose "General Preferences"
            // heading is wider than the old flat labels) has room. Kept well under the
            // combined Main/EQ/PL/Library footprint at 1x (the rule of thumb is to stay
            // under ~95% of the open dashboard, so it never dwarfs the player).
            .with_inner_size(self.prefs_size.unwrap_or(PREFS_DEFAULT_SIZE))
            .with_decorations(false)
            .with_resizable(false)
            .with_taskbar(false)
            // Always on top while open, whatever the global always-on-top setting, so
            // clicking another window never buries the open Preferences dialog. It is a
            // standalone window (not one of main's owned children), so topmost keeps it
            // above the player and Library windows. Selecting Preferences again also
            // raises it (see `prefs_raise`), covering the rare case of two topmost
            // windows where a sibling can still come forward.
            .with_window_level(egui::WindowLevel::AlwaysOnTop);
        // Pin the position only on first open (clamped on-screen above). After that the
        // window manager owns the position: the title bar hands off to an interactive
        // move (StartDrag), which the user can drag partly off-screen, and the real
        // position is read back from the viewport each frame below. Re-asserting
        // with_position every frame would fight the move and pin it back on-screen.
        if first_open {
            builder = builder.with_position(pos);
        }
        let id = egui::ViewportId::from_hash_of("linamp_prefs");
        // When the current skin ships general-purpose window art (GEN.BMP/GENEX.BMP),
        // frame Preferences with the skin's own composited window exactly like the Media
        // Library, so the two read as one themed pair. Otherwise fall back to the flat
        // skin-coloured title bar and border (the look before this change), so a skin
        // without the parts still gets a legible, on-theme dialog.
        let gen = self.ui_mode == UiMode::Classic
            && self.classic.as_ref().map(|c| c.has_gen()).unwrap_or(false);
        let mut close = false;
        ctx.show_viewport_immediate(id, builder, |ctx, _| {
            // Raise and focus the window when Preferences was just (re)selected, so it
            // comes back to the front if it had slipped behind another window.
            if self.prefs_raise {
                ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                self.prefs_raise = false;
            }
            // Track this window's focus so the app stays "focused" (and the GEN frame
            // shows its active variant) while Preferences is the frontmost window, the
            // same way the Library feeds `app_focused`. Read next frame by
            // `render_classic` (children render after the aggregate is computed).
            self.geom.foc_prefs = ctx
                .input(|i| i.viewport().focused)
                .unwrap_or(self.geom.foc_prefs);
            // The window manager moves the window during a title-bar drag (StartDrag),
            // so the app is no longer the position authority. Read the real outer
            // position back each frame (egui logical points, the same space as
            // prefs_pos) so the resize grip and the next reopen use the window's current
            // location. Skip the very first frame, whose position is the one we just
            // asked for and which the compositor may not have reported yet.
            if !first_open {
                if let Some(r) = ctx.input(|i| i.viewport().outer_rect) {
                    self.prefs_pos = Some(r.min);
                }
            }
            if gen {
                self.ui_prefs_gen(ctx, &mut close);
            } else {
                self.ui_prefs_fallback(ctx, &mut close);
            }
            // Bottom-right resize grip: grow from the default size up to twice it in
            // each dimension. The navigation pane keeps its fixed width (egui gives the
            // extra room to the open page), and Preferences neither docks nor has
            // columns to redistribute.
            let cur = ctx.screen_rect().size();
            let grip_col = self.skin_window_bg();
            let grip = resize_grip(
                ctx,
                egui::Rect::from_min_size(self.prefs_pos.unwrap_or(pos), cur),
                &mut self.prefs_resize,
                PREFS_DEFAULT_SIZE,
                PREFS_DEFAULT_SIZE * 2.0,
                grip_col,
                RESIZE_GRIP,
            );
            if let Some(ns) = grip.size {
                // Resize through the viewport builder (its `with_inner_size` is read
                // from `prefs_size` at the top of the next frame and applied by eframe
                // before the UI is drawn, so the paint always matches the window size).
                // Sending a ViewportCommand::InnerSize here instead would resize the OS
                // window a second time, out of sync with the builder and the paint,
                // leaving the newly exposed band unpainted on every frame of the drag
                // (the "trails" artifact).
                self.prefs_size = Some(ns);
                // The rounded-corner region is sized to the window; re-apply it next frame.
                self.prefs_rounded = None;
            }
            self.apply_base_cursor(ctx, "LinAmp Preferences");
            // Show the skin's resize cursor over the grip (and through an off-window
            // drag). Preferences does not dock, so there is nothing to snap to.
            if grip.active {
                self.set_resize_cursor(ctx, "LinAmp Preferences");
            }
            if ctx.input(|i| i.viewport().close_requested() || i.key_pressed(egui::Key::Escape)) {
                close = true;
            }
        });
        // Round the OS window's corners to match the painted border, scaled to physical
        // pixels for the current DPI. Only the flat fallback is rounded; the GEN frame
        // paints its own square classic corners, so there the window stays rectangular.
        if gen {
            self.prefs_rounded = None;
        } else {
            let corner = 6.0_f32;
            let ppp = ctx.pixels_per_point();
            if self.prefs_rounded != Some(ppp) {
                let radius = (corner * ppp).round() as i32;
                if platform_win::round_popup_window("LinAmp Preferences", radius, true) {
                    self.prefs_rounded = Some(ppp);
                } else {
                    ctx.request_repaint();
                }
            }
        }
        if close {
            self.prefs_open = false;
        }
    }

    /// Preferences framed by the skin's GEN.BMP window (title bar with the caption in
    /// the skin's own font, side walls, bottom bar and close button), mirroring the
    /// Media Library's `ui_library_gen`. The content is inset into the frame interior.
    fn ui_prefs_gen(&mut self, ctx: &egui::Context, close: &mut bool) {
        // Shared app-level focus, so Preferences doesn't dim merely because another
        // LinAmp window was clicked (matches the Library).
        let active = self.geom.app_focused;
        // The frame insets (native px). Preferences is a fixed-size dialog drawn 1:1 in
        // points, so the insets are used directly as points (no per-window scaling like
        // the Library, whose window zooms with the player).
        let (top, right, bottom, left) = classic::ClassicSkin::gen_insets();
        let (top, right, bottom, left) = (top as f32, right as f32, bottom as f32, left as f32);
        egui::CentralPanel::default()
            .frame(egui::Frame::none())
            .show(ctx, |ui| {
                let full = ui.max_rect();
                let (w, h) = (full.width().round() as u32, full.height().round() as u32);
                // Close button (top-right corner of the frame). Interact before composing
                // so the pressed sprite can be baked into the frame texture. Armed on a
                // press over it, it keeps the pressed sprite for the whole press even if
                // the cursor moves off, and fires on release only if the cursor is still
                // over it, matching the classic windows and the Library.
                let close_rect = egui::Rect::from_min_max(
                    egui::pos2(full.right() - 20.0, full.top() + 3.0),
                    egui::pos2(full.right() - 6.0, full.top() + 17.0),
                );
                let (p_pressed, p_released, p_down, p_pos) = ui.input(|i| {
                    (
                        i.pointer.primary_pressed(),
                        i.pointer.primary_released(),
                        i.pointer.primary_down(),
                        i.pointer.latest_pos(),
                    )
                });
                let over_close = p_pos.is_some_and(|p| close_rect.contains(p));
                if p_pressed && over_close {
                    self.prefs_close_armed = true;
                }
                let close_pressed = self.prefs_close_armed && p_down;
                if p_released {
                    if self.prefs_close_armed && over_close {
                        *close = true;
                    }
                    self.prefs_close_armed = false;
                }
                // (Re)compose and cache the frame texture, now including the press state.
                let sig = (self.theme.name.clone(), active, close_pressed, w, h);
                if self.prefs_tex_sig.as_ref() != Some(&sig) {
                    if let Some(img) = self.classic.as_ref().and_then(|c| {
                        c.compose_gen_window(w, h, active, close_pressed, "LinAmp Preferences")
                    }) {
                        let color = egui::ColorImage::from_rgba_unmultiplied(
                            [w as usize, h as usize],
                            img.as_raw(),
                        );
                        self.prefs_tex = Some(ctx.load_texture(
                            "prefs_gen",
                            color,
                            egui::TextureOptions::NEAREST,
                        ));
                        self.prefs_tex_sig = Some(sig);
                    }
                }
                if let Some(tex) = &self.prefs_tex {
                    let uv = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
                    ui.painter().image(tex.id(), full, uv, Color32::WHITE);
                }
                // Drag the title bar (minus the close corner) to move the window.
                let drag_rect = egui::Rect::from_min_max(
                    full.min,
                    egui::pos2(full.right() - 24.0, full.top() + top),
                );
                let dresp = ui.interact(
                    drag_rect,
                    ui.id().with("prefs_gen_drag"),
                    egui::Sense::click_and_drag(),
                );
                // Hand the move to the window manager (interactive _NET_WM_MOVERESIZE via
                // StartDrag) so the window can be dragged partly off-screen. A programmatic
                // OuterPosition move is clamped back on-screen by the compositor, which is
                // why app-driven dragging could never leave the edge. The real position is
                // read back from the viewport each frame in `ui_prefs`.
                if dresp.drag_started() {
                    ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
                }
                // Inset the content into the frame interior.
                let content = egui::Rect::from_min_max(
                    egui::pos2(full.left() + left, full.top() + top),
                    egui::pos2(full.right() - right, full.bottom() - bottom),
                );
                ui.allocate_new_ui(egui::UiBuilder::new().max_rect(content), |ui| {
                    self.ui_prefs_body(ctx, ui, close);
                });
            });
    }

    /// Preferences framed by the flat skin-coloured title bar and a 2px border, used
    /// when the skin has no GEN.BMP window art. This is the look before the skinned
    /// frame existed, kept as the fallback. Mirrors the Library's fallback title bar.
    fn ui_prefs_fallback(&mut self, ctx: &egui::Context, close: &mut bool) {
        let win_bg = self.skin_window_bg();
        let nav_list_fill = self.skin_nav_colors().0;
        let corner = 6.0_f32;
        egui::CentralPanel::default()
            .frame(egui::Frame::none().fill(nav_list_fill))
            .show(ctx, |ui| {
                self.ui_prefs_flat_titlebar(ctx, ui, win_bg, close);
                self.ui_prefs_body(ctx, ui, close);
            });
        // Themed border just inside the window edge, matching the rounded corner region
        // applied in `ui_prefs`. Inset one point so the whole 2px line stays inside the
        // region (a line centred on the edge would have its outer half clipped at the
        // corners). Same approach as the classic menus (#108).
        let vr = ctx.screen_rect().shrink(1.0);
        ctx.layer_painter(egui::LayerId::new(
            egui::Order::Foreground,
            egui::Id::new("linamp_prefs_border"),
        ))
        .rect_stroke(
            vr,
            egui::Rounding::same(corner - 1.0),
            egui::Stroke::new(2.0_f32, win_bg),
        );
    }

    /// The flat fallback title bar: the skin window-background colour, the caption on
    /// the left, and a boxed × on the right. Drag it to move the window (skip when over
    /// the ×). Drawn into `ui` so it sits above the shared content body.
    fn ui_prefs_flat_titlebar(
        &mut self,
        ctx: &egui::Context,
        ui: &mut egui::Ui,
        win_bg: Color32,
        close: &mut bool,
    ) {
        let (bar, bar_resp) = ui.allocate_exact_size(
            egui::vec2(ui.available_width(), 20.0),
            egui::Sense::click_and_drag(),
        );
        let txt = crate::skin::contrast_text(win_bg);
        ui.painter().rect_filled(bar, 0.0, win_bg);
        ui.painter().text(
            bar.left_center() + egui::vec2(6.0, 0.0),
            egui::Align2::LEFT_CENTER,
            "LinAmp Preferences",
            egui::FontId::proportional(13.0),
            txt,
        );
        let x_rect = egui::Rect::from_min_size(
            egui::pos2(bar.right() - 18.0, bar.center().y - 7.0),
            egui::vec2(14.0, 14.0),
        );
        let x_resp = ui.interact(x_rect, ui.id().with("prefs_close"), egui::Sense::click());
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
        // Drag starts only off the × box. Hand the move to the window manager so the
        // window can go partly off-screen (same StartDrag path as the GEN title bar; the
        // real position is read back each frame in `ui_prefs`).
        if bar_resp.drag_started() && !x_resp.hovered() {
            ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
        }
    }

    /// The Preferences content: the category tree, the selected page, and the footer.
    /// Laid out with `show_inside` so it sits inside either the flat fallback window or
    /// the GEN frame interior, painted on the Media Library nav palette so it matches
    /// the library whatever skin is applied.
    fn ui_prefs_body(&mut self, ctx: &egui::Context, ui: &mut egui::Ui, close: &mut bool) {
        let (nav_list_fill, nav_row_col, nav_sel_bg, nav_sel_col, nav_dash) =
            self.skin_nav_colors();
        // Left: the category tree, a list-fill background with skin item text, the skin
        // selection colours, and a dashed gray tree guide under each group. Categories
        // sit under bold group headings, indented beneath them; a None heading renders
        // its categories at the top level with no guide.
        let nav_frame = egui::Frame::none()
            .fill(nav_list_fill)
            .inner_margin(egui::Margin::same(4.0));
        egui::SidePanel::left("prefs_cats")
            .resizable(false)
            .exact_width(160.0)
            .frame(nav_frame)
            .show_inside(ui, |ui| {
                ui.add_space(2.0);
                ui.visuals_mut().selection.bg_fill = nav_sel_bg;
                ui.visuals_mut().selection.stroke = egui::Stroke::NONE;
                // Flush the selectable rows to the left margin so top-level nodes line
                // up with the headings.
                ui.spacing_mut().button_padding.x = 0.0;
                let font = egui::FontId::proportional(13.0);
                let groups: &[(Option<&str>, &[PrefTab])] = &[
                    (
                        Some("General Preferences"),
                        &[PrefTab::Display, PrefTab::Playback, PrefTab::Playlist, PrefTab::Tags],
                    ),
                    (Some("Media Library"), &[PrefTab::Conversion]),
                    (None, &[PrefTab::OptionalFeatures, PrefTab::About]),
                ];
                for (heading, tabs) in groups {
                    let indent = heading.is_some();
                    if let Some(h) = heading {
                        ui.add_space(2.0);
                        ui.label(
                            egui::RichText::new(*h)
                                .strong()
                                .color(nav_row_col)
                                .font(font.clone()),
                        );
                    }
                    let mut rects: Vec<egui::Rect> = Vec::with_capacity(tabs.len());
                    for &tab in *tabs {
                        let selected = self.prefs_tab == tab;
                        let txt = if selected { nav_sel_col } else { nav_row_col };
                        // Leading space clears the gutter for the painted guide on
                        // indented children; top-level rows start flush.
                        let mut job = egui::text::LayoutJob::default();
                        job.append(
                            tab.label(),
                            if indent { 15.0 } else { 0.0 },
                            egui::text::TextFormat {
                                font_id: font.clone(),
                                color: txt,
                                ..Default::default()
                            },
                        );
                        let resp = ui.selectable_label(selected, job);
                        if resp.clicked() {
                            self.prefs_tab = tab;
                        }
                        if indent {
                            rects.push(resp.rect);
                        }
                    }
                    if indent && !rects.is_empty() {
                        library_ui::paint_nav_tree(ui.painter(), &rects, nav_dash, 1.0);
                    }
                }
            });
        // Bottom: Close on the left, a compact version on the right (always visible,
        // whichever category is open). Painted on the nav's list fill so the footer
        // matches the category tree.
        egui::TopBottomPanel::bottom("prefs_footer")
            .frame(egui::Frame::side_top_panel(&ctx.style()).fill(nav_list_fill))
            .show_inside(ui, |ui| {
                style_prefs_panel(ui, nav_list_fill, nav_row_col, nav_sel_bg, nav_sel_col);
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    if ui.button("Close").clicked() {
                        *close = true;
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        // Keep the version label clear of the bottom-right resize grip.
                        ui.add_space(RESIZE_GRIP - 4.0);
                        ui.label(
                            egui::RichText::new(format!("LinAmp {}", env!("CARGO_PKG_VERSION")))
                                .small()
                                .weak(),
                        );
                    });
                });
                ui.add_space(4.0);
            });
        // Right: the selected category's page, in a scroll area so a category can grow
        // tall without the fixed window clipping it. Each arm just dispatches to a
        // `prefs_page_*` method, see `pref_section` for the pattern that keeps every
        // page consistent as options are added.
        egui::CentralPanel::default()
            .frame(egui::Frame::central_panel(&ctx.style()).fill(nav_list_fill))
            .show_inside(ui, |ui| {
                style_prefs_panel(ui, nav_list_fill, nav_row_col, nav_sel_bg, nav_sel_col);
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| match self.prefs_tab {
                        PrefTab::Display => self.prefs_page_display(ctx, ui),
                        PrefTab::Playback => self.prefs_page_playback(ui),
                        PrefTab::Conversion => self.prefs_page_conversion(ui),
                        PrefTab::Playlist => self.prefs_page_playlist(ui),
                        PrefTab::Tags => self.prefs_page_tags(ui),
                        PrefTab::OptionalFeatures => self.prefs_page_optional_features(ui),
                        PrefTab::About => self.prefs_page_about(ui),
                    });
            });
    }

    /// One titled group of related settings, drawn with uniform spacing so every
    /// Preferences page reads the same however many groups it grows.
    ///
    /// Extending the Preferences window:
    /// * add a setting: drop a widget into the relevant `pref_section` below;
    /// * add a group: another `pref_section(ui, "Title", |ui| { … })`;
    /// * add a category: a [`PrefTab`] variant (wired into `ALL` + `label`)
    ///   and a matching `prefs_page_*` method dispatched from `ui_prefs`.
    fn pref_section(ui: &mut egui::Ui, title: &str, add: impl FnOnce(&mut egui::Ui)) {
        ui.add_space(2.0);
        ui.label(egui::RichText::new(title).strong());
        ui.add_space(2.0);
        add(ui);
        ui.add_space(12.0);
    }

    fn prefs_page_display(&mut self, ctx: &egui::Context, ui: &mut egui::Ui) {
        Self::pref_section(ui, "Window size", |ui| {
            let mut dbl = self.prefs.double_size;
            ui.radio_value(&mut dbl, false, "1x, pixel-for-pixel");
            ui.radio_value(&mut dbl, true, "2x, scaled for modern screens (Ctrl+D)");
            if dbl != self.prefs.double_size {
                self.set_double_size(ctx, dbl);
            }
        });
        Self::pref_section(ui, "Window", |ui| {
            let mut aot = self.prefs.always_on_top;
            if ui.checkbox(&mut aot, "Always on top").changed() {
                self.prefs.always_on_top = aot;
                self.prefs.save();
            }
            let mut disable_cur = self.prefs.disable_skin_cursors;
            if ui
                .checkbox(&mut disable_cur, "Disable skin cursors")
                .changed()
            {
                self.prefs.disable_skin_cursors = disable_cur;
                self.prefs.save();
                // Drop the custom cursors immediately when turning the option off;
                // re-enabling reapplies them as windows are hovered.
                if disable_cur {
                    self.clear_skin_cursors();
                } else {
                    self.render.cur_cursor = [None; 3];
                }
            }
        });
        Self::pref_section(ui, "Icon", |ui| {
            let mut dark = self.prefs.icon_dark_mode;
            if ui.checkbox(&mut dark, "Dark Mode").changed() {
                self.set_icon_dark_mode(ctx, dark);
            }
            // On Linux a GNOME dock draws the taskbar/launcher icon from a cached theme
            // icon it only refreshes on a session restart. Show the caveat only when it
            // actually applies: Normal (taskbar) mode, and the choice differs from the
            // colour installed at startup. Windows updates its taskbar button live.
            #[cfg(target_os = "linux")]
            if !self.prefs.tray_enabled && self.prefs.icon_dark_mode != self.icon_dark_mode_applied
            {
                ui.add_space(4.0);
                ui.weak("Requires a restart of LinAmp and Gnome Panel to take effect.");
            }
        });
        Self::pref_section(ui, "System tray", |ui| {
            let mut tray = self.prefs.tray_enabled;
            ui.radio_value(&mut tray, false, "Normal: (Taskbar)");
            ui.radio_value(&mut tray, true, "System Tray Icon");
            if tray != self.prefs.tray_enabled {
                self.set_window_mode(ctx, tray);
            }
            // The tray behaviour note is only relevant when tray mode is selected.
            if tray {
                ui.add_space(4.0);
                ui.weak("In system tray mode, close will minimize to the system tray. Exit from the tray icon or select exit from the main menu to close.");
            }
        });
    }

    fn prefs_page_optional_features(&mut self, ui: &mut egui::Ui) {
        ui.add_space(2.0);
        ui.colored_label(
            egui::Color32::from_rgb(200, 150, 60),
            "These are obsolete or incomplete features included for \"full parity\". \
             They may not function exactly as expected, enable at your own risk.",
        );
        ui.add_space(10.0);
        Self::pref_section(ui, "Minibrowser", |ui| {
            let mut on = self.prefs.minibrowser_enabled;
            if ui.checkbox(&mut on, "Enable the Minibrowser").changed() {
                self.prefs.minibrowser_enabled = on;
                if !on {
                    self.mb_visible = false;
                }
                self.prefs.save();
            }
            ui.add_enabled_ui(self.prefs.minibrowser_enabled, |ui| {
                ui.label("Address (opens in your default browser):");
                let mut url = self.prefs.minibrowser_url.clone();
                if ui
                    .add(egui::TextEdit::singleline(&mut url).desired_width(260.0))
                    .changed()
                {
                    self.prefs.minibrowser_url = url;
                    self.prefs.save();
                }
            });
        });
        Self::pref_section(ui, "Numeric keypad", |ui| {
            let mut on = self.prefs.numpad_transport;
            if ui
                .checkbox(&mut on, "Bind the numeric keypad to transport controls")
                .changed()
            {
                self.prefs.numpad_transport = on;
                self.prefs.save();
            }
        });
    }

    fn prefs_page_playback(&mut self, ui: &mut egui::Ui) {
        Self::pref_section(ui, "On stop or pause", |ui| {
            // Only genuine, persistent settings belong in Preferences. One-shot
            // actions ("Stop after current", Ctrl+V) and view toggles ("time
            // remaining", Ctrl+T) keep their shortcuts but are deliberately not
            // shown here, a settings checkbox implies it sticks, and they don't.
            let mut fade = self.prefs.fade_on_stop;
            let mut changed = ui
                .checkbox(&mut fade, "Fade out when stopping or pausing (Shift+V to force)")
                .changed();
            let mut secs = self.prefs.fade_on_stop_secs;
            ui.add_enabled_ui(fade, |ui| {
                if ui
                    .add(egui::Slider::new(&mut secs, 1.0..=12.0).suffix(" s"))
                    .changed()
                {
                    changed = true;
                }
            });
            if changed {
                self.prefs.fade_on_stop = fade;
                self.prefs.fade_on_stop_secs = secs;
                self.prefs.save();
            }
        });

        Self::pref_section(ui, "Crossfade", |ui| {
            // Overlaps the end of one track with the start of the next. Off by
            // default (the classic player's default is a gapless cut).
            let mut on = self.prefs.crossfade;
            let mut changed = ui
                .checkbox(&mut on, "Crossfade between tracks")
                .changed();
            let mut secs = self.prefs.crossfade_secs;
            ui.add_enabled_ui(on, |ui| {
                if ui
                    .add(egui::Slider::new(&mut secs, 1.0..=12.0).suffix(" s"))
                    .changed()
                {
                    changed = true;
                }
            });
            if changed {
                self.prefs.crossfade = on;
                self.prefs.crossfade_secs = secs;
                self.prefs.save();
                self.apply_crossfade_pref();
            }
        });
    }

    /// The rip / convert settings page. Format first, then Bitrate and Sample rate as
    /// dropdowns whose options depend on the chosen format (bitrate only for the lossy
    /// formats; Opus is fixed at 48 kHz), then the output folder. These are global and
    /// applied to every conversion for now.
    fn prefs_page_conversion(&mut self, ui: &mut egui::Ui) {
        use crate::encode::OutputFormat;
        ui.add_space(2.0);
        ui.weak("Default for Audio Encoding.");
        ui.add_space(10.0);

        Self::pref_section(ui, "Format", |ui| {
            let mut fmt = self.prefs.convert_format;
            egui::ComboBox::from_id_salt("convert_format")
                .selected_text(fmt.display())
                .show_ui(ui, |ui| {
                    for &f in OutputFormat::ALL {
                        ui.selectable_value(&mut fmt, f, f.display());
                    }
                });
            if fmt != self.prefs.convert_format {
                self.prefs.convert_format = fmt;
                // Keep the bitrate valid for the newly chosen format.
                if fmt.is_lossy() {
                    self.prefs.convert_bitrate_kbps =
                        fmt.clamp_bitrate(self.prefs.convert_bitrate_kbps);
                }
                self.prefs.save();
            }
        });

        let fmt = self.prefs.convert_format;

        Self::pref_section(ui, "Bitrate", |ui| {
            if fmt.is_lossy() {
                let mut br = fmt.clamp_bitrate(self.prefs.convert_bitrate_kbps);
                egui::ComboBox::from_id_salt("convert_bitrate")
                    .selected_text(format!("{br} kbps"))
                    .show_ui(ui, |ui| {
                        for &p in fmt.bitrate_presets() {
                            ui.selectable_value(&mut br, p, format!("{p} kbps"));
                        }
                    });
                if br != self.prefs.convert_bitrate_kbps {
                    self.prefs.convert_bitrate_kbps = br;
                    self.prefs.save();
                }
            } else {
                ui.weak(format!("{} is lossless; bitrate does not apply.", fmt.display()));
            }
        });

        Self::pref_section(ui, "Sample rate", |ui| {
            if fmt == OutputFormat::Opus {
                ui.weak("Opus always encodes at 48000 Hz.");
            } else {
                let mut rate = self.prefs.convert_sample_rate;
                let label = match rate {
                    Some(r) => format!("{r} Hz"),
                    None => "Same as source".to_string(),
                };
                egui::ComboBox::from_id_salt("convert_rate")
                    .selected_text(label)
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut rate, None, "Same as source");
                        for &r in fmt.sample_rate_presets() {
                            ui.selectable_value(&mut rate, Some(r), format!("{r} Hz"));
                        }
                    });
                if rate != self.prefs.convert_sample_rate {
                    self.prefs.convert_sample_rate = rate;
                    self.prefs.save();
                }
            }
        });

        Self::pref_section(ui, "Output folder", |ui| {
            ui.horizontal(|ui| {
                if ui.button("Browse...").clicked() {
                    if let Some(dir) = self.pick_dialog(|| rfd::FileDialog::new().pick_folder()) {
                        self.prefs.convert_dir = Some(dir);
                        self.prefs.save();
                    }
                }
                if self.prefs.convert_dir.is_some() && ui.button("Clear").clicked() {
                    self.prefs.convert_dir = None;
                    self.prefs.save();
                }
            });
            ui.add_space(2.0);
            let cur = self
                .prefs
                .convert_dir
                .as_ref()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| "Not set; you will be asked when you convert.".to_string());
            ui.weak(cur);
        });
    }

    fn prefs_page_playlist(&mut self, ui: &mut egui::Ui) {
        Self::pref_section(ui, "ID3 tag loading", |ui| {
            let mut onload = self.prefs.tag_on_load;
            ui.radio_value(&mut onload, true, "On load, scan when files are added");
            ui.radio_value(&mut onload, false, "On view, lazily as rows scroll in");
            if onload != self.prefs.tag_on_load {
                self.prefs.tag_on_load = onload;
                self.prefs.save();
                if onload {
                    self.playlist.load_all();
                }
            }
        });
        Self::pref_section(ui, "Open with... (system menu)", |ui| {
            let mut mode = self.prefs.open_with;
            ui.radio_value(
                &mut mode,
                crate::config::OpenWith::Enqueue,
                "Enqueue: add opened files to the current playlist",
            );
            ui.radio_value(
                &mut mode,
                crate::config::OpenWith::Clear,
                "Clear queue: replace the playlist with the opened files",
            );
            if mode != self.prefs.open_with {
                self.prefs.open_with = mode;
                self.prefs.save();
            }
        });
    }

    fn prefs_page_tags(&mut self, ui: &mut egui::Ui) {
        Self::pref_section(ui, "ID3v2 version on write", |ui| {
            let mut v23 = self.prefs.id3v23;
            ui.radio_value(&mut v23, true, "ID3v2.3 (most compatible)");
            ui.radio_value(&mut v23, false, "ID3v2.4 (newer standard)");
            if v23 != self.prefs.id3v23 {
                self.prefs.id3v23 = v23;
                self.prefs.save();
            }
            ui.add_space(4.0);
            ui.weak("Both handled on read");
        });
    }

    fn prefs_page_about(&mut self, ui: &mut egui::Ui) {
        Self::pref_section(ui, "LinAmp", |ui| {
            ui.label(format!("Version {}", env!("CARGO_PKG_VERSION")));
            ui.label(
                egui::RichText::new(format!("build {} - {}", env!("GIT_HASH"), env!("COMMIT_DATE")))
                    .small()
                    .color(Color32::GRAY),
            );
            ui.add_space(6.0);
            ui.hyperlink_to(
                "github.com/cjjulius/linamp",
                "https://github.com/cjjulius/linamp",
            );
        });
    }
}

/// Bump a window's top-left so a window of `size` sits fully on the virtual desktop
/// (all monitors). Falls back to the given position if the desktop bounds are unknown.
fn clamp_pos_onscreen(ctx: &egui::Context, pos: egui::Pos2, size: egui::Vec2) -> egui::Pos2 {
    match platform_win::virtual_desktop_rect(ctx) {
        Some(vr) => clamp_pos_into(pos, size, vr),
        None => pos,
    }
}

/// Keep a `size`-sized window at `pos` within `bounds`: pull it left or up when it
/// would spill off the right or bottom edge, but always keep the top-left corner
/// visible (so a window larger than `bounds` is pinned to the top-left rather than
/// hidden). Mirrors the clamp the classic windows use.
fn clamp_pos_into(pos: egui::Pos2, size: egui::Vec2, bounds: egui::Rect) -> egui::Pos2 {
    let max_x = (bounds.max.x - size.x).max(bounds.min.x);
    let max_y = (bounds.max.y - size.y).max(bounds.min.y);
    egui::pos2(pos.x.clamp(bounds.min.x, max_x), pos.y.clamp(bounds.min.y, max_y))
}

#[cfg(test)]
mod tests {
    use super::clamp_pos_into;
    use egui::{pos2, vec2, Rect};

    fn desktop() -> Rect {
        Rect::from_min_size(pos2(0.0, 0.0), vec2(1920.0, 1080.0))
    }

    #[test]
    fn onscreen_position_is_unchanged() {
        let p = pos2(200.0, 150.0);
        assert_eq!(clamp_pos_into(p, vec2(520.0, 360.0), desktop()), p);
    }

    #[test]
    fn offscreen_right_and_bottom_bump_back_in() {
        // Player near the right/bottom edge: the window would spill off, so it is
        // pulled in to sit flush against the right/bottom edge.
        let got = clamp_pos_into(pos2(1700.0, 900.0), vec2(520.0, 360.0), desktop());
        assert_eq!(got, pos2(1920.0 - 520.0, 1080.0 - 360.0));
    }

    #[test]
    fn negative_position_bumps_to_the_top_left() {
        let got = clamp_pos_into(pos2(-40.0, -40.0), vec2(520.0, 360.0), desktop());
        assert_eq!(got, pos2(0.0, 0.0));
    }

    #[test]
    fn window_larger_than_desktop_keeps_top_left_visible() {
        let got = clamp_pos_into(pos2(500.0, 500.0), vec2(4000.0, 4000.0), desktop());
        assert_eq!(got, pos2(0.0, 0.0));
    }
}
