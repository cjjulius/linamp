//! Classic multi-window geometry: per-window input, drag / dock / snap,
//! window-shade and double-size.

use super::*;

impl super::LinAmpApp {
    /// Per-window input in local (viewport) coordinates.
    pub(super) fn handle_window_input(&mut self, ctx: &egui::Context, win: ClWin) {
        let outer = ctx.input(|i| i.viewport().outer_rect);
        let monitor = ctx.input(|i| i.viewport().monitor_size);
        let local = ctx.pointer_latest_pos();
        let screen_cursor = match (outer, local) {
            (Some(o), Some(l)) => Some(o.min + l.to_vec2()),
            _ => None,
        };
        let (pressed, released, down, secondary, scroll) = ctx.input(|i| {
            (
                i.pointer.primary_pressed(),
                i.pointer.primary_released(),
                i.pointer.primary_down(),
                i.pointer.secondary_clicked(),
                i.raw_scroll_delta.y,
            )
        });
        let shaded = match win {
            ClWin::Main => self.main_shade,
            ClWin::Eq => self.eq_shade,
            ClWin::Pl => self.pl_shade,
        };

        // A primary press on any classic window also dismisses an open Media
        // Library menu (its own OS window never sees the click). Keyed on the
        // primary press only — the same gesture that dismisses a classic popup —
        // so the library's own right-click (secondary) doesn't trip it.
        if pressed {
            self.classic_clicked = true;
        }

        // Clicking any main window dismisses an open menu (click-away).
        if pressed && self.popup.is_some() && !self.popup_fresh {
            self.popup = None;
        }
        // Escape closes an open menu. The popup no longer steals focus, so its own
        // viewport won't see the key — the focused window catches it here instead.
        if self.popup.is_some() && ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.popup = None;
        }

        if secondary && win == ClWin::Main {
            if let Some(l) = local {
                self.open_popup_in(Popup::Options, win, l);
            }
        }

        // Right-click on the playlist list area: a track menu when a row is under
        // the cursor, otherwise the playlist-management menu for empty space.
        if secondary && win == ClWin::Pl && !shaded {
            if let Some(l) = local {
                let nx = l.x / self.scale;
                let ny = l.y / self.scale;
                if let Some(classic::PlRegion::List) =
                    classic::pl_region_at(nx, ny, self.pl_px_w, self.pl_px_h)
                {
                    let row = classic::pl_row_at(ny, self.pl_first);
                    if row < self.playlist.len() {
                        // Winamp rule: right-clicking a row outside the current
                        // selection selects just it; a row already in the
                        // selection keeps the whole multi-selection.
                        if !self.pl_selection.contains(&row) {
                            self.pl_selection.clear();
                            self.pl_selection.insert(row);
                        }
                        // Remember the exact row so Play / Edit target it even when
                        // it's one of several selected.
                        self.pl_ctx_row = Some(row);
                        self.open_popup_in(Popup::PlTrack, win, l);
                    } else {
                        self.open_popup_in(Popup::PlVoid, win, l);
                    }
                }
            }
        }

        if win == ClWin::Pl && !shaded && scroll != 0.0 {
            let max = self.playlist.len().saturating_sub(1) as i32;
            let step = if scroll > 0.0 { -3 } else { 3 };
            self.pl_first = (self.pl_first as i32 + step).clamp(0, max.max(0)) as usize;
        }

