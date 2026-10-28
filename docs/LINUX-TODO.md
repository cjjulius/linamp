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
  real or emulated disc. (The real-disc read is confirmed: on Pop!_OS 2026-10-28 a
  10-track audio CD read its TOC correctly, durations 3:37..6:33, total 49:21, and
  played/skipped cleanly. Still to do: the pure-function extraction and its unit
  tests.)
- [ ] **`rounded_rect_rects`** (`src/app/platform_win.rs`, in the `x11util`
  module): add unit tests (radius 0 is a single rect; corner inset is symmetric
  top/bottom). This module is Linux-only, so it does not compile on Windows and
  the test must be added and run there.

## Phase 1 fixes needing hardware verification

- [ ] **CD short-read fix (P2), Windows-side, still pending** (`src/cdda.rs`): the
  Windows `read_sectors` advances `next_lba` by the full requested count even on a
  short `DeviceIoControl` read, silently skipping sectors. The fix (loop to fill
  the full count, or advance by sectors actually returned) belongs on the Windows
  machine. Linux is confirmed unaffected: the Linux `read_sectors` uses the
  `CDROMREADAUDIO` ioctl, which is all-or-nothing (reads every requested frame or
  returns -1), so a partial read can't advance `next_lba` past what was read.
  Verified end-to-end on Pop!_OS with a real 10-track audio CD (see below): no
  skipped sectors, smooth real-time playback.

## Broadened tag editing (`feat/tag-editing`, #89)

Cross-platform (egui + lofty), QA'd on Windows. Pop!_OS verified 2026-10-28:

- [x] **Tag editor window sizing + off-screen clamp.** The window height is now
  computed from the tallest tab's row count, and the window is clamped onto the
  desktop via `virtual_desktop_rect` (which has an X11 path). Verify the editor
  fits its content (no clipped bottom) for MP3 and for the single-column non-MP3
  view, and that opening it near a screen edge keeps it on-screen.
  Pop!_OS: MP3 window (660x547) sized to the taller ID3 tab, ID3v2 column (BPM /
  Compilation) fully visible with Update/Cancel; read-only formats size down
  (MIDI 660x201, WAV 660x270). With the main window at the bottom-right corner
  (1500,1050) the editor opened fully on-screen (right 693, bottom 351 on 1920x1200).
- [x] **Non-MP3 and MP2/MP1 tag writing.** FLAC/Ogg/M4A show one "Tags" column;
  MP2/MP1 use the ID3 v1/v2 tabs like MP3. Edit a field, save, reopen, and confirm
  it stuck on each (lofty is cross-platform, so this should just work). Known gap:
  the M4A compilation flag does not persist.
  Pop!_OS: FLAC title edit wrote a Vorbis `TITLE=` comment to disk; MP2 title edit
  (ID3v2 enabled) wrote an ID3v2 `TIT2` frame (UTF-16) to disk. Both confirmed by
  reading the file bytes after Update. MP1 shares the MP2 path.
  Finding (cross-platform, not fixed): for the single-tag formats the second tab
  is still labelled "ID3" (`tag_ui.rs:374`) even though its content header and
  fields are the generic "Tags" column; for FLAC/Ogg/M4A that label is wrong
  (they are not ID3). The tab label should follow the single-tag flag like the
  content does.
- [x] **Basic Info for all formats.** MIDI shows size, format (type + track count)
  and length (via `midi::duration`); other audio shows duration/bitrate/sample
  rate/channels; a read-only format (e.g. WAV) shows only the Basic Info tab (no
  ID3 tab, no message). Spot-check a few.
  Pop!_OS: MIDI showed Size 576 bytes, Format "MIDI (type 0, 1 track(s))", Length
  30 seconds, Basic-Info-only; WAV showed Format WAV + duration/bitrate/rate/
  channels, Basic-Info-only (no ID3 tab, no message); MP3/MP2/FLAC Basic Info
  correct (Format strings "MPEG-1 Layer 3/2", FLAC, etc.).

## Notes

- Mint verification is deferred across the board (no Mint box on hand); the
  Pop!_OS checks stand in for Linux until one is available.
- Done and merged to main: Phase 2 code-maintenance refactors (S3/S4, C6, C7,
  perf C2/C4/C5/P1), the 2.x bug fixes (#171 fade-on-pause incl. the C-key, #172
  always-on-top dialog trap), and the C8 `intern` CString guard (compiles on
  Linux). All verified on Pop!_OS before merge.
- The code review and its resolution plan are in `docs/CODE-REVIEW.md`.
