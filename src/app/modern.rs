//! The modern (non-classic) UI widgets: display, visualiser, seek bar,
//! transport, equalizer and playlist.

use egui::{Color32, RichText};

use super::*;

impl super::LinAmpApp {
    pub(super) fn ui_display(&mut self, ui: &mut egui::Ui, dt: f32) {
        let (title, dur) = match self.playlist.current_track() {
            Some(t) => (t.display_name(), t.duration),
            None => ("LinAmp - no track loaded".to_string(), None),
        };

        let pos = if self.seeking {
            Duration::from_secs_f32(self.seek_preview)
        } else {
            self.player.position()
        };

        let lcd = self.theme.lcd;
        let lcd_dim = self.theme.lcd_dim;
        egui::Frame::none()
            .fill(self.theme.list_bg)
            .inner_margin(egui::Margin::symmetric(10.0, 8.0))
            .stroke(Stroke::new(1.0_f32, lcd_dim))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    self.ui_album_art(ui, 64.0);
                    ui.add_space(8.0);
                    ui.vertical(|ui| {
                        ui.horizontal(|ui| {
                            let state_str = match self.player.state() {
                                PlayState::Playing => "\u{25B6}",
                                PlayState::Paused => "II",
                                PlayState::Stopped => "\u{25A0}",
                            };
                            ui.label(RichText::new(state_str).color(lcd).monospace().size(16.0));
                            ui.add_space(6.0);
                            ui.label(
                                RichText::new(fmt_time(pos)).color(lcd).monospace().size(22.0),
                            );
                            if let Some(d) = dur {
                                ui.label(
                                    RichText::new(format!("/ {}", fmt_time(d)))
                                        .color(lcd_dim)
                                        .monospace()
                                        .size(14.0),
                                );
                            }
                        });

                        let shown = marquee(&title, self.marquee_offset);
                        ui.label(RichText::new(shown).color(lcd).monospace().size(14.0));

                        if let Some(t) = self.playlist.current_track() {
                            let khz = t
                                .sample_rate
                                .map(|s| format!("{} kHz", s / 1000))
                                .unwrap_or_default();
                            let kbps =
                                t.bitrate.map(|b| format!("{b} kbps")).unwrap_or_default();
                            let ch = match t.channels {
                                Some(1) => "mono",
                                Some(_) => "stereo",
                                None => "",
                            };
                            ui.label(
                                RichText::new(format!("{kbps}   {khz}   {ch}"))
                                    .color(lcd_dim)
                                    .monospace()
                                    .size(11.0),
                            );
                        }
                    });
                });
            });

        let _ = dt;
    }

    pub(super) fn ui_viz(&mut self, ui: &mut egui::Ui) {
        let (resp, painter) =
            ui.allocate_painter(egui::vec2(ui.available_width(), 60.0), egui::Sense::click());
        if resp.clicked() {
            self.viz_mode = self.viz_mode.next();
        }
        let rect = resp.rect;
        painter.rect_filled(rect, 2.0, self.theme.viz_bg);
        painter.rect_stroke(rect, 2.0, Stroke::new(1.0_f32, self.theme.lcd_dim));

        match self.viz_mode {
            VizMode::Off => {
                painter.text(
                    rect.center(),
                    egui::Align2::CENTER_CENTER,
                    "visualization off (click to cycle)",
                    egui::FontId::proportional(11.0),
                    self.theme.lcd_dim,
                );
            }
            VizMode::Spectrum => {
                self.viz.snapshot(&mut self.viz_samples, FFT_SIZE);
                let rate = self.viz.rate();
                let bars =
                    viz::compute_bars(&self.viz_samples, &*self.fft, FFT_SIZE, VIZ_BANDS, rate);
                for (i, b) in self.viz_bars.iter_mut().enumerate() {
                    let target = bars.get(i).copied().unwrap_or(0.0);
                    if target > *b {
                        *b = target;
                    } else {
                        *b = (*b * 0.82).max(target); // peak decay
                    }
                }
                let n = self.viz_bars.len();
                let gap = 2.0;
                let bw = ((rect.width() - gap * (n as f32 + 1.0)) / n as f32).max(1.0);
                for (i, &b) in self.viz_bars.iter().enumerate() {
                    let x0 = rect.left() + gap + i as f32 * (bw + gap);
                    let h = b * (rect.height() - 4.0);
                    let bar = egui::Rect::from_min_max(
                        egui::pos2(x0, rect.bottom() - 2.0 - h),
                        egui::pos2(x0 + bw, rect.bottom() - 2.0),
                    );
                    let spec = &self.theme.viz_spectrum;
                    let col = if spec.is_empty() {
                        self.theme.lcd
                    } else {
                        let idx = (((1.0 - b) * (spec.len() - 1) as f32).round() as usize)
                            .min(spec.len() - 1);
                        spec[idx]
                    };
                    painter.rect_filled(bar, 0.0, col);
                }
            }
            VizMode::Oscilloscope => {
                let n = (rect.width() as usize).clamp(16, 2048);
                self.viz.snapshot(&mut self.viz_samples, n);
                let mid = rect.center().y;
                let amp = (rect.height() / 2.0) - 3.0;
                let denom = (self.viz_samples.len().max(2) - 1) as f32;
                let step = rect.width() / denom;
                let pts: Vec<egui::Pos2> = self
                    .viz_samples
                    .iter()
                    .enumerate()
                    .map(|(i, &s)| {
                        egui::pos2(rect.left() + i as f32 * step, mid - s.clamp(-1.0, 1.0) * amp)
                    })
                    .collect();
                if pts.len() >= 2 {
                    painter.add(egui::Shape::line(pts, Stroke::new(1.5_f32, self.theme.viz_osc)));
                }
            }
        }
    }

    pub(super) fn ui_seekbar(&mut self, ui: &mut egui::Ui) {
        let dur = self
            .playlist
            .current_track()
            .and_then(|t| t.duration)
            .map(|d| d.as_secs_f32())
            .unwrap_or(0.0);

        if !self.seeking {
            self.seek_preview = self.player.position().as_secs_f32();
        }

        let max = if dur > 0.0 { dur } else { 1.0 };
        self.seek_preview = self.seek_preview.clamp(0.0, max);
        let resp = ui.add_enabled(
            dur > 0.0,
            egui::Slider::new(&mut self.seek_preview, 0.0..=max)
                .show_value(false)
                .trailing_fill(true),
        );
        if resp.dragged() {
            self.seeking = true;
        }
        if resp.drag_stopped() {
            self.player.seek(Duration::from_secs_f32(self.seek_preview));
            self.seeking = false;
        }
    }

    pub(super) fn ui_transport(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            if ui.button(RichText::new("|<").monospace()).on_hover_text("Previous (Z)").clicked() {
                self.prev();
            }
            if ui.button(RichText::new(" > ").monospace()).on_hover_text("Play (X)").clicked() {
                self.play_pause();
            }
            if ui.button(RichText::new("II").monospace()).on_hover_text("Pause (C)").clicked() {
                self.player.pause();
            }
            if ui.button(RichText::new("[]").monospace()).on_hover_text("Stop (V)").clicked() {
                self.player.stop();
            }
            if ui.button(RichText::new(">|").monospace()).on_hover_text("Next (B)").clicked() {
                self.next();
            }

            ui.separator();

            let mut vol = self.player.volume();
            ui.label("Vol");
            if ui
                .add(egui::Slider::new(&mut vol, 0.0..=1.0).show_value(false))
                .changed()
            {
                self.player.set_volume(vol);
            }
        });

        ui.horizontal(|ui| {
            toggle(ui, &mut self.shuffle, "Shuffle (S)");
            toggle(ui, &mut self.repeat_all, "Repeat (R)");
            toggle(ui, &mut self.repeat_one, "Repeat 1");
            ui.separator();
            if ui.button("Open files (L)").clicked() {
                self.open_files();
            }
            if ui.button("Open folder").clicked() {
                self.open_folder();
            }
        });
    }

    pub(super) fn ui_equalizer(&mut self, ui: &mut egui::Ui) {
        egui::CollapsingHeader::new("Equalizer")
            .default_open(false)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    if toggle(ui, &mut self.eq_enabled, "EQ On").changed() {
                        self.apply_eq();
                    }
                    ui.separator();
                    ui.label("Preamp");
                    if ui
                        .add(egui::Slider::new(&mut self.eq_preamp_db, -12.0..=12.0).suffix(" dB"))
                        .changed()
                    {
                        self.apply_eq();
                    }
                    if ui.button("Reset").clicked() {
                        self.eq_bands = [0.0; 10];
                        self.eq_preamp_db = 0.0;
                        self.apply_eq();
                    }
                    if toggle(ui, &mut self.eq_auto, "Auto").changed() {
                        self.apply_eq_auto_gain();
                        self.apply_eq();
                    }
                    egui::ComboBox::from_id_salt("eq_preset")
                        .selected_text("Presets")
                        .show_ui(ui, |ui| {
                            for (i, (name, _, _)) in crate::eq::PRESETS.iter().enumerate() {
                                if ui.selectable_label(false, *name).clicked() {
                                    self.apply_eq_preset(i);
                                }
                            }
                        });
                });
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    for (i, label) in EQ_LABELS.iter().enumerate() {
                        ui.vertical(|ui| {
                            if ui
                                .add(
                                    egui::Slider::new(&mut self.eq_bands[i], -12.0..=12.0)
                                        .vertical()
                                        .show_value(false),
                                )
                                .changed()
                            {
                                self.apply_eq_auto_gain();
                                self.apply_eq();
                            }
                            ui.label(RichText::new(*label).size(9.0).color(self.theme.lcd_dim));
                        });
                    }
                });
            });
    }

    pub(super) fn ui_playlist(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label("Find:");
            ui.add(
                egui::TextEdit::singleline(&mut self.pl_search)
                    .hint_text("filter playlist")
                    .desired_width(130.0),
            );
            if !self.pl_search.is_empty() && ui.button("x").clicked() {
                self.pl_search.clear();
            }
            ui.separator();
            ui.label("Sort:");
            let prev = self.sort_key;
            egui::ComboBox::from_id_salt("sort_combo")
                .selected_text(self.sort_key.label())
                .show_ui(ui, |ui| {
                    for k in [
                        SortKey::Custom,
                        SortKey::Title,
                        SortKey::Artist,
                        SortKey::Album,
                        SortKey::Duration,
                    ] {
                        ui.selectable_value(&mut self.sort_key, k, k.label());
                    }
                });
            if self.sort_key != prev {
                self.sort_playlist();
            }
        });

        ui.horizontal(|ui| {
            ui.label(
                RichText::new(format!("{} tracks", self.playlist.len())).color(Color32::GRAY),
            );
            if ui.button("Save").clicked() {
                self.save_playlist_dialog();
            }
            if ui.button("Load").clicked() {
                self.load_playlist_dialog();
            }
            if ui.button("Clear").clicked() {
                self.playlist.clear();
                self.player.stop();
            }
        });
        ui.separator();

        let filtering = !self.pl_search.trim().is_empty();
        let needle = self.pl_search.to_lowercase();
        let current = self.playlist.current;

        let mut to_play: Option<usize> = None;
        let mut to_play_next: Option<usize> = None;
        let mut to_tag: Option<usize> = None;
        let mut to_explore: Option<std::path::PathBuf> = None;
        let mut to_remove: Option<usize> = None;
        let mut reorder: Option<(usize, usize)> = None;

        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for (i, track) in self.playlist.tracks.iter().enumerate() {
                    if filtering && !track.display_name().to_lowercase().contains(&needle) {
                        continue;
                    }
                    let is_cur = Some(i) == current;

                    let row = ui
                        .horizontal(|ui| {
                            if !filtering {
                                ui.dnd_drag_source(Id::new(("pl_grip", i)), i, |ui| {
                                    ui.label(RichText::new("\u{2630}").color(Color32::GRAY));
                                });
                            }

                            ui.label(
                                RichText::new(format!("{:>3}.", i + 1))
                                    .monospace()
                                    .color(Color32::GRAY),
                            );

                            let name = track.display_name();
                            let text = if is_cur {
                                RichText::new(name).color(self.theme.list_current).strong()
                            } else {
                                RichText::new(name).color(self.theme.list_normal)
                            };
                            let resp = ui.add(
                                egui::Label::new(text).sense(egui::Sense::click()).truncate(),
                            );
                            if resp.double_clicked() {
                                to_play = Some(i);
                            }
                            resp.context_menu(|ui| {
                                ui.set_min_width(150.0);
                                if ui.button("Play").clicked() {
                                    to_play = Some(i);
                                    ui.close_menu();
                                }
                                if ui.button("Play Next").clicked() {
                                    to_play_next = Some(i);
                                    ui.close_menu();
                                }
                                ui.separator();
                                if ui.button("Edit file tags…").clicked() {
                                    to_tag = Some(i);
                                    ui.close_menu();
                                }
                                if ui.button("Explore folder").clicked() {
                                    to_explore = Some(track.path.clone());
                                    ui.close_menu();
                                }
                                ui.separator();
                                if ui.button("Remove").clicked() {
                                    to_remove = Some(i);
                                    ui.close_menu();
                                }
                            });

                            if let Some(d) = track.duration {
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        ui.label(
                                            RichText::new(fmt_time(d))
                                                .monospace()
                                                .color(Color32::GRAY),
                                        );
                                    },
                                );
                            }
                        })
                        .response;

                    // Drag-and-drop reordering (only in unfiltered view).
                    if !filtering {
                        if row.dnd_hover_payload::<usize>().is_some() {
                            let rect = row.rect;
                            let above = ui
                                .input(|inp| inp.pointer.interact_pos())
                                .map(|p| p.y < rect.center().y)
                                .unwrap_or(false);
                            let y = if above { rect.top() } else { rect.bottom() };
                            ui.painter().hline(
                                rect.x_range(),
                                y,
                                Stroke::new(2.0_f32, self.theme.accent),
                            );
                        }
                        if let Some(payload) = row.dnd_release_payload::<usize>() {
                            let rect = row.rect;
                            let above = ui
                                .input(|inp| inp.pointer.interact_pos())
                                .map(|p| p.y < rect.center().y)
                                .unwrap_or(false);
                            let target = if above { i } else { i + 1 };
                            reorder = Some((*payload, target));
                        }
                    }
                }
            });

        if let Some(i) = to_play {
            self.play_index(i);
        }
        if let Some(i) = to_play_next {
            let (cur, _, _) =
                reorder_play_next(&mut self.playlist.tracks, self.playlist.current, &[i]);
            self.playlist.current = cur;
            self.sort_key = SortKey::Custom;
        }
        if let Some(i) = to_tag {
            // Edit by row index (not path) so saving refreshes the playlist entry.
            self.open_tag_editor_at(i);
        }
        if let Some(p) = to_explore {
            open_in_file_manager(&p);
        }
        if let Some(i) = to_remove {
            self.playlist.remove(i);
        }
        if let Some((from, to)) = reorder {
            self.move_track(from, to);
        }
    }
}
