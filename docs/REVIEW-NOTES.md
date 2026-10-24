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

- Skins (#76): added three more bundled skins — ZaxonRemake1-0 (brushed metal),
  XMMS-Turquoise (dark/turquoise) and Vizor1-01 (biomechanical artwork). Each was
  rendered with --render-all and visually checked across the main/EQ/playlist
  windows, and all three load and compose under the every-bundled-skin test. They
  come from the webamp project's curated demo-skin set (same provenance as the
  already-bundled MacOSXAqua, TopazAmp, Green-Dimension and Skinner_Atlas). Before
  release: confirm the redistribution terms are acceptable alongside the existing
  skins, and give each a final visual pass. Note Vizor1-01's playlist uses a bronze
  background with dark text by design (not a rendering bug).

- Parity: mouse wheel over the main window now adjusts volume (scroll up louder,
  down quieter), matching classic Winamp; the playlist still scrolls and the EQ
  window has no wheel action. The direction/clamp logic is unit-tested; the feel
  (step size, direction) wants a quick manual check — scroll over the main window
  and confirm the volume slider moves the expected way. Found during a Winamp
  classic-parity audit; the other gaps found were filed as issues #139 (drag the
  EQ graph to draw the curve) and #140 (double-click a band/preamp/balance to
  reset), not implemented this pass.

