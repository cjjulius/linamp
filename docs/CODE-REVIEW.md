# LinAmp code and architecture review

Date: 2026-10-27. Branch: `chore/code-maintenance`. Scope: all of `src/` (~23k
lines, 37 files), shipped features only. Targets: Windows 10/11 and stock latest
Linux Mint and Pop!_OS.

This is a review and a resolution plan. It changes no product code. Each finding
is rated by Severity (impact if left), Effort (S under ~1h localized, M a few
files, L cross-cutting or a refactor) and Risk (chance the fix itself regresses
something). The "Resolution sequence" below is the part to act from; the full
findings grouped by subsystem follow as reference.

## Executive summary

LinAmp is in genuinely good shape. The engine, content and skin layers are
decoupled with typed APIs and lock-free `*Shared` handles; platform code is
isolated behind a dispatch shim over per-OS modules, so a Windows-only or
Linux-only change really can be made in one file; untrusted parsers (audio, tags,
skins, CD-TOC, trackers, MIDI, playlists) are panic-free in production; the
classic render path is fingerprint-cached and repaint-throttled; and comment
style is consistent. The hand-rolled Win32 FFI is careful about struct layouts,
null checks and GDI lifetimes, and the `unsafe impl Send for WindowsCdDrive` is
sound.

The liabilities cluster into six themes:
1. The `app.rs` god-object: one `LinAmpApp` with ~110 flat fields and ~130
   methods, extended by seven submodules that each take the full `&mut self`.
   This is the single biggest thing working against "edit a couple of modules and
   be confident nothing else broke," and it is the headline architectural item.
2. A handful of avoidable per-frame or per-tick UI costs (a full library
   result-set clone, a SQLite query and session hashing on the repaint path),
   which matter most on low-end hardware.
3. Two real-time audio-thread issues: a `Mutex` taken from the audio callback and
   a per-packet heap allocation in decode.
4. A small set of latent panics or overflows on hostile input (an oscilloscope
   indexing bug, a few unchecked size arithmetic spots, an uncapped skin BMP
   allocation).
5. Lifecycle edges in the Win32 integration (a tray teardown race, a couple of
   GDI leaks) and two stability gaps (a startup crash when no audio device
   exists, non-atomic config saves).
6. Broad test-coverage gaps in otherwise-pure, easily testable code.

Nothing here is an emergency. The recommended order is: shore up test coverage
first (it is nearly free and the later fixes lean on it), then take the localized
low-risk wins, then the deliberate structural work, and consciously skip a few
low-value or high-risk items.

## Architecture assessment

### Strengths to preserve
- The per-OS dispatch-shim pattern (`tray.rs`, `mediakeys.rs`, `fileassoc.rs`,
  `cdda.rs`, `app/platform_win.rs`): keep new platform work inside these modules.
- Lock-free audio handoff for EQ and balance (`EqShared`, `BalanceShared`). The
  visualizer is the one exception and should join them (finding P1).
- The fingerprint-cached compositor (`app/render.rs`) and shared headless preview
  path (`--render-skin`): a paused player costs almost nothing, and skins are
  vettable without the UI.
- Pure, separately-tested helper modules (`dock`, `selection`, `text`, `region`,
  `eqf`, `skin`): more logic should move toward this shape.

### The `app.rs` god-object (priority)
One `LinAmpApp` owns essentially all mutable state. "EQ state" alone is
`eq_enabled / eq_preamp_db / eq_bands / eq_auto / eq_active / eq_save_name /
eq_presets` scattered among unrelated fields, and all seven UI submodules can
touch any of them through `&mut self`. The cost is real: whole-crate blast radius
for a field change, pure leaf logic that cannot be unit-tested without building
the whole app, and the pervasive "snapshot into locals, collect deferred actions,
apply later" style that exists mostly to dodge borrow conflicts.

Recommended path, lowest risk first:
1. **Field-bundle sub-structs** (behavior-preserving, mechanical): group related
   fields into `EqState`, `PlaylistViewState`, `LibraryViewState`,
   `ClassicRenderState` (`classic_tex / eq_tex / pl_tex / tex_fp / shape_sig /
   cur_cursor / active_region / pressed_btn`), `WindowGeom` (`win_pos / drag /
   resize / shades / foc_*`) and `PopupState`. Methods stay `&mut self` but read
   `self.eq.bands`. This shrinks the top level to roughly eight members and makes
   each bundle independently testable. Effort L, Risk Med, done incrementally one
   bundle at a time behind the existing tests.
