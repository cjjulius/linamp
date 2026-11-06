# LinAmp, Feature Roadmap

A Winamp-inspired audio player for Linux and Windows, written in Rust
(`eframe`/`egui` for UI, `rodio`/`symphonia` for audio, `lofty` for tags).

Legend: everything listed is done unless it is flagged Planned or Partial.

> Planned work and known shortfalls are tracked as GitHub issues in
> [cjjulius/linamp](https://github.com/cjjulius/linamp/issues), that's the
> backlog of record. The Planned: items below are kept only as a quick overview.

## Core playback
- Decode MP3, MPEG Layer I/II (`.mp1`/`.mp2`/`.mpa`), FLAC, OGG/Vorbis, Opus, WAV,
  AIFF, CAF, AAC/M4A (`.m4a`/`.mp4`/`.m4b`/`.aac`), Monkey's Audio (`.ape`), WavPack
  (`.wv`), True Audio (`.tta`), Musepack (`.mpc`/`.mp+`/`.mpp`, SV7 and SV8),
  AC-3 (`.ac3`/`.eac3`, stereo and 5.1 downmix), the audio track of video
  containers (`.mp4`/`.m4v`/`.3gp`/`.mov`/`.mkv`/`.webm`/`.ogv`; MPEG-TS
  `.ts`/`.m2ts`/`.mts` via `mpeg2ts-reader`; AVI `.avi` via `filmcraft-avi` for
  MP3/MP2/PCM/AC-3; FLV `.flv` via `scuffle-flv` for MP3/AAC; MPEG program stream
  `.mpg`/`.mpeg`/`.vob` via `mpeg-ps` for MPEG-audio/AC-3), tracker modules
  (`.mod`/`.xm`/`.s3m`/`.it`) and MIDI (`.mid`/`.midi`, via a bundled SoundFont)
- Windows Media Audio (`.wma`) in the optional FFmpeg build (`--features ffmpeg`),
  decoded via `crate::ffdec` over the system FFmpeg libraries (LGPL). No permissive
  pure-Rust WMA decoder exists, so the default build stays pure Rust and omits it;
  the opt-in build links libav* and plays WMA v1/v2/Pro/Lossless downmixed to stereo
- WavPack is decoded by the pure-Rust `wavicle` crate (symphonia 0.6's wavpack
  feature is a non-functional stub); lossless 16- and 24-bit integer, mono/stereo,
  any sample rate. Float, 8-bit, hybrid/lossy and multichannel `.wv` are refused
  cleanly (skipped, never played as noise)
- Musepack is decoded by the pure-Rust `oxideav-musepack` crate (symphonia has no
  Musepack decoder); both stream generations, SV7 (`MP+`) and SV8 (`MPCK`), mono and
  stereo. Vetted within 1 LSB of the ffmpeg `mpc8` oracle on real music before adding.
  The rare more-than-two-channel streams are refused cleanly
- Audio CD playback (#160), verified on Windows and Linux hardware
- WMA (#162): no permissive pure-Rust decoder exists, so it is supported only in the
  optional FFmpeg build (see above); the default pure-Rust build does not play it.
  AC-3 is raw `.ac3`/`.eac3` only (not yet inside mkv/mp4 containers)
- Play / Pause / Stop / Next / Previous
- Seek bar (scrub) with position + total time
- Time display toggles elapsed / remaining (−mm:ss), Ctrl+T or click the time
- Volume control, level and balance remembered across runs
- Repeat-all / Repeat-one / Shuffle (shuffle never replays the current track)
- Auto-advance on track end
- Stereo balance (the balance slider pans the output, with centre snap)
- Stop after current track (Ctrl+V) and stop with fade-out (Shift+V)
- Open location / internet-radio streaming (Ctrl+L or the Options menu),
  HTTP/HTTPS with the live ICY song title
- Skips tracks that can't be opened or decoded instead of stalling
- Crossfade between tracks (Preferences > Playback, on/off with a 1-12s length)
- Planned: Gapless playback (#42), ReplayGain (#44)
- Planned: Playback speed / pitch

## Audio conversion and CD ripping
- Convert any library or playlist track to MP3, Ogg Vorbis, Opus, FLAC, WAV or AIFF
- Rip audio CD tracks, the whole disc or a selection, into a folder named after the
  album, named `[Artist] - [Track]` and tagged from the online lookup (ID3v1+ID3v2,
  or Vorbis comments for Ogg/Opus)
- A background queue runs jobs one at a time with per-file progress; add, remove or
  cancel while it runs, shown in the Media Library's Converting pane; finished files
  are added to the library automatically
- Format, bitrate, sample rate and output folder set in Preferences > Conversion

## Playlist
- Add files, add folder (recursive), remove, clear
- Double-click / right-click to play; current-track highlight
- Per-track duration column
- Save/Load M3U, M3U8 and PLS
- Drag-and-drop files/folders/playlists from the OS onto the window (dropped on
  the playlist, they insert at the row under the cursor; elsewhere they append)
- Drag-and-drop reorder (grip handle)
- Sort by title / artist / album / duration (+ manual order)
- Search box to filter the playlist
- Multi-selection (Winamp-style): click selects one, Ctrl+click toggles,
  Shift+click / Shift+arrows select a range, Home/End jump; Invert/Crop and
  Remove operate on the selection
- Editor extras (Rem/Misc menus): remove missing files, randomize, reverse
- Robust playlist loading, M3U/PLS decoded leniently (tolerates a BOM or a
  stray non-UTF-8 byte), PLS ordered by `File<N>` index and Windows-authored
  relative paths (backslashes) resolve on Linux too
- Jump-to-file (J) and jump-to-time (Ctrl+J)

## Metadata / Library
- Read tags for every supported format (ID3v1/ID3v2, Vorbis comments, MP4 atoms,
  APEv2)
- Tag editor (Alt+3 / options menu, Edit file tags): view and edit the full field
  set (track / disc / title / artist / album / year / genre / comment / composer /
  original artist / copyright / URL / encoded-by / BPM / compilation), saved back to
  the file. MP3 shows the ID3v1 and ID3v2 tags as a column pair (with Copy to/from
  ID3v1); other formats edit their single native tag. Writes to MP3, FLAC, Ogg, Opus,
  M4A/MP4 and APE, each field in the key other players read (native MP4 cpil/tmpo,
  standard Vorbis/APE keys)
- Mass editor: select several rows in the playlist or library ("Mass Edit Tags",
  Update All). It pre-fills each field with the most common value across the
  selection, outlines the fields you change, and writes only those to every selected
  file, leaving untouched fields alone
- Audio properties (kbps / kHz / channels)
- SQLite media library (scan folders, search, add to playlist)
  - stored at `%LOCALAPPDATA%/LinAmp/library.db` (Win) / `~/.local/share/LinAmp/` (Linux)
  - reachable in the classic UI via the options menu's Media Library item (opens
  in its own window); resizable from the bottom-right grip, between its default size
  and the monitor size, and double-click the title bar to maximize / restore. The
  navigation pane keeps its width and the track columns take the extra room
  - tag edits made in the editor update the library row immediately (no rescan);
  search treats `_` and `%` as literal text, not SQL wildcards
- Bookmarks, Alt+I bookmarks the current track; Media Library, Bookmarks
  view lists them (play, search filter, remove)
- Planned: smart views / playlists

## UI / UX (Winamp feel)
- Dark skin, LCD-style display, scrolling marquee title
- LinAmp logo (green lightning bolt on a disc) as the app/window + taskbar
  icon; vector source kept in `assets/linamp_logo.svg` for reuse
- Main player + Equalizer + Playlist as separate windows that dock & snap
- Window-shade mode (double-click a title bar to collapse)
- Keyboard shortcuts matching Winamp, see the [README](README.md#keyboard-shortcuts)
- Preferences window (Ctrl+P): tag-load mode and 1×/2× sizing, persisted;
  shows the running `LinAmp <version>` (with build hash + date). Resizable from the
  bottom-right grip, between its default size and twice that in each dimension; the
  navigation pane keeps its width
- Accented / Vietnamese track text is ASCII-folded in the classic bitmap font
  (example: "Trường Sơn" becomes "Truong Son") instead of showing blank gaps
- Always on top (options menu + Preferences)
- Single-instance, launching LinAmp a second time just exits instead of opening a
  rival set of windows
- Minibrowser, "Open in browser" opens the system browser (xdg-open on
  Linux); the SHOUTcast directory pane is still planned (#146)

## Theming / Skins
- Pixel-perfect classic skin renderer, composites real `.wsz` sprite
  sheets (`MAIN/CBUTTONS/TITLEBAR/SHUFREP/POSBAR/VOLUME/BALANCE/MONOSTER/
  PLAYPAUS/NUMBERS/TEXT`) into the exact 275×116 main window
- Renders skin artwork (background picture) and custom buttons with
  pressed/selected states, custom LCD digits and bitmap-font title marquee
- Borderless window with skin titlebar (drag to move, close/minimize)
- Working controls: prev/play/pause/stop/next/eject, shuffle/repeat,
  seek bar, volume + balance sliders; EQ/PL buttons bridge to the modern view
- Classic skin is the whole UI (borderless). The egui "modern" view is
  retained only as a fallback when no skin is present.
- Title bars rebranded to LinAmp on a readable dark backing plate (main / EQ /
  playlist), legible over any skin.
- Window-shade mode, double-click a title bar to collapse that window to
  its 14px shade bar (main shows title + time); double-click to restore; the
  window stack resizes to fit. Close/minimize work on shade bars too.
- Load real Winamp `.wsz` skins; also derives egui/modern-view colour theme
  from `PLEDIT.TXT` / `VISCOLOR.TXT` / `MAIN.BMP`
- Skin picker listing the bundled `./skins`, "Load .wsz…", reset to default
- Headless tools: `--check-skin` (colours) and `--render-skin in.wsz out.png`
  (classic-window PNG preview)
- Test skins bundled in `skins/` incl. artwork skins (Akira, Battle Angel
  Alita, Iria, Evangelion/rei) + base-2.91, Green-Dimension, TopazAmp, MacOSXAqua
- Classic Equalizer window (`EQMAIN.BMP`), on/auto/presets buttons,
  preamp + 10 band sliders (drag), graph area; docks under the main window
- Classic Playlist window (`PLEDIT.BMP`), tiled frame, track list in the
  skin's pledit colours, current-track highlight, click-to-play, wheel scroll
- Upper-left options menu (and right-click): open file/folder, Media
  Library, toggle
  EQ/Playlist, load skin, modern view, exit
- Title bars rebranded to LinAmp (main / equalizer / playlist)
- `--render-all` headless preview (main + EQ + playlist stacked)
- Playlist bottom bar wired: ADD (file/folder), REM (remove selected / crop /
  remove all), SEL (all/none/invert), MISC (sort title/artist/album/duration),
  LIST (new/save/load), click-to-select, double-click-to-play, wheel scroll
- Live spectrum / oscilloscope in the classic viz area (click to cycle)
- Planned: Window-shade polish
- Planned: True "modern" (Winamp 3/5 `.wal` freeform) skin engine, large separate phase

## Equalizer
- 10-band + preamp UI, on/off, reset
- Per-band biquad DSP (RBJ peaking filters), the sliders and preamp shape
  the sound live; applied as a rodio `Source` ahead of the visualizer
- Built-in presets (Rock, Pop, Jazz, Classical, Dance, Bass/Treble boost, …)
  via the classic Presets button and the modern view's dropdown
- EQ preset library, import/export Winamp `.eqf` plus named save/load
- Auto gain, sets the preamp to offset the loudest boosted band so the
  EQ is less likely to clip
- EQ state (on/auto/preamp/bands) remembered across runs

## Visualization
- Live spectrum analyzer (FFT) and oscilloscope (click to cycle), skin-coloured
- Album art display from embedded tags (with placeholder)

## System integration
- Open files/folders via CLI args and the OS "Open with" menu, backed by per-user
  file associations (Windows registry under HKCU; Linux `.desktop` + xdg-mime),
  never seizing the default handler; single-instance forwarding opens into the
  running window
- Global media keys (Play/Pause, Stop, Previous, Next) that work while LinAmp is
  unfocused, minimized or trayed (Windows media hotkeys; MPRIS over D-Bus on Linux)
- System-tray mode: close minimizes to a tray icon with a menu, toggled in
  Preferences

## Audio / devices
- Uses the system default output device; follows it live (example: headphones
  plugged in mid-session) by rebuilding the stream and resuming in place
- Seek bar jumps within the track (drag the main position slider)

## Windows (classic)
- Separate OS windows for main / EQ / playlist (detach & move freely),
  shown as a single taskbar entry (one program)
- Movement via the OS window-drag (smooth); magnetic snap to each other and to
  screen edges on release; dragging the main window moves the docked group
- Playlist resize via the bottom-right grip, both height (29px steps) and
  width (25px steps), snapping to Winamp steps; the frame tiles to any width
- Remembers the last skin and the playlist (tracks + current + playlist-window
  size) across runs; restored on launch unless files are passed on the CLI
- Planned: Finer group-docking; remember full window layout between runs

## Testing
- `cargo test` runs the suite (playlist ops, M3U/PLS parse, skin colours,
  classic window geometry/hit-testing, preferences, EQ/balance DSP math,
  visualizer). Pure-logic and cross-platform (Windows + Linux).
- CI (GitHub Actions): every push/PR builds and tests on `ubuntu-latest`
  and `windows-latest`; tags (`v*`) and manual runs also upload release
  binaries for both platforms. Linux needs `libasound2-dev` (installed in CI).

## Planned / Backlog
Big-ticket items; the full, current backlog is in
[GitHub Issues](https://github.com/cjjulius/linamp/issues).
- Planned: Gapless playback (#42), ReplayGain (#44)
- Planned: True "modern" (Winamp 3/5 `.wal` freeform) skin engine (#20)

## Keyboard shortcuts
The classic Winamp 2.x shortcut set is complete; see
[docs/SHORTCUTS.md](docs/SHORTCUTS.md) for the full reference (the README has a
quick summary).
