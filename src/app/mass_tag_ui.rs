//! The mass tag editor: edit the tags of several selected files at once, from the
//! playlist or the Media Library. Modeled on the single-file tag editor
//! ([`super::tag_ui`]), but it carries a set of files, pre-fills each field with
//! the most common value across them (the mode), and on "Update All" writes only
//! the fields the user actually edited into every file, leaving unedited fields
//! (and unique values like Title) alone. See [`crate::tags::write_mass`].

use std::path::PathBuf;
use std::time::Duration;

use egui::RichText;

use super::*;
use crate::tags::{FieldMask, Id3Fields};

/// Which tab the mass editor shows.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum MassTab {
    Basic,
    Tags,
}

/// A file fed into the mass editor, with the playlist row to refresh after a save
/// (`None` when it came from the library) and the cached duration for the Basic
/// Info total length (so the aggregate needs no per-file parse).
pub(super) struct MassSource {
    pub path: PathBuf,
    pub pl_index: Option<usize>,
    pub duration: Option<Duration>,
}

/// Quick aggregates for the Basic Info tab (no per-file deep parse).
struct MassBasic {
    files: usize,
    selected: usize,
    total_size: u64,
    total_secs: u64,
    /// Editable formats, label and count, most common first.
    formats: Vec<(String, usize)>,
    /// Selected formats we can't tag-edit (MIDI, tracker modules, …), shown in
    /// parentheses after the editable ones so the Formats counts still sum to the
    /// selection.
    formats_other: Vec<(String, usize)>,
    artists: usize,
    albums: usize,
}

pub(super) struct MassTagEditor {
    /// The editable files, in selection order.
    paths: Vec<PathBuf>,
    /// Playlist rows to refresh after a save (a subset of `paths`' origins).
    pl_indices: Vec<usize>,
    /// The main column's field values, pre-filled with the per-field mode. This is the
    /// ID3v2 tag for MP3, or the single native tag for other formats.
    fields: Id3Fields,
    /// The pre-filled mode, kept so a field whose value ends up different from it
    /// counts as edited even if the live change flag missed the edit.
    initial: Id3Fields,
    /// Which main-column fields the user edited; only these are written on Update All.
    mask: FieldMask,
    /// The ID3v1 column (MP3 selections only), pre-filled with the per-field mode of
    /// the selected files' ID3v1 tags, with its own change mask. Shown beside the
    /// ID3v2 column when [`Self::dominant_id3`] is set, like the single-file editor.
    v1: Id3Fields,
    initial_v1: Id3Fields,
    v1_mask: FieldMask,
    tab: MassTab,
    /// The most common tag model uses ID3 (MP3/MP2/MP1), so the Tags tab resembles
    /// the ID3 editor and is labeled "ID3"; otherwise it is labeled "Tags".
    dominant_id3: bool,
    /// The selection spans more than one file type, so the footer notes it.
    multi_type: bool,
    basic: MassBasic,
    status: String,
}

/// The most common value in `vals`, ties broken to the first one seen. Empty
/// strings count like any other value (a field most files leave blank pre-fills
/// blank). Returns an empty string for an empty input.
fn mode_str(vals: &[String]) -> String {
    let mut counts: Vec<(&str, usize)> = Vec::new();
    for v in vals {
        if let Some(e) = counts.iter_mut().find(|(k, _)| *k == v.as_str()) {
            e.1 += 1;
        } else {
            counts.push((v.as_str(), 1));
        }
    }
    // Keep the first-seen value on a tie: replace only on a strictly greater count.
    let mut best: Option<(&str, usize)> = None;
    for (k, c) in counts {
        if best.is_none_or(|(_, bc)| c > bc) {
            best = Some((k, c));
        }
    }
    best.map(|(k, _)| k.to_string()).unwrap_or_default()
}

/// Pre-fill every field with its mode across the selected files.
fn compute_modes(items: &[Id3Fields]) -> Id3Fields {
    let col = |pick: fn(&Id3Fields) -> &String| -> String {
        let vals: Vec<String> = items.iter().map(|i| pick(i).clone()).collect();
        mode_str(&vals)
    };
    let trues = items.iter().filter(|i| i.compilation).count();
    let compilation = if items.is_empty() {
        false
    } else if trues * 2 == items.len() {
        items[0].compilation
    } else {
        trues * 2 > items.len()
    };
    Id3Fields {
        track: col(|i| &i.track),
        title: col(|i| &i.title),
        artist: col(|i| &i.artist),
        album: col(|i| &i.album),
        year: col(|i| &i.year),
        genre: col(|i| &i.genre),
        comment: col(|i| &i.comment),
        composer: col(|i| &i.composer),
        orig_artist: col(|i| &i.orig_artist),
        copyright: col(|i| &i.copyright),
        url: col(|i| &i.url),
        encoded_by: col(|i| &i.encoded_by),
        disc: col(|i| &i.disc),
        bpm: col(|i| &i.bpm),
        compilation,
    }
}

