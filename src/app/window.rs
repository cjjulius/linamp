//! Classic multi-window geometry: per-window input, drag / dock / snap,
//! window-shade and double-size.

use super::*;

impl super::LinAmpApp {
    /// Per-window input in local (viewport) coordinates.
    /// The EQ hit-test region at base coords, shade-aware: the collapsed bar has
    /// its own volume/balance/close regions; the full window uses `eq_region_at`.
    fn eq_region(&self, x: f32, y: f32) -> Option<classic::EqRegion> {
        if self.geom.eq_shade {
            classic::eq_shade_region_at(x, y)
        } else {
            classic::eq_region_at(x, y)
        }
    }

    pub(super) fn handle_window_input(&mut self, ctx: &egui::Context, win: ClWin) {
        let outer = ctx.input(|i| i.viewport().outer_rect);
        let monitor = ctx.input(|i| i.viewport().monitor_size);
        let local = ctx.pointer_latest_pos();
        let screen_cursor = match (outer, local) {
            (Some(o), Some(l)) => Some(o.min + l.to_vec2()),
            _ => None,
        };
        let (pressed, released, down, secondary, scroll, mods, dbl_click) = ctx.input(|i| {
            (
                i.pointer.primary_pressed(),
                i.pointer.primary_released(),
                i.pointer.primary_down(),
                i.pointer.secondary_clicked(),
                i.raw_scroll_delta.y,
                i.modifiers,
                i.pointer.button_double_clicked(egui::PointerButton::Primary),
            )
        });
        // Platform-aware Ctrl (Command on macOS) for selection gestures.
        let sel_ctrl = mods.ctrl || mods.command;
        let sel_shift = mods.shift;
        let shaded = match win {
            ClWin::Main => self.geom.main_shade,
            ClWin::Eq => self.geom.eq_shade,
            ClWin::Pl => self.geom.pl_shade,
        };

        // Clicking any classic window dismisses an open menu (click-away). The
        // library's context menu is now a classic popup too, so this one path
        // closes whichever menu is up, including one opened from the library.
        if pressed && self.popups.cur.is_some() && !self.popups.fresh {
            self.popups.cur = None;
        }
        // Escape closes an open menu. The popup no longer steals focus, so its own
        // viewport won't see the key, the focused window catches it here instead.
        if self.popups.cur.is_some() && ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.popups.cur = None;
        }

        if secondary && win == ClWin::Main {
            if let Some(l) = local {
                // Right-click on the Stop button opens the stop options (classic
                // the classic player); anywhere else on the main window opens the Options menu.
                let (nx, ny) = (l.x / self.scale, l.y / self.scale);
                if !self.geom.main_shade && classic::region_at(nx, ny) == Some(Btn::Stop) {
                    self.open_popup_in(Popup::StopMenu, win, l);
                } else {
                    self.open_popup_in(Popup::Options, win, l);
                }
            }
        }

        // Right-click on the playlist list area: a track menu when a row is under
        // the cursor, otherwise the playlist-management menu for empty space.
        if secondary && win == ClWin::Pl && !shaded {
            if let Some(l) = local {
                let nx = l.x / self.scale;
                let ny = l.y / self.scale;
                if let Some(classic::PlRegion::List) =
                    classic::pl_region_at(nx, ny, self.plview.px_w, self.plview.px_h)
                {
                    let row = classic::pl_row_at(ny, self.plview.first);
                    if row < self.playlist.len() {
                        // the classic rule: right-clicking a row outside the current
                        // selection selects just it; a row already in the
                        // selection keeps the whole multi-selection.
                        if !self.plview.selection.contains(&row) {
                            self.plview.selection.clear();
                            self.plview.selection.insert(row);
                            self.plview.anchor = Some(row);
                            self.plview.lead = Some(row);
                        }
                        // Remember the exact row so Play / Edit target it even when
                        // it's one of several selected.
                        self.plview.ctx_row = Some(row);
                        self.open_popup_in(Popup::PlTrack, win, l);
                    } else {
                        self.open_popup_in(Popup::PlVoid, win, l);
                    }
                }
            }
        }

