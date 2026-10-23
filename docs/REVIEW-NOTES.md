# Review notes — feat/feature-set

Items needing a human eye or interaction to verify (not fully validatable by
code plus a working/not-working binary): visual and behavior changes.

## 2026-10-22

- #105 volume not remembered: fixed by adding volume and balance to the session
  change-detection hash in `persist_session_if_changed` (they were saved by
  `build_session` but never marked the session dirty, so a volume-only change was
  lost on the next launch). Verify interactively: change the volume, quit,
  relaunch, and confirm the level is restored. End-to-end persistence was
  confirmed by code inspection only — the automated test harness could not move
  the volume control via synthetic mouse or keyboard input.

- #115 ID3v2 to ID3v1 copy corruption: reproduced (writing a multibyte ID3v1
  field panicked inside lofty when truncating mid-character, aborting the save
  and corrupting the file) and fixed by capping each v1 field to its byte limit
  on a char boundary (comment to 28 for the v1.1 track byte). Covered by a
  regression test. Optional interactive sanity check: open the tag editor on a
  file with long/non-Latin tags, copy ID3v2 to ID3v1, save, and confirm the file
  still plays.

- #97 non-ASCII text readability (partial): the classic bitmap-font path now
  ASCII-folds accented and Vietnamese characters to their base letter (e.g.
  "Trường Sơn" renders as "Truong Son") instead of blank gaps. Visual change —
  confirm accented/Vietnamese track titles read sensibly in the classic
  main/EQ/playlist windows. Full non-Latin glyph rendering still needs the font
  atlas approach from the issue; #97 left open for that.

- #96 Media Library search perf: completes the issue (debounce was already in
  place) by skipping the unchanged artists() query on artist/album selection
  changes. Perf-only, no user-visible change. Optional sanity check: clicking
  artists and albums still filters the album pane and track list correctly.

- #46 playlist editor extras: added "Remove missing files" (Rem menu) and
  "Randomize list" / "Reverse list" (Misc menu). The operations are unit-tested;
  the menu wiring and popup sizing (Rem now 4 rows, Misc now 6) need a visual
  check — open the playlist bottom-bar Rem/Misc menus and confirm the new items
  show and behave.

- #114 show app version: the Preferences window now shows "LinAmp <version>"
  (from CARGO_PKG_VERSION) at the bottom; prefs window height bumped to fit.
  Visual check: open Preferences (Ctrl+P) and confirm the version line shows and
  nothing is clipped. (Could not screenshot via synthetic input in the harness.)

- #106 Media Library titlebar: the inactive (unfocused) title now measures glyph
  widths from the clean SELECTED font row, so the stray pixel on 'Y' is excluded
  and kerning matches the active title. The active render is byte-for-byte
  unchanged (verified). Visual check: unfocus the app and confirm the Media
  Library title ("LINAMP LIBRARY") has no stray pixel on 'Y' and even spacing.

- Time display toggle parity: the main-window time counter now toggles between
  elapsed and remaining on a single click, matching classic Winamp. It
  previously required a double-click. The double-click detection state
  (time_last_click) was removed. No unit test applies (egui click handler).
  Interaction check: single-click the time counter and confirm it flips to the
  remaining (negative) time and back; the Ctrl+T shortcut and the Preferences
  checkbox should still work too.

- LCD time formatting dedup (refactor): the elapsed/remaining + mm:ss + 99-minute
  cap logic was duplicated between the full and window-shade time displays; both
  now call one MainState::lcd_mmss helper. Behavior is unchanged and covered by a
  new unit test plus the existing byte-identical render tests, so no human/visual
  check is required for this one.

- Playlist file decoding robustness (bug-fix): .m3u/.pls parsing used
  read_to_string, so a single non-UTF-8 byte or a leading UTF-8 BOM made the
  whole file load as zero tracks. It now reads bytes, decodes leniently, and
  strips a BOM. Fully covered by a unit test; no manual check required.

- Windows-authored playlist paths (bug-fix): relative .m3u/.pls entries that use
  backslash separators (e.g. "Music\Song.mp3", as nearly all Winamp-era
  playlists do) were treated as literal filenames and never resolved on Linux.
  resolve() now converts backslashes to separators for relative entries. The
  path logic is unit-tested; a good end-to-end human check is to load an actual
  Windows-authored .m3u with relative backslash paths and confirm the tracks
  appear.

- PLS entry ordering (bug-fix): .pls parsing used line order and ingested any
  key starting with "File" (e.g. a stray "FileFoo="). It now orders entries by
  the N in "File<N>=" per the PLS spec and ignores non-File<N> keys. Fully
  covered by a unit test; no manual check required.