2. **Narrowed `&mut` slices** passed to submodules (`render`, `window`) instead of
   full `&mut self`: higher risk from cross-field interdependencies. Defer until
   the bundles exist.

### `classic.rs` monolith
`classic.rs` is ~2000 lines of logic (plus ~665 of tests) with three `impl
ClassicSkin` blocks interleaved with free functions and const tables. It is
navigable but monolithic, and sprite source coordinates are inline integer
literals duplicated from the same Winamp table the named hit-rect `REGIONS` come
from (finding S4), so a parity fix must be made twice in two forms that cannot be
diffed. Recommended seam: `classic/geometry.rs` (the pure `Btn`/region enums,
`REGIONS`, band and button coordinate tables, and the hit-testers, already pure
and tested) and `classic/compose.rs` (the compositor plus blit primitives), with
the per-window sprite rects lifted into const tables of named dest+src structs so
one test can assert draw-rects and hit-rects stay aligned. Effort M, Risk Med.

### Rust memory safety and the `unsafe` surface
70 `unsafe` blocks across six files, all platform or device FFI, and they are
mostly sound. The review confirmed correct struct sizes, null checks, GDI brush
lifetimes, handle ownership and `CloseHandle`-on-every-path for the CD drive. Two
idiom improvements stand out: model crossfade and the restored EQ band set as
types that make invalid states unrepresentable (findings P4, M6), and tighten a
few spots that do unchecked size arithmetic on attacker-controlled counts
(findings S6, S7, S9). The Win32 lifecycle edges (I1, I3, I4) are about teardown
ordering and leaks, not unsafety per se.

### Documentation drift
`docs/ARCHITECTURE.md` is a good account but predates the system-integration
modules (`tray`, `mediakeys`, `fileassoc`, `openq`) and the native `cdda` FFI,
and it describes `app/platform_win.rs` as "the OS FFI boundary" while that module
is now both Win32 and X11 and reads as Windows-only by name (finding C9). Refresh
it alongside the structural work.

## Resolution sequence

### Phase 0: test coverage first (dimension E)

Unit tests are nearly free (cargo runs them every build) and the later fixes rely
on them to catch regressions. These are the gaps worth closing before touching
anything else, by module:

- **tags.rs**: the `write_id3`/`read_id3` tests early-return when a sample file is
  absent, so in a clean checkout or CI the whole tag write path is silently
  untested. Add a round-trip test that synthesizes its own MP3 (or ships a tiny
  fixture), plus `fields_from_tag` mojibake repair and an ID3v2-removed read-back.
- **playback DSP wrappers**: `EqSource` (disabled passthrough, per-channel state,
  mid-stream `reload()`, `try_seek` clearing filter memory) and `BalanceSource`
  (hard-pan zeros one channel, mono passthrough, seek resets the channel counter)
  are untested though pure; `compute_bars` is tested only on silence.
- **CD TOC reduction**: extract `toc_from_entries(entries, leadout) -> Vec<CdTrack>`
  from `read_toc` (both OSes) and test data-first disc, audio/data mix,
  single-track-to-leadout, and the zero-length guard. Today it needs real hardware.
- **cursor/region/skin/text**: the custom `.cur`/`.ani` DIB decoder, `region`
  concave and out-of-bounds-vertex cases, `skin::parse_pledit`/`sample_bmp`, and
  `text::marquee` are untested despite driving theming and window shapes.
- **playlist/library/config/eqf**: PLS save round-trip, backward shuffle nav,
  shuffle-bag rebuild on count change; `LibView::RecentlyAdded` ordering,
  `remove_paths`, artist/album search matching; a malformed (not just empty)
  config falling back to default, and `sig` sensitivity to a track-list change;
  multi-preset and truncated-tail EQF cases.
- **fileassoc/openq/main**: `fileassoc` has zero tests (lift the signature builder
  and the MIME-mapping helpers out from behind `cfg` and assert every
  `SUPPORTED_EXTS` entry maps to a MIME type); assert openq drains oldest-first
  and ignores a stray `.part`; give the `main` instance-lock test a pid-suffixed
  filename and factor the relative-path resolution into a tested pure helper.