/// Combine the live change flags with a value-vs-mode diff, so a field the user
/// edited is written even if the per-frame change flag missed it. A field left at
/// the mode value with no interaction stays unmarked (its per-file value is kept).
fn effective_mask(base: FieldMask, cur: &Id3Fields, init: &Id3Fields) -> FieldMask {
    FieldMask {
        track: base.track || cur.track != init.track,
        title: base.title || cur.title != init.title,
        artist: base.artist || cur.artist != init.artist,
        album: base.album || cur.album != init.album,
        year: base.year || cur.year != init.year,
        genre: base.genre || cur.genre != init.genre,
        comment: base.comment || cur.comment != init.comment,
        composer: base.composer || cur.composer != init.composer,
        orig_artist: base.orig_artist || cur.orig_artist != init.orig_artist,
        copyright: base.copyright || cur.copyright != init.copyright,
        url: base.url || cur.url != init.url,
        encoded_by: base.encoded_by || cur.encoded_by != init.encoded_by,
        disc: base.disc || cur.disc != init.disc,
        bpm: base.bpm || cur.bpm != init.bpm,
        compilation: base.compilation || cur.compilation != init.compilation,
    }
}

/// True when the most common tag model among `paths` uses ID3 (ties count as ID3).
fn dominant_id3(paths: &[PathBuf]) -> bool {
    let id3 = paths.iter().filter(|p| crate::tags::uses_id3(p)).count();
    id3 * 2 >= paths.len()
}

/// True when `paths` carry more than one distinct file extension.
fn multi_type(paths: &[PathBuf]) -> bool {
    let mut seen: Vec<String> = Vec::new();
    for p in paths {
        let ext = p
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_ascii_lowercase())
            .unwrap_or_default();
        if !seen.contains(&ext) {
            seen.push(ext);
        }
    }
    seen.len() > 1
}

/// Count each file's format label, most common first.
fn format_breakdown(paths: &[PathBuf]) -> Vec<(String, usize)> {
    let mut counts: Vec<(String, usize)> = Vec::new();
    for p in paths {
        let label = crate::tags::format_name(p);
        if let Some(e) = counts.iter_mut().find(|(k, _)| *k == label) {
            e.1 += 1;
        } else {
            counts.push((label, 1));
        }
    }
    counts.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    counts
}

/// Number of distinct non-empty values of one field across the read file fields.
fn distinct_nonempty(items: &[Id3Fields], pick: fn(&Id3Fields) -> &String) -> usize {
    let mut seen: Vec<&str> = Vec::new();
    for i in items {
        let v = pick(i).trim();
        if !v.is_empty() && !seen.contains(&v) {
            seen.push(v);
        }
    }
    seen.len()
}

/// One grid row (label + field), one row of grid spacing. Matches the single-file
/// editor's grid so the window height stays in step.
const ROW_H: f32 = 23.0;
/// Fixed chrome: the header box, tab strip, frame margins, button row, padding.
const CHROME: f32 = 138.0;
/// Reserved for the save-error / multiple-types footer line.
const FOOTER_H: f32 = 24.0;