- Library search wildcard escaping (bug-fix): the search built a LIKE pattern
  without escaping, so a literal '_' or '%' in the typed text acted as a SQL
  wildcard (e.g. "track_01" also matched "trackX01"). Wildcards are now escaped
  with an ESCAPE clause. Fully covered by a unit test; no manual check required.

- Tag edit not reflected in library (bug-fix): saving edited tags wrote the file
  and refreshed the playlist row but never updated the library DB, so the Media
  Library kept showing stale tags until a rescan. A new membership-gated
  Library::update_tags now updates a known track's tags in place (preserving
  play count and rating) and is a no-op for tracks not in the library. The DB
  method is unit-tested. Human check: edit a track's tags from the Media Library
  and confirm the row's title/artist/album update immediately without a rescan.

- Library view refresh after tag edit (bug-fix, follow-up): last run updated the
  library DB on a tag edit, but the on-screen library list is cached and stayed
  stale until re-navigation. save_tag_editor now calls refresh_library when a
  library row actually changed, so the edited title/artist/album appear
  immediately. UI glue (no unit test); the earlier human check (edit a Media
  Library row, confirm it updates without a rescan) now covers this end to end.

- Playlist multi-selection (feature): added Winamp-style selection to the classic
  playlist so Invert/Crop make sense. Semantics implemented:
  * Plain click / Up / Down: select only that row (sets the anchor).
  * Ctrl+click: toggle that row in/out of the selection.
  * Ctrl+Up/Down: move the focus and add that row (keyboard individual select; it
    never deselects, so arrowing can't silently drop a row).
  * Shift+click and Shift+Up/Down: select the contiguous range from the anchor.
  * Home / End: jump the selection to the first / last track (Shift extends,
    Ctrl adds).
  * The highlight reflects the whole selection; the focus/lead row is scrolled in.
  The pure selection logic (src/selection.rs) is unit-tested, but the mouse/
  keyboard wiring and highlight need a human interaction check: verify ctrl-click
  toggling, shift-range (mouse and keyboard), Home/End, that a double-click still
  plays, and that Invert/Crop operate on the new multi-selection. If any specific
  key should behave differently (esp. Ctrl+Up/Down), it's easy to adjust.

## 2026-10-23 (parity cron)

- #97 (partial): extended the classic bitmap-font ASCII-folding to cover Latin
  letters that were still rendering garbled — the g/h block (incl. Turkish Ğ and
  Esperanto ĝ/ĥ) and the tails of n/t/u (ŋ, ŧ, and Hungarian Ű/Ų/ű/ų). Covered by
  the ascii_fold unit test. Optional visual check: a track title containing Ğ or
  Ű shows as "G"/"U" in the classic marquee, not a blank or garbled cell.
  (A QA song is prepared for this: qa-music/Aria Nocturne - Crystal Harbor.mp3
  now has the title "Ğűitar Ĥűŋŧ", which should render as GUITAR HUNT.)

- #18 (partial): a collapsed equalizer on a skin that ships neither eq_ex.bmp nor
  EQMAIN.BMP used to render as a fully transparent bar — the close box was still
  clickable but invisible, so the window looked broken. The EQ shade now falls
  back to a solid bar painted in the skin's text-background colour plus the
  titlebar close button, so it always reads as a real, restorable bar. Covered by
  two unit tests (opaque output with and without a titlebar.bmp). Optional visual
  check: load a minimal skin missing those bitmaps, collapse the EQ, and confirm
  the shade bar is visible with a working close button. (The bundled skins all
  ship EQMAIN, so this fallback only shows on sparse third-party skins.)

- #18 (partial, cont.): the main window's volume and balance sliders now draw a
  synthesized groove + thumb when the skin omits VOLUME.BMP / BALANCE.BMP, so the
  level and pan stay visible instead of sitting in a dead recess. The thumb
  position is unit-tested; the appearance is a plain grey groove with a lighter
  thumb. Optional visual check: on a skin missing those bitmaps, confirm the
  volume/balance levels read clearly and track the controls. (All bundled skins
  ship both bitmaps, so this only shows on sparse third-party skins.)

- #18 (follow-up): aligned the synthesized volume/balance thumb with the actual
  click-to-value mapping — the fallback thumb is now 14px wide with a (track-14)
  travel, matching the real VOLUME/BALANCE sprite and the drag handler, so the
  drawn thumb sits exactly where a click on it lands. Unit-tested; no visual
  review needed beyond the existing fallback check above.
