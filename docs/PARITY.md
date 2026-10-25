# Winamp 2.666 parity

LinAmp's classic-mode target is Winamp 2.666 specifically. This map tracks
that version's feature set: what's done, what's open and what's deferred to
later Winamp lines. The goal is simple, complete all 2.x issues and resolve the
parity bugs, and LinAmp matches 2.666.

For how each change was verified, see [REVIEW-NOTES.md](REVIEW-NOTES.md); for the
keyboard and mouse reference, see [SHORTCUTS.md](SHORTCUTS.md).

## Core features, present and verified

These classic behaviours are implemented and were confirmed against the 2.666
feature set:

- Transport: play, pause, stop (plus stop-after-current and stop-with-fade),
  next, previous, seeking.
- Playlist: add files/folders, remove, crop, remove-missing, reverse, sort
  (title / artist / album / duration / filename / path), per-track file info,
  the elapsed/total time readout and m3u / m3u8 / pls load and save.
- Shuffle (plays through the whole list before repeating) and repeat.
- Equalizer: 10 bands plus preamp, on/off, Auto, built-in presets, a custom
  named-preset library (save / load / delete) and Winamp .EQF import/export.
- Classic `.wsz` skins, including skinned cursors.
- Windows: main / equalizer / playlist, window-shade (collapsed) mode with a
  mini transport and position slider, title-bar double-click to shade, double
  size and always-on-top. Window positions, visibility and shade state persist
  across runs.
- Visualization: spectrum analyzer, oscilloscope and off, cycled by click.
- Clutterbar (O / A / I / D / V) and the elapsed/remaining time toggle.
- Volume and stereo balance.
- Drag-and-drop of files, folders and playlists from the OS.
- Tag reading for every format lofty supports; ID3v1 and ID3v2 text-frame
  editing (title, artist, album, year, track, genre, comment, composer,
  original artist, copyright, URL, encoded-by).
- Media Library window.

## Implemented, awaiting a visual sign-off

Done in code and unit-tested where possible; each needs a quick in-app check to
close:

- #17, window-shade polish: the mini position/seek slider on the collapsed
  main bar (always-on-top and the mini transport were already done).
- #45, bookmarks (Alt+I) with a recall list in the Options menu.
- #144 (partial), numeric-keypad transport (now opt-in under Optional
  Features), Ctrl+Z (start of list) and Ctrl+R (reverse). The numpad-only
  platform path is dropped (per your call).
- #15, Open location (Ctrl+L): stream an HTTP(S) URL (internet radio or a
  remote file). Decoded on a background thread; needs a live-stream test.
- #146, Minibrowser: a themed window (Options > Show Minibrowser) that shows a
  persisted, user-editable address (default = the repo) and opens it in the system
  browser, a nod to the original, not a real web view. Opt-in under Preferences >
  Optional Features.

Preferences > Optional Features gathers the obsolete/incomplete "full parity" extras
(the Minibrowser and numeric-keypad transport), off by default with an enable-at-your-
own-risk note.

## Remaining 2.666 functional-parity work (this run's closeout list)

This is the finish line: when these are done, LinAmp is at 2.666 functional
parity. Scope: #77 is in; #40 and #43 are out (see Deferred).

Audio format compatibility (current focus, per the maintainer):

- #158, tracker modules (.mod/.xm/.s3m/.it). Needs libopenmpt, a native C
  dependency in the build/bundle; awaiting the OK to add it.
- #159, MIDI (.mid/.midi). Needs rustysynth plus a bundled multi-MB SoundFont;
  awaiting the OK.
- #160, Audio CD (CDDA). Niche and hardware-dependent (raw device reads); low
  priority.

Other remaining:

- #89, broaden tag editing. ID3v2 text frames are done; remaining is non-MP3
  writing (FLAC/OGG/M4A), album art, ID3v2.3-vs-2.4 and related Preferences.
- #77, surface all persistent settings in the Preferences window.
- #149, EQ ON/AUTO/PRESETS buttons misaligned on some skins (MacOSXAqua,
  Osmosis); needs a visual repro vs real Winamp.
- #154 (reopened), classic menus read too long on re-right-click; a fresh-viewport
  fix is pushed (each menu opens its own window), awaiting user confirmation.

Resolved on feat/winamp-parity-3: #157 (MPEG Layer I/II .mp1/.mp2/.mpa via a hinted
symphonia decode), #156 (resize clamp, user-verified on Linux), #97
(accented/Vietnamese text now ASCII-folds to readable base letters; full non-Latin
scripts in the classic bitmap path stay out of scope by design), and #144 (Ctrl+A
always-on-top completes the shortcut set; Alt+W needs a tray (#40), Ctrl+E EasyMove
and Ctrl+keypad-0 are out of scope).
Shipped in 0.4.9: #15, #45, #17, #146. Also closed: #76 (vetted skins bundled)
and #49 (PipeWire verified on Pop!_OS, Mint assumed working).

## Deferred, beyond the 2.666 functional-parity bar

- Out of scope for this run (maintainer call): #40, system integration (global
  media keys, tray icon, file associations); #43, crossfade (a 2.x output plugin,
  not core).
- 3.x: #20 modern `.wal` skin engine, #78 Media Library buildout, #139 EQ graph
  drag-to-draw.
- 5.x: #42 gapless playback, #44 ReplayGain.
