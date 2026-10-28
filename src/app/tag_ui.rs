//! The file-info / ID3 tag editor window (the `Alt+3` dialog).

use std::path::PathBuf;

use egui::RichText;

use super::*;

/// State for the file-info / tag editor window.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum TagTab {
    Basic,
    Id3,
}

pub(super) struct TagEditor {
    index: usize,
    path: PathBuf,
    info: crate::tags::FileInfo,
    v1: crate::tags::Id3Fields,
    v2: crate::tags::Id3Fields,
    v1_on: bool,
    v2_on: bool,
    tab: TagTab,
    editable: bool,
    /// true for a format with a single native tag (FLAC/Ogg/M4A): the Id3 tab
    /// shows one "Tags" column instead of the ID3v1/ID3v2 pair, and saving writes
    /// that one tag.
    single: bool,
    status: String,
}

/// Read a file's tags for the editor. MP3 keeps the ID3v1/v2 pair; other formats
/// read their single native tag into `v2` (shown as one "Tags" column). Returns
/// `(v1, v2, v1_on, v2_on, single)`.
fn read_editor_tags(
    path: &std::path::Path,
) -> (crate::tags::Id3Fields, crate::tags::Id3Fields, bool, bool, bool) {
    if crate::tags::uses_id3(path) {
        let (v1, v2) = crate::tags::read_id3(path);
        let (on1, on2) = (v1.is_some(), v2.is_some());
        (v1.unwrap_or_default(), v2.unwrap_or_default(), on1, on2, false)
    } else {
        (Default::default(), crate::tags::read_single(path), false, true, true)
    }
}

/// One grid row (label + field) in points, including the grid's vertical
/// spacing. The editor renders at the default scale, so this is a constant.
const TAG_ROW_H: f32 = 23.0;
/// Fixed chrome height around the content box: the path field, the tab strip, the
/// group-frame margins, the button row and the panel padding.
const TAG_WINDOW_CHROME: f32 = 132.0;
/// Extra height reserved for the save-error status line (editable files only).
const TAG_STATUS_H: f32 = 24.0;

/// Height of the content box, sized to the tallest tab. When editable the ID3 tab
/// is tallest: 15 fields (see `tag_fields_grid`), the 2-line Comment counting as
/// an extra row, plus a header row. Otherwise only the 9-row Basic Info tab shows.
/// Keep the counts in step with the render code.
fn tag_box_height(editable: bool) -> f32 {
    let rows = if editable { 17.0 } else { 9.0 };
    rows * TAG_ROW_H
}

/// The genre dropdown shared by the ID3 tabs (closes on a keyboard pick).
fn genre_combo(ui: &mut egui::Ui, salt: &str, val: &mut String) {
    let combo = egui::ComboBox::from_id_salt(salt)
        .selected_text(if val.is_empty() { ", ".into() } else { val.clone() })
        .width(140.0)
        .show_ui(ui, |ui| {
            let mut picked = false;
            for g in crate::tags::GENRES {
                if ui.selectable_label(val == g, *g).clicked() {
                    *val = (*g).to_string();
                    picked = true;
                }
            }
            picked
        });
    if combo.inner == Some(true) {
        ui.memory_mut(|m| m.close_popup());
        combo.response.request_focus();
    }
}

