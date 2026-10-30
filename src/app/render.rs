//! The classic window render pipeline: a per-window fingerprint gates a
//! sprite re-composite and GPU re-upload, so windows only redraw on change.
//! See docs/ARCHITECTURE.md (Render pipeline).

use super::*;

impl super::LinAmpApp {
    /// A cheap fingerprint of everything `compose_window(win)` draws from, so the
    /// expensive re-composite + GPU re-upload can be skipped when nothing changed.
    fn window_fp(&self, win: ClWin) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        let ps: u8 = match self.player.state() {
            PlayState::Stopped => 0,
            PlayState::Playing => 1,
            PlayState::Paused => 2,
        };
        self.theme.name.hash(&mut h);
        ps.hash(&mut h);
        // Active/inactive title bar follows app focus (all windows together).
        self.geom.app_focused.hash(&mut h);
        match win {
            ClWin::Main => {
                self.geom.main_shade.hash(&mut h);
                // Clutterbar lit indicators: A = always-on-top, D = double-size.
                // Without these the cached texture wouldn't refresh when the toggle
                // changes (from Preferences or the clutterbar), so the on-screen
                // indicator lagged until some other input happened to change.
                self.prefs.always_on_top.hash(&mut h);
                self.prefs.double_size.hash(&mut h);
                self.render.pressed_btn.hash(&mut h);
                self.shuffle.hash(&mut h);
                self.repeat_all.hash(&mut h);
                self.eq_visible.hash(&mut h);
                self.pl_visible.hash(&mut h);
                self.time_remaining.hash(&mut h);
                self.time_blink_hidden.hash(&mut h);
                (self.viz_mode as u8).hash(&mut h);
                self.player.volume().to_bits().hash(&mut h);
                self.balance.to_bits().hash(&mut h);
                let pos = self
                    .classic_seek_preview
                    .unwrap_or_else(|| self.player.position().as_secs_f32());
                pos.to_bits().hash(&mut h);
                self.marquee_offset.to_bits().hash(&mut h);
                self.playlist.current.hash(&mut h);
                if let Some(t) = self.playlist.current_track() {
                    // Hash the title fields directly; display_name() would allocate
                    // a String every frame on this animating path.
                    t.artist.hash(&mut h);
                    t.title.hash(&mut h);
                    t.bitrate.hash(&mut h);
                    t.sample_rate.hash(&mut h);
                    t.channels.hash(&mut h);
                    t.duration.hash(&mut h);
                }
                self.album_art.is_some().hash(&mut h);
                // Spectrum animation (rising/falling bars and peaks).
                for b in self.cviz_bars.iter().chain(self.cviz_peaks.iter()) {
                    b.to_bits().hash(&mut h);
                }
            }
            ClWin::Eq => {
                self.geom.eq_shade.hash(&mut h);
                if self.geom.eq_shade {
                    // The collapsed EQ shows the (shared) volume + balance sliders.
                    self.player.volume().to_bits().hash(&mut h);
                    self.balance.to_bits().hash(&mut h);
                }
                self.eq.enabled.hash(&mut h);
                self.eq.auto.hash(&mut h);
                self.eq.active.hash(&mut h);
                self.eq.preamp_db.to_bits().hash(&mut h);
                for b in &self.eq.bands {
                    b.to_bits().hash(&mut h);
                }
            }
            ClWin::Pl => {
                self.geom.pl_shade.hash(&mut h);
                self.plview.px_w.to_bits().hash(&mut h);
                self.plview.px_h.to_bits().hash(&mut h);
                self.plview.first.hash(&mut h);
                self.pl_pressed_button().hash(&mut h);
                self.time_remaining.hash(&mut h);
                self.time_blink_hidden.hash(&mut h);
                self.playlist.len().hash(&mut h);
                self.playlist.current.hash(&mut h);
                // The bottom-bar / shade time advances once a second while playing.
                self.player.position().as_secs().hash(&mut h);
                // Visible rows' identity + text, catches selection, current-row
                // and lazy tag loads without hashing the whole playlist. (Tags only
                // load for visible rows, so off-screen rows can't change unseen.)
                let vis = if self.geom.pl_shade {
                    0
                } else {
                    classic::pl_visible_rows(self.plview.px_h as u32)
                };
                let end = (self.plview.first + vis).min(self.playlist.len());
                for i in self.plview.first..end {
                    (Some(i) == self.playlist.current).hash(&mut h);
                    self.plview.selection.contains(&i).hash(&mut h);
                    if let Some(t) = self.playlist.tracks.get(i) {
                        t.artist.hash(&mut h);
                        t.title.hash(&mut h);
                        t.duration.hash(&mut h);
                    }
                }
                if self.geom.pl_shade {
                    if let Some(t) = self.playlist.current_track() {
                        t.artist.hash(&mut h);
                        t.title.hash(&mut h);
                        t.duration.hash(&mut h);
                    }
                }
            }
        }
        h.finish()
    }

    pub(super) fn render_window(&mut self, ctx: &egui::Context, win: ClWin) {
        self.handle_window_input(ctx, win);
        self.handle_dropped(ctx, Some(win));

        // Refresh the analyzer data for the main window.
        if win == ClWin::Main && !self.geom.main_shade {
            self.step_viz();
        }

        // Keep the scroll offset in range. Playing or enqueueing from the library
        // changes the playlist length without touching the offset (only the scroll
        // handler clamped it), so replacing a long, scrolled playlist with a shorter
        // one could leave `first` past the end, where `first..end` is empty and the
        // whole list renders blank even while a track plays. Clamp it here so the
        // list is always visible.
        if win == ClWin::Pl {
            let len = self.playlist.len();
            // When the whole list fits the window there is nothing to scroll, so snap to
            // the top: a stale non-zero offset (e.g. left after the window grew) would
            // otherwise leave the list scrolled down with blank rows and the thumb pinned
            // to the top. Otherwise keep the loose clamp so a delete-at-bottom blank can
            // persist until the next scroll.
            let visible = classic::pl_visible_rows(self.plview.px_h as u32);
            self.plview.first = if classic::max_pl_first(len, visible) == 0 {
                0
            } else {
                classic::clamp_pl_first(self.plview.first, len)
            };
        }

        // Lazily read tags for the playlist rows about to be shown (on-view mode).
        if win == ClWin::Pl && !self.geom.pl_shade {
            let vis = classic::pl_visible_rows(self.plview.px_h as u32);
            let end = (self.plview.first + vis).min(self.playlist.len());
            for i in self.plview.first..end {
                self.playlist.ensure_loaded(i);
            }
        }

        // Re-composite + re-upload only when this window's inputs changed; the GPU
        // keeps the cached texture otherwise (near-zero cost when idle/paused).
        let idx = win_idx(win);
        let fp = self.window_fp(win);
        let tex_empty = match win {
            ClWin::Main => self.render.classic_tex.is_none(),
            ClWin::Eq => self.render.eq_tex.is_none(),
            ClWin::Pl => self.render.pl_tex.is_none(),
        };
        if self.render.tex_fp[idx] != fp || tex_empty {
            let img = self.compose_window(win);
            self.render.tex_fp[idx] = fp;
            let name = match win {
                ClWin::Main => "cl_main",
                ClWin::Eq => "cl_eq",
                ClWin::Pl => "cl_pl",
            };
            let slot = match win {
                ClWin::Main => &mut self.render.classic_tex,
                ClWin::Eq => &mut self.render.eq_tex,
                ClWin::Pl => &mut self.render.pl_tex,
            };
            let _ = upload_tex(slot, ctx, &img, name);
        }
        let tex = match win {
            ClWin::Main => self.render.classic_tex.clone(),
            ClWin::Eq => self.render.eq_tex.clone(),
            ClWin::Pl => self.render.pl_tex.clone(),
        }
        .expect("composed above");
        let nw = match win {
            ClWin::Pl if !self.geom.pl_shade => self.plview.px_w,
            _ => classic::MAIN_W as f32,
        };
        let nh = self.win_native_h(win);

        egui::CentralPanel::default()
            .frame(egui::Frame::none().fill(Color32::BLACK))
            .show(ctx, |ui| {
                let rect = ui.max_rect();
                let uv = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
                ui.painter().image(tex.id(), rect, uv, Color32::WHITE);

                // Hover tooltips over the classic buttons. These are hover-only
                // widgets, so they don't intercept the manual click/drag handling;
                // skipped while a window is shaded (its buttons aren't shown).
                let tips = match win {
                    ClWin::Main if !self.geom.main_shade => classic::main_tooltips(),
                    ClWin::Eq if !self.geom.eq_shade => classic::eq_tooltips(),
                    ClWin::Pl if !self.geom.pl_shade => {
                        classic::pl_tooltips(self.plview.px_w as i32, self.plview.px_h as i32)
                    }
                    _ => Vec::new(),
                };
                if nw > 0.0 && nh > 0.0 {
                    let (sx, sy) = (rect.width() / nw, rect.height() / nh);
                    for (label, x, y, w, h) in tips {
                        let min = rect.min + egui::vec2(x as f32 * sx, y as f32 * sy);
                        let r = egui::Rect::from_min_size(min, egui::vec2(w as f32 * sx, h as f32 * sy));
                        ui.interact(r, ui.id().with(("tip", label, x, y)), egui::Sense::hover())
                            .on_hover_text(label);
                    }
                }
                // Skin-defined mouse cursor for the region under the pointer.
                self.update_cursor(ctx, win, rect);
            });
    }

    fn compose_window(&self, win: ClWin) -> image::RgbaImage {
        let cs = self.classic.as_ref().unwrap();
        match win {
            ClWin::Main => {
                let st = self.make_main_state();
                if self.geom.main_shade {
                    cs.compose_main_shade(&st)
                } else {
                    cs.compose_main(&st)
                }
            }
            ClWin::Eq => {
                if self.geom.eq_shade {
                    cs.compose_eq_shade(self.geom.app_focused, self.player.volume(), self.balance)
                } else {
                    cs.compose_eq(&self.make_eq_state())
                }
            }
            ClWin::Pl => {
                if self.geom.pl_shade {
                    let (text, time) = match self.playlist.current_track() {
                        Some(t) => {
                            let n = self.playlist.current.map(|i| i + 1).unwrap_or(0);
                            let dur = t.duration.map(|d| d.as_secs_f32()).unwrap_or(0.0);
                            // Playing/paused: the live position, toggling
                            // elapsed/remaining like the main LCD. Stopped: the
                            // track length.
                            let time = if self.player.state() != PlayState::Stopped {
                                classic::fmt_pl_time_signed(
                                    self.player.position().as_secs_f32(),
                                    dur,
                                    self.time_remaining,
                                )
                            } else if dur > 0.0 {
                                classic::fmt_pl_time(dur)
                            } else {
                                String::new()
                            };
                            (format!("{}. {}", n, t.display_name()), time)
                        }
                        None => (String::new(), String::new()),
                    };
                    cs.compose_pl_shade(self.geom.app_focused, &text, &time, self.pl_colors().normal)
                } else {
                    // Build only the visible window; compose skips `first` rows, so
                    // the slice starts at pl_first and we pass first: 0 below.
                    let vis = classic::pl_visible_rows(self.plview.px_h as u32);
                    let end = (self.plview.first + vis).min(self.playlist.len());
                    let rows = self.build_pl_rows(self.plview.first, end);
                    let colors = self.pl_colors();
                    let elapsed_secs = match self.player.state() {
                        PlayState::Stopped => 0.0,
                        _ => self.player.position().as_secs_f32(),
                    };
                    let cur_duration = self
                        .playlist
                        .current_track()
                        .and_then(|t| t.duration)
                        .map(|d| d.as_secs_f32())
                        .unwrap_or(0.0);
                    let mut total_secs = 0.0;
                    let mut total_partial = false;
                    for t in &self.playlist.tracks {
                        match t.duration {
                            Some(d) => total_secs += d.as_secs_f32(),
                            None => total_partial = true,
                        }
                    }
                    let st = classic::PlState {
                        focused: self.geom.app_focused,
                        colors,
                        rows: &rows,
                        first: 0,
                        scroll_first: self.plview.first,
                        total_rows: self.playlist.len(),
                        pressed: self.pl_pressed_button(),
                        elapsed_secs,
                        cur_duration,
                        time_remaining: self.time_remaining,
                        total_secs,
                        total_partial,
                        time_hidden: self.time_blink_hidden,
                    };
                    cs.compose_playlist(&st, self.plview.px_w as u32, self.plview.px_h as u32)
                }
            }
        }
    }
}
