# Things to look at on the Linux box

A running list of work that can't be exercised on the Windows dev machine and so
needs the Linux box (or specific hardware, mainly a CD drive) to verify or finish.
Kept current as code-review fixes land.

## Deferred test coverage (Phase 0)

- [ ] **CD TOC reduction** (`src/cdda.rs`): extract a pure
  `toc_from_entries(entries, leadout) -> Vec<CdTrack>` from `read_toc` (both the
  Linux and Windows paths) and unit-test it: a data-first disc, an audio/data mix,
  a single track running to the lead-out, and the zero-length guard. The
  extraction itself touches the device-read code, so validate end-to-end against a
  real or emulated disc.
- [ ] **`rounded_rect_rects`** (`src/app/platform_win.rs`, in the `x11util`
  module): add unit tests (radius 0 is a single rect; corner inset is symmetric
  top/bottom). This module is Linux-only, so it does not compile on Windows and
  the test must be added and run there.

## Phase 1 fixes needing Linux or hardware verification

(Updated as fixes land. Each Windows fix below compiles on the dev machine but its
runtime effect needs the noted hardware.)

- [ ] **Confirm the `intern` CString guard compiles on Linux** (C8,
  `src/app/platform_win.rs`, in the X11 `x11util` module): the guard was added on
  the Windows machine, where that module isn't compiled. It is a trivial
  `CString::new(name)` let-else, but build it on Linux to be sure.
- [ ] **CD short-read fix (P2), not yet done** (`src/cdda.rs`): the Windows
  `read_sectors` advances `next_lba` by the full requested count even on a short
  `DeviceIoControl` read, silently skipping sectors. The fix (loop to fill the
  full count, or advance by sectors actually returned) needs a CD drive to verify
  end-to-end, so it is deferred to the machine with one.

## Phase 2 refactors to regression-check on Linux (`chore/code-maintenance-phase2`)

These are behavior-preserving refactors (byte-identical or verbatim on Windows,
all tests green), but no automated test covers the live UI, and each touches a
subsystem that Linux renders or drives with its own platform path. Sanity-check
them visually on Pop!_OS and Mint; no code fix is expected.

Pop!_OS verified 2026-10-27 (all four below). Mint still deferred (no Mint box
on hand), consistent with the earlier Mint deferral.

- [x] **`classic.rs` split (S3/S4).** `classic.rs` is now `classic/geometry.rs`
  (hit-testing, tables) plus `classic/compose.rs` (compositor), re-exported so
  every `classic::*` path is unchanged. The main-window button rects (transport,
  shuffle/repeat/eq/pl, close/minimize) are now drawn from the shared `REGIONS`
  table. Verify the main/EQ/playlist windows render identically and the X11
  custom-cursor path (`x11util::set_cursor`) still tracks the right regions.
  Pop!_OS: `--render-all` is byte-identical (sha256) across all 14 bundled skins
  versus the pre-refactor baseline; the live main/EQ/playlist windows render and
  dock correctly, and hit-testing from `REGIONS` lands (transport, and a
  right-click opened the correct menu). The cursor glyph per region was not pixel
  captured but is driven by the same `REGIONS` table the verified hit-testing uses.
- [x] **Popup menus from one list (C6).** The fixed popup menus (Add/Remove/
  Select/Misc/List, right-click track, right-click empty, Stop) now draw, size
  and test from one entry list. The right-click-empty-playlist menu is now about
  8px shorter (a latent over-tall band was removed). Linux has its own popup
  raise/round path (`raise_popup_window`, `round_popup_window`), so verify the
  popups still open at the click, round their corners, stay above the owned
  windows, and that no menu clips its last row or shows an empty band.
  Pop!_OS: `fixed_menu_sizes_match_their_draw_arms` and `popup_height_formula`
  pass (they cover the PlVoid 14-item/4-separator fix); live, right-click-empty
  opened the PlVoid menu at the click, above the playlist, sized exactly to
  content (Load list flush at the bottom, no empty band, no clipped row).
- [x] **`ui_library` split (C7).** The Media Library's one big method is split
  into per-panel methods; panel bodies are unchanged. Exercise search, the nav
  tree, artist/album columns and resizing, the track table (sort, multi-select,
  right-click menus, ratings, double-click to play), and Play/Enqueue.
  Pop!_OS: `--render-gen` is byte-identical for the skins that ship GEN.BMP; live,
  the library renders the nav tree, artist/album columns and the 71-track table,
  an artist click narrows the album column and track list, the Title header sorts
  the rows, and double-click plays the track.