        if win == ClWin::Pl && !shaded && scroll != 0.0 {
            // Scrolling up is always free down to the top. Scrolling down stops with the
            // last song at the bottom (max_pl_first), but if the offset already sits past
            // that (blank space left by a resize or a delete), down holds it there rather
            // than snapping up; a few wheel-ups then ease it back into range.
            let visible = classic::pl_visible_rows(self.plview.px_h as u32);
            let max = classic::max_pl_first(self.playlist.len(), visible);
            let first = self.plview.first;
            self.plview.first = if scroll > 0.0 {
                first.saturating_sub(3)
            } else {
                (first + 3).min(max.max(first))
            };
        }

        // Mouse wheel over the main window adjusts volume, like the classic player
        // (scroll up louder, down quieter). The playlist uses the wheel to scroll;
        // the EQ window has no wheel action. Applies in shade mode too.
        if win == ClWin::Main && scroll != 0.0 {
            let v = wheel_volume(self.player.volume(), scroll, 0.05);
            self.player.set_volume(v);
        }

        // Double-click an EQ band or preamp to reset it to 0 dB (flat), like the classic player
        // (#140). Uses egui's own double-click detection (fires on the second
        // release), which is reliable regardless of frame timing.
        if dbl_click && win == ClWin::Eq && !shaded {
            if let Some(l) = local {
                let (nx, ny) = (l.x / self.scale, l.y / self.scale);
                if let Some(er @ (classic::EqRegion::Preamp | classic::EqRegion::Band(_))) =
                    classic::eq_region_at(nx, ny)
                {
                    self.reset_eq_slider(er);
                }
            }
        }

        // Double-click the balance slider to re-centre it (#140), as the classic player does.
        if dbl_click && win == ClWin::Main && !shaded {
            if let Some(l) = local {
                let (nx, ny) = (l.x / self.scale, l.y / self.scale);
                if classic::region_at(nx, ny) == Some(Btn::Balance) {
                    self.balance = 0.0;
                    self.player.set_balance(0.0);
                }
            }
        }