- Parity (#140, partial): double-clicking an EQ preamp or band slider now resets
  it to 0 dB (flat), like Winamp. The double-click detection and reset wiring want
  a quick manual check — double-click a band and the preamp and confirm each
  snaps to centre and the sound updates. The double-click timing logic is
  unit-tested. Balance double-click-to-centre is still open under #140.

- Parity (#140, complete): double-clicking the main window's balance slider now
  re-centres it (balance 0), matching Winamp; this completes #140 alongside the EQ
  band/preamp reset. Both use egui's native double-click detection. Manual check:
  pan the balance off-centre, double-click it, and confirm it snaps back to centre.

- #18 (QA fix): a collapsed EQ on a skin missing both eq_ex.bmp and EQMAIN.BMP
  previously rendered unreadable (black-on-black) with no sliders. It now draws a
  solid bar in the skin's background colour plus synthesized volume/balance thumbs
  and a close marker, in a tone picked to contrast that background (light on dark,
  dark on light), so it's minimally usable on any skin. Unit-tested. Manual check:
  load qa-tools/test-skins/base_no_eq.wsz, collapse the EQ, and confirm the bar and
  its volume/balance thumbs are visible and draggable.

- #141 (shuffle bag): shuffle now plays every track once per cycle (a shuffled
  permutation) before repeating, instead of picking at random with replacement, and
  never replays the current track back-to-back. The play-through property is
  unit-tested, but the feel wants a listen-test with shuffle on: confirm Next walks
  through the whole list without early repeats, a new cycle reshuffles, and Previous
  steps back sensibly (prev-in-shuffle walks back through the bag, which is a
  reasonable approximation of Winamp's history-based prev, not an exact match).

- #142 (partial, .EQF interop): the EQ Presets menu now has Import/Export preset
  (.eqf), reading and writing Winamp's "Winamp EQ library file v1.1" format. The
  byte<->dB mapping and round-trip are unit-tested, but the file-dialog path wants
  a manual check: import a real Winamp .eqf (e.g. a winamp.q1 library) and confirm
  the curve looks right, then export and re-import to confirm the shape survives.
  Note Winamp's EQ range is wider than LinAmp's +/-12 dB, so imported presets are
  scaled into LinAmp's range (shape preserved, extremes compressed). The custom
  named-preset library (save/load presets within LinAmp) is still open under #142.

- #130 (right-click Stop menu): right-clicking the main window's Stop button now
  opens a stop-options menu (Stop / Stop after current / Stop with fade out)
  instead of the generic Options menu; right-clicking elsewhere still opens
  Options. The underlying actions were already implemented and the Stop hit region
  is already tested, so this is UI wiring. Manual check: right-click the Stop
  button, confirm the three options appear and each works, and that right-clicking
  elsewhere on the main window still shows the Options menu.

- #144 (partial, 2.x keyboard shortcuts): added the classic numeric-keypad
  transport set — 0 open file(s), 5 play, 4/6 previous/next track, 8/2 volume
  up/down, 9/7 seek forward/back 5s, 3/1 skip forward/back 10 tracks — plus Ctrl+Z
  (jump to start of list) and Ctrl+R (reverse the playlist). The jump-by-N clamping
  and the reverse are unit-tested. Manual checks: (1) the keypad digits do the
  above; note egui's logical key can't distinguish the numpad from the top number
  row, so these also fire for the top-row digits — confirm that's acceptable, or
  this needs a physical-key path. (2) Verify Ctrl+Z and Ctrl+R against real Winamp
  2.666 (these letter combos came from the issue's research, not re-confirmed this
  cycle). Still open under #144: Ctrl+A always-on-top (resolve the playlist
  select-all conflict), Ctrl+E EasyMove, Alt+W toggle the main window, and
  Ctrl+keypad-0 open-location (ties to #15 Open URL).

- #145 (closed, remember window layout): this was already implemented — window
  positions, open/visible state and window-shade state all persist to session.json
  and restore on launch, with an off-screen clamp (tested `nudge_onto`) that pulls a
  saved layout back onto the desktop after a resolution drop. Closed as resolved;
  this cycle only corrected a stale code comment that claimed positions weren't
  remembered and added a legacy-session assertion. Manual check to confirm the
  close is right: move the windows, window-shade one, close another, relaunch, and
  confirm the exact layout (positions + shade + which windows are open) comes back;
  optionally drop screen resolution and confirm an off-screen window is nudged back.

- #17 (partial, window-shade polish): added the mini position/seek slider to the
  collapsed main window bar — a track plus a thumb that tracks playback, draggable
  and click-to-seek, reusing the expanded posbar's seek drag. Coordinates are from
  the Webamp reference (track at x226, 17px wide; 3px thumb). The hit region is
  unit-tested; the appearance can't be verified headlessly (there's no shade-render
  path). Manual check: collapse the main window (the title-bar shade button), play a
  track, and confirm the mini slider fills as it plays and that clicking/dragging it
  seeks. The always-on-top toggle and mini-transport parts of #17 were already done;
  this leaves #17 only needing a visual sign-off on the new slider.

- #45 (bookmarks, Alt+I): bookmark the current track with Alt+I; recall from a new
  Bookmarks list in the Options menu (click a bookmark to play it, adding it back to
  the playlist if needed), plus "Add current track" and "Clear bookmarks". Bookmarks
  persist to a bookmarks.json next to the config. The store (add/dedup-by-path/remove
  and JSON round-trip) is unit-tested. Manual checks: play a track and press Alt+I
  (status shows "Bookmarked: …"); open Options > Bookmarks and click the entry to
  play it; confirm bookmarks survive a restart. Known gaps for a follow-up: no
  per-item remove in the menu yet (only Clear all), and stream/URL bookmarks wait on
  #15 (Open URL). The menu is a new popup opened after the Options menu closes
  (classic popups aren't nested) — worth a quick look that the hand-off feels right.

- Docs (#77-related): added docs/SHORTCUTS.md, a full keyboard/mouse reference
  derived from the input handler, and linked it from the README. This is the
  "source of truth" #77 asks to audit the Preferences window against. No code
  change, so nothing to validate beyond a read-through that the listed shortcuts
  match what the app actually does. #77 itself stays open: the Preferences window
  deliberately surfaces only persistent settings (one-shot actions and view toggles
  keep their shortcuts but aren't shown there), so that's a design call to confirm.

- #45 (bookmarks hardening): reviewed this branch's new code and tightened the
  bookmarks feature — playing a bookmark whose file has since moved/been deleted now
  reports "Bookmark file not found" instead of appending a permanently-unplayable
  row to the playlist, and the Bookmarks menu now offers "Remove current track from
  bookmarks" when the playing track is already bookmarked (otherwise "Add current").
  Covered by the existing store tests. Manual check: bookmark a track, then move or
  delete its file and click the bookmark — expect the status message, not a dead
  playlist row; and confirm the add/remove entry flips based on the current track.
  Full arbitrary per-item removal from the list is still a follow-up under #45.

- #142 (closed, custom EQ preset library): the EQ Presets menu now lists user-named
  presets below the built-ins and has "Save current as…", which opens a name dialog
  and stores the current curve (replacing one of the same name). Presets persist to
  eq_presets.json; the store (save/replace-by-name/remove + JSON round-trip) is
  unit-tested. With .EQF import/export already done, this completes #142's scope.
  Manual check: set a curve, Save current as… with a name, pick another preset, then
  reopen the menu and load your saved one back; confirm it survives a restart. A
  menu affordance to delete a single saved preset is tracked as a small follow-up.

- #147 (closed, delete a saved EQ preset): the EQ Presets menu now shows "Delete a
  saved preset…" when the library is non-empty; it opens a list (same deferred-popup
  pattern as Options > Bookmarks) where clicking an entry removes that preset and
  re-saves the library. Manual check: save a couple of presets, then Delete a saved
  preset… and remove one — confirm it disappears from the list and stays gone after
  a restart. Deletion by name is covered by the config store test.

- Code quality (no behavior change): routed the EQ preset menu delete through the
  by-name store method (clearing a dead-code warning) and broadened the ID3v2
  round-trip test to assert every editable frame, not just a sample. No manual
  review needed beyond confirming tests pass.

- #15 (Open location / stream playback, Ctrl+L): added streaming of an HTTP(S) URL
  (internet radio or a remote file). Ctrl+L (or the dialog) opens a URL; a background
  thread fetches it with ureq and decodes with symphonia directly — rodio's Decoder
  needs Read+Seek, which a live stream isn't, so the samples flow through a bounded
  channel into a rodio Source that reuses the EQ / analyzer / balance pipeline. Only
  the MIME→extension mapping is unit-tested; the streaming path CANNOT be verified
  headlessly, so it needs a real live test. Manual checks: press Ctrl+L, enter a
  direct audio URL (e.g. a .mp3) and an internet-radio stream URL (MP3/AAC) — confirm
  each connects and plays with EQ/analyzer working, and that Stop ends it cleanly.
  Known limitations to note: the now-playing LCD/title stays blank while streaming
  (no playlist track backs it); seeking is a no-op on a live stream; and an output
  device change restarts the stream from live. New deps: ureq, symphonia.

- #146 (Minibrowser): a themed "nod" to Winamp's SHOUTcast pane rather than a real
  web view (LinAmp has none). A Minibrowser window (Options > Show Minibrowser) shows
  the configured address and an "Open in browser" button that launches the system
  default browser (via the `open` crate). The address is a persisted, user-editable
  Preferences setting (Display tab) defaulting to the project repo. The default value
  is unit-tested; the window itself is UI, so it needs a visual check. Manual checks:
  Options > Show Minibrowser opens a themed window; "Open in browser" opens the URL
  in your browser; set a different URL in Preferences and confirm it persists across
  a restart and is what the button opens. Note: the window is themed via the skin's
  egui colors with an on-theme border (like the menus), not a full gen.bmp classic
  frame; open-state isn't persisted (it opens from the menu each session).

- Optional Features (Preferences): added a new "Optional Features" Preferences
  category that leads with an enable-at-your-own-risk note (obsolete/incomplete
  "full parity" extras). It now gates two opt-ins, both off by default: "Enable the
  Minibrowser" (with its address field; the Options > Minibrowser item and window
  only appear when enabled) and "Bind the numeric keypad to transport controls"
  (the keypad shortcuts from #144 now do nothing unless this is on). Default-off is
  unit-tested. Manual check: open Preferences > Optional Features, confirm the note
  and both toggles; enabling the Minibrowser makes its Options entry appear, and
  enabling the keypad makes 0/1-9 drive transport; both settings survive a restart.

- #15 polish: the main title now shows the stream location (scheme trimmed) while an
  "Open location" URL is playing, instead of staying blank. The URL-display helper is
  unit-tested. Manual check: play a stream and confirm the title shows the address
  and scrolls. Parsing live ICY stream-title metadata (the actual song name) is a
  follow-up.

- #15 (ICY stream titles): radio streams that send ICY metadata now show the current
  song in the title (falling back to the address when a stream has no metadata). The
  decode thread requests `Icy-MetaData: 1`, strips the interleaved metadata blocks
  before decoding, and publishes the latest `StreamTitle` to the UI. The metadata
  stripping and title parsing (including a clean-EOF edge case) are unit-tested; the
  live path needs a real ICY radio stream to confirm. Manual check: play an Icecast/
  SHOUTcast MP3 stream and confirm the title shows the song and updates as it changes.

- #15 robustness (bug fix): opening a location no longer risks freezing the UI if a
  server connects but stalls — the fetch now has a 10s connect timeout (no read
  timeout, so live streams keep flowing) and the open handshake waits at most 20s
  before returning a "timed out opening stream" error. Hard to unit-test (needs a
  dead/slow host); manual check: Ctrl+L a bad host or an unreachable URL and confirm
  the app stays responsive and shows the error rather than hanging.

- #148 (non-blocking stream connect): opening a location no longer blocks the UI at
  all — the connect/probe runs off-thread and the app polls each frame, showing
  "Connecting: …" until the format is known (or a 20s timeout / error). Replaces the
  earlier bounded-but-still-blocking open. Manual check: Ctrl+L a slow or unreachable
  URL and confirm the UI stays fully responsive (windows drag, transport works) while
  it connects, then plays or shows the error/timeout. Note: a device change during a
  stream now stops it rather than restarting from live (re-open with Ctrl+L).

## 2026-10-24 (QA follow-ups)

- #15 (stream title not showing with a playlist loaded, bug fix): the title only
  showed the stream/ICY metadata when there was no current playlist track, so with a
  playlist loaded the stale track name shadowed the stream. The stream (what's
  actually playing) now takes precedence over any still-selected playlist track.
  Can't be verified headlessly. Manual check: load a playlist, then Ctrl+L an ICY
  radio stream and confirm the scrolling title shows the stream address and then the
  live song name, not the old playlist track.

- Bookmarks menu actions off-screen with a long list (bug fix): the Add/Remove/Clear
  actions sat after an unbounded scroll area, so ~20 bookmarks pushed them past the
  window's bottom edge and they became unreachable. The actions are now pinned to a
  bottom panel and the list scrolls in the space above them. Manual check: add 20+
  bookmarks, open Options > Bookmarks, and confirm the list scrolls while Add current
  / Clear bookmarks stay visible and clickable at the bottom.

- Popup menu one-frame flash (bug fix): opening a classic menu briefly flashed the
  popup window at a default position before it moved to the click point (the OS
  creates the window, then `with_position` moves it). The popup is now kept hidden on
  its opening frame and revealed the next frame, once positioned. Visual check:
  open the Options menu and the playlist/EQ menus repeatedly and confirm there's no
  longer a flash of the window drawing elsewhere first.

- EQ buttons misaligned on some skins (OPEN, needs repro): reported on MacOSXAqua1-5
  and Osmosis_2 — the ON/AUTO/PRESETS buttons don't line up with their slots. The
  draw coords and the click hit-regions both match the canonical Winamp EQMAIN.BMP
  layout (On 14,18 26x12; Auto 40,18 32x12; Presets 217,18 44x12) that every other
  bundled skin renders correctly, so the coords were left unchanged rather than risk
  breaking the other 11 skins. Needs a visual repro on these two skins compared with
  real Winamp 2.666 to find what differs. Filed as an issue.

- Minibrowser "embed a real browser" (OPEN, design decision): the current window is a
  themed simulation that opens the address in the system browser (the agreed option
  B — a nod to the original, no web view). A request to embed an actual browser view
  would need a native webview dependency (e.g. wry/WebView2) overlaid on the egui
  window, a significant scope change. Left as-is for now per the user.

- Media Library Bookmarks view (wired up): the left-nav "Bookmarks" entry was a dead
  placeholder label; it's now a real, clickable view listing the same bookmarks as
  Options > Bookmarks. Each row shows the track's full library metadata when it's
  indexed, otherwise the bookmark's saved title (so bookmarks to un-scanned files
  still list and play). Double-click plays; the row context menu's "Remove" says
  "Remove bookmark" here and drops it from the store (not the library DB); the search
  box filters the list. The DB lookup (track_for_path) is unit-tested; the view/menu
  wiring is UI. Manual check: add a few bookmarks (Alt+I), open the Media Library,
  click "Bookmarks", confirm they list, double-click plays, right-click > Remove
  bookmark drops just that one, and the search box filters. The "Playlists" nav entry
  is still an intentional placeholder (saved-playlist management is issue #150).

- Open location in the Options menu (discoverability): "Open location… (Ctrl+L)" is
  now in the right-click Options menu, just under Open file(s)/Open folder, so a
  stream can be started with the mouse instead of only the keyboard shortcut. The
  Options menu height is now computed from the item count (including the optional
  Minibrowser toggle) rather than a fixed value, so the last row no longer risks
  clipping when the Minibrowser item is shown. Manual check: right-click the main
  window, pick "Open location…", and confirm the URL dialog opens; with the
  Minibrowser enabled in Preferences, confirm the Options menu still shows every row
  (Exit at the bottom) without clipping.