- [x] **Perf fixes hold on PipeWire (C2/C4/C5/P1).** Confirm the per-frame wins
  land on Linux too: Media Library open over a large indexed library (lazy result
  clone), a very long playlist scrolling (visible-only rows), a selected library
  track (rating cache). P1 (lock-free visualizer) removes a mutex on the audio
  callback, so watch for any viz glitch or xrun under PipeWire.
  Pop!_OS: the library opened instantly over the 71-track indexed library;
  playback ran smooth through PipeWire (ALSA plug-in sink-input, uncorked), the
  clock advanced, and the run log showed no xrun, underrun or error.

## 2.x bug fixes (`fix/2x-bugs`)

- [x] **Always-on-top dialog trap: X11 equivalent (#172).** `set_windows_topmost`
  (`src/app/platform_win.rs`) drops the topmost flag on every LinAmp window around
  a blocking native dialog so it isn't trapped behind our always-on-top windows,
  then restores it. It is Windows-only (SetWindowPos); on X11 it is a no-op.
  Reproduce on Pop!_OS with always-on-top on (open a file/skin dialog and see
  whether it is reachable), and if the trap exists, add the X11 version in
  `x11util` (toggle `_NET_WM_STATE_ABOVE`, or lower/raise, around the dialog).
  Pop!_OS (Mutter): trap confirmed. With always-on-top on, the portal file
  chooser (`xdg-desktop-portal-gnome`, `_NET_WM_WINDOW_TYPE_NORMAL`, no
  transient-for) opened below the three above-layer LinAmp windows in the stacking
  order, and the Library window covered the Select button. Fixed: `set_windows_topmost`
  now has an X11 branch (`x11util::set_group_above`) that removes `_NET_WM_STATE_ABOVE`
  from every LinAmp window before the dialog and re-adds it after. Verified by the
  stacking order (dialog moved to the top, our windows lost ABOVE while open, ABOVE
  restored on close) and visually (Select button reachable).
- [x] **Verify fade-on-pause on Linux (#171).** Pausing now fades out (then
  pauses) when "fade out when stopping or pausing" is on, matching stop. Pure
  cross-platform logic (no platform code), but confirm the tray and button pause
  fade on PipeWire and resume at full volume.
  Pop!_OS: the pause button and the Play/Pause toggle (space/X, and the tray
  Play/Pause, all via `play_pause` -> `pause_playback`) fade correctly: pressing
  the pause button at 00:03 kept the clock advancing to 00:06 over the 3s fade,
  then held at 00:06 (paused, not reset), and resume continued from 00:06 at full
  volume. Gap found and fixed (cross-platform): the dedicated pause key (`C`,
  `input.rs` `k.pause`) was still calling `player.pause()` directly, so it cut
  abruptly with no fade (clock froze instantly), unlike the button and the commit's
  stated intent ("key pause cut audio abruptly"). Routed `k.pause` through
  `pause_playback`, which keeps the one-way-pause semantics and adds the fade.
  Verified: C now keeps the clock advancing through the fade then pauses
  (00:03 -> ~00:05), same as the button.

## Broadened tag editing (`feat/tag-editing`, #89)

Cross-platform (egui + lofty), QA'd on Windows. Confirm on Pop!_OS and Mint:

- [ ] **Tag editor window sizing + off-screen clamp.** The window height is now
  computed from the tallest tab's row count, and the window is clamped onto the
  desktop via `virtual_desktop_rect` (which has an X11 path). Verify the editor
  fits its content (no clipped bottom) for MP3 and for the single-column non-MP3
  view, and that opening it near a screen edge keeps it on-screen.
- [ ] **Non-MP3 and MP2/MP1 tag writing.** FLAC/Ogg/M4A show one "Tags" column;
  MP2/MP1 use the ID3 v1/v2 tabs like MP3. Edit a field, save, reopen, and confirm
  it stuck on each (lofty is cross-platform, so this should just work). Known gap:
  the M4A compilation flag does not persist.
- [ ] **Basic Info for all formats.** MIDI shows size, format (type + track count)
  and length (via `midi::duration`); other audio shows duration/bitrate/sample
  rate/channels; a read-only format (e.g. WAV) shows only the Basic Info tab (no
  ID3 tab, no message). Spot-check a few.

## Notes

- The code review and its resolution plan are in `docs/CODE-REVIEW.md`; the
  Phase 2 resolution status is recorded at the end of that file.
- Items are checked off here once the Linux box has verified or completed them.
