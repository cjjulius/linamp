# Review notes, feat/feature-set

Items needing a human eye or interaction to verify (not fully validatable by
code plus a working/not-working binary): visual and behavior changes.

## 2026-10-26 (crossfade, off by default, needs audio QA)

- Crossfade between tracks added (#43), off by default. When on, the end of one
  track overlaps the start of the next: Preferences > Playback > Crossfade has the
  toggle and a 1-12s length slider. Mechanism: the player keeps the outgoing track
  on its own sink fading out while the new one fades in; the app triggers it when
  the current track reaches (length - crossfade seconds), advancing the playlist
  early, and the normal end-of-track auto-advance covers the last track. Only finite
  local tracks crossfade; streams, "stop after current", and "repeat one" are
  excluded, and the hard-cut path is unchanged when the pref is off. The ramp math
  (crossfade_step) is unit-tested, but the actual blend needs an ear: verify it
  sounds smooth (no click, pop, or volume dip at the handoff), that a manual skip
  mid-crossfade doesn't jump to full volume, and that gapless-ish back-to-back
  tracks don't double-trigger. Suggested: set 4-6s and listen across two short
  tracks, then try skipping during the overlap.
- Follow-up (2026-10-26): pausing mid-crossfade now holds both tracks. Earlier,
  pause/resume only touched the incoming sink, so a pause during the overlap left
  the outgoing track still playing, and the ramp kept advancing while paused (it
  would reach the end and drop the outgoing track silently). Pause/resume now act
  on both sinks and the ramp freezes while not playing. The freeze is unit-tested;
  the audible part to confirm: start a crossfade, hit pause during the overlap, and
  both tracks should stop together and resume together with the blend intact.
- Follow-up (2026-10-26): stopping with fade-out near a track's end no longer jumps
  to the next track. The near-end crossfade trigger did not check for an in-progress
  stop-fade, so hitting Stop (with "fade out when stopping" on, or Shift+V) inside
  the last few seconds of a track would start the next one instead of stopping. The
  gate now also holds back while a stop-fade is running. The gate is unit-tested
  (crossfade_due); to confirm by ear with crossfade on: start a track, let it reach
  the final few seconds, hit Stop, and it should fade out and stop rather than cross
  into the next track.
- Follow-up (2026-10-26): seeking during a crossfade now cancels the overlap. A seek
  only moved the incoming track, so the outgoing one kept playing underneath for the
  rest of the fade (two tracks at once). A seek now stops the outgoing track and
  brings the incoming one to full gain. This path touches live audio sinks, so it is
  not unit-tested; to confirm by ear with crossfade on: let a track reach its final
  few seconds so the next one starts fading in, then drag the seek bar, and you should
  hear only the one (now full-volume) track, not both.
- Follow-up (2026-10-26): crossfade between two tracks on the same CD now cuts cleanly
  instead of overlapping. A single optical drive can't serve two read streams at once,
  so overlapping an outgoing and incoming track made the head seek back and forth
  between them and stutter, worse the further apart the tracks sit on the disc. A move
  between a file and the CD (either direction) still crossfades, since only one stream
  touches the drive. Confirm by ear: with crossfade on, moving from one CD track to
  another is a clean cut (no stutter), while CD to file and file to CD still blend.

## 2026-10-26 (Windows CD, best-effort/untested)

- Windows audio-CD device layer added (#160), per the maintainer's "write it
  best-effort, verify later." A new WindowsCdDrive in src/cdda.rs implements the
  shared CdDrive trait with DeviceIoControl (IOCTL_CDROM_READ_TOC for the track
  list, IOCTL_CDROM_RAW_READ for raw 2352-byte CDDA sectors), hand-declared Win32
  FFI (no new crate), routed through the Linux box's BufferedCdSource. The
  Options "Open Audio CD" item and open_audio_cd now build on Windows too. It
  COMPILES on Windows and the MSF-to-LBA conversion is unit-tested. Linux CD is
  unchanged.
- Update (2026-10-26): verified on real Windows hardware (a USB BD-RE at D: with an
  audio CD). The device path works end to end at the read level: the drive opens,
  the TOC read returns the audio tracks, and raw CDDA sector reads return real
  2352-byte sectors (two distinct sectors read back with differing audio data). This
  is covered by a hardware-guarded test, reads_real_disc_sectors_when_present, that
  skips when no drive or disc is present. So the raw-read byte offset, the CDROM_TOC
  layout, and the D: default are confirmed. What is still unconfirmed is playback by
  ear: open an audio CD from Options to Open Audio CD and confirm it plays cleanly
  through the buffer without stutter, and that seeking and track changes behave.
- Fix (2026-10-26): on Windows the CD loaded its tracks and times but played only
  silence. Cause: IOCTL_CDROM_RAW_READ rejects more than 27 raw sectors per call
  (error 87), but playback reads 75-sector (one second) batches, so every read on
  the background thread failed and the reader ended the track at once. The single
  sector reads the earlier hardware test used stayed under the limit, so it hid.
  WindowsCdDrive::read_sectors now splits a request into chunks of 27 and
  concatenates, and the hardware test now also reads a full 75-sector batch so the
  limit can not regress unnoticed. Needs a by-ear recheck on Windows: open an audio
  CD and confirm it now plays.

## 2026-10-26 (tracker length)

- Tracker modules played past the time the playlist showed (confirmed on a looping
  .mod: shown 1:57, clock ran to 2:02+ while "time remaining" sat at 0). Cause: a
  module reports its song length (duration_seconds) but xmrsplayer follows the
  module's loop/jump effects a little past that before set_max_loop_count(1) halts
  it, so audio outran the shown length. Linear modules (e.g. sample_scale.mod) are
  unaffected, which is why it reproduced only on looping ones. TrackerSource now caps
  output at the reported duration, so playback stops at the shown length (the natural
  pattern end, where a loop would restart) and the seek/time display stay in sync. A
  test asserts actual playback == reported length (never over). Verify by ear with a
  looping module: the clock should reach the shown length and the track should end /
  advance there, not keep playing.

## 2026-10-26

- Audio CD playback (Linux), native drive reads (#160). The stutter where a CD
  played a few seconds, stopped, then respun was diagnosed on the Linux box: it was
  not LinAmp's CDDA code (that device layer was unwritten), it was the disc being
  played through the desktop's GVfs cdda:// mount as WAV files. That FUSE path
  delivers around 164 kB/s in bursts, at or below the 176 kB/s real-time need, so
  the output starved, the drive spun down then seeked and spun up to resume. The fix
  is a real Linux device layer: raw CDROMREADTOCHDR / CDROMREADTOCENTRY /
  CDROMREADAUDIO ioctls on /dev/sr0 (no libcdio, so the licence stays clean), feeding
  the shared CdAudioSource groundwork. Reads run on a background thread into a
  32-batch (about 32 second) prefetch buffer, so the multi-second read spikes the
  drive produces are absorbed instead of starving playback. Verified on real hardware
  (10-track disc, /dev/sr0): a real-time consumer saw zero underruns across a run
  where the drive spiked to 2.6s on one batch and 1.5s on the next, the buffer rode
  both out. All Linux-gated (cfg(target_os = "linux") plus a Linux-only libc dep), so
  the Windows and macOS builds are unchanged, their Open-Audio-CD entry is absent and
  the cdda:// play path returns a clean "Linux only" error. Needs a human check on
  Pop!_OS: insert an audio CD, Options menu to Open Audio CD, confirm the tracks load
  and the first plays through without the stop/respin stutter, and that seeking within
  a track lands and keeps playing. Known artifact, not the bug: about 2 seconds of
  silence at the very start of a track while the drive spins up (physically
  unavoidable, the reader has no buffer yet at t=0); playback is smooth after that.
  Also note: the user must be in the "cdrom" group (or have the session ACL on the
  device) to read /dev/sr0.

- Audio CD, transparent GVfs redirect (Linux, #160). Following on from the native
  CDDA reads: the natural way to reach a CD is to Add file(s) and pick the tracks
  the desktop shows, which are slow FUSE WAV files under
  /run/user/<uid>/gvfs/cdda:host=sr0/Track N.wav. Adding those now transparently
  stores a native cdda:// entry instead (detected in playlist add_file / add_folder,
  so every entry point is covered: Add file(s), Add folder, Open, drag-drop). The
  disc TOC is cached about 10s per device so adding a folder reads the disc once, not
  per file. So the user does not need the Options to Open Audio CD item, the ordinary
  add flow plays natively. Needs a human check on Pop!_OS: with a CD in, Add file(s),
  pick the GVfs Track N.wav files, confirm they play gaplessly off the drive (not the
  stuttering FUSE path) and show as Track N with the right length. Known cosmetic
  edge: adding the whole GVfs folder orders tracks lexically (1, 10, 2, ...), the
  track labels and lengths are still correct, and Open Audio CD lists them in disc
  order.

- Audio CD, distro-portability hardening (Linux, #160). Three additive, guarded
  changes, none of which alter the GNOME/GVfs path that Mint and Pop!_OS use, both
  re-verified on a real Pop!_OS disc (native TOC read plus GVfs redirect resolve the
  disc correctly): (1) the ioctl request is now cast with `as _` so it builds on musl
  (Alpine, Void), a no-op on glibc; (2) `default_device` falls back to scanning
  `/dev/sr0..7` when `/dev/cdrom` is absent, so a drive at sr1+ is found (the GVfs
  redirect already reads the real device from the mount path); (3) a best-effort KDE
  Plasma kio-fuse detector (`.../kio-fuse-*/audiocd/.../Track N.*`) so the auto
  redirect can work on Plasma too. The KDE detector requires both a `kio-fuse` and an
  `audiocd` path component plus a numeric file name, so it cannot fire on a GVfs or
  ordinary path. KDE is UNVERIFIED (no Plasma box here), it needs a human check on a
  real KDE desktop: insert a CD, Add file(s), pick the tracks, confirm native
  playback. On any desktop where the redirect does not fire, Options to Open Audio CD
  still plays the disc natively.

- Single-instance guard: LinAmp now holds an advisory file lock
  (data dir / LinAmp / instance.lock) at startup, so a second launch exits instead
  of opening a second, conflicting set of classic windows. The headless flags
  (--check-cur, --render-*, etc.) are exempt. The OS releases the lock on exit or
  crash, so a stale lock can't wedge startup; a failure to create the lock lets the
  app start anyway. The lock refuse/release logic is unit-tested, but the actual
  "second launch exits" behaviour needs a human check on Windows AND Pop!_OS: launch
  LinAmp, launch it again, and confirm the second one does not open a second window
  set (it should just exit). Note: it does not yet forward a file passed to the
  second launch to the running instance (so "open with" while running is a no-op for
  now, not a new window), that arg-forwarding is a follow-up.

- MIDI seek felt laggy (a second or two) because every seek replayed the whole
  sequence from the top and re-synthesized up to the target. A forward seek now
  continues from the current position (tracked as seq_pos) and only renders the
  delta; a backward seek still restarts then fast-forwards. Dragging the seek bar
  ahead is now quick; a long backward jump still pays for the re-render (rustysynth
  can't seek directly). Verify by ear: drag the MIDI seek bar forward in small and
  large steps and confirm it responds quickly, and that a jump back still lands and
  plays correctly.
- .mod length report (plays past the shown time): could not reproduce. Measured on
  the qa sample, the shown length, the tracker's reported duration and the actual
  rendered length all agree (1:32 / 92.16s). Most likely Repeat was on (which
  replays the track) or an older build. No code change; needs the maintainer to
  confirm the build commit (Preferences) and whether Repeat was enabled.

## 2026-10-25 (later 9)

- Drag-drop insert follow-up: inserting dropped tracks mid-list shifts the rows
  below the drop, but the playlist selection wasn't shifted with them, so selected
  rows would highlight the wrong tracks after a drop. move_tail_to now returns the
  insertion index and handle_dropped shifts the selection to match. Covered by the
  move_tail_to tests; verify by hand: select a few rows, drop a file above them, and
  confirm the same tracks stay selected.

## 2026-10-25 (later 8)

- Drag-and-drop now inserts at the drop point. Dropping files or folders onto the
  playlist window inserts them at the row under the release point instead of always
  appending; dropping on the main/EQ window (or in the modern fallback) still
  appends. Folders and playlists expand in place, unsupported files are skipped
  (same as menu Add), and the now-playing track is preserved. The insert/reorder
  math (move_tail_to) is unit-tested; the pointer-to-row mapping is UI, so verify by
  hand on Windows and Linux: drag files onto the top, middle and bottom of the
  playlist (and into an empty list) and confirm they land where dropped; drag onto
  the main/EQ window and confirm it appends.

## 2026-10-25 (later 7)

- Tag editor opened slowly on large non-MP3 files. read_info always ran the MPEG
  frame scanner, which scans a whole non-MPEG file before failing (~60s on a 13MB
  .aiff), freezing the File Info window as it opened. Gated the scan to MPEG
  extensions; other formats were already showing blank MPEG fields, just slowly, so
  behaviour is unchanged apart from opening instantly. Verify: open the tag editor
  on a large .wav/.aiff and confirm it appears immediately.
- Version bumped to 0.5.3 (building toward a 0.5.x release).

## 2026-10-25 (later 6)

- MP2 (MPEG Layer II) didn't actually decode. The symphonia dependency enabled
  only the "mp3" (Layer III) feature, so a real .mp2/.mpa failed with "unsupported
  codec" (confirmed against qa-music/Generated_tests/sample_scale.mp2), #157's
  Layer I/II support was never wired up. Enabled symphonia's "mp1" and "mp2"
  features; a test now decodes and seeks the sample. With the format decoding, the
  existing seek path works, so the requested MP2 seeking works too. Verify in-app
  on Windows and Linux: play an .mp2 and drag the seek bar, it should jump and
  keep playing. (Layer I .mp1 is enabled too but untested for lack of a sample.)

## 2026-10-25 (later 5)

- MIDI seeking added. rustysynth's sequencer can't seek and MIDI state is
  cumulative, so a seek restarts the sequence and fast-forwards (renders and
  discards) to the target; synthesis is far faster than realtime so it's quick, and
  the voice/controller state we land on is exactly what normal playback to that
  point would give (no hanging notes). Verify by ear: start a MIDI, drag the seek
  bar forward and back, and confirm playback jumps and continues cleanly from the
  new spot.

## 2026-10-25 (later 4)

- CD playback stutter (#160): on the first real-hardware test, CD audio played but
  stuttered, the drive audibly spun up again mid-track and track changes were slow.
  Cause: CdAudioSource read one 2352-byte sector (~13ms of audio) per audio refill,
  so it hit the drive constantly and any re-seek/spin-up became a gap. It now reads
  ~1 second of sectors per access (READAHEAD_SECTORS) through a new batched
  CdDrive::read_sectors, so the drive streams. The default read_sectors just loops
  read_sector; the device layer (Linux box) should override it to read the whole run
  in one request for the real win. Needs a human ear on the drive to confirm the
  stutter is gone. Two items for the Linux box, which owns the device + TOC/playlist
  code: (1) implement a true batched read_sectors (single ioctl/DeviceIoControl per
  run); (2) the CD tracks show up labelled as WAV, set a sensible per-track title/
  format where the TOC is turned into playlist entries.

## 2026-10-25 (later 3)

- #154 menu sizing, proper fix. The earlier "measure the content and hug it"
  attempt didn't work: it read ui.min_rect() on a CentralPanel, which is the whole
  window, so the window always stayed at the formula estimate. The formula was off
  two ways, the per-row budget overestimated (main Options menu left an empty band
  below Exit) and the playlist menus' item counts were too low (PlVoid counted 12/4
  but draws 14/5, PlMisc counted 6 but draws 8), so those clipped their last rows.
  Fixed the row/separator constants to egui's real layout (20px row, 8px separator,
  margins) and corrected the counts; removed the dead measurement machinery
  (popup_content_h). A test now pins each fixed menu's item/separator counts against
  the size formula so they can't drift again. Verify visually on Windows and Linux:
  right-click the main, EQ and playlist windows (and the playlist's bottom-bar
  fly-outs) a few times each, every menu should hug its items, with nothing clipped
  and no empty band, on repeat opens.

## 2026-10-25 (later 2)

- Supported-formats list claimed Opus and WavPack, which symphonia 0.5 can't
  decode, so those files were accepted into playlists then silently skipped at
  play time. Dropped .opus and .wv from SUPPORTED_EXTS (and from FEATURES.md /
  README.md); a decode test now asserts the symphonia registry has decoders for
  the formats we advertise and none for Opus/WavPack. Behaviour change to note:
  any .opus/.wv files are no longer recognised in the open dialog or added to
  playlists (they never actually played). FEATURES/README also now list trackers
  and MIDI as shipped rather than planned. No in-app audio check needed.

## 2026-10-25 (later)

- MIDI release/reverb tail was being cut off: playback stopped one ~23ms block
  after the last MIDI event, so note releases and the reverb/chorus decay were
  clipped. It now keeps rendering after the sequence ends until the tail falls
  below a silence threshold (with a ~3s safety cap so a stuck voice can't render
  forever). A test confirms a one-note MIDI rings out past the note and still
  terminates within the cap. Verify by ear: play a MIDI with a sustained/reverbed
  ending and confirm it fades out naturally instead of stopping abruptly.

## 2026-10-25

- #149 EQ slider thumb off the groove: confirmed it was ours, not the skins. The
  11px thumb was inset 2px into its 14px band cell, so its centre landed at local
  x 7, while every skin draws its groove centred at local x ~6 (measured
  identically on the base skin, MacOSXAqua and Osmosis). Dropped the inset to 1 so
  the thumb centres on the groove; a regression test now asserts the thumb centre
  sits within a pixel of the measured groove for those three skins. It was a ~1px
  shift that only showed on crisp/narrow grooves (MacOSXAqua, Osmosis), wide
  grooves like the base skin's masked it. Verify visually: open the EQ on
  MacOSXAqua and Osmosis and confirm each thumb now rides its bar, then spot-check
  a few wide-groove skins (base, Green Dimension, Topaz) to confirm nothing drifted.

## 2026-10-22

- #105 volume not remembered: fixed by adding volume and balance to the session
  change-detection hash in `persist_session_if_changed` (they were saved by
  `build_session` but never marked the session dirty, so a volume-only change was
  lost on the next launch). Verify interactively: change the volume, quit,
  relaunch and confirm the level is restored. End-to-end persistence was
  confirmed by code inspection only, the automated test harness could not move
  the volume control via synthetic mouse or keyboard input.

- #115 ID3v2 to ID3v1 copy corruption: reproduced (writing a multibyte ID3v1
  field panicked inside lofty when truncating mid-character, aborting the save
  and corrupting the file) and fixed by capping each v1 field to its byte limit
  on a char boundary (comment to 28 for the v1.1 track byte). Covered by a
  regression test. Optional interactive sanity check: open the tag editor on a
  file with long/non-Latin tags, copy ID3v2 to ID3v1, save and confirm the file
  still plays.

- #97 non-ASCII text readability (partial): the classic bitmap-font path now
  ASCII-folds accented and Vietnamese characters to their base letter (example:
  "Trường Sơn" renders as "Truong Son") instead of blank gaps. Visual change,
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
  check, open the playlist bottom-bar Rem/Misc menus and confirm the new items
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
  backslash separators (example: "Music\Song.mp3", as nearly all Winamp-era
  playlists do) were treated as literal filenames and never resolved on Linux.
  resolve() now converts backslashes to separators for relative entries. The
  path logic is unit-tested; a good end-to-end human check is to load an actual
  Windows-authored .m3u with relative backslash paths and confirm the tracks
  appear.

- PLS entry ordering (bug-fix): .pls parsing used line order and ingested any
  key starting with "File" (example: a stray "FileFoo="). It now orders entries by
  the N in "File<N>=" per the PLS spec and ignores non-File<N> keys. Fully
  covered by a unit test; no manual check required.

- Library search wildcard escaping (bug-fix): the search built a LIKE pattern
  without escaping, so a literal '_' or '%' in the typed text acted as a SQL
  wildcard (example: "track_01" also matched "trackX01"). Wildcards are now escaped
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
  letters that were still rendering garbled, the g/h block (incl. Turkish Ğ and
  Esperanto ĝ/ĥ) and the tails of n/t/u (ŋ, ŧ and Hungarian Ű/Ų/ű/ų). Covered by
  the ascii_fold unit test. Optional visual check: a track title containing Ğ or
  Ű shows as "G"/"U" in the classic marquee, not a blank or garbled cell.
  (A QA song is prepared for this: qa-music/Aria Nocturne - Crystal Harbor.mp3
  now has the title "Ğűitar Ĥűŋŧ", which should render as GUITAR HUNT.)

- #18 (partial): a collapsed equalizer on a skin that ships neither eq_ex.bmp nor
  EQMAIN.BMP used to render as a fully transparent bar, the close box was still
  clickable but invisible, so the window looked broken. The EQ shade now falls
  back to a solid bar painted in the skin's text-background colour plus the
  titlebar close button, so it always reads as a real, restorable bar. Covered by
  two unit tests (opaque output with and without a titlebar.bmp). Optional visual
  check: load a minimal skin missing those bitmaps, collapse the EQ and confirm
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
  click-to-value mapping, the fallback thumb is now 14px wide with a (track-14)
  travel, matching the real VOLUME/BALANCE sprite and the drag handler, so the
  drawn thumb sits exactly where a click on it lands. Unit-tested; no visual
  review needed beyond the existing fallback check above.

- Skins (#76): added three more bundled skins, ZaxonRemake1-0 (brushed metal),
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
  (step size, direction) wants a quick manual check, scroll over the main window
  and confirm the volume slider moves the expected way. Found during a Winamp
  classic-parity audit; the other gaps found were filed as issues #139 (drag the
  EQ graph to draw the curve) and #140 (double-click a band/preamp/balance to
  reset), not implemented this pass.

- Parity (#140, partial): double-clicking an EQ preamp or band slider now resets
  it to 0 dB (flat), like Winamp. The double-click detection and reset wiring want
  a quick manual check, double-click a band and the preamp and confirm each
  snaps to centre and the sound updates. The double-click timing logic is
  unit-tested. Balance double-click-to-centre is still open under #140.

- Parity (#140, complete): double-clicking the main window's balance slider now
  re-centres it (balance 0), matching Winamp; this completes #140 alongside the EQ
  band/preamp reset. Both use egui's native double-click detection. Manual check:
  pan the balance off-centre, double-click it and confirm it snaps back to centre.

- #18 (QA fix): a collapsed EQ on a skin missing both eq_ex.bmp and EQMAIN.BMP
  previously rendered unreadable (black-on-black) with no sliders. It now draws a
  solid bar in the skin's background colour plus synthesized volume/balance thumbs
  and a close marker, in a tone picked to contrast that background (light on dark,
  dark on light), so it's minimally usable on any skin. Unit-tested. Manual check:
  load qa-tools/test-skins/base_no_eq.wsz, collapse the EQ and confirm the bar and
  its volume/balance thumbs are visible and draggable.

- #141 (shuffle bag): shuffle now plays every track once per cycle (a shuffled
  permutation) before repeating, instead of picking at random with replacement, and
  never replays the current track back-to-back. The play-through property is
  unit-tested, but the feel wants a listen-test with shuffle on: confirm Next walks
  through the whole list without early repeats, a new cycle reshuffles and Previous
  steps back sensibly (prev-in-shuffle walks back through the bag, which is a
  reasonable approximation of Winamp's history-based prev, not an exact match).

- #142 (partial, .EQF interop): the EQ Presets menu now has Import/Export preset
  (.eqf), reading and writing Winamp's "Winamp EQ library file v1.1" format. The
  byte-to-dB mapping and round-trip are unit-tested, but the file-dialog path wants
  a manual check: import a real Winamp .eqf (example: a winamp.q1 library) and confirm
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
  transport set, 0 open file(s), 5 play, 4/6 previous/next track, 8/2 volume
  up/down, 9/7 seek forward/back 5s, 3/1 skip forward/back 10 tracks, plus Ctrl+Z
  (jump to start of list) and Ctrl+R (reverse the playlist). The jump-by-N clamping
  and the reverse are unit-tested. Manual checks: (1) the keypad digits do the
  above; note egui's logical key can't distinguish the numpad from the top number
  row, so these also fire for the top-row digits, confirm that's acceptable, or
  this needs a physical-key path. (2) Verify Ctrl+Z and Ctrl+R against real Winamp
  2.666 (these letter combos came from the issue's research, not re-confirmed this
  cycle). Still open under #144: Ctrl+A always-on-top (resolve the playlist
  select-all conflict), Ctrl+E EasyMove, Alt+W toggle the main window, and
  Ctrl+keypad-0 open-location (ties to #15 Open URL).

- #145 (closed, remember window layout): this was already implemented, window
  positions, open/visible state and window-shade state all persist to session.json
  and restore on launch, with an off-screen clamp (tested `nudge_onto`) that pulls a
  saved layout back onto the desktop after a resolution drop. Closed as resolved;
  this cycle only corrected a stale code comment that claimed positions weren't
  remembered and added a legacy-session assertion. Manual check to confirm the
  close is right: move the windows, window-shade one, close another, relaunch, and
  confirm the exact layout (positions + shade + which windows are open) comes back;
  optionally drop screen resolution and confirm an off-screen window is nudged back.

- #17 (partial, window-shade polish): added the mini position/seek slider to the
  collapsed main window bar, a track plus a thumb that tracks playback, draggable
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
  (classic popups aren't nested), worth a quick look that the hand-off feels right.

- Docs (#77-related): added docs/SHORTCUTS.md, a full keyboard/mouse reference
  derived from the input handler, and linked it from the README. This is the
  "source of truth" #77 asks to audit the Preferences window against. No code
  change, so nothing to validate beyond a read-through that the listed shortcuts
  match what the app actually does. #77 itself stays open: the Preferences window
  deliberately surfaces only persistent settings (one-shot actions and view toggles
  keep their shortcuts but aren't shown there), so that's a design call to confirm.

- #45 (bookmarks hardening): reviewed this branch's new code and tightened the
  bookmarks feature, playing a bookmark whose file has since moved/been deleted now
  reports "Bookmark file not found" instead of appending a permanently-unplayable
  row to the playlist, and the Bookmarks menu now offers "Remove current track from
  bookmarks" when the playing track is already bookmarked (otherwise "Add current").
  Covered by the existing store tests. Manual check: bookmark a track, then move or
  delete its file and click the bookmark, expect the status message, not a dead
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
  preset… and remove one, confirm it disappears from the list and stays gone after
  a restart. Deletion by name is covered by the config store test.

- Code quality (no behavior change): routed the EQ preset menu delete through the
  by-name store method (clearing a dead-code warning) and broadened the ID3v2
  round-trip test to assert every editable frame, not just a sample. No manual
  review needed beyond confirming tests pass.

- #15 (Open location / stream playback, Ctrl+L): added streaming of an HTTP(S) URL
  (internet radio or a remote file). Ctrl+L (or the dialog) opens a URL; a background
  thread fetches it with ureq and decodes with symphonia directly, rodio's Decoder
  needs Read+Seek, which a live stream isn't, so the samples flow through a bounded
  channel into a rodio Source that reuses the EQ / analyzer / balance pipeline. Only
  the MIME-to-extension mapping is unit-tested; the streaming path CANNOT be verified
  headlessly, so it needs a real live test. Manual checks: press Ctrl+L, enter a
  direct audio URL (example: a .mp3) and an internet-radio stream URL (MP3/AAC), confirm
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
  server connects but stalls, the fetch now has a 10s connect timeout (no read
  timeout, so live streams keep flowing) and the open handshake waits at most 20s
  before returning a "timed out opening stream" error. Hard to unit-test (needs a
  dead/slow host); manual check: Ctrl+L a bad host or an unreachable URL and confirm
  the app stays responsive and shows the error rather than hanging.

- #148 (non-blocking stream connect): opening a location no longer blocks the UI at
  all, the connect/probe runs off-thread and the app polls each frame, showing
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
  bookmarks, open Options > Bookmarks and confirm the list scrolls while Add current
  / Clear bookmarks stay visible and clickable at the bottom.

- Popup menu one-frame flash (bug fix): opening a classic menu briefly flashed the
  popup window at a default position before it moved to the click point (the OS
  creates the window, then `with_position` moves it). The popup is now kept hidden on
  its opening frame and revealed the next frame, once positioned. Visual check:
  open the Options menu and the playlist/EQ menus repeatedly and confirm there's no
  longer a flash of the window drawing elsewhere first.

- EQ buttons misaligned on some skins (OPEN, needs repro): reported on MacOSXAqua1-5
  and Osmosis_2, the ON/AUTO/PRESETS buttons don't line up with their slots. The
  draw coords and the click hit-regions both match the canonical Winamp EQMAIN.BMP
  layout (On 14,18 26x12; Auto 40,18 32x12; Presets 217,18 44x12) that every other
  bundled skin renders correctly, so the coords were left unchanged rather than risk
  breaking the other 11 skins. Needs a visual repro on these two skins compared with
  real Winamp 2.666 to find what differs. Filed as an issue.

- Minibrowser "embed a real browser" (OPEN, design decision): the current window is a
  themed simulation that opens the address in the system browser (the agreed option
  B, a nod to the original, no web view). A request to embed an actual browser view
  would need a native webview dependency (example: wry/WebView2) overlaid on the egui
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
  window, pick "Open location…" and confirm the URL dialog opens; with the
  Minibrowser enabled in Preferences, confirm the Options menu still shows every row
  (Exit at the bottom) without clipping.

- Media Library Bookmarks view live refresh (bug fix): adding (Alt+I or Options >
  Bookmarks), removing or clearing a bookmark now rebuilds the library's Bookmarks
  list immediately when that view is open, instead of only on the next click.
  Manual check: open the Media Library on Bookmarks, play a track and press Alt+I,
  and confirm the new entry appears in the list without re-selecting the view.
## 2026-10-25 (parity-3 cron)

- #156 (playlist resize past the screen bottom, bug fix): dragging the playlist's
  bottom-right grip to the bottom of the screen grew the window upward (the WM
  pushed the top edge up to keep it on-screen) instead of just stopping. The resize
  now caps the height so the window's bottom can't pass the desktop bottom
  (classic::pl_max_height, unit-tested). Manual check (best on X11, where it was
  observed): drag the playlist taller until its bottom reaches the screen edge and
  confirm it simply stops, the top edge stays put and the window doesn't climb.

- #154 (classic menus a little long, fix): the popup menus budgeted 24px per row
  but render an 18px item, and the inter-row spacing was loose, so menus read
  taller than their content (the Media Library's menus sit tighter). Row budget is
  now 21px via a shared MENU_ROW_H constant and the inter-row spacing is pinned to
  2px, so every classic popup hugs its items. Visual change, confirm the Options,
  playlist and EQ-presets menus look tighter (closer to the Media Library's) with no
  row clipped and the last item (example: Exit) still fully visible.

- #144 (Ctrl+A always-on-top, completing the shortcut set): Ctrl+A now toggles
  always-on-top when a non-playlist window has focus, and still selects-all in the
  playlist window (each classic window is its own viewport, so keys reach only the
  focused one). Manual check: with the main or EQ window focused, press Ctrl+A and
  confirm always-on-top toggles (status line shows it); with the playlist focused,
  Ctrl+A still selects all tracks. The other #144 items are not being implemented:
  Alt+W (show/hide main window) needs a tray to restore and the main window is the
  root viewport (ties to #40, deferred); Ctrl+E EasyMove is a niche drag-from-
  anywhere mode; Ctrl+keypad-0 can't be told from top-row 0 in egui (Ctrl+L already
  opens a location). #144 closed on that basis.

## 2026-10-25 (Slack thread follow-ups)

- #156 (resize clamp): user-verified working, marking confirmed on Linux.
- #154 (menu sizing): reopened. The 21px row-budget change wasn't enough on Linux,
  user reports the classic menu still reads too long there. Likely different popup
  metrics on Linux (font/DPI/spacing); needs a screenshot and a Linux-specific pass.

- #154 (real fix, supersedes the row-budget tweak): the per-row formula over-budgeted
  the popup window on both Windows and Linux, leaving an empty band below the last
  item. The window is now sized to the menu's MEASURED content height, on the first
  (hidden) frame the content height is captured (ui.min_rect), and from the next frame
  the window is sized to it (clamped to the old formula as a ceiling for the scrolling
  menus), with a fixed 6px frame margin and the rounded-corner region rebuilt when the
  size changes. Manual check (both platforms): open the Options, playlist right-click,
  and EQ-presets menus and confirm there's no empty space below the last row, nothing
  is clipped, and the scrolling menus (Bookmarks, EQ presets) still scroll.

- #144 follow-up (Ctrl+A fall-through, per maintainer): in the playlist, Ctrl+A
  selects all; pressing it again once everything is already selected now toggles
  always-on-top (so the playlist keeps a keyboard path to it). Not standard Winamp
  behaviour, added by request. Manual check: in the playlist press Ctrl+A (all
  select), then Ctrl+A again and confirm always-on-top toggles (status line shows it).

- #144 (revert fall-through): removed the repeat-Ctrl+A-toggles-always-on-top touch;
  it isn't Winamp 2.666 behaviour. Ctrl+A in the playlist selects all; from other
  windows it toggles always-on-top.
- #154 (re-click sizing): menus were correctly sized on the first open but reverted
  to too-tall on subsequent right-clicks, the popup OS window was reused and kept a
  prior size. Each open now gets a fresh viewport id (and window), so it is sized to
  its measured content every time. Manual check: right-click the main/playlist windows
  several times in a row and confirm every menu hugs its items (no empty band), not
  just the first.

## 2026-10-25 (audio format compatibility)

- #157 (MPEG Layer I/II playback): .mp1/.mp2/.mpa now play. rodio 0.20's Decoder
  can't pass symphonia a format hint, so these are decoded in a new src/decode.rs via
  symphonia directly (with an extension hint) into an in-memory seekable source; the
  file/stream pipelines were unified behind Player::append_source. The extensions are
  added to SUPPORTED_EXTS and the open-file filter. Routing + the error path are
  unit-tested, but the live decode couldn't be verified here (no MP2 encoder on this
  box). Manual check: open a real .mp2 (and .mp1 if you have one) and confirm it plays
  with EQ/analyzer working. Note: these files are decoded eagerly (whole file up front)
  rather than streamed, so a very large one has a brief load pause; MP1/MP2 are rare
  and usually short, so this is acceptable for now.

- #164 (ISO-MP4 panic, bug fix): opening .m4a/.mp4/.m4b panicked (rodio 0.20's
  Decoder hits an unreachable! seeking an ISO-MP4), which the skip-unplayable guard
  couldn't catch, so it crashed the app. These extensions now route through
  crate::decode (symphonia with a hint), which returns a Result instead of panicking,
  so they play (or fail cleanly, caught by the guard). m4a/mp4/m4b added to
  HINTED_EXTS; mp4/m4b added to SUPPORTED_EXTS. Manual check: open an .m4a (AAC) and
  confirm it plays with EQ/analyzer and no crash. Note: like #157, these decode
  up front into memory for now, fine for tracks, heavier for a long file; a seekable
  source is the #163 follow-up.

- #163 (seekable decode, part 1): crate::decode no longer decodes the whole file up
  front. open_file now returns a lazy, seekable SymphoniaSource that decodes packets
  on demand and implements try_seek via symphonia's seek, so .mp2/.m4a/etc no longer
  sit fully decoded in memory and can be scrubbed. Covered by a real decode+seek unit
  test against the committed MP3 fixture (open_file works for any symphonia format via
  the hint). Manual check: play an .m4a, drag the seek bar and confirm it scrubs and
  keeps playing. Remaining #163 work: route the everyday formats (mp3/flac/ogg/wav)
  through this path too so symphonia is the single local decoder.

- #160 (Audio CD, groundwork): added src/cdda.rs with the device-agnostic pieces, 
  the CD track model, CDDA timing (2352 bytes/sector, 75 sectors/sec, 44.1 kHz
  stereo), and the cdda://<device>/<track> playlist pseudo-path. All unit-tested. No
  device code yet: the per-OS TOC and raw-sector reads (DeviceIoControl on Windows,
  CDROM ioctls on Linux) and the playback source are the next step and need a real
  optical drive to develop and verify, which this dev box doesn't have. No GPL deps
  (libcdio is avoided). Nothing to verify in the app yet; the groundwork is covered
  by code.

- #160 (Audio CD, groundwork cont.): added cdda::pcm_s16le_to_f32, converts a raw
  CDDA sector (16-bit little-endian stereo PCM) to interleaved f32 for the rodio
  pipeline, so the per-OS device layer can share one playback source. Device-
  independent and unit-tested (normalization + samples-per-sector). Still no device
  reads yet (those need a real drive); nothing to verify in the app.

- #160 (Audio CD, playback contract): added the CdDrive trait (what the per-OS
  device layer must provide: read_toc + read_sector) and CdAudioSource, a rodio
  Source that plays a track by reading its sectors through any CdDrive and converting
  to f32, with seeking. Verified against a fake drive (full-track length, seek to a
  sector, seek-past-end clamp). This leaves only the real device I/O (DeviceIoControl
  / CDROM ioctls) per-OS, which needs a drive. Nothing to verify in the app yet.

## 2026-10-25 (decode unification + decisions)

- #163 part 2 (single decode path): local files now ALL decode through
  crate::decode::open_file (symphonia + hint), the lazy seekable source; rodio 0.20's
  Decoder is no longer used for files. Added symphonia wav/aiff/adpcm features so the
  single path covers WAV/AIFF too. This removes rodio's ISO-MP4 open panic class and
  gives uniform seeking. Unit-tested via the MP3 fixture (decode+seek) and a
  generated WAV. IMPORTANT human verification: this changes the path EVERY local file
  takes, so please play one of each common format (mp3, flac, ogg, wav, m4a) and
  confirm each plays, seeks and sounds right with no gaps or stutter, and that
  track-to-track advance is clean. (Note: WavPack .wv and Opus .opus have no symphonia
  decoder in 0.5, so those extensions won't play via this path, they didn't before
  either; can be dropped from the supported list or revisited.)
- #77: closed as-is per your call, stop-after-current / stop-with-fade stay in the
  Stop button's right-click menu; Preferences remains persistent-settings-only.

- #159 (MIDI, integration): added src/midi.rs, a MidiSource rodio Source that
  synthesizes .mid/.midi via rustysynth (pure Rust) and a SoundFont, through the same
  EQ/analyzer/balance pipeline. play_file routes mid/midi here; mid/midi added to
  SUPPORTED_EXTS; build.rs now also mirrors a soundfont/ folder next to the binary
  (like skins/). The SoundFont is discovered at soundfont/*.sf2; MIDI errors cleanly
  if none is present (caught by the skip guard). Compiles; the error path is
  unit-tested. NOT yet playable: it needs a bundled SoundFont (.sf2), see the
  decision below. Once one is in soundfont/, manual check: play a .mid and confirm it
  renders through the EQ/analyzer. Seeking is unsupported for MIDI (rustysynth has no
  seek). Open decision: which redistributable SoundFont to bundle (GeneralUser GS is
  free to distribute); committing a multi-MB .sf2 to the public repo is the remaining
  step.

- #159 (MIDI, complete): bundled the GeneralUser GS SoundFont (soundfont/, free to
  redistribute, ~32 MB) and verified end-to-end, a generated one-note MIDI renders
  audio through rustysynth + the SoundFont (unit-tested). MIDI now plays. Manual
  check: play one of the demo MIDIs placed in qa-music (Bond.mid, Dance.mid, Jump!.mid)
  and confirm it sounds right through the EQ/analyzer. Also: qa-music is now gitignored
  so local test media never gets committed. Note: I added MIDI samples to qa-music but
  couldn't source license-clean .mp2/.m4a samples via download, drop your own of
  those (and a flac/ogg) into qa-music to exercise #157/#164 and the #163 single path.

- #158 (tracker modules): .mod/.xm/.s3m/.it now play, via xmrs + xmrsplayer (pure
  Rust). This is better than the approved libopenmpt path, no native C dependency, so
  it builds everywhere with no DLL bundling. src/tracker.rs loads the module (xmrs) and
  renders it lazily (xmrsplayer) as a seekable rodio Source; the player borrows the
  module so a self_cell holds them together. Routed in play_file; mod/xm/s3m/it added
  to SUPPORTED_EXTS. Verified end-to-end rendering against the xmrs example module
  (note.xm, placed in qa-music). Manual check: drop a few real .mod/.xm/.s3m/.it files
  in qa-music and confirm they play and sound right through the EQ/analyzer, and that
  the track ends (auto-advances) rather than looping forever.

## 2026-10-26 (#40 system-tray icon, Windows side, needs manual + Linux QA)

- System-tray icon added (#40), on by default (Prefs field tray_enabled). The
  native tray runs on its own thread and reports clicks over a channel the app
  drains each frame (src/tray.rs). Windows uses Shell_NotifyIcon on a message-only
  window (hand-rolled FFI, no new crate); left-click toggles all LinAmp windows
  (hide to tray / restore), right-click opens a transport menu (Play/Pause, Stop,
  Previous, Next, Restore LinAmp, Exit LinAmp). Hiding to tray saves which of the
  eq/playlist/library windows were open and restores exactly those. The menu
  command ids map to actions in a pure, unit-tested helper; a disabled tray still
  returns a working (empty) receiver.
- Manual check (Windows): launch LinAmp and confirm the LinAmp icon appears in the
  notification area. Left-click it: the window (and any EQ/playlist/library windows)
  should disappear with no taskbar button, then a second left-click should bring
  back exactly those windows, focused. Right-click: each menu item should drive the
  matching transport action, Restore should bring the window back, Exit should quit
  cleanly. Confirm the icon disappears when the app exits (no stale ghost icon).
- Linux side implemented with ksni 0.3.7 (features = ["blocking"], pure-Rust zbus,
  no GTK or libdbus C dependency). LinAmp registers a StatusNotifierItem that ships
  its own ARGB pixmap (from assets/linamp_logo_32.png), so the icon shows without a
  matching icon theme. Left-click (activate) sends Toggle, the menu mirrors the
  Windows one. The service runs on ksni's own background thread; the Handle is held
  for the app's lifetime. This path could not be compiled here (Windows box, and
  cross-checking fails on the C deps rusqlite/alsa), so it needs a build + run on
  Pop!_OS. Manual check (Linux): confirm the icon appears in the tray, left-click
  hides/restores the windows, the right-click menu drives transport, Exit quits. If
  the desktop has no StatusNotifierItem host, spawn fails and LinAmp just runs
  without a tray (no crash), which is the intended fallback.
- Open question for review: minimize-to-tray is currently only the tray left-click.
  The classic Minimize button still minimizes to the taskbar as before. Decide
  whether the Close/Minimize buttons should hide to tray too (and whether that
  should be a preference).

## 2026-10-26 (#40 tray reworked into two window modes, needs manual + Linux QA)

- The tray is now a two-mode window setting (Preferences > Display > System tray),
  and it supersedes the "on by default" note above. Default is Normal mode.
  - Normal mode: a taskbar button, no tray icon. Minimize iconifies to the taskbar.
    The close (X) button and an OS close (Alt+F4 / taskbar Close) exit. Main-menu
    Exit exits.
  - System tray mode: a tray icon, no taskbar button. Minimize and the close (X)
    button hide to the tray (saving/restoring exactly the windows that were open).
    An OS close (Alt+F4) also hides. The only ways to quit are main-menu Exit and
    the tray's Exit.
- Applied at launch, not live: egui can only set the taskbar-button visibility at
  window creation, so the mode is read once at startup (main.rs sets with_taskbar,
  and the app snapshots tray_mode). Changing the pref shows "Takes effect when you
  restart LinAmp." A genuine Exit sets force_quit so it quits even in tray mode.
- Safety: tray_mode is only active when the tray actually started. If the tray
  backend fails (for example a Linux desktop with no StatusNotifierItem host), the
  app behaves as Normal so a close still exits rather than hiding to nothing.
- Manual check (Windows): set System tray mode, restart. Confirm there is no
  taskbar button, the X and Minimize both hide to the tray, left-click restores,
  the tray menu's Exit and the main menu's Exit both quit, and switching back to
  Normal (after a restart) brings the taskbar button back with no tray icon.
- Linux QA (Pop!_OS): confirm the same on the ksni tray. Note: stock GNOME shows
  StatusNotifierItems only with the AppIndicator/KStatusNotifier extension. Without
  a tray host the icon won't appear; the app stays in Normal-like behavior (no
  stranded window), but the taskbar button was still suppressed at launch, so on
  such a desktop prefer Normal mode. Mint Cinnamon has a tray by default.

## 2026-10-26 (build hygiene: mirror music/ next to the exe)

- build.rs now mirrors music/ into target/<profile>/music (like skins/ and
  soundfont/), and first_run_music_dir prefers the copy next to the executable.
  This keeps the pristine repo music/ folder from being written to when a dev or
  release build loads the demo track (the demo loader can rewrite its ID3 tags).
  The installer already bundles music/, so shipped builds are unaffected. No
  behavior change for users; purely protects the source tree during testing.

## 2026-10-26 (#40 window mode now applies live, no restart)

- Supersedes the "applied at launch, not live" note above. Switching the mode in
  Preferences > Display > System tray now takes effect immediately: the tray icon
  appears or disappears, the taskbar button flips, and the close/minimize behaviour
  changes, with no restart. The Preferences hint no longer mentions restarting.
- Mechanism: set_window_mode re-spawns the tray to match and calls
  platform_win::set_taskbar_button. On Windows that drives ITaskbarList
  (AddTab / DeleteTab) on the main window found by title "LinAmp", the same COM
  interface winit uses for its taskbar option, so a live toggle matches the startup
  state cleanly. The window is still created with the saved mode at launch
  (main.rs with_taskbar), so first paint is already correct.
- Linux: the tray icon and behaviour toggle live too. The taskbar-button toggle is
  a best-effort X11 _NET_WM_STATE_SKIP_TASKBAR message (reused from the child-window
  grouping code); it will not work under Wayland or on a WM that ignores the hint,
  where the taskbar aspect then effectively needs a relaunch. Needs Pop!_OS QA.
- Manual check (Windows): with the window shown, toggle to System tray and confirm
  the taskbar button disappears and the tray icon appears at once; toggle back and
  the taskbar button returns. Then in tray mode, the close (X) hides to tray without
  a restart. Leaving tray mode while hidden should bring the window back.

## 2026-10-26 (#40 fixes: tray freeze on hide, and tray-menu highlight)

- Freeze fix: hiding to the tray now minimizes the window instead of hiding it
  with Visible(false). A window hidden with Visible(false) stops receiving
  WM_PAINT on Windows, so eframe never runs update() again and the tray's queued
  clicks (Restore, Play/Pause, Exit) were never drained, the window froze. winit's
  request_redraw uses RedrawWindow(RDW_INTERNALPAINT), which still posts WM_PAINT
  to a minimized window (it keeps WS_VISIBLE) but not a hidden one, and eframe
  keeps running update() while minimized, so the tray stays responsive. In tray
  mode the taskbar button is already removed, so a minimized window shows nothing.
- Tray-menu highlight fix: the tray's helper window was message-only (HWND_MESSAGE),
  which can never be the foreground window, so SetForegroundWindow before
  TrackPopupMenu failed and the menu never tracked the mouse (no hover highlight,
  no click-away dismiss). It's now a normal top-level window, created hidden and
  never shown (WS_EX_TOOLWINDOW keeps it out of the taskbar and Alt+Tab), so the
  menu gets standard hover highlighting and dismissal.
- Manual check (Windows): in tray mode, minimize or close to the tray, then
  left-click the tray icon to restore, and use the right-click menu's Play/Pause
  and Exit. All should work now, and the menu items should highlight on hover.

## 2026-10-26 (#40 follow-up: taskbar button not removed, and still no menu highlight)

- Root cause of both: when the tray helper window became a normal top-level
  window, it was titled "LinAmp", the same as the main player window. So
  FindWindowW("LinAmp") used by the live taskbar toggle (and window grouping)
  could resolve to the helper window instead of the player, and DeleteTab then
  ran on the wrong window, leaving the real taskbar button in place (bar->tray
  looked broken; tray->bar only looked fine because the button was never actually
  removed). Fixed by titling the helper window "LinAmp Tray".
- Menu highlight: a tray popup menu only highlights and dismisses when its owner
  window is the foreground window, and SetForegroundWindow won't take on a window
  that has never been shown. The helper window is now shown with SW_SHOWNA (zero
  size, off-screen, WS_EX_TOOLWINDOW), so it draws nothing but can take the
  foreground, and the menu gets the standard hover highlight.
- Manual check (Windows): switch Normal/System tray in Preferences and confirm the
  taskbar button appears/disappears both ways; right-click the tray icon and
  confirm the menu items highlight as the mouse moves over them.

## 2026-10-26 (#40 tray menu: skin-themed and owner-drawn, with working highlight)

- The native tray menu is now owner-drawn in the current skin's colours, from the
  same source as LinAmp's egui menus: theme.rs menu_palette gives (bg, text,
  hover_bg, hover_text, border), where bg/text/border match window_fill /
  override_text_color / window_stroke, and the hovered row uses the skin's
  selection colour. The app pushes this palette to the tray whenever the skin
  changes (apply_theme sites) and when the tray respawns, via Tray::set_menu_colors
  (shared atomics read by the tray thread). Off Windows it's a no-op (the Linux
  ksni menu follows the desktop theme).
- Mechanism (Windows): MF_OWNERDRAW items + SetMenuInfo (MNS_NOCHECK, skin bg
  brush); WM_MEASUREITEM sizes rows from the label, WM_DRAWITEM paints bg/hover,
  text and a thin themed border down the sides and across the top/bottom. Separators
  are disabled owner-draw rows drawn as a line.
- The hover highlight now works because the menu is forced to the foreground with
  AttachThreadInput before TrackPopupMenu (SetForegroundWindow alone is blocked
  from a background thread by the foreground lock, which is why hover did nothing
  before). Owner-draw then highlights the hovered row in the skin colours.
- Manual check (Windows): right-click the tray icon; the menu should be dark/skin-
  coloured with a themed border, and moving the mouse over items should highlight
  them in the skin's selection colour. Load a different skin and confirm the menu
  colours follow it.

## 2026-10-26 (#40 fixes: layout forgotten on tray exit, and tray-menu font)

- Window layout was lost when exiting from the tray: hide_to_tray closes the
  EQ/playlist/library windows (sets their visible flags false) and minimizes the
  main one, and persist_session_if_changed kept saving that hidden state, so the
  exit wrote "only the main window" as the layout. Now persistence is skipped while
  hidden_to_tray (as it already was during shutdown), so the last saved layout is
  the real on-screen one, and exiting from the tray then reopening restores the
  windows and positions that were open.
- Tray-menu font: the owner-drawn menu now uses LinAmp's bundled UI font (Noto
  Sans, the same assets/fonts/NotoSans-Regular.ttf the egui menus use), loaded via
  AddFontMemResourceEx and sized to the system DPI, so the menu matches the app's
  own menus instead of the system font. Falls back to the system menu font if the
  font can't be created.
- Manual check (Windows): open the EQ and playlist, position the windows, hide to
  the tray, then Exit from the tray menu. Reopen and confirm the same windows come
  back in the same places. Also confirm the tray menu's text now matches LinAmp's
  own menus.

## 2026-10-26 (#40 Linux parity checklist, for work on the Linux machine)

The tray / window-mode work is verified on Windows. None of the Linux side has
been compiled or run (developed on Windows; cross-check fails on the C deps). The
two other #40 parts are split out: media keys (#165) and file associations (#166).
To bring Linux to parity, verify on Pop!_OS (and Mint):
- Compiles: cargo build with the ksni 0.3.7 dep (blocking feature, zbus 5).
- Tray icon shows: stock GNOME (Pop!_OS) needs the AppIndicator / KStatusNotifier
  extension for StatusNotifierItems; Mint Cinnamon has a tray by default. With no
  tray host, tray_mode stays off and the window behaves normally (no stranding).
- Left-click toggles the window; the right-click menu drives transport; Exit quits.
- RISK, hide-to-tray freeze: the Windows fix minimizes instead of hiding, relying
  on winit posting WM_PAINT to a minimized window (RDW_INTERNALPAINT). That is
  Windows-specific; on X11/Wayland a minimized window may not keep repainting, so
  eframe could stop running update() and the tray would go unresponsive like it
  did on Windows before the fix. Test minimize-to-tray then a tray click; if it
  freezes, the Linux path likely needs a different hide (e.g. keep the window
  mapped but off-screen) so the event loop keeps running.
- Taskbar button: Windows toggles it live via ITaskbarList. Linux is a best-effort
  X11 _NET_WM_STATE_SKIP_TASKBAR message (no effect under Wayland or a WM that
  ignores it), so the no-taskbar-button aspect may need a relaunch or more work.
- Tray menu theming: the skin-coloured owner-drawn menu is Windows-only. The Linux
  ksni menu follows the desktop theme (SNI menus are host-drawn), which is expected.