/// The full field grid for an ID3v2 tag or a single native tag (FLAC/Ogg/M4A).
/// `salt` keeps the grid and its genre combo uniquely identified per column.
fn tag_fields_grid(ui: &mut egui::Ui, f: &mut crate::tags::Id3Fields, salt: &str) {
    egui::Grid::new(salt).num_columns(2).spacing([6.0, 4.0]).show(ui, |ui| {
        let row = |ui: &mut egui::Ui, l: &str, v: &mut String| {
            ui.label(l);
            ui.add(egui::TextEdit::singleline(v).desired_width(170.0));
            ui.end_row();
        };
        row(ui, "Track", &mut f.track);
        row(ui, "Disc", &mut f.disc);
        row(ui, "Title", &mut f.title);
        row(ui, "Artist", &mut f.artist);
        row(ui, "Album", &mut f.album);
        row(ui, "Year", &mut f.year);
        ui.label("Genre");
        genre_combo(ui, &format!("{salt}_genre"), &mut f.genre);
        ui.end_row();
        ui.label("Comment");
        ui.add(egui::TextEdit::multiline(&mut f.comment).desired_width(170.0).desired_rows(2));
        ui.end_row();
        row(ui, "Composer", &mut f.composer);
        row(ui, "Orig. Artist", &mut f.orig_artist);
        row(ui, "Copyright", &mut f.copyright);
        row(ui, "URL", &mut f.url);
        row(ui, "Encoded by", &mut f.encoded_by);
        row(ui, "BPM", &mut f.bpm);
        ui.label("Compilation");
        ui.checkbox(&mut f.compilation, "");
        ui.end_row();
    });
}

impl super::LinAmpApp {
    /// Open the file-info / tag editor for the selected row (or current track).
    pub(super) fn open_tag_editor(&mut self) {
        let idx = if self.plview.selection.len() == 1 {
            self.plview.selection.iter().next().copied()
        } else {
            self.playlist.current
        }
        .or(if self.playlist.is_empty() { None } else { Some(0) });

        let Some(index) = idx else {
            self.status = "No track to edit.".into();
            return;
        };
        self.open_tag_editor_at(index);
    }

    /// Open the tag editor for a specific playlist row, so saving writes back to
    /// that row (and refreshes its cached metadata).
    pub(super) fn open_tag_editor_at(&mut self, index: usize) {
        let Some(track) = self.playlist.tracks.get(index) else {
            return;
        };
        let path = track.path.clone();
        let (v1, v2, v1_on, v2_on, single) = read_editor_tags(&path);
        let info = crate::tags::read_info(&path);
        let editable = crate::tags::can_edit(&path);
        // No read-only banner: a non-editable file simply shows the Basic Info tab
        // alone (the ID3 tab is hidden). Status is for save errors only.
        let status = String::new();
        self.tag_editor = Some(TagEditor {
            index,
            path,
            info,
            v1,
            v2,
            v1_on,
            v2_on,
            tab: TagTab::Basic,
            editable,
            single,
            status,
        });
    }

    /// Open the tag editor for a file by path (example: a Media Library row that may
    /// not be in the playlist). `index` is a sentinel so saving doesn't touch a
    /// playlist row.
    pub(super) fn open_tag_editor_path(&mut self, path: std::path::PathBuf) {
        let (v1, v2, v1_on, v2_on, single) = read_editor_tags(&path);
        let info = crate::tags::read_info(&path);
        let editable = crate::tags::can_edit(&path);
        // No read-only banner: a non-editable file simply shows the Basic Info tab
        // alone (the ID3 tab is hidden). Status is for save errors only.
        let status = String::new();
        self.tag_editor = Some(TagEditor {
            index: usize::MAX,
            path,
            info,
            v1,
            v2,
            v1_on,
            v2_on,
            tab: TagTab::Basic,
            editable,
            single,
            status,
        });
    }

