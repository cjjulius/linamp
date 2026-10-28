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

- [ ] **`classic.rs` split (S3/S4).** `classic.rs` is now `classic/geometry.rs`
  (hit-testing, tables) plus `classic/compose.rs` (compositor), re-exported so
  every `classic::*` path is unchanged. The main-window button rects (transport,
  shuffle/repeat/eq/pl, close/minimize) are now drawn from the shared `REGIONS`
  table. Verify the main/EQ/playlist windows render identically and the X11
  custom-cursor path (`x11util::set_cursor`) still tracks the right regions.
- [ ] **Popup menus from one list (C6).** The fixed popup menus (Add/Remove/
  Select/Misc/List, right-click track, right-click empty, Stop) now draw, size
  and test from one entry list. The right-click-empty-playlist menu is now about
  8px shorter (a latent over-tall band was removed). Linux has its own popup
  raise/round path (`raise_popup_window`, `round_popup_window`), so verify the
  popups still open at the click, round their corners, stay above the owned
  windows, and that no menu clips its last row or shows an empty band.
- [ ] **`ui_library` split (C7).** The Media Library's one big method is split
  into per-panel methods; panel bodies are unchanged. Exercise search, the nav
  tree, artist/album columns and resizing, the track table (sort, multi-select,
  right-click menus, ratings, double-click to play), and Play/Enqueue.
- [ ] **Perf fixes hold on PipeWire (C2/C4/C5/P1).** Confirm the per-frame wins
  land on Linux too: Media Library open over a large indexed library (lazy result
  clone), a very long playlist scrolling (visible-only rows), a selected library
  track (rating cache). P1 (lock-free visualizer) removes a mutex on the audio
  callback, so watch for any viz glitch or xrun under PipeWire.

## Notes

- The code review and its resolution plan are in `docs/CODE-REVIEW.md`; the
  Phase 2 resolution status is recorded at the end of that file.
- Items are checked off here once the Linux box has verified or completed them.
