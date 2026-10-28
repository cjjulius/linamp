# Things to look at on the Linux box

A running list of work that can't be exercised on the Windows dev machine and so
needs the Linux box (or specific hardware, mainly a CD drive) to verify or finish.
Kept current as fixes land: items are checked off once the Linux box has verified
them, and removed once the work has merged to main.

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

## Phase 1 fixes needing hardware verification

- [ ] **CD short-read fix (P2), not yet done** (`src/cdda.rs`): the Windows
  `read_sectors` advances `next_lba` by the full requested count even on a short
  `DeviceIoControl` read, silently skipping sectors. The fix (loop to fill the
  full count, or advance by sectors actually returned) needs a CD drive to verify
  end-to-end, so it is deferred to the machine with one.

## Broadened tag editing (`feat/tag-editing`, #89)

Cross-platform (egui + lofty), QA'd on Windows. Confirm on Pop!_OS:

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

- Mint verification is deferred across the board (no Mint box on hand); the
  Pop!_OS checks stand in for Linux until one is available.
- Done and merged to main: Phase 2 code-maintenance refactors (S3/S4, C6, C7,
  perf C2/C4/C5/P1), the 2.x bug fixes (#171 fade-on-pause incl. the C-key, #172
  always-on-top dialog trap), and the C8 `intern` CString guard (compiles on
  Linux). All verified on Pop!_OS before merge.
- The code review and its resolution plan are in `docs/CODE-REVIEW.md`.