    /// Write the editor's tags back to the file and refresh the playlist row.
    /// Returns `true` when the write succeeded (so the caller can close).
    pub(super) fn save_tag_editor(&mut self) -> bool {
        let (index, path, v1, v2, v1_on, v2_on, single) = match &self.tag_editor {
            Some(ed) if ed.editable => (
                ed.index,
                ed.path.clone(),
                ed.v1.clone(),
                ed.v2.clone(),
                ed.v1_on,
                ed.v2_on,
                ed.single,
            ),
            _ => return false,
        };
        let result = if single {
            // Non-MP3 formats carry one native tag, held in `v2`.
            crate::tags::write_single(&path, &v2)
        } else {
            crate::tags::write_id3(
                &path,
                v1_on.then_some(&v1),
                v2_on.then_some(&v2),
                self.prefs.id3v23,
            )
        };
        if result.is_ok() {
            // Refresh the playlist row (when the edit came from the playlist).
            let mut refreshed = None;
            if let Some(t) = self.playlist.tracks.get_mut(index) {
                t.loaded = false;
                t.load();
                refreshed = Some(t.clone());
            }
            // Reflect the new tags in the library DB, but only for a track it
            // already knows (a no-op otherwise).
            let lib_changed = if let Some(lib) = &self.library {
                let track = refreshed.unwrap_or_else(|| {
                    let mut f = crate::playlist::Track::new(path.clone());
                    f.load();
                    f
                });
                lib.update_tags(&track)
            } else {
                false
            };
            // Re-query the visible library so the edited row shows its new tags
            // right away instead of staying stale until the next re-navigation.
            if lib_changed {
                self.refresh_library();
            }
        }
        // On success the caller closes the window, so only an error needs a
        // status message left behind.
        if let Err(e) = &result {
            if let Some(ed) = self.tag_editor.as_mut() {
                ed.status = format!("Error: {e}");
            }
        }
        result.is_ok()
    }

    /// The tag editor window (own viewport, like preferences / library).
    pub(super) fn ui_tag_editor(&mut self, ctx: &egui::Context) {
        let Some(editable) = self.tag_editor.as_ref().map(|e| e.editable) else {
            return;
        };
        // Size the window to its tallest tab from the (fixed) row counts, so no
        // content is clipped and the height doesn't jump when switching tabs. The
        // ID3 tab is the tallest when the file is editable; otherwise only the
        // Basic Info tab shows.
        let box_h = tag_box_height(editable);
        let win_h = box_h + TAG_WINDOW_CHROME + if editable { TAG_STATUS_H } else { 0.0 };
        let size = egui::vec2(660.0, win_h);
        // Keep the window on-screen: a tall editor opened near the bottom/right edge
        // would otherwise spill off the desktop.
        let mut screen = self.geom.win_pos[0] + egui::vec2(32.0, 32.0);
        if let Some(vr) = virtual_desktop_rect(ctx) {
            screen.x = screen.x.clamp(vr.min.x, (vr.max.x - size.x).max(vr.min.x));
            screen.y = screen.y.clamp(vr.min.y, (vr.max.y - size.y).max(vr.min.y));
        }
        let builder = egui::ViewportBuilder::default()
            .with_title("LinAmp, File Info")
            .with_position(screen)
            .with_inner_size(size)
            .with_resizable(false)
            .with_taskbar(false)
            .with_window_level(self.window_level());
        let id = egui::ViewportId::from_hash_of("linamp_tags");
        let mut close = false;
        let mut do_save = false;
        ctx.show_viewport_immediate(id, builder, |ctx, _| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let ed = self.tag_editor.as_mut().unwrap();
                let on = ed.editable;

                // Escape closes the window without saving. Consume it here, before
                // the text fields can swallow it, but let an open dropdown close
                // first.
                if !ctx.memory(|m| m.any_popup_open())
                    && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape))
                {
                    close = true;
                }
                // While Alt is held the letters are menu shortcuts, not text, so
                // drop the character events before a focused field types them (example:
                // Alt-U would otherwise insert a "u"). AltGr (Ctrl+Alt) is left
                // alone so accented input still works.
                ctx.input_mut(|i| {
                    if i.modifiers.alt && !i.modifiers.ctrl {
                        i.events.retain(|e| !matches!(e, egui::Event::Text(_)));
                    }
                });

                // Theme-derived colours so the dialog tracks the active skin.
                let line = ui.visuals().widgets.noninteractive.bg_stroke;
                let panel = ui.visuals().panel_fill;
                let idle_fill = ui.visuals().faint_bg_color;
                // Outline idle widgets so buttons/fields don't blur into the panel.
                ui.visuals_mut().widgets.inactive.bg_stroke = line;