/// The ID3v1 field grid (the v1 subset, with ID3v1's char limits), recording into
/// `mask` which fields the user edits. Mirrors the single-file editor's ID3v1 column.
fn mass_v1_fields_grid(ui: &mut egui::Ui, f: &mut Id3Fields, mask: &mut FieldMask, salt: &str) {
    use crate::tags::{V1_MAX_TEXT, V1_MAX_YEAR};
    egui::Grid::new(salt).num_columns(2).spacing([6.0, 4.0]).show(ui, |ui| {
        let lim = |ui: &mut egui::Ui, l: &str, v: &mut String, dirty: &mut bool, n: usize| {
            ui.label(l);
            let r = ui.add(egui::TextEdit::singleline(v).desired_width(150.0).char_limit(n));
            *dirty |= r.changed();
            ui.end_row();
        };
        lim(ui, "Track", &mut f.track, &mut mask.track, 3);
        lim(ui, "Title", &mut f.title, &mut mask.title, V1_MAX_TEXT);
        lim(ui, "Artist", &mut f.artist, &mut mask.artist, V1_MAX_TEXT);
        lim(ui, "Album", &mut f.album, &mut mask.album, V1_MAX_TEXT);
        lim(ui, "Year", &mut f.year, &mut mask.year, V1_MAX_YEAR);
        ui.label("Genre");
        let before = f.genre.clone();
        egui::ComboBox::from_id_salt(format!("{salt}_genre"))
            .selected_text(f.genre.clone())
            .width(140.0)
            .show_ui(ui, |ui| {
                for g in crate::tags::GENRES {
                    if ui.selectable_label(&f.genre == g, *g).clicked() {
                        f.genre = (*g).to_string();
                    }
                }
            });
        if f.genre != before {
            mask.genre = true;
        }
        ui.end_row();
        ui.label("Comment");
        let r = ui.add(
            egui::TextEdit::multiline(&mut f.comment)
                .desired_width(150.0)
                .desired_rows(2)
                .char_limit(V1_MAX_TEXT),
        );
        mask.comment |= r.changed();
        ui.end_row();
    });
}

/// The shared field grid, recording into `mask` which fields the user edits.
/// Mirrors `tag_ui::tag_fields_grid` so the two editors read the same.
fn mass_fields_grid(ui: &mut egui::Ui, f: &mut Id3Fields, mask: &mut FieldMask, salt: &str) {
    egui::Grid::new(salt).num_columns(2).spacing([6.0, 4.0]).show(ui, |ui| {
        let row = |ui: &mut egui::Ui, l: &str, v: &mut String, dirty: &mut bool| {
            ui.label(l);
            let r = ui.add(egui::TextEdit::singleline(v).desired_width(170.0));
            *dirty |= r.changed();
            ui.end_row();
        };
        row(ui, "Track", &mut f.track, &mut mask.track);
        row(ui, "Disc", &mut f.disc, &mut mask.disc);
        row(ui, "Title", &mut f.title, &mut mask.title);
        row(ui, "Artist", &mut f.artist, &mut mask.artist);
        row(ui, "Album", &mut f.album, &mut mask.album);
        row(ui, "Year", &mut f.year, &mut mask.year);
        ui.label("Genre");
        let before = f.genre.clone();
        egui::ComboBox::from_id_salt(format!("{salt}_genre"))
            .selected_text(f.genre.clone())
            .width(140.0)
            .show_ui(ui, |ui| {
                for g in crate::tags::GENRES {
                    if ui.selectable_label(&f.genre == g, *g).clicked() {
                        f.genre = (*g).to_string();
                    }
                }
            });
        if f.genre != before {
            mask.genre = true;
        }
        ui.end_row();
        ui.label("Comment");
        let r =
            ui.add(egui::TextEdit::multiline(&mut f.comment).desired_width(170.0).desired_rows(2));
        mask.comment |= r.changed();
        ui.end_row();
        row(ui, "Composer", &mut f.composer, &mut mask.composer);
        row(ui, "Orig. Artist", &mut f.orig_artist, &mut mask.orig_artist);
        row(ui, "Copyright", &mut f.copyright, &mut mask.copyright);
        row(ui, "URL", &mut f.url, &mut mask.url);
        row(ui, "Encoded by", &mut f.encoded_by, &mut mask.encoded_by);
        row(ui, "BPM", &mut f.bpm, &mut mask.bpm);
        ui.label("Compilation");
        let r = ui.checkbox(&mut f.compilation, "");
        mask.compilation |= r.changed();
        ui.end_row();
    });
}