        if pressed {
            self.render.active_region = None;
            self.eq.active = None;
            if let Some(l) = local {
                let nx = l.x / self.scale;
                let ny = l.y / self.scale;
                if shaded {
                    use classic::ShadeHit as H;
                    // The collapsed EQ bar also carries the volume/balance sliders;
                    // handle those first (a press adjusts the value and sets
                    // eq_active so the drag follows) so they aren't swallowed by the
                    // bar's window-drag. Otherwise the generic shade controls apply.
                    let eq_slider = if win == ClWin::Eq {
                        match classic::eq_shade_region_at(nx, ny) {
                            Some(
                                er @ (classic::EqRegion::ShadeVolume
                                | classic::EqRegion::ShadeBalance),
                            ) => {
                                self.eq.active = Some(er);
                                self.apply_eq_shade_slider(er, nx);
                                true
                            }
                            _ => false,
                        }
                    } else {
                        false
                    };
                    if !eq_slider {
                        let hit = if win == ClWin::Main {
                            classic::main_shade_region_at(nx, ny)
                        } else {
                            classic::shade_region_at(nx, ny)
                        };
                        match hit {
                            Some(H::Close) => self.close_window(ctx, win),
                            Some(H::Restore) => self.toggle_shade(win),
                            Some(H::TitleBar) => self.start_drag(ctx, win, screen_cursor),
                            Some(H::Options) => self.open_popup_in(Popup::Options, win, l),
                            Some(H::Prev) => self.trigger_classic(ctx, Btn::Prev),
                            Some(H::Play) => self.trigger_classic(ctx, Btn::Play),
                            Some(H::Pause) => self.trigger_classic(ctx, Btn::Pause),
                            Some(H::Stop) => self.trigger_classic(ctx, Btn::Stop),
                            Some(H::Next) => self.trigger_classic(ctx, Btn::Next),
                            Some(H::Eject) => self.trigger_classic(ctx, Btn::Eject),
                            Some(H::Pos) => {
                                // Click/drag the mini-posbar to seek; reuses the
                                // expanded seek drag machinery (active_region).
                                self.render.active_region = Some(Btn::Seek);
                                self.apply_classic_slider(Btn::Seek, nx);
                            }
                            None => {}
                        }
                    }
                } else {
                    match win {
                        ClWin::Main => {
                            let r = classic::region_at(nx, ny);
                            self.render.active_region = r;
                            match r {
                                Some(Btn::TitleBar) => {
                                    self.render.active_region = None;
                                    self.start_drag(ctx, win, screen_cursor);
                                }
                                Some(b @ (Btn::Seek | Btn::Volume | Btn::Balance)) => {
                                    self.apply_classic_slider(b, nx)
                                }
                                _ => {}
                            }
                        }
                        ClWin::Eq => {
                            let r = self.eq_region(nx, ny);
                            self.eq.active = r;
                            match r {
                                Some(classic::EqRegion::TitleBar) => {
                                    self.eq.active = None;
                                    self.start_drag(ctx, win, screen_cursor);
                                }
                                Some(er @ (classic::EqRegion::Preamp | classic::EqRegion::Band(_))) => {
                                    // A press sets the slider from the click position;
                                    // a double-click resets it to 0 dB (handled below,
                                    // on the second release) (#140).
                                    self.apply_eq_slider(er, ny);
                                }
                                Some(
                                    er @ (classic::EqRegion::ShadeVolume
                                    | classic::EqRegion::ShadeBalance),
                                ) => self.apply_eq_shade_slider(er, nx),
                                _ => {}
                            }
                        }
                        ClWin::Pl => {
                            use classic::PlRegion as P;
                            match classic::pl_region_at(nx, ny, self.plview.px_w, self.plview.px_h) {
                                Some(P::Close) => self.close_window(ctx, ClWin::Pl),
                                Some(P::TitleBar) => self.start_drag(ctx, win, screen_cursor),
                                Some(P::Resize) => {
                                    self.geom.resize_pl =
                                        Some(egui::vec2(self.plview.px_w - nx, self.plview.px_h - ny))
                                }
                                Some(P::Scroll) => {
                                    // On the thumb: grab it (no jump) and track the cursor
                                    // relatively, from the global cursor so the drag keeps
                                    // tracking off-window. Above/below the thumb: page the
                                    // list up/down, like a classic scrollbar track click.
                                    let len = self.playlist.len();
                                    let visible = classic::pl_visible_rows(self.plview.px_h as u32);
                                    let max = classic::max_pl_first(len, visible);
                                    let thumb_top =
                                        classic::pl_scroll_thumb_y(self.plview.first, max, self.plview.px_h);
                                    let thumb_bot = thumb_top + classic::PL_SCROLL_THUMB_H as f32;
                                    let page = visible.max(1);
                                    if ny < thumb_top {
                                        self.plview.first = self.plview.first.saturating_sub(page);
                                    } else if ny >= thumb_bot {
                                        // Page down only while the end isn't already visible.
                                        if self.plview.first < max {
                                            self.plview.first = (self.plview.first + page).min(max);
                                        }
                                    } else {
                                        let gy = global_cursor_points(ctx)
                                            .map(|g| (g.y - self.geom.win_pos[win_idx(ClWin::Pl)].y) / self.scale)
                                            .unwrap_or(ny);
                                        self.pl_scroll_grab(gy);
                                    }
                                }
                                Some(P::List) => {
                                    let row = classic::pl_row_at(ny, self.plview.first);
                                    if row < self.playlist.len() {
                                        let now = Instant::now();
                                        // A plain repeat-click on the same row plays
                                        // it; a ctrl/shift click is a selection
                                        // gesture, never a play.
                                        let dbl = !sel_ctrl
                                            && !sel_shift
                                            && self
                                                .plview.last_click
                                                .map(|(r, t)| {
                                                    r == row
                                                        && now.duration_since(t).as_millis() < 350
                                                })
                                                .unwrap_or(false);
                                        if dbl {
                                            self.play_index(row);
                                        } else {
                                            self.pl_select_with(
                                                row,
                                                sel_ctrl,
                                                sel_shift,
                                                crate::selection::CtrlMode::Toggle,
                                            );
                                        }
                                        self.plview.last_click = Some((row, now));
                                    }
                                }
                                Some(menu @ (P::AddMenu | P::RemMenu | P::SelMenu | P::MiscMenu | P::ListMenu)) => {
                                    // Open on release (below) so this click can't dismiss it.
                                    self.plview.menu_press = Some(menu);
                                }
                                // Mini-player transport buttons mirror the main window's.
                                Some(P::MiniPrev) => self.trigger_classic(ctx, Btn::Prev),
                                Some(P::MiniPlay) => self.trigger_classic(ctx, Btn::Play),
                                Some(P::MiniPause) => self.trigger_classic(ctx, Btn::Pause),
                                Some(P::MiniStop) => self.trigger_classic(ctx, Btn::Stop),
                                Some(P::MiniNext) => self.trigger_classic(ctx, Btn::Next),
                                Some(P::MiniEject) => self.trigger_classic(ctx, Btn::Eject),
                                None => {}
                            }
                        }
                    }
                }
            }
        }

