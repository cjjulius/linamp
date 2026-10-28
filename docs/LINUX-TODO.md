# Things to look at on the Linux box

A running list of work that can't be exercised on the Windows dev machine and so
needs the Linux box (or specific hardware, mainly a CD drive) to verify or finish.
Kept current as fixes land: items are checked off once the Linux box has verified
them, and removed once the work has merged to main.

## Deferred test coverage (Phase 0)

- [x] **CD TOC reduction** (`src/cdda.rs`): extract a pure
  `toc_from_entries(entries, leadout) -> Vec<CdTrack>` from `read_toc` (both the
  Linux and Windows paths) and unit-test it: a data-first disc, an audio/data mix,
  a single track running to the lead-out, and the zero-length guard. The
  extraction itself touches the device-read code, so validate end-to-end against a
  real or emulated disc. Done: added `TrackEntry` + `toc_from_entries` (named
  `TrackEntry` to avoid the Linux `TocEntry` ioctl struct), both `read_toc` paths
  now build entries and call it, and five unit tests cover single-to-lead-out,
  leading data track, audio/data mix, zero-length drop, and data-only error. The
  real-disc read is confirmed too: on Pop!_OS 2026-10-28 a 10-track audio CD read
  its TOC correctly (durations 3:37..6:33, total 49:21) and played/skipped cleanly.
- [x] **`rounded_rect_rects`** (`src/app/platform_win.rs`, in the `x11util`
  module): add unit tests (radius 0 is a single rect; corner inset is symmetric
  top/bottom). This module is Linux-only, so it does not compile on Windows and
  the test must be added and run there. Done: added a `#[cfg(test)]` submodule in
  `x11util` with the zero-radius single-rect case (incl. negative-radius clamp) and
  the top/bottom inset symmetry case.

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

## Notes

- Mint verification is deferred across the board (no Mint box on hand); the
  Pop!_OS checks stand in for Linux until one is available.
- Done and merged to main: Phase 2 code-maintenance refactors (S3/S4, C6, C7,
  perf C2/C4/C5/P1), the 2.x bug fixes (#171 fade-on-pause incl. the C-key, #172
  always-on-top dialog trap), the C8 `intern` CString guard (compiles on Linux),
  and the broadened tag editing (#89: FLAC/Ogg/M4A and MP2/MP1 writing, more ID3v2
  frames, ID3v2.3/2.4, per-format Basic Info, window sizing/clamp) plus the
  toc_from_entries / x11util test coverage. All verified on Pop!_OS before merge.
  Remaining tag gap: the M4A compilation flag does not persist; album art is #174.
- The code review and its resolution plan are in `docs/CODE-REVIEW.md`.
