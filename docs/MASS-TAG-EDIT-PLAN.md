# Mass Tag Editing: design and plan

Branch: feat/mass-tag-edit. This is the plan for editing the tags of several
selected files at once, from both the Playlist and the Media Library.

## Goal

Let the user pick several tracks (Ctrl/Shift or mouse multi-select), open one
editor, and set shared fields (artist, album, year, genre, and the rest) across
all of them in a single pass, without clobbering the fields they leave alone.

## Where it appears

- Playlist context menu: a "Mass Edit Tags..." item that shows only when more
  than one row is selected.
- Media Library track context menu: the same item, shown only when more than one
  track is selected.
- Both the classic popup menus and the modern (egui) context menus.

## Keyboard shortcut

Winamp 2.666 has no distinct multi-file tag keybind: Alt+3 there opens the
single-file "file info" box. So we reuse Alt+3. With exactly one row selected it
opens the normal single-file editor (unchanged). With more than one row selected
it opens the mass editor. Nothing is written until the user clicks "Update All",
so an accidental multi-select plus Alt+3 is harmless: the dialog names the file
count and a Cancel/Escape backs out with no change.

## The editor

Modeled on the single-file tag editor (same window chrome, tab strip, button row,
mnemonics), with two differences:

1. The save button reads "Update All" (still Alt+U).
2. It carries a set of files, not one.

### Tabs

- Basic Info: quick aggregates only (see below).
- Tags: one shared field grid (the full ID3v2 / native field set, the same grid
  the single-file editor shows for a FLAC/Ogg/M4A file). The tab is labeled "ID3"
  (Alt+I) when the most common selected type uses ID3 (MP3/MP2/MP1), else "Tags"
  (Alt+A), so it resembles the most-used tag's style. There is no separate
  ID3v1/ID3v2 split in mass mode: you edit the logical fields, and each file is
  written in its own native tag.

### Pre-fill: the mode

Each field is pre-filled with the most common value across the selected files
for that field (the mode). Ties break to the first occurrence in selection order.
Example: three tracks, two with "Cowboy Man" in Artist, pre-fills Artist to
"Cowboy Man".

### What gets written

A field is written to every selected file only if the user edits it in the
dialog. Fields left untouched are preserved per file, so unique values (Title,
Track number) are not flattened to the mode. This is the standard, non-destructive
mass-tag behavior and matches the "use the fields that are changed as indicators"
rule. Editing a field and leaving it equal to the mode still counts as an edit, so
the user can deliberately normalize a field to the dominant value.

Writing is a merge: each file's existing tag is read, the changed fields are
overlaid, and the tag is saved in place. Frames the editor does not manage (album
art, album artist, and so on) survive. A file whose values already match after the
overlay is skipped (no needless rewrite).

### Mixed file types

The changed fields are applied to every selected file regardless of its tag type,
each through its own native keys. So entering an Artist on a selection of three
MP3s and one FLAC writes that Artist into the FLAC's Vorbis comments too. A field
with no mapping in a given format is dropped silently (lofty does not map it).
When the selection spans more than one file type, a single line at the bottom
reads "Multiple File Types Selected" and nothing more.

ID3v1 on MP3 is left untouched in mass mode (not created, not removed). The modern
ID3v2 tag carries the edits. This keeps the write non-destructive and avoids the
ID3v1 ASCII-folding and 30-byte caps on a bulk pass. A follow-up could add an
opt-in ID3v1 update.

### Basic Info aggregates

Quick to compute, no per-file deep parse:

- File count (and, if some selected rows were not editable, "N of M selected").
- Total size in bytes (a cheap metadata read per file).
- Total length (summed from the cached per-row durations the playlist/library
  already hold).
- Format breakdown (for example "3 MP3, 1 FLAC"), from the extensions.
- Distinct artists and distinct albums, from the values read for the mode.

## Non-editable and CD rows

Rows that are not tag-editable (WAV, MIDI, tracker modules) and audio-CD tracks
(no file on disk) are filtered out of the selection. The mass editor acts on the
editable files only, and its count reflects that. If nothing editable is selected,
it does not open and a status line says so.

## Code shape

- `src/tags.rs`: add a merge write that applies only the changed fields to a
  file's existing primary tag, leaving other tags and unedited fields intact
  (ID3v2-only for MP3 so ID3v1 is preserved; the native tag for the rest). Unit
  tested in memory across tag types, plus a round-trip on the bundled sample.
- `src/app/mass_tag_ui.rs` (new): the `MassTagEditor` state, the mode and
  aggregate computation (pure, unit-tested), and the window render.
- `src/app.rs`: a `mass_tag_editor` field, its init, and the `ui_mass_tag_editor`
  call.
- `src/app/menu.rs`, `src/app/modern.rs`, `src/app/library_ui.rs`: the context
  menu items (classic and modern, playlist and library) gated on a multi-row
  selection, plus the `LibAction` variant and dispatch.
- `src/app/input.rs`: Alt+3 opens the mass editor when more than one playlist row
  is selected.

## Tests

Pure helpers get unit tests: mode selection (including ties and empties), the
"changed fields only" overlay, the multi-type detection, and the merge write
across tag types. The window render and the live multi-select gestures are visual
and go on the QA list.