        if down {
            let idx = win_idx(win);
            // Drive the scrollbar drag from the global cursor, not the viewport-local
            // pointer: the local pointer goes None once the cursor leaves the window, which
            // froze the drag and, on re-entry near the top, snapped the thumb to the top.
            // The global cursor keeps increasing as you pull down past the window, so the
            // offset holds at the bottom (pl_scroll_first_at clamps it there).
            if win == ClWin::Pl && self.plview.scroll_drag {
                if let Some(g) = global_cursor_points(ctx) {
                    let ny = (g.y - self.geom.win_pos[win_idx(ClWin::Pl)].y) / self.scale;
                    self.pl_scroll_drag(ny);
                }
                ctx.request_repaint();
            }
            let is_my_drag = self.geom.drag.as_ref().map(|d| d.win == win).unwrap_or(false);
            if is_my_drag {
                let manual = self.geom.drag.as_ref().unwrap().manual;
                if manual {
                    // Live move: drive position from the global cursor (feedback-
                    // free), moving the docked group in tandem each frame.
                    if let Some(g) = global_cursor_points(ctx) {
                        let d = self.geom.drag.as_ref().unwrap();
                        let mut target = g - d.grab;
                        let group = d.group.clone();
                        let lib_offset = d.lib_offset;
                        let press_pos = d.press_pos;
                        // Clamp so the dragged window, and, for the main window, its
                        // whole docked block (EQ/PL + library), stops at the screen
                        // edge instead of being pushed off it.
                        // Clamp to the whole virtual desktop (all monitors), so the
                        // block can be dragged onto another display but not off the
                        // desktop. Only the dragged block's bounding offsets matter.
                        if let Some(vr) = virtual_desktop_rect(ctx) {
                            let mut lo = egui::vec2(0.0, 0.0);
                            let mut hi = self.win_inner_size(win);
                            for (mi, off) in &group {
                                lo = lo.min(*off);
                                hi = hi.max(*off + self.win_inner_size(idx_win(*mi)));
                            }
                            if let Some(loff) = lib_offset {
                                lo = lo.min(loff);
                                hi = hi.max(loff + self.lib_size());
                            }
                            let min_x = vr.min.x - lo.x;
                            let min_y = vr.min.y - lo.y;
                            target.x = target.x.clamp(min_x, (vr.max.x - hi.x).max(min_x));
                            target.y = target.y.clamp(min_y, (vr.max.y - hi.y).max(min_y));
                        }
                        self.geom.win_pos[idx] = target;
                        for (mi, off) in group {
                            self.geom.win_pos[mi] = target + off;
                        }
                        if let Some(off) = lib_offset {
                            self.geom.lib_pos = target + off;
                        }
                        if (target - press_pos).length() > 3.0 {
                            if let Some(dm) = self.geom.drag.as_mut() {
                                dm.moved = true;
                            }
                        }
                        if idx == 0 {
                            self.geom.root_last = None; // keep root in sync while moving
                        }
                    }
                    ctx.request_repaint();
                } else if !self.geom.drag.as_ref().unwrap().started_os {
                    // OS fallback: hand off to the OS window-drag past a threshold.
                    if let Some(sc) = screen_cursor {
                        if (sc - self.geom.drag.as_ref().unwrap().press_screen).length() > 4.0 {
                            ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
                            if let Some(dm) = self.geom.drag.as_mut() {
                                dm.started_os = true;
                            }
                        }
                    }
                }
            }
            if let Some(l) = local {
                let nx = l.x / self.scale;
                let ny = l.y / self.scale;
                if let Some(r) = self.render.active_region {
                    if matches!(r, Btn::Seek | Btn::Volume | Btn::Balance) {
                        self.apply_classic_slider(r, nx);
                    }
                }
                if let Some(er) = self.eq.active {
                    match er {
                        classic::EqRegion::ShadeVolume | classic::EqRegion::ShadeBalance => {
                            self.apply_eq_shade_slider(er, nx)
                        }
                        _ => self.apply_eq_slider(er, ny),
                    }
                }
                if let Some(grab) = self.geom.resize_pl {
                    // Resize smoothly during the drag; the 25/29px grid is applied
                    // only on release (#61), so the rows don't re-flow and jiggle
                    // as the size jumps between grid steps mid-drag.
                    self.plview.px_h = (ny + grab.y).max(87.0);
                    self.plview.px_w = (nx + grab.x).max(classic::MAIN_W as f32);
                    // Stop growing at the screen edge: cap the height so the window's
                    // bottom can't pass the bottom of the desktop. Otherwise the WM
                    // pushes the top edge up to keep the window on-screen, so the
                    // playlist appears to grow upward instead of just stopping (#156).
                    // pl_px_h is in unscaled skin pixels; on-screen height is
                    // pl_px_h * scale, so convert the available space back by /scale.
                    if let Some(vr) = virtual_desktop_rect(ctx) {
                        let top_y = self.geom.win_pos[win_idx(ClWin::Pl)].y;
                        if let Some(max_h) = classic::pl_max_height(vr.max.y, top_y, self.scale) {
                            self.plview.px_h = self.plview.px_h.min(max_h);
                        }
                    }
                }
            }
        }