                // Path, in a sunken box that sets it apart from the tags below.
                egui::Frame::group(ui.style())
                    .fill(ui.visuals().extreme_bg_color)
                    .show(ui, |ui| {
                        ui.set_min_width(624.0);
                        ui.add(
                            egui::Label::new(RichText::new(&ed.info.path).strong()).truncate(),
                        );
                    });
                ui.add_space(4.0);

                // Tab strip that sits on, and joins into, the content box below.
                let top_round = egui::Rounding { nw: 4.0, ne: 4.0, sw: 0.0, se: 0.0 };
                let tab_btn = |ui: &mut egui::Ui, label: &str, active: bool| {
                    let fill = if active { panel } else { idle_fill };
                    ui.add(
                        egui::Button::new(mnem(label, 0))
                            .fill(fill)
                            .stroke(line)
                            .rounding(top_round)
                            .min_size(egui::vec2(0.0, 24.0)),
                    )
                };
                let prev_y = ui.spacing().item_spacing.y;
                ui.spacing_mut().item_spacing.y = 0.0;
                let (basic_resp, id3_resp) = ui
                    .horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 2.0;
                        let b = tab_btn(ui, "Basic Info", ed.tab == TagTab::Basic);
                        // A non-editable file has no writable tag, so only Basic Info
                        // shows: no ID3 tab.
                        let i = on.then(|| tab_btn(ui, "ID3", ed.tab == TagTab::Id3));
                        (b, i)
                    })
                    .inner;
                if basic_resp.clicked() {
                    ed.tab = TagTab::Basic;
                }
                if let Some(i) = &id3_resp {
                    if i.clicked() {
                        ed.tab = TagTab::Id3;
                    }
                }
                if !on {
                    ed.tab = TagTab::Basic;
                }

                let group = egui::Frame::group(ui.style()).fill(panel).show(ui, |ui| {
                    ui.set_min_width(624.0);
                    ui.set_min_height(box_h);
                    match ed.tab {
                        TagTab::Basic => {
                            egui::Grid::new("basic_grid")
                                .num_columns(2)
                                .spacing([12.0, 4.0])
                                .show(ui, |ui| {
                                    let i = &ed.info;
                                    let mut r = |k: &str, v: String| {
                                        ui.label(k);
                                        ui.label(v);
                                        ui.end_row();
                                    };
                                    r("Size", format!("{} bytes", i.size_bytes));
                                    r("Length", format!("{} seconds", i.length_secs));
                                    r("Format", i.mpeg.clone());
                                    r("Bitrate", format!("{} kbps", i.bitrate_kbps));
                                    r("Sample rate", format!("{} Hz", i.sample_rate_hz));
                                    r("Channels", i.channel_mode.clone());
                                    r("Copyrighted", if i.copyrighted { "Yes" } else { "No" }.into());
                                    r("Original", if i.original { "Yes" } else { "No" }.into());
                                    r("Emphasis", i.emphasis.clone());
                                });
                            None
                        }
                        TagTab::Id3 if ed.single => {
                            // One native tag (FLAC/Ogg Vorbis comments, MP4 atoms):
                            // a single column, no ID3v1/ID3v2 split or copy buttons.
                            ui.label(RichText::new("Tags").strong());
                            ui.add_space(2.0);
                            ui.add_enabled_ui(on, |ui| {
                                tag_fields_grid(ui, &mut ed.v2, "single_grid");
                            });
                            None
                        }
                        TagTab::Id3 => {
                            let resps = ui.columns(2, |cols| {
                                // ----- ID3v1 (left) -----
                                let r1 = cols[0].checkbox(&mut ed.v1_on, mnem("ID3v1 Tag", 4));
                                cols[0].add_enabled_ui(on && ed.v1_on, |ui| {
                                    egui::Grid::new("v1_grid").num_columns(2).spacing([6.0, 4.0]).show(ui, |ui| {
                                        let f = &mut ed.v1;
                                        let lim = |ui: &mut egui::Ui, l: &str, v: &mut String, n: usize| {
                                            ui.label(l);
                                            ui.add(egui::TextEdit::singleline(v).desired_width(150.0).char_limit(n));
                                            ui.end_row();
                                        };
                                        lim(ui, "Track", &mut f.track, 3);
                                        lim(ui, "Title", &mut f.title, crate::tags::V1_MAX_TEXT);
                                        lim(ui, "Artist", &mut f.artist, crate::tags::V1_MAX_TEXT);
                                        lim(ui, "Album", &mut f.album, crate::tags::V1_MAX_TEXT);
                                        lim(ui, "Year", &mut f.year, crate::tags::V1_MAX_YEAR);
                                        ui.label("Genre");
                                        genre_combo(ui, "v1_genre", &mut f.genre);
                                        ui.end_row();
                                        ui.label("Comment");
                                        ui.add(egui::TextEdit::multiline(&mut f.comment).desired_width(150.0).desired_rows(2).char_limit(crate::tags::V1_MAX_TEXT));
                                        ui.end_row();
                                    });
                                });
                                // ----- ID3v2 (right) -----
                                let r2 = cols[1].checkbox(&mut ed.v2_on, mnem("ID3v2 Tag", 4));
                                cols[1].add_enabled_ui(on && ed.v2_on, |ui| {
                                    tag_fields_grid(ui, &mut ed.v2, "v2_grid");
                                });
                                (r1, r2)
                            });
                            Some(resps)
                        }
                    }
                });
                let checks = group.inner;
                // Erase the seam under the active tab so it reads as one surface
                // with the content box (the classic folder-tab join).
                let active_rect = match (&id3_resp, ed.tab) {
                    (Some(i), TagTab::Id3) => i.rect,
                    _ => basic_resp.rect,
                };
                // Span from the tab's own bottom edge to the box's top edge so
                // the join holds even if the tab row is taller than the button.
                let box_top = group.response.rect.top();
                let tab_bot = active_rect.bottom();
                let seam = egui::Rect::from_x_y_ranges(
                    (active_rect.left() + line.width)..=(active_rect.right() - line.width),
                    (tab_bot.min(box_top) - 1.0)..=(tab_bot.max(box_top) + 1.0),
                );
                ui.painter().rect_filled(seam, 0.0, panel);
                ui.spacing_mut().item_spacing.y = prev_y;

                // Only surface an error (example: a failed write), no "changed"
                // chatter, the dialog is busy enough.
                ui.add_space(6.0);
                if !ed.status.is_empty() {
                    ui.label(RichText::new(&ed.status).color(ui.visuals().error_fg_color));
                }
                ui.add_space(4.0);

                // Shortcuts: Alt-B/Alt-I pick the tab, Alt-1/2 toggle the tags,
                // Alt-U update, Alt-C cancel, Alt-T/Alt-F copy to/from ID3v1 (read
                // raw so a focused field doesn't swallow them). Each shortcut also
                // moves keyboard focus to the control it acts on, so a following
                // Tab continues from there.
                let (k_basic, k_id3, k_v1, k_v2, k_up, k_cancel, k_to, k_from) = ctx.input(|i| {
                    let a = i.modifiers.alt;
                    (
                        a && i.key_pressed(egui::Key::B),
                        a && i.key_pressed(egui::Key::I),
                        a && i.key_pressed(egui::Key::Num1),
                        a && i.key_pressed(egui::Key::Num2),
                        a && i.key_pressed(egui::Key::U),
                        a && i.key_pressed(egui::Key::C),
                        a && i.key_pressed(egui::Key::T),
                        a && i.key_pressed(egui::Key::F),
                    )
                });
                if k_basic {
                    ed.tab = TagTab::Basic;
                    basic_resp.request_focus();
                }
                if k_id3 {
                    if let Some(i) = &id3_resp {
                        ed.tab = TagTab::Id3;
                        i.request_focus();
                    }
                }
                let id3 = ed.tab == TagTab::Id3;
                if on && id3 && k_v1 {
                    ed.v1_on = !ed.v1_on;
                    if let Some((r1, _)) = &checks {
                        r1.request_focus();
                    }
                }
                if on && id3 && k_v2 {
                    ed.v2_on = !ed.v2_on;
                    if let Some((_, r2)) = &checks {
                        r2.request_focus();
                    }
                }
                if on && id3 && k_up {
                    do_save = true;
                }

                // Copying to/from ID3v1 only applies to MP3 (the single-tag
                // formats have no ID3v1).
                let can_copy = on && id3 && ed.v2_on && !ed.single;
                let mut do_copy_to = k_to && can_copy;
                let mut do_copy_from = k_from && can_copy;
                ui.horizontal(|ui| {
                    // Update and the copy buttons only apply to the ID3 tab.
                    if id3 {
                        ui.add_enabled_ui(on, |ui| {
                            if ui.button(mnem("Update", 0)).clicked() {
                                do_save = true;
                            }
                        });
                    }
                    if ui.button(mnem("Cancel", 0)).clicked() {
                        close = true;
                    }
                    if id3 && !ed.single {
                        ui.add_enabled_ui(can_copy, |ui| {
                            let to = ui.button(mnem("Copy to ID3v1", 5));
                            if to.clicked() {
                                do_copy_to = true;
                            }
                            if k_to {
                                to.request_focus();
                            }
                            let from = ui.button(mnem("Copy from ID3v1", 5));
                            if from.clicked() {
                                do_copy_from = true;
                            }
                            if k_from {
                                from.request_focus();
                            }
                        });
                    }
                });
                if k_cancel {
                    close = true;
                }
                // Copy only non-blank source fields so a copy never wipes a value.
                if do_copy_to {
                    ed.v1_on = true; // checking v1 un-greys its fields first
                    let cp = |dst: &mut String, src: &str, n: usize| {
                        if !src.trim().is_empty() {
                            *dst = src.chars().take(n).collect();
                        }
                    };
                    let max = usize::MAX;
                    cp(&mut ed.v1.track, &ed.v2.track, max);
                    cp(&mut ed.v1.title, &ed.v2.title, crate::tags::V1_MAX_TEXT);
                    cp(&mut ed.v1.artist, &ed.v2.artist, crate::tags::V1_MAX_TEXT);
                    cp(&mut ed.v1.album, &ed.v2.album, crate::tags::V1_MAX_TEXT);
                    cp(&mut ed.v1.year, &ed.v2.year, crate::tags::V1_MAX_YEAR);
                    cp(&mut ed.v1.genre, &ed.v2.genre, max);
                    cp(&mut ed.v1.comment, &ed.v2.comment, crate::tags::V1_MAX_TEXT);
                }
                if do_copy_from {
                    let cp = |dst: &mut String, src: &str| {
                        if !src.trim().is_empty() {
                            *dst = src.to_string();
                        }
                    };
                    cp(&mut ed.v2.track, &ed.v1.track);
                    cp(&mut ed.v2.title, &ed.v1.title);
                    cp(&mut ed.v2.artist, &ed.v1.artist);
                    cp(&mut ed.v2.album, &ed.v1.album);
                    cp(&mut ed.v2.year, &ed.v1.year);
                    cp(&mut ed.v2.genre, &ed.v1.genre);
                    cp(&mut ed.v2.comment, &ed.v1.comment);
                }
            });
            self.apply_base_cursor(ctx, "LinAmp, File Info");
            // Window-close (the title-bar X): discard without saving. Escape is
            // handled at the top of the panel, before the fields can swallow it.
            if ctx.input(|i| i.viewport().close_requested()) {
                close = true;
            }
        });
        // Cancel/Esc discards; Update closes only when the write succeeds, so a
        // failed save leaves the window open with its error.
        if close {
            self.tag_editor = None;
        } else if do_save && self.save_tag_editor() {
            self.tag_editor = None;
        }
    }
}