        if pressed {
            self.active_region = None;
            self.eq_active = None;
            if let Some(l) = local {
                let nx = l.x / self.scale;
                let ny = l.y / self.scale;
                if shaded {
                    use classic::ShadeHit as H;
                    // The main shade bar carries extra controls; EQ/PL shades only
                    // have close + title bar.
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
                        None => {}
                    }
                } else {
                    match win {
                        ClWin::Main => {
                            let r = classic::region_at(nx, ny);
                            self.active_region = r;
                            match r {
                                Some(Btn::TitleBar) => {
                                    self.active_region = None;
                                    self.start_drag(ctx, win, screen_cursor);
                                }
                                Some(b @ (Btn::Seek | Btn::Volume | Btn::Balance)) => {
                                    self.apply_classic_slider(b, nx)
                                }
                                _ => {}
                            }
                        }
                        ClWin::Eq => {
                            let r = classic::eq_region_at(nx, ny);
                            self.eq_active = r;
                            match r {
                                Some(classic::EqRegion::TitleBar) => {
                                    self.eq_active = None;
                                    self.start_drag(ctx, win, screen_cursor);
                                }
                                Some(er @ (classic::EqRegion::Preamp | classic::EqRegion::Band(_))) => {
                                    self.apply_eq_slider(er, ny)
                                }
                                _ => {}
                            }
                        }
                        ClWin::Pl => {
                            use classic::PlRegion as P;
                            match classic::pl_region_at(nx, ny, self.pl_px_w, self.pl_px_h) {
                                Some(P::Close) => self.close_window(ctx, ClWin::Pl),
                                Some(P::TitleBar) => self.start_drag(ctx, win, screen_cursor),
                                Some(P::Resize) => {
                                    self.resize_pl =
                                        Some(egui::vec2(self.pl_px_w - nx, self.pl_px_h - ny))
                                }
                                Some(P::List) => {
                                    let row = classic::pl_row_at(ny, self.pl_first);
                                    if row < self.playlist.len() {
                                        let now = Instant::now();
                                        let dbl = self
                                            .pl_last_click
                                            .map(|(r, t)| r == row && now.duration_since(t).as_millis() < 350)
                                            .unwrap_or(false);
                                        if dbl {
                                            self.play_index(row);
                                        } else {
                                            self.pl_selection.clear();
                                            self.pl_selection.insert(row);
                                        }
                                        self.pl_last_click = Some((row, now));
                                    }
                                }
                                Some(menu @ (P::AddMenu | P::RemMenu | P::SelMenu | P::MiscMenu | P::ListMenu)) => {
                                    // Open on release (below) so this click can't dismiss it.
                                    self.pl_menu_press = Some(menu);
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
            let is_my_drag = self.drag.as_ref().map(|d| d.win == win).unwrap_or(false);
            if is_my_drag {
                let manual = self.drag.as_ref().unwrap().manual;
                if manual {
                    // Live move: drive position from the global cursor (feedback-
                    // free), moving the docked group in tandem each frame.
                    if let Some(g) = global_cursor_points(ctx) {
                        let d = self.drag.as_ref().unwrap();
                        let mut target = g - d.grab;
                        let group = d.group.clone();
                        let lib_offset = d.lib_offset;
                        let press_pos = d.press_pos;
                        // Clamp so the dragged window — and, for the main window, its
                        // whole docked block (EQ/PL + library) — stops at the screen
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
                        self.win_pos[idx] = target;
                        for (mi, off) in group {
                            self.win_pos[mi] = target + off;
                        }
                        if let Some(off) = lib_offset {
                            self.lib_pos = target + off;
                        }
                        if (target - press_pos).length() > 3.0 {
                            if let Some(dm) = self.drag.as_mut() {
                                dm.moved = true;
                            }
                        }
                        if idx == 0 {
                            self.root_last = None; // keep root in sync while moving
                        }
                    }
                    ctx.request_repaint();
                } else if !self.drag.as_ref().unwrap().started_os {
                    // OS fallback: hand off to the OS window-drag past a threshold.
                    if let Some(sc) = screen_cursor {
                        if (sc - self.drag.as_ref().unwrap().press_screen).length() > 4.0 {
                            ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
                            if let Some(dm) = self.drag.as_mut() {
                                dm.started_os = true;
                            }
                        }
                    }
                }
            }
            if let Some(l) = local {
                let nx = l.x / self.scale;
                let ny = l.y / self.scale;
                if let Some(r) = self.active_region {
                    if matches!(r, Btn::Seek | Btn::Volume | Btn::Balance) {
                        self.apply_classic_slider(r, nx);
                    }
                }
                if let Some(er) = self.eq_active {
                    self.apply_eq_slider(er, ny);
                }
                if let Some(grab) = self.resize_pl {
                    // Resize smoothly during the drag; the 25/29px grid is applied
                    // only on release (#61), so the rows don't re-flow and jiggle
                    // as the size jumps between grid steps mid-drag.
                    self.pl_px_h = (ny + grab.y).max(87.0);
                    self.pl_px_w = (nx + grab.x).max(classic::MAIN_W as f32);
                }
            }
        }

        // Finalize a drag once the button is up (covers release after the modal
        // OS drag, which may swallow the release event).
        if !down {
            if let Some(d) = self.drag.take() {
                if d.win == win {
                    self.finalize_drag(d, outer, monitor);
                } else {
                    self.drag = Some(d);
                }
            }
        }

        if released {
            if self.resize_pl.is_some() {
                // Quantize to Winamp's 25px (width) / 29px (height) grid now the
                // drag is finished (#61).
                let hstep = 29.0;
                self.pl_px_h = ((self.pl_px_h / hstep).round() * hstep).max(87.0);
                let wstep = 25.0;
                let min_w = classic::MAIN_W as f32;
                self.pl_px_w =
                    (((self.pl_px_w - min_w) / wstep).round() * wstep + min_w).max(min_w);
            }
            self.resize_pl = None;
            if let Some(l) = local {
                let nx = l.x / self.scale;
                let ny = l.y / self.scale;
                if win == ClWin::Main {
                    if let Some(r) = self.active_region {
                        if classic::region_at(nx, ny) == Some(r) {
                            if r == Btn::Options || r == Btn::ClutterO {
                                self.open_popup_in(Popup::Options, win, l);
                            } else if r == Btn::Time {
                                // Double-click the time display toggles elapsed vs
                                // remaining (shown as a negative) everywhere it's shown.
                                let now = Instant::now();
                                let dbl = self
                                    .time_last_click
                                    .map(|t| now.duration_since(t).as_millis() < 350)
                                    .unwrap_or(false);
                                if dbl {
                                    self.time_remaining = !self.time_remaining;
                                    self.time_last_click = None;
                                } else {
                                    self.time_last_click = Some(now);
                                }
                            } else {
                                self.trigger_classic(ctx, r);
                            }
                        }
                    }
                    if self.active_region == Some(Btn::Seek) {
                        if let Some(p) = self.classic_seek_preview.take() {
                            self.player.seek(Duration::from_secs_f32(p));
                        }
                    }
                }
                if win == ClWin::Eq {
                    if let Some(er) = self.eq_active {
                        if classic::eq_region_at(nx, ny) == Some(er) {
                            if er == classic::EqRegion::Presets {
                                self.open_popup_in(Popup::EqPresets, ClWin::Eq, l);
                            } else {
                                self.trigger_eq(ctx, er);
                            }
                        }
                    }
                }
                if win == ClWin::Pl {
                    if let Some(r) = self.pl_menu_press.take() {
                        if classic::pl_region_at(nx, ny, self.pl_px_w, self.pl_px_h) == Some(r) {
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
            self.active_region = None;
            self.eq_active = None;
            self.pl_menu_press = None;
        }

        if win == ClWin::Main {
            self.pressed_btn = if down {
                match self.active_region {
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
                    self.win_pos[idx] = r.min;
                }
                for (mi, off) in &d.group {
                    self.win_pos[*mi] = self.win_pos[idx] + *off;
                }
                if let Some(off) = d.lib_offset {
                    self.lib_pos = self.win_pos[idx] + off;
                }
            }
            // Magnetic snap to screen edges / other windows; move the group together.
            let before = self.win_pos[idx];
            let size = self.win_inner_size(d.win);
            let snapped = self.snap_pos(idx, before, size, monitor);
            let delta = snapped - before;
            self.win_pos[idx] = snapped;
            for (mi, _) in &d.group {
                self.win_pos[*mi] += delta;
            }
            if d.lib_offset.is_some() {
                self.lib_pos += delta;
            }
            if d.win == ClWin::Main {
                self.root_last = None;
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
                Some(g) => (true, g - self.win_pos[idx]),
                None => (false, egui::Vec2::ZERO),
            };
            self.drag = Some(Dragging {
                win,
                manual,
                grab,
                press_pos: self.win_pos[idx],
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
            egui::Rect::from_min_size(app.win_pos[i], app.win_inner_size(idx_win(i)))
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
                        self.win_pos[i] = p;
                    }
                }
            }
            if vis(self, 1) && vis(self, 2) {
                // Push the lower one below the higher so they don't overlap.
                let (hi, lo) = if self.win_pos[1].y <= self.win_pos[2].y {
                    (1, 2)
                } else {
                    (2, 1)
                };
                if let Some(p) = push_off(rect(self, lo), rect(self, hi)) {
                    self.win_pos[lo] = p;
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
                    self.win_pos[di] = p;
                    moved = true;
                }
                if vis(self, oi) && rects_docked(rect(self, oi), rect(self, 0)) {
                    if let Some(p) = push_off(rect(self, di), rect(self, oi)) {
                        self.win_pos[di] = p;
                        moved = true;
                    }
                }
                if !moved {
                    break;
                }
            }
        }
        self.root_last = None;
    }

    pub(super) fn win_rect_pts(&self, idx: usize) -> egui::Rect {
        let w = match idx {
            0 => ClWin::Main,
            1 => ClWin::Eq,
            _ => ClWin::Pl,
        };
        egui::Rect::from_min_size(self.win_pos[idx], self.win_inner_size(w))
    }

    /// The full drag group for a main-window move: the EQ/playlist windows (by
    /// offset) and the library (by offset) that are transitively docked to main —
    /// including chains that pass *through* the library (e.g. main–library–playlist).
    fn main_drag_group(&self) -> (Vec<(usize, egui::Vec2)>, Option<egui::Vec2>) {
        let rects = [
            self.win_rect_pts(0),
            self.win_rect_pts(1),
            self.win_rect_pts(2),
            egui::Rect::from_min_size(self.lib_pos, self.lib_size()),
        ];
        let vis = [true, self.eq_visible, self.pl_visible, self.lib_visible];
        let in_group = dock_group_from_main(&rects, &vis);
        let group = [1usize, 2]
            .into_iter()
            .filter(|&j| in_group[j])
            .map(|j| (j, self.win_pos[j] - self.win_pos[0]))
            .collect();
        let lib_offset = in_group[3].then(|| self.lib_pos - self.win_pos[0]);
        (group, lib_offset)
    }

    /// Canonical stacking: EQ then playlist dock directly below the main window,
    /// in order, regardless of which was opened first.
    pub(super) fn restack(&mut self) {
        let base = self.win_pos[0];
        let mut y = base.y + self.main_h() * self.scale;
        if self.eq_visible {
            self.win_pos[1] = egui::pos2(base.x, y);
            y += self.eq_h() * self.scale;
        }
        if self.pl_visible {
            self.win_pos[2] = egui::pos2(base.x, y);
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
            others.push(Rect::from_min_size(self.win_pos[j], self.win_inner_size(jwin)));
        }
        // The library is a snap target too (but never pushes / is pushed).
        if self.lib_visible {
            others.push(Rect::from_min_size(self.lib_pos, self.lib_size()));
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
            ClWin::Main => self.main_shade = !self.main_shade,
            ClWin::Eq => self.eq_shade = !self.eq_shade,
            ClWin::Pl => self.pl_shade = !self.pl_shade,
        }
        let delta = self.win_inner_size(win).y - old_h;
        for (j, &m) in moved.iter().enumerate() {
            if m {
                self.win_pos[j].y += delta;
            }
        }
        self.root_last = None;
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
            let main = self.win_pos[0];
            self.lib_pos = main + (self.lib_pos - main) * ratio;
        }
        self.root_last = None; // force the root window to resize next frame
    }
}
