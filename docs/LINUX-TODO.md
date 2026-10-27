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

## Notes

- The code review and its resolution plan are in `docs/CODE-REVIEW.md`.
- Items are checked off here once the Linux box has verified or completed them.
