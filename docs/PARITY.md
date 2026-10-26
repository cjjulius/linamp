# Winamp 2.666 parity

LinAmp's classic-mode target is Winamp 2.666 specifically. This map tracks that
version's feature set: what's done, what genuinely remains and what belongs to
later Winamp lines (and so is out of scope until 2.666 is complete).

For how each change was verified, see [REVIEW-NOTES.md](REVIEW-NOTES.md); for the
keyboard and mouse reference, see [SHORTCUTS.md](SHORTCUTS.md).

## Finish line

As of feat/winamp-parity-3, LinAmp meets the Winamp 2.666 functional feature set.
The only in-scope item not fully landed on every platform is the audio-CD device
layer (#160): the Linux reads are implemented and verified on real hardware, the
Windows device layer still needs a real optical drive to write and verify and so is
being built on the Windows box. A few completed changes are also waiting on a quick
visual sign-off (see below), those are confirmations, not missing features.

Everything else still open in the tracker is either later-line work (Winamp 3/5+)
or an internal refactor, listed under Deferred.

## Core features, present and verified

These classic behaviours are implemented and were confirmed against the 2.666
feature set:

- Transport: play, pause, stop (plus stop-after-current and stop-with-fade),
  next, previous, seeking.
- Playlist: add files/folders, remove, crop, remove-missing, reverse, sort
  (title / artist / album / duration / filename / path), per-track file info,
  jump-to-file (J) and jump-to-time (Ctrl+J), the elapsed/total time readout and
  m3u / m3u8 / pls load and save.
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
- Bookmarks (Alt+I) with a recall list in the Options menu and a Media Library
  Bookmarks view.
- Open location (Ctrl+L) and an Options-menu entry: stream an HTTP(S) URL
  (internet radio or a remote file), decoded on a background thread, with the live
  ICY stream title shown in the scrolling display.
- Tag reading for every format lofty supports; ID3v1 and ID3v2 text-frame editing
  for MP3 (title, artist, album, year, track, genre, comment, composer, original
  artist, copyright, URL, encoded-by), the stock 2.666 File Info set and more.

### Audio formats decoded

All local decoding now flows through one symphonia path with a format hint
(`src/decode.rs`), which also powers streaming; there is no second decoder to
disagree with it:

- MPEG audio: Layer III (.mp3) and Layer I/II (.mp1/.mp2/.mpa) (#157).
- The ISO-MP4 family: .m4a/.mp4/.m4b, including the former open-panic (#164).
- FLAC, Ogg Vorbis, WAV and AIFF (#163).
- MIDI (.mid/.midi), synthesized by rustysynth with a bundled GeneralUser GS
  SoundFont (#159).
- Tracker modules (.mod/.xm/.s3m/.it), rendered by the pure-Rust xmrs/xmrsplayer
  (#158, no native C dependency, so the build stays portable).

## Implemented, awaiting a visual sign-off

Done in code and unit-tested where possible; each needs a quick in-app check to
close, not further work:

- #154, classic menu sizing on repeat right-clicks: each popup now opens its own
  fresh viewport so it always hugs its items. Confirm the Options, playlist and
  EQ-presets menus stay tight on repeated opens (no empty band).
- Cross-format playback feel: open an .mp2, an .m4a, a FLAC/OGG/WAV, a .mid and a
  tracker module and confirm each plays with the EQ and analyzer responding. The
  routing and decode are unit-tested (including render tests for MIDI and a
  tracker), but audio quality is a human call.

## Genuinely remaining 2.666 work

- #160, Audio CD (CDDA). The device-independent half is complete and tested, the
  CD model, timing, the `cdda://` pseudo-path, PCM conversion, the `CdAudioSource`
  rodio source and the `CdDrive` contract (read_toc / read_sector). The Linux device
  layer is now implemented and verified on real hardware: raw `CDROMREADTOCHDR` /
  `CDROMREADTOCENTRY` / `CDROMREADAUDIO` ioctls on `/dev/sr0`, read on a background
  thread into a deep prefetch buffer so the drive streams without the stutter-respin
  (see REVIEW-NOTES). Opened from Options to Open Audio CD, Linux only. What remains
  is the Windows device layer (DeviceIoControl TOC and raw sector reads), which needs
  a real drive and disc to write and verify, that work is on the Windows box.

## Resolved on feat/winamp-parity-3

- #149, EQ slider thumb sat a pixel off the groove on crisp skins (MacOSXAqua,
  Osmosis): the 11px thumb was inset 2px into its 14px band cell; every skin draws
  its groove centred at local x ~6, so the inset is now 1 (regression-tested).
- #163, unified local decoding on direct symphonia with a hint; rodio's file
  Decoder is gone, so the m4a/mp2 decoder-mismatch class of bugs is closed.
- #157 (MPEG Layer I/II), #164 (ISO-MP4 open-panic), #159 (MIDI), #158 (trackers),
  see "Audio formats decoded".
- #77, shortcut-only stop actions: closed as-is, stop-after-current and
  stop-with-fade live in the Stop button's right-click menu (#130); Preferences
  stays limited to persistent settings (maintainer call, decision 4a).
- #156 (resize clamp, user-verified on Linux), #97 (accented/Vietnamese text
  ASCII-folds to readable base letters; full non-Latin scripts in the classic
  bitmap path stay out of scope by design), #144 (Ctrl+A always-on-top completes
  the shortcut set).
- Shipped in 0.4.9: #15, #45, #17, #146. Also closed: #76 (vetted skins bundled)
  and #49 (PipeWire verified on Pop!_OS, Mint assumed working).

## Deferred, beyond the 2.666 functional-parity bar

These are tracked but out of scope until 2.666 is signed off; several are later
Winamp lines rather than 2.x behaviour at all.

- Maintainer call, out of scope for this run: #40, system integration (global
  media keys, tray icon, file associations); #43, crossfade (a 2.x output plugin,
  not core).
- Beyond 2.666 (post-2.x era), not stock 2.666 behaviour:
  - #89, broaden tag editing. The 2.666-relevant part (MP3 ID3v1/v2 text frames)
    is done; the remainder, writing FLAC/OGG/M4A tags, album art and the
    ID3v2.3-vs-2.4 and related Preferences, is modern-era work (stock 2.666 edited
    MP3 tags only). Hold until the parity bar is signed off.
  - #108, drop shadows on the classic context menus (a post-2.x window-compositing
    nicety).
- 3.x: #20 modern `.wal` skin engine, #78 Media Library buildout, #139 EQ graph
  drag-to-draw, #150/#151/#152 Media Library saved playlists / genre-year / drag to
  editor.
- 5.x: #42 gapless playback, #44 ReplayGain, #153 Media Library album-art pane.
- Internal (no user-facing parity effect): #111 collapse per-window state into
  arrays, #116 consolidate context menus onto one system, #69 native-Wayland single
  composite window.