- **core pure helpers**: `platform_win::rounded_rect_rects` (r=0, r clamped,
  symmetry) and `app::pl_scroll_to` boundary clamps.

### Phase 1: quick wins (low risk, localized, do these next)

- **[C2] Library clones the whole result set every frame** (C, Sev High, S, Low).
  `app/library_ui.rs:1193`: `targets` is `self.lib_results.clone()` (up to ~2000
  `Track`s) built unconditionally, read only inside the play/enqueue guards. Fix:
  build it lazily inside `if do_play || do_enqueue`.
- **[S1] Oscilloscope viz can panic** (D, Sev Med, S, Low). `classic.rs:1734`:
  `samples[0]` with only a `.max(2)` guard panics on a 0 or 1 sample scope frame.
  Fix: early-return when `samples.len() < 2`.
- **[C5] SQLite `rating()` query on the repaint path** (C, Sev Med, S, Low).
  `app/library_ui.rs:560`: runs every frame a library row is selected. Fix: cache
  the rating keyed on the selected path, recompute on selection or rate change.
- **[C4] `build_pl_rows` allocates the whole playlist** (C, Sev Med, S, Low).
  `app.rs:2047` formats a string per track though only `pl_first..end` is drawn.
  Fix: build only the visible window, as `render.rs:131` already computes it.
- **[P3] Per-packet `SampleBuffer` alloc on the audio thread** (C, Sev Med, S,
  Low). `decode.rs:55`. Fix: reuse an `Option<SampleBuffer>`, reallocating only on
  a spec or capacity change (as `stream.rs`/`midi.rs` already do).
- **[I2] fileassoc records success even when registry writes failed** (D, Sev Med,
  S, Low). `fileassoc.rs`: `register` discards every `set_value` bool and writes
  the skip-stamp unconditionally, so a partial registration never self-heals. Fix:
  OR the results and write the stamp only when all succeeded.
- **[P2] Windows CD short read skips sectors** (D, Sev Med, S, Low). `cdda.rs`:
  callers advance `next_lba` by the full requested count even on a short
  `DeviceIoControl` read. Fix: loop to fill the full count (short read is an error)
  or advance by sectors actually returned, and document the `read_sectors`
  contract.
- **[M5] Non-atomic config saves; load discards the whole file on any parse error**
  (D, Sev Med, S, Low). `config.rs`: `save` writes in place; `session.json` is the
  file most likely caught half-written, and the next `load` resets the entire
  session. Fix: write to a temp sibling and rename over the target.
- **[M3] No secondary SQLite indexes** (C, Sev Med, S, Low). `library.rs`: browse
  and smart-view queries full-scan and sort. Fix: add a v2 migration with an
  `(artist, album, title)` index plus single-column indexes on `play_count`,
  `date_added`, `last_played`, `rating`.
- **[S5] `wincursor` HCURSOR cache never destroyed** (D/C, Sev Med, S, Low). Each
  skin switch leaks one GDI cursor for process life. Fix: `DestroyCursor` and
  clear unreferenced entries on skin change; confirm callers key by skin identity.
- **[M1] Session hashed on the persist tick** (C, Sev Low, S, Low). `config.rs`
  `sig` serializes the whole session (all track paths) to JSON and hashes it; the
  caller throttles to 300ms (not per frame, as first reported), so cost is modest
  but scales with playlist size. Fix: serialize directly into the hasher, or
  dirty-flag on mutation instead of hashing.
- Smaller localized items: **[S6]** cap decoded skin BMP dimensions
  (`classic.rs:345`); **[S7]/[S9]** `checked_add`/`saturating_add` on the `.ani`
  and `REGION.TXT` size arithmetic; **[M9]** keep the existing year/track frame on
  a non-`u32` parse instead of deleting it (`tags.rs:290`); **[M4]** use
  `prepare_cached` for the fixed-SQL statements; **[M8]** factor the duplicated
  `xorshift`+Fisher-Yates into `shuffle_indices`; **[C8]** make
  `platform_win.rs:169` `CString::new` fallible; **[C9]** rename `platform_win.rs`
  and factor the one `theme_border` helper; **[I3]** `RemoveFontMemResourceEx` the
  tray menu font on teardown; **[I4]** clear `GWLP_USERDATA` before dropping the
  `TrayCtx` Box; **[I7]** pass the UTF-16 unit count to the Win32 menu text calls;
  **[I8]** standardize the tray/mediakeys backends on `Option<Self>`; **[P10]/[P11]**
  simplify the `stream.rs` first-packet branch and fix the `player.rs` pre-warm
  comment.