impl super::LinAmpApp {
    /// Open the mass tag editor over `sources`. Non-editable rows and audio-CD
    /// tracks (no file) are filtered out; if nothing editable remains the editor
    /// does not open and a status line says so.
    pub(super) fn open_mass_tag_editor(&mut self, sources: Vec<MassSource>) {
        let selected = sources.len();
        // Drop audio-CD tracks (no file), then split real files into the tag-editable
        // ones and the rest (MIDI, tracker modules, …). Both halves feed the Basic
        // Info format breakdown so its counts match the selection; only the editable
        // half is actually edited.
        let (editable, other): (Vec<MassSource>, Vec<MassSource>) = sources
            .into_iter()
            .filter(|s| !crate::cdda::is_cdda_path(&s.path.to_string_lossy()))
            .partition(|s| crate::tags::can_edit(&s.path));
        if editable.is_empty() {
            self.status = "No editable files selected.".into();
            return;
        }
        let paths: Vec<PathBuf> = editable.iter().map(|s| s.path.clone()).collect();
        let other_paths: Vec<PathBuf> = other.iter().map(|s| s.path.clone()).collect();
        let pl_indices: Vec<usize> = editable.iter().filter_map(|s| s.pl_index).collect();

        // Read each file's fields once (needed for the mode); reuse for the
        // distinct-artist/album aggregates so Basic Info adds no further reads.
        let read: Vec<Id3Fields> = paths.iter().map(|p| crate::tags::read_primary(p)).collect();
        let fields = compute_modes(&read);
        // ID3v1 column pre-fill: the mode of each MP3's ID3v1 tag (missing counts as
        // blank). Non-MP3 files have no ID3v1 and are excluded. Blank when no MP3s.
        let v1_read: Vec<Id3Fields> = paths
            .iter()
            .filter(|p| crate::tags::uses_id3(p))
            .map(|p| crate::tags::read_id3(p).0.unwrap_or_default())
            .collect();
        let v1 = compute_modes(&v1_read);

        let total_size = paths
            .iter()
            .map(|p| std::fs::metadata(p).map(|m| m.len()).unwrap_or(0))
            .sum();
        let total_secs = editable.iter().filter_map(|s| s.duration).map(|d| d.as_secs()).sum();
        let basic = MassBasic {
            files: paths.len(),
            selected,
            total_size,
            total_secs,
            formats: format_breakdown(&paths),
            formats_other: format_breakdown(&other_paths),
            artists: distinct_nonempty(&read, |i| &i.artist),
            albums: distinct_nonempty(&read, |i| &i.album),
        };

        self.mass_tag_editor = Some(MassTagEditor {
            dominant_id3: dominant_id3(&paths),
            multi_type: multi_type(&paths),
            paths,
            pl_indices,
            initial: fields.clone(),
            fields,
            mask: FieldMask::default(),
            initial_v1: v1.clone(),
            v1,
            v1_mask: FieldMask::default(),
            tab: MassTab::Tags,
            basic,
            status: String::new(),
        });
    }

    /// Open the mass editor over the current playlist selection (each row carries
    /// its index so saving refreshes it, and its cached duration for Basic Info).
    pub(super) fn open_mass_tag_editor_playlist(&mut self) {
        let mut rows: Vec<usize> = self.plview.selection.iter().copied().collect();
        rows.sort_unstable();
        let sources: Vec<MassSource> = rows
            .into_iter()
            .filter_map(|i| {
                self.playlist.tracks.get(i).map(|t| MassSource {
                    path: t.path.clone(),
                    pl_index: Some(i),
                    duration: t.duration,
                })
            })
            .collect();
        self.open_mass_tag_editor(sources);
    }

    /// Open the mass editor over the given Media Library result rows.
    pub(super) fn open_mass_tag_editor_library(&mut self, rows: &[usize]) {
        let sources: Vec<MassSource> = rows
            .iter()
            .filter_map(|&i| {
                self.lib.results.get(i).map(|t| MassSource {
                    path: t.path.clone(),
                    pl_index: None,
                    duration: t.duration,
                })
            })
            .collect();
        self.open_mass_tag_editor(sources);
    }