        // Finalize a drag once the button is up (covers release after the modal
        // OS drag, which may swallow the release event).
        if !down {
            if let Some(d) = self.geom.drag.take() {
                if d.win == win {
                    self.finalize_drag(d, outer, monitor);
                } else {
                    self.geom.drag = Some(d);
                }
            }
            // End a scrollbar drag on release even when the cursor is off-window, where the
            // `released` edge below may not reach this viewport.
            if win == ClWin::Pl {
                self.plview.scroll_drag = false;
            }
        }

        if released {
            if self.geom.resize_pl.is_some() {
                // Quantize to the classic player's 25px (width) / 29px (height) grid now the
                // drag is finished (#61).
                let hstep = 29.0;
                self.plview.px_h = ((self.plview.px_h / hstep).round() * hstep).max(87.0);
                let wstep = 25.0;
                let min_w = classic::MAIN_W as f32;
                self.plview.px_w =
                    (((self.plview.px_w - min_w) / wstep).round() * wstep + min_w).max(min_w);
            }
            self.geom.resize_pl = None;
            self.plview.scroll_drag = false;
            if let Some(l) = local {
                let nx = l.x / self.scale;
                let ny = l.y / self.scale;
                if win == ClWin::Main {
                    if let Some(r) = self.render.active_region {
                        if classic::region_at(nx, ny) == Some(r) {
                            if r == Btn::Options || r == Btn::ClutterO {
                                self.open_popup_in(Popup::Options, win, l);
                            } else {
                                // Btn::Time toggles elapsed/remaining on a single
                                // click via trigger_classic, matching classic
                                // the classic player's time display behaviour.
                                self.trigger_classic(ctx, r);
                            }
                        }
                    }
                    if self.render.active_region == Some(Btn::Seek) {
                        if let Some(p) = self.classic_seek_preview.take() {
                            self.player.seek(Duration::from_secs_f32(p));
                        }
                    }
                }
                if win == ClWin::Eq {
                    if let Some(er) = self.eq.active {
                        if self.eq_region(nx, ny) == Some(er) {
                            if er == classic::EqRegion::Presets {
                                self.open_popup_in(Popup::EqPresets, ClWin::Eq, l);
                            } else {
                                self.trigger_eq(ctx, er);
                            }
                        }
                    }
                }
                if win == ClWin::Pl {
                    if let Some(r) = self.plview.menu_press.take() {
                        if classic::pl_region_at(nx, ny, self.plview.px_w, self.plview.px_h) == Some(r) {
                            use classic::PlRegion as P;
                            let kind = match r {
                                P::AddMenu => Popup::PlAdd,
                                P::RemMenu => Popup::PlRem,
                                P::SelMenu => Popup::PlSel,
                                P::MiscMenu => Popup::PlMisc,
                                _ => Popup::PlList,
                            };
                            self.open_popup_in(kind, ClWin::Pl, l);
                        }
                    }
                }
            }
            self.render.active_region = None;
            self.eq.active = None;
            self.plview.menu_press = None;
        }