### Phase 2: bigger rocks (deliberate, verify carefully)

- **[C1] Decompose the `app.rs` god-object** (B, Sev High, L, Med). Field-bundle
  sub-structs first, incrementally. See the architecture section.
- **[S3]/[S4] Split `classic.rs` and de-duplicate sprite coordinates** (A/B, Sev
  Med, M-L, Med). `geometry.rs` + `compose.rs`, sprite rects as data. See above.
- **[P1] Make the visualizer lock-free** (C, Sev Med, M, Med). `viz.rs:36` takes a
  `Mutex` from the audio callback. Fix: a single-producer single-consumer ring
  (atomic write cursor + seqlock or double buffer), matching EQ/balance.
- **[M2] Scan holds the WAL writer lock across tag-read I/O** (C/D, Sev Med, M,
  Med). `library.rs:323`: `Track::from_path` (blocking tag read) runs inside the
  batch transaction, so the app connection's rating/play writes can stall or drop.
  Fix: read tags outside the transaction, open it only around the inserts.
- **[C3] Startup crashes when no audio device exists** (D, Sev Med, High if it
  fires, M, Med). `app.rs:694` `Player::new().expect(...)`. Fix: return `Result`,
  keep `player: Option<Player>` (or a null sink), surface it on the status line,
  and reacquire via the existing `poll_device` path. Important for the
  Linux/PipeWire login race.
- **[I1] Tray/mediakeys teardown can hang** (D, Sev Med, M, Med). `tray.rs:949`
  with `935`: spawn waits only 200ms for the HWND; if `Drop` then sees `hwnd == 0`
  it skips the quit message and `join()`s a thread blocked forever in
  `GetMessageW`. Fix: a real readiness signal (condvar/`Barrier`); in `Drop`, wait
  for publication before deciding. Same for the mediakeys thread id.
- **[C6] Popup menu heights are hand-counted and the test re-hardcodes them** (A/B,
  Sev Med, M, Med). `app/menu.rs`. Fix: drive both the drawing and the height (and
  the test) from one `Vec<MenuEntry>` per arm.
- **[C7] `ui_library` is one ~800-line method** (A, Sev Med, M, Low). Extract
  search bar / nav / columns / track table / apply-actions, ideally onto the
  `LibraryViewState` bundle from C1.

### Phase 3: skip or defer (conscious no for now)

- **[S12]** Rewrite the per-pixel classic blits to row-slice copies (C, Low, M,
  Med): the fingerprint gate keeps recompositing rare, so the risk of new clipping
  bugs outweighs the gain. Revisit only if profiling shows it matters.
- **[P4]** Re-model crossfade as an enum (B, Med, L, High risk): valuable for
  clarity but touches the audio path heavily; do it only alongside C1 and behind
  the crossfade tests.
- **[M6]** `eq_bands: Vec<f32>` to `Option<[f32;10]>` (B, Low, M, Med): a real
  modeling improvement but a persisted-format change; fold into the C1 bundles.
- **[P9]** Mid-stream sample-spec changes in `decode.rs` (D, Low, M): rare in the
  target formats; defer.
- **[I10]** Move Windows file-association registration off the startup thread (C,
  Low, M, Med): the stamp already makes steady-state launches near-free, so only
  worth it if the first-launch hitch is measurable.

## Full findings by subsystem (reference)

The ratings and locations above are the authoritative list. For traceability the
findings came from five subsystem passes: core app/UI (C1-C10), playback engine
(P1-P11), content/metadata (M1-M10), skin/classic (S1-S12), and system
integration (I1-I10). Items not called out by ID in the phases above are the
remaining Phase 0 test-coverage gaps (C10, P5-P8, M10, S2, S8, S10, S11, I5, I6,
I9), consolidated in Phase 0.