    /// Write the changed fields into every file. Returns `true` when the caller may
    /// close (nothing changed, or every write that had work to do succeeded).
    pub(super) fn save_mass_tag_editor(&mut self) -> bool {
        let Some(ed) = self.mass_tag_editor.as_ref() else {
            return false;
        };
        // A field counts as edited if the live change flag caught it OR its value now
        // differs from the pre-filled mode, so an edit is never silently dropped.
        let mask = effective_mask(ed.mask, &ed.fields, &ed.initial);
        // The ID3v1 column only exists for an MP3-dominant selection.
        let dual = ed.dominant_id3;
        let v1_mask = if dual {
            effective_mask(ed.v1_mask, &ed.v1, &ed.initial_v1)
        } else {
            FieldMask::default()
        };
        if !mask.any() && !v1_mask.any() {
            return true; // user changed nothing; just close
        }
        let id3v23 = self.prefs.id3v23;
        let fields = ed.fields.clone();
        let v1 = ed.v1.clone();
        let paths = ed.paths.clone();
        let pl_indices = ed.pl_indices.clone();

        let mut errs = 0usize;
        let mut written: Vec<PathBuf> = Vec::new();
        for p in &paths {
            // MP3s in an MP3-dominant selection write both ID3 tags (the v1/v2 pair);
            // every other case writes the single main tag.
            let result = if dual && crate::tags::uses_id3(p) {
                crate::tags::write_mass_id3_dual(p, &v1, &v1_mask, &fields, &mask, id3v23)
            } else {
                crate::tags::write_mass(p, &fields, &mask, id3v23)
            };
            match result {
                Ok(true) => written.push(p.clone()),
                Ok(false) => {}
                Err(_) => errs += 1,
            }
        }
        // Refresh any playlist rows that came from the playlist.
        for &i in &pl_indices {
            if let Some(t) = self.playlist.tracks.get_mut(i) {
                t.loaded = false;
                t.load();
            }
        }
        // Reflect the new tags in the library DB for tracks it knows.
        if let Some(lib) = &self.library {
            for p in &written {
                let mut f = crate::playlist::Track::new(p.clone());
                f.load();
                lib.update_tags(&f);
            }
        }
        if !written.is_empty() {
            self.refresh_library();
        }
        if errs > 0 {
            if let Some(ed) = self.mass_tag_editor.as_mut() {
                ed.status = format!("Error: {errs} file(s) could not be written.");
            }
            return false;
        }
        true
    }

    /// The mass tag editor window (own viewport, like the single-file editor).
    pub(super) fn ui_mass_tag_editor(&mut self, ctx: &egui::Context) {
        if self.mass_tag_editor.is_none() {
            return;
        }
        let box_h = 17.0 * ROW_H; // the Tags tab is the tallest, 15 fields + header
        let win_h = box_h + CHROME + FOOTER_H;
        let size = egui::vec2(660.0, win_h);
        let mut screen = self.geom.win_pos[0] + egui::vec2(32.0, 32.0);
        if let Some(vr) = virtual_desktop_rect(ctx) {
            screen.x = screen.x.clamp(vr.min.x, (vr.max.x - size.x).max(vr.min.x));
            screen.y = screen.y.clamp(vr.min.y, (vr.max.y - size.y).max(vr.min.y));
        }
        let builder = egui::ViewportBuilder::default()
            .with_title("LinAmp, Mass Edit Tags")
            .with_position(screen)
            .with_inner_size(size)
            .with_resizable(false)
            .with_taskbar(false)
            .with_window_level(self.window_level());
        let id = egui::ViewportId::from_hash_of("linamp_mass_tags");
        let mut close = false;
        let mut do_save = false;
        ctx.show_viewport_immediate(id, builder, |ctx, _| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let ed = self.mass_tag_editor.as_mut().unwrap();

                // Escape closes without saving, unless a dropdown is open.
                if !ctx.memory(|m| m.any_popup_open())
                    && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape))
                {
                    close = true;
                }
                // While Alt is held, drop the character events so a focused field
                // doesn't type the menu letter (Alt-U, Alt-A, ...). AltGr is left be.
                ctx.input_mut(|i| {
                    if i.modifiers.alt && !i.modifiers.ctrl {
                        i.events.retain(|e| !matches!(e, egui::Event::Text(_)));
                    }
                });

                let line = ui.visuals().widgets.noninteractive.bg_stroke;
                let panel = ui.visuals().panel_fill;
                let idle_fill = ui.visuals().faint_bg_color;
                ui.visuals_mut().widgets.inactive.bg_stroke = line;

                // Header: how many files this acts on.
                egui::Frame::group(ui.style())
                    .fill(ui.visuals().extreme_bg_color)
                    .show(ui, |ui| {
                        ui.set_min_width(624.0);
                        let label = if ed.basic.files == ed.basic.selected {
                            format!("Editing {} files", ed.basic.files)
                        } else {
                            format!(
                                "Editing {} editable files (of {} selected)",
                                ed.basic.files, ed.basic.selected
                            )
                        };
                        ui.add(egui::Label::new(RichText::new(label).strong()).truncate());
                    });
                ui.add_space(4.0);