        if win == ClWin::Main {
            self.render.pressed_btn = if down {
                match self.render.active_region {
                    Some(b @ (Btn::Prev | Btn::Play | Btn::Pause | Btn::Stop | Btn::Next | Btn::Eject
                    | Btn::Shuffle | Btn::Repeat | Btn::Seek | Btn::Volume | Btn::Balance)) => Some(b),
                    _ => None,
                }
            } else {
                None
            };
        }
    }

    fn finalize_drag(&mut self, d: Dragging, outer: Option<egui::Rect>, monitor: Option<egui::Vec2>) {
        let idx = win_idx(d.win);
        let did_move = if d.manual { d.moved } else { d.started_os };
        if did_move {
            // OS fallback moved the window out from under win_pos; re-sync it.
            if !d.manual {
                if let Some(r) = outer {
                    self.geom.win_pos[idx] = r.min;
                }
                for (mi, off) in &d.group {
                    self.geom.win_pos[*mi] = self.geom.win_pos[idx] + *off;
                }
                if let Some(off) = d.lib_offset {
                    self.geom.lib_pos = self.geom.win_pos[idx] + off;
                }
            }
            // Magnetic snap to screen edges / other windows; move the group together.
            let before = self.geom.win_pos[idx];
            let size = self.win_inner_size(d.win);
            let snapped = self.snap_pos(idx, before, size, monitor);
            let delta = snapped - before;
            self.geom.win_pos[idx] = snapped;
            for (mi, _) in &d.group {
                self.geom.win_pos[*mi] += delta;
            }
            if d.lib_offset.is_some() {
                self.geom.lib_pos += delta;
            }
            if d.win == ClWin::Main {
                self.geom.root_last = None;
            }
            // Main never covered; EQ/playlist never cover each other (they stack).
            self.resolve_overlaps(d.win);
        } else {
            // A click with no move: double-click toggles the window shade.
            let now = Instant::now();
            let dbl = self
                .tb_last_click
                .map(|(w, t)| w == d.win && now.duration_since(t).as_millis() < 350)
                .unwrap_or(false);
            if dbl {
                self.toggle_shade(d.win);
                self.tb_last_click = None;
            } else {
                self.tb_last_click = Some((d.win, now));
            }
        }
    }

    fn start_drag(&mut self, ctx: &egui::Context, win: ClWin, screen_cursor: Option<egui::Pos2>) {
        if let Some(sc) = screen_cursor {
            // Only the main window carries a group: the set of windows actually
            // docked (edge-flush) to it. EQ/playlist always move on their own,
            // even when docked to each other.
            // The main window carries its whole transitively-docked block (EQ/PL
            // and the library, including chains through the library). Other windows
            // move on their own.
            let (group, lib_offset) = if win == ClWin::Main {
                self.main_drag_group()
            } else {
                (Vec::new(), None)
            };
            let idx = win_idx(win);
            let (manual, grab) = match global_cursor_points(ctx) {
                Some(g) => (true, g - self.geom.win_pos[idx]),
                None => (false, egui::Vec2::ZERO),
            };
            self.geom.drag = Some(Dragging {
                win,
                manual,
                grab,
                press_pos: self.geom.win_pos[idx],
                press_screen: sc,
                started_os: false,
                moved: false,
                group,
                lib_offset,
            });
        }
    }

    /// After a drag, resolve overlaps: main is never covered and EQ/playlist never
    /// cover each other. The just-dragged window gives way (a child moves off main
    /// and its sibling; a dragged main pushes the children off instead).
    fn resolve_overlaps(&mut self, dragged: ClWin) {
        let rect = |app: &Self, i: usize| {
            egui::Rect::from_min_size(app.geom.win_pos[i], app.win_inner_size(idx_win(i)))
        };
        let vis = |app: &Self, i: usize| match i {
            0 => true,
            1 => app.eq_visible,
            2 => app.pl_visible,
            _ => false,
        };
        if dragged == ClWin::Main {
            // Children give way: push each off main, then separate them if needed.
            for i in [1usize, 2usize] {
                if vis(self, i) {
                    if let Some(p) = push_off(rect(self, i), rect(self, 0)) {
                        self.geom.win_pos[i] = p;
                    }
                }
            }
            if vis(self, 1) && vis(self, 2) {
                // Push the lower one below the higher so they don't overlap.
                let (hi, lo) = if self.geom.win_pos[1].y <= self.geom.win_pos[2].y {
                    (1, 2)
                } else {
                    (2, 1)
                };
                if let Some(p) = push_off(rect(self, lo), rect(self, hi)) {
                    self.geom.win_pos[lo] = p;
                }
            }
        } else {
            // The dragged child moves off main, and stacks below its sibling only
            // when that sibling is itself docked to main; a floating sibling must
            // not block docking directly under main.
            let di = win_idx(dragged);
            let oi = if di == 1 { 2 } else { 1 };
            for _ in 0..4 {
                let mut moved = false;
                if let Some(p) = push_off(rect(self, di), rect(self, 0)) {
                    self.geom.win_pos[di] = p;
                    moved = true;
                }
                if vis(self, oi) && rects_docked(rect(self, oi), rect(self, 0)) {
                    if let Some(p) = push_off(rect(self, di), rect(self, oi)) {
                        self.geom.win_pos[di] = p;
                        moved = true;
                    }
                }
                if !moved {
                    break;
                }
            }
        }
        self.geom.root_last = None;
    }

    pub(super) fn win_rect_pts(&self, idx: usize) -> egui::Rect {
        let w = match idx {
            0 => ClWin::Main,
            1 => ClWin::Eq,
            _ => ClWin::Pl,
        };
        egui::Rect::from_min_size(self.geom.win_pos[idx], self.win_inner_size(w))
    }

    /// The full drag group for a main-window move: the EQ/playlist windows (by
    /// offset) and the library (by offset) that are transitively docked to main, 
    /// including chains that pass *through* the library (example: main–library–playlist).
    fn main_drag_group(&self) -> (Vec<(usize, egui::Vec2)>, Option<egui::Vec2>) {
        let rects = [
            self.win_rect_pts(0),
            self.win_rect_pts(1),
            self.win_rect_pts(2),
            egui::Rect::from_min_size(self.geom.lib_pos, self.lib_size()),
        ];
        let vis = [true, self.eq_visible, self.pl_visible, self.lib_visible];
        let in_group = dock_group_from_main(&rects, &vis);
        let group = [1usize, 2]
            .into_iter()
            .filter(|&j| in_group[j])
            .map(|j| (j, self.geom.win_pos[j] - self.geom.win_pos[0]))
            .collect();
        let lib_offset = in_group[3].then(|| self.geom.lib_pos - self.geom.win_pos[0]);
        (group, lib_offset)
    }

    /// Canonical stacking: EQ then playlist dock directly below the main window,
    /// in order, regardless of which was opened first.
    pub(super) fn restack(&mut self) {
        let base = self.geom.win_pos[0];
        let mut y = base.y + self.main_h() * self.scale;
        if self.eq_visible {
            self.geom.win_pos[1] = egui::pos2(base.x, y);
            y += self.eq_h() * self.scale;
        }
        if self.pl_visible {
            self.geom.win_pos[2] = egui::pos2(base.x, y);
        }
    }

    /// Magnetic snap of `pos` (rect of `size`) to screen edges and other windows.
    fn snap_pos(
        &self,
        idx: usize,
        pos: egui::Pos2,
        size: egui::Vec2,
        monitor: Option<egui::Vec2>,
    ) -> egui::Pos2 {
        use egui::Rect;
        let r = Rect::from_min_size(pos, size);
        let mut others = Vec::new();
        for j in 0..3 {
            if j == idx {
                continue;
            }
            let vis = match j {
                0 => true,
                1 => self.eq_visible,
                2 => self.pl_visible,
                _ => false,
            };
            if !vis {
                continue;
            }
            let jwin = match j {
                0 => ClWin::Main,
                1 => ClWin::Eq,
                _ => ClWin::Pl,
            };
            others.push(Rect::from_min_size(self.geom.win_pos[j], self.win_inner_size(jwin)));
        }
        // The library is a snap target too (but never pushes / is pushed).
        if self.lib_visible {
            others.push(Rect::from_min_size(self.geom.lib_pos, self.lib_size()));
        }
        snap_rect(r, &others, monitor)
    }

    /// Current on-screen rects of the three windows (None when hidden).
    fn window_rects(&self) -> [Option<egui::Rect>; 3] {
        [
            Some(self.win_rect_pts(0)),
            self.eq_visible.then(|| self.win_rect_pts(1)),
            self.pl_visible.then(|| self.win_rect_pts(2)),
        ]
    }

    fn toggle_shade(&mut self, win: ClWin) {
        let idx = win_idx(win);
        // Collapsing/expanding changes this window's height. Windows docked in a
        // flush stack below it should move by the same amount so the stack stays
        // intact instead of overlapping (only docked windows are affected).
        let before = self.window_rects();
        let old_h = before[idx].map(|r| r.height()).unwrap_or(0.0);
        let moved = docked_group_below(&before, idx);
        match win {
            ClWin::Main => self.geom.main_shade = !self.geom.main_shade,
            ClWin::Eq => self.geom.eq_shade = !self.geom.eq_shade,
            ClWin::Pl => self.geom.pl_shade = !self.geom.pl_shade,
        }
        let delta = self.win_inner_size(win).y - old_h;
        for (j, &m) in moved.iter().enumerate() {
            if m {
                self.geom.win_pos[j].y += delta;
            }
        }
        self.geom.root_last = None;
    }

    pub(super) fn set_double_size(&mut self, _ctx: &egui::Context, dbl: bool) {
        if self.prefs.double_size == dbl {
            return;
        }
        // Remember whether the library rides with the group so it can re-dock to
        // the resized stack; a floating library is left to resize in place.
        let lib_docked = self.library_docked();
        let old_scale = self.scale;
        self.prefs.double_size = dbl;
        self.scale = self.prefs.scale();
        self.prefs.save();
        self.restack();
        if lib_docked && old_scale > 0.0 {
            // The whole group scales from the main window's top-left, so scaling the
            // library's offset from main by the same ratio keeps it docked.
            let ratio = self.scale / old_scale;
            let main = self.geom.win_pos[0];
            self.geom.lib_pos = main + (self.geom.lib_pos - main) * ratio;
        }
        self.geom.root_last = None; // force the root window to resize next frame
    }
}

/// Volume after one mouse-wheel notch over the main window: scrolling up is
/// louder, down is quieter, clamped to 0..=1 (the classic player main-window wheel
/// behaviour). `step` is the per-notch change.
pub(crate) fn wheel_volume(current: f32, scroll: f32, step: f32) -> f32 {
    let delta = if scroll > 0.0 {
        step
    } else if scroll < 0.0 {
        -step
    } else {
        0.0
    };
    (current + delta).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::wheel_volume;

    #[test]
    fn wheel_volume_scrolls_and_clamps() {
        assert!((wheel_volume(0.5, 1.0, 0.05) - 0.55).abs() < 1e-6, "up is louder");
        assert!((wheel_volume(0.5, -1.0, 0.05) - 0.45).abs() < 1e-6, "down is quieter");
        assert_eq!(wheel_volume(0.98, 1.0, 0.05), 1.0, "clamps at max");
        assert_eq!(wheel_volume(0.02, -1.0, 0.05), 0.0, "clamps at min");
        assert_eq!(wheel_volume(0.5, 0.0, 0.05), 0.5, "no scroll, no change");
    }
}