                // Tab strip joined to the content box, like the single-file editor.
                let top_round = egui::Rounding { nw: 4.0, ne: 4.0, sw: 0.0, se: 0.0 };
                let tab_btn = |ui: &mut egui::Ui, label: &str, pos: usize, active: bool| {
                    let fill = if active { panel } else { idle_fill };
                    ui.add(
                        egui::Button::new(mnem(label, pos))
                            .fill(fill)
                            .stroke(line)
                            .rounding(top_round)
                            .min_size(egui::vec2(0.0, 24.0)),
                    )
                };
                let prev_y = ui.spacing().item_spacing.y;
                ui.spacing_mut().item_spacing.y = 0.0;
                // The second tab resembles the most common tag model: "ID3" (Alt-I)
                // for an ID3-dominant selection, else "Tags" (Alt-A).
                let (tag_label, tag_pos) = if ed.dominant_id3 { ("ID3", 0) } else { ("Tags", 1) };
                let (basic_resp, tags_resp) = ui
                    .horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 2.0;
                        let b = tab_btn(ui, "Basic Info", 0, ed.tab == MassTab::Basic);
                        let t = tab_btn(ui, tag_label, tag_pos, ed.tab == MassTab::Tags);
                        (b, t)
                    })
                    .inner;
                if basic_resp.clicked() {
                    ed.tab = MassTab::Basic;
                }
                if tags_resp.clicked() {
                    ed.tab = MassTab::Tags;
                }

                let group = egui::Frame::group(ui.style()).fill(panel).show(ui, |ui| {
                    ui.set_min_width(624.0);
                    ui.set_min_height(box_h);
                    match ed.tab {
                        MassTab::Basic => {
                            egui::Grid::new("mass_basic_grid")
                                .num_columns(2)
                                .spacing([12.0, 4.0])
                                .show(ui, |ui| {
                                    let b = &ed.basic;
                                    let mut r = |k: &str, v: String| {
                                        ui.label(k);
                                        ui.label(v);
                                        ui.end_row();
                                    };
                                    r("Files", format!("{}", b.files));
                                    r("Total size", format!("{} bytes", b.total_size));
                                    if b.total_secs > 0 {
                                        let m = b.total_secs / 60;
                                        let s = b.total_secs % 60;
                                        r("Total length", format!("{m}:{s:02}"));
                                    }
                                    let join = |v: &[(String, usize)]| {
                                        v.iter()
                                            .map(|(k, c)| format!("{c} {k}"))
                                            .collect::<Vec<_>>()
                                            .join(", ")
                                    };
                                    // Editable formats, then any uneditable ones in
                                    // parentheses, e.g. "8 MP3, 1 FLAC (2 MIDI, 1 Tracker module)".
                                    let mut fmts = join(&b.formats);
                                    if !b.formats_other.is_empty() {
                                        fmts = format!("{fmts} ({})", join(&b.formats_other));
                                    }
                                    r("Formats", fmts);
                                    r("Artists", format!("{}", b.artists));
                                    r("Albums", format!("{}", b.albums));
                                });
                        }
                        MassTab::Tags if ed.dominant_id3 => {
                            // MP3-dominant selection: the ID3v1/ID3v2 column pair, like
                            // the single-file editor. The main column (`fields`) is the
                            // ID3v2 tag; `v1` is the ID3v1 subset.
                            ui.columns(2, |cols| {
                                cols[0].label(RichText::new("ID3v1").strong());
                                cols[0].add_space(2.0);
                                mass_v1_fields_grid(
                                    &mut cols[0],
                                    &mut ed.v1,
                                    &mut ed.v1_mask,
                                    "mass_v1_grid",
                                );
                                cols[1].label(RichText::new("ID3v2").strong());
                                cols[1].add_space(2.0);
                                mass_fields_grid(
                                    &mut cols[1],
                                    &mut ed.fields,
                                    &mut ed.mask,
                                    "mass_v2_grid",
                                );
                            });
                        }
                        MassTab::Tags => {
                            ui.label(RichText::new("Tags").strong());
                            ui.add_space(2.0);
                            mass_fields_grid(ui, &mut ed.fields, &mut ed.mask, "mass_grid");
                        }
                    }
                });
                // Erase the seam under the active tab so it reads as one surface.
                let active_rect = match ed.tab {
                    MassTab::Tags => tags_resp.rect,
                    MassTab::Basic => basic_resp.rect,
                };
                let box_top = group.response.rect.top();
                let tab_bot = active_rect.bottom();
                let seam = egui::Rect::from_x_y_ranges(
                    (active_rect.left() + line.width)..=(active_rect.right() - line.width),
                    (tab_bot.min(box_top) - 1.0)..=(tab_bot.max(box_top) + 1.0),
                );
                ui.painter().rect_filled(seam, 0.0, panel);
                ui.spacing_mut().item_spacing.y = prev_y;

                // Footer: a save error, or the single multiple-types note.
                ui.add_space(6.0);
                if !ed.status.is_empty() {
                    ui.label(RichText::new(&ed.status).color(ui.visuals().error_fg_color));
                } else if ed.multi_type {
                    ui.label(RichText::new("Multiple File Types Selected").weak());
                }
                ui.add_space(4.0);

                // Shortcuts: Alt-B Basic, Alt-I/Alt-A the Tags tab, Alt-U Update All,
                // Alt-C Cancel. Read raw so a focused field doesn't swallow them.
                let (k_basic, k_second, k_up, k_cancel) = ctx.input(|i| {
                    let a = i.modifiers.alt;
                    let second = if ed.dominant_id3 {
                        i.key_pressed(egui::Key::I)
                    } else {
                        i.key_pressed(egui::Key::A)
                    };
                    (
                        a && i.key_pressed(egui::Key::B),
                        a && second,
                        a && i.key_pressed(egui::Key::U),
                        a && i.key_pressed(egui::Key::C),
                    )
                });
                if k_basic {
                    ed.tab = MassTab::Basic;
                    basic_resp.request_focus();
                }
                if k_second {
                    ed.tab = MassTab::Tags;
                    tags_resp.request_focus();
                }
                let on_tags = ed.tab == MassTab::Tags;
                if on_tags && k_up {
                    do_save = true;
                }

                let mut copy_to_v1 = false;
                let mut copy_from_v1 = false;
                ui.horizontal(|ui| {
                    if on_tags && ui.button(mnem("Update All", 0)).clicked() {
                        do_save = true;
                    }
                    if ui.button(mnem("Cancel", 0)).clicked() {
                        close = true;
                    }
                    // Copy between the columns, like the single-file editor (MP3 only).
                    if on_tags && ed.dominant_id3 {
                        if ui.button(mnem("Copy to ID3v1", 5)).clicked() {
                            copy_to_v1 = true;
                        }
                        if ui.button(mnem("Copy from ID3v1", 5)).clicked() {
                            copy_from_v1 = true;
                        }
                    }
                });
                if k_cancel {
                    close = true;
                }
                // Copy only non-blank source fields (never wipe a value), capping to
                // ID3v1's limits when copying into it, and mark the copied fields edited
                // so Update All writes them.
                if copy_to_v1 {
                    let cp = |dst: &mut String, src: &str, dirty: &mut bool, n: usize| {
                        if !src.trim().is_empty() {
                            *dst = src.chars().take(n).collect();
                            *dirty = true;
                        }
                    };
                    let max = usize::MAX;
                    cp(&mut ed.v1.track, &ed.fields.track, &mut ed.v1_mask.track, max);
                    cp(&mut ed.v1.title, &ed.fields.title, &mut ed.v1_mask.title, crate::tags::V1_MAX_TEXT);
                    cp(&mut ed.v1.artist, &ed.fields.artist, &mut ed.v1_mask.artist, crate::tags::V1_MAX_TEXT);
                    cp(&mut ed.v1.album, &ed.fields.album, &mut ed.v1_mask.album, crate::tags::V1_MAX_TEXT);
                    cp(&mut ed.v1.year, &ed.fields.year, &mut ed.v1_mask.year, crate::tags::V1_MAX_YEAR);
                    cp(&mut ed.v1.genre, &ed.fields.genre, &mut ed.v1_mask.genre, max);
                    cp(&mut ed.v1.comment, &ed.fields.comment, &mut ed.v1_mask.comment, crate::tags::V1_MAX_TEXT);
                }
                if copy_from_v1 {
                    let cp = |dst: &mut String, src: &str, dirty: &mut bool| {
                        if !src.trim().is_empty() {
                            *dst = src.to_string();
                            *dirty = true;
                        }
                    };
                    cp(&mut ed.fields.track, &ed.v1.track, &mut ed.mask.track);
                    cp(&mut ed.fields.title, &ed.v1.title, &mut ed.mask.title);
                    cp(&mut ed.fields.artist, &ed.v1.artist, &mut ed.mask.artist);
                    cp(&mut ed.fields.album, &ed.v1.album, &mut ed.mask.album);
                    cp(&mut ed.fields.year, &ed.v1.year, &mut ed.mask.year);
                    cp(&mut ed.fields.genre, &ed.v1.genre, &mut ed.mask.genre);
                    cp(&mut ed.fields.comment, &ed.v1.comment, &mut ed.mask.comment);
                }
            });
            self.apply_base_cursor(ctx, "LinAmp, Mass Edit Tags");
            if ctx.input(|i| i.viewport().close_requested()) {
                close = true;
            }
        });
        // Close on Cancel/Escape, or after a successful save. `close` short-circuits
        // so a save is not attempted when the user is just closing.
        if close || (do_save && self.save_mass_tag_editor()) {
            self.mass_tag_editor = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn f(artist: &str, album: &str) -> Id3Fields {
        Id3Fields { artist: artist.into(), album: album.into(), ..Default::default() }
    }

    #[test]
    fn mode_picks_most_common_then_first_on_tie() {
        assert_eq!(mode_str(&["a".into(), "b".into(), "a".into()]), "a");
        // Tie between "a" and "b": the first seen wins.
        assert_eq!(mode_str(&["a".into(), "b".into()]), "a");
        assert_eq!(mode_str(&["b".into(), "a".into()]), "b");
        // Empty counts like any value: if most are blank, the mode is blank.
        assert_eq!(mode_str(&["".into(), "".into(), "x".into()]), "");
        assert_eq!(mode_str(&[] as &[String]), "");
    }

    #[test]
    fn compute_modes_fills_each_field() {
        let items = [f("Cowboy Man", "A"), f("Cowboy Man", "B"), f("Other", "A")];
        let m = compute_modes(&items);
        assert_eq!(m.artist, "Cowboy Man", "two of three share the artist");
        assert_eq!(m.album, "A", "two of three share the album");
    }

    #[test]
    fn compute_modes_compilation_majority() {
        let t = Id3Fields { compilation: true, ..Default::default() };
        let fa = Id3Fields { compilation: false, ..Default::default() };
        assert!(compute_modes(&[t.clone(), t.clone(), fa.clone()]).compilation);
        assert!(!compute_modes(&[t.clone(), fa.clone(), fa.clone()]).compilation);
    }

    #[test]
    fn dominant_and_multi_type() {
        let p = |s: &str| PathBuf::from(s);
        // Three MP3 and one FLAC: ID3 dominant, and multiple types.
        let sel = [p("a.mp3"), p("b.mp3"), p("c.mp3"), p("d.flac")];
        assert!(dominant_id3(&sel));
        assert!(multi_type(&sel));
        // All FLAC: not ID3 dominant, single type.
        let flacs = [p("a.flac"), p("b.flac")];
        assert!(!dominant_id3(&flacs));
        assert!(!multi_type(&flacs));
        // Even split counts as ID3 dominant (tie to ID3).
        assert!(dominant_id3(&[p("a.mp3"), p("b.flac")]));
    }

    #[test]
    fn distinct_counts_ignore_blanks_and_dupes() {
        let items = [f("A", "X"), f("A", "Y"), f("", "X")];
        assert_eq!(distinct_nonempty(&items, |i| &i.artist), 1);
        assert_eq!(distinct_nonempty(&items, |i| &i.album), 2);
    }

    #[test]
    fn effective_mask_catches_value_edits_and_live_flags() {
        let init = Id3Fields { artist: "Old".into(), album: "A".into(), ..Default::default() };
        // No live flags, but artist differs from the mode: it is marked.
        let cur = Id3Fields { artist: "New".into(), album: "A".into(), ..Default::default() };
        let m = effective_mask(FieldMask::default(), &cur, &init);
        assert!(m.artist, "edited-away-from-mode artist is written");
        assert!(!m.album, "untouched album stays unmarked");
        // A live flag with no value change (re-affirm the mode) still marks the field.
        let same = init.clone();
        let base = FieldMask { year: true, ..Default::default() };
        assert!(effective_mask(base, &same, &init).year, "live-flagged field is written");
        // Nothing touched, nothing marked.
        assert!(!effective_mask(FieldMask::default(), &same, &init).any());
    }

    #[test]
    fn format_breakdown_counts_most_common_first() {
        let p = |s: &str| PathBuf::from(s);
        let b = format_breakdown(&[p("a.mp3"), p("b.flac"), p("c.mp3")]);
        assert_eq!(b[0], ("MP3".to_string(), 2));
        assert_eq!(b[1], ("FLAC".to_string(), 1));
    }
}
