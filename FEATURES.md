# LinAmp — Feature Roadmap

A Winamp-inspired audio player for Linux and Windows, written in Rust
(`eframe`/`egui` for UI, `rodio`/`symphonia` for audio, `lofty` for tags).

Legend: ✅ done - 🔜 planned - 🧪 partial

> Planned work and known shortfalls are tracked as **GitHub issues** in
> [cjjulius/linamp](https://github.com/cjjulius/linamp/issues) — that's the
> backlog of record. The 🔜 items below are kept only as a quick overview.

## Core playback
- ✅ Decode MP3, FLAC, OGG/Vorbis, WAV, AAC/M4A, Opus, WavPack, AIFF
- 🔜 More classic formats, tracked as issues: MPEG Layer I/II `.mp1`/`.mp2`
  (#157), tracker modules `.mod`/`.xm`/`.s3m`/`.it` (#158), MIDI (#159), audio
  CD (#160). AC-3 (#161) and WMA (#162) are **won't-fix** — restrictive /
  proprietary licensing (and AC-3 was never a native classic-Winamp format)
- ✅ Play / Pause / Stop / Next / Previous
- ✅ Seek bar (scrub) with position + total time
- ✅ Time display toggles elapsed ↔ remaining (−mm:ss) — Ctrl+T or click the time
- ✅ Volume control — level and balance remembered across runs
- ✅ Repeat-all / Repeat-one / Shuffle (shuffle never replays the current track)
- ✅ Auto-advance on track end
- ✅ Stereo balance (the balance slider pans the output, with centre snap)
- ✅ Stop after current track (Ctrl+V) and stop with fade-out (Shift+V)
- ✅ Open location / internet-radio streaming (Ctrl+L or the Options menu) —
  HTTP/HTTPS with the live ICY song title
- ✅ Skips tracks that can't be opened or decoded instead of stalling
- 🔜 Crossfade, gapless, ReplayGain
- 🔜 Playback speed / pitch

## Playlist
- ✅ Add files, add folder (recursive), remove, clear
- ✅ Double-click / right-click to play; current-track highlight
- ✅ Per-track duration column
- ✅ Save/Load M3U, M3U8 and PLS
- ✅ Drag-and-drop files/folders/playlists from the OS onto the window (appended)
- ✅ Drag-and-drop reorder (grip handle)
- ✅ Sort by title / artist / album / duration (+ manual order)
- ✅ Search box to filter the playlist
- ✅ **Multi-selection** (Winamp-style): click selects one, Ctrl+click toggles,
  Shift+click / Shift+arrows select a range, Home/End jump; Invert/Crop and
  Remove operate on the selection
- ✅ Editor extras (Rem/Misc menus): remove missing files, randomize, reverse
- ✅ Robust playlist loading — M3U/PLS decoded leniently (tolerates a BOM or a
  stray non-UTF-8 byte), PLS ordered by `File<N>` index, and Windows-authored
  relative paths (backslashes) resolve on Linux too
- ✅ Jump-to-file (J) and jump-to-time (Ctrl+J)

## Metadata / Library
- ✅ Read ID3v2 / Vorbis / FLAC tags (title/artist/album)
- ✅ **Tag editor** (Alt+3 / options menu → Edit file tags): view and edit
  title / artist / album / album-artist / year / track / genre / comment, saved
  back to the file. MP3 (ID3v2) today; modular, so more formats follow (reading
  already works for all)
- ✅ Audio properties (kbps / kHz / channels)
- ✅ SQLite media library (scan folders, search, add to playlist)
  — stored at `%LOCALAPPDATA%/LinAmp/library.db` (Win) / `~/.local/share/LinAmp/` (Linux)
  — reachable in the classic UI via options menu → **Media Library…** (opens in
  its own window)
  — tag edits made in the editor update the library row immediately (no rescan);
  search treats `_` and `%` as literal text, not SQL wildcards
- ✅ Bookmarks — Alt+I bookmarks the current track; Media Library → Bookmarks
  view lists them (play, search filter, remove)
- 🔜 Album art, smart views / playlists

## UI / UX (Winamp feel)
- ✅ Dark skin, LCD-style display, scrolling marquee title
- ✅ LinAmp logo (green lightning bolt on a disc) as the app/window + taskbar
  icon; vector source kept in `assets/linamp_logo.svg` for reuse
- ✅ Main player + Equalizer + Playlist as separate windows that dock & snap
- ✅ Window-shade mode (double-click a title bar to collapse)
- ✅ Keyboard shortcuts matching Winamp — see the [README](README.md#keyboard-shortcuts)
- ✅ Preferences window (Ctrl+P): tag-load mode and 1×/2× sizing, persisted;
  shows the running `LinAmp <version>` (with build hash + date)
- ✅ Accented / Vietnamese track text is ASCII-folded in the classic bitmap font
  (e.g. "Trường Sơn" → "Truong Son") instead of showing blank gaps 🧪
- ✅ Always on top (options menu + Preferences)
- ✅ Minibrowser — "Open in browser" opens the system browser (xdg-open on
  Linux); the SHOUTcast directory pane is still planned (#146)

## Theming / Skins
- ✅ **Pixel-perfect classic skin renderer** — composites real `.wsz` sprite
  sheets (`MAIN/CBUTTONS/TITLEBAR/SHUFREP/POSBAR/VOLUME/BALANCE/MONOSTER/
  PLAYPAUS/NUMBERS/TEXT`) into the exact 275×116 main window
- ✅ Renders skin **artwork** (background picture) and **custom buttons** with
  pressed/selected states, custom LCD digits and bitmap-font title marquee
- ✅ Borderless window with skin titlebar (drag to move, close/minimize)
- ✅ Working controls: prev/play/pause/stop/next/eject, shuffle/repeat,
  seek bar, volume + balance sliders; EQ/PL buttons bridge to the modern view
- ✅ Classic skin is the whole UI (borderless). The egui "modern" view is
  retained only as a fallback when no skin is present.
- ✅ Title bars rebranded to LinAmp on a readable dark backing plate (main / EQ /
  playlist), legible over any skin.
- ✅ **Window-shade mode** — double-click a title bar to collapse that window to
  its 14px shade bar (main shows title + time); double-click to restore; the
  window stack resizes to fit. Close/minimize work on shade bars too.
- ✅ Load real Winamp `.wsz` skins; also derives egui/modern-view colour theme
  from `PLEDIT.TXT` / `VISCOLOR.TXT` / `MAIN.BMP`
- ✅ Skin picker (auto-discovers `./skins`), "Load .wsz…", reset to default
- ✅ Headless tools: `--check-skin` (colours) and `--render-skin in.wsz out.png`
  (classic-window PNG preview)
- ✅ Test skins bundled in `skins/` incl. artwork skins (Akira, Battle Angel
  Alita, Iria, Evangelion/rei) + base-2.91, Green-Dimension, TopazAmp, MacOSXAqua
- ✅ **Classic Equalizer window** (`EQMAIN.BMP`) — on/auto/presets buttons,
  preamp + 10 band sliders (drag), graph area; docks under the main window
- ✅ **Classic Playlist window** (`PLEDIT.BMP`) — tiled frame, track list in the
  skin's pledit colours, current-track highlight, click-to-play, wheel scroll
- ✅ Upper-left **options menu** (and right-click): open file/folder, Media
  Library, toggle
  EQ/Playlist, load skin, modern view, exit
- ✅ Title bars rebranded to LinAmp (main / equalizer / playlist)
- ✅ `--render-all` headless preview (main + EQ + playlist stacked)
- ✅ Playlist bottom bar wired: ADD (file/folder), REM (remove selected / crop /
  remove all), SEL (all/none/invert), MISC (sort title/artist/album/duration),
  LIST (new/save/load) — click-to-select, double-click-to-play, wheel scroll
- ✅ Live spectrum / oscilloscope in the classic viz area (click to cycle)
- 🔜 Window-shade polish
- 🔜 True "modern" (Winamp 3/5 `.wal` freeform) skin engine — large separate phase

## Equalizer
- ✅ 10-band + preamp UI, on/off, reset
- ✅ Per-band biquad DSP (RBJ peaking filters) — the sliders and preamp shape
  the sound live; applied as a rodio `Source` ahead of the visualizer
- ✅ Built-in presets (Rock, Pop, Jazz, Classical, Dance, Bass/Treble boost, …)
  via the classic **Presets** button and the modern view's dropdown
- ✅ EQ preset library — import/export Winamp `.eqf` plus named save/load
- ✅ **Auto** gain — sets the preamp to offset the loudest boosted band so the
  EQ is less likely to clip
- ✅ EQ state (on/auto/preamp/bands) remembered across runs

## Visualization
- ✅ Live spectrum analyzer (FFT) and oscilloscope (click to cycle), skin-coloured
- ✅ Album art display from embedded tags (with placeholder)

## System integration
- ✅ Open files/folders via CLI args ("Open with LinAmp")
- 🔜 Global media keys, system tray, taskbar controls, file associations

## Audio / devices
- ✅ Uses the system default output device; follows it live (e.g. headphones
  plugged in mid-session) by rebuilding the stream and resuming in place
- ✅ Seek bar jumps within the track (drag the main position slider)

## Windows (classic)
- ✅ **Separate OS windows** for main / EQ / playlist (detach & move freely),
  shown as a single taskbar entry (one program)
- ✅ Movement via the OS window-drag (smooth); magnetic snap to each other and to
  screen edges on release; dragging the main window moves the docked group
- ✅ Playlist resize via the bottom-right grip — both height (29px steps) and
  width (25px steps), snapping to Winamp steps; the frame tiles to any width
- ✅ Remembers the last skin and the playlist (tracks + current + playlist-window
  size) across runs; restored on launch unless files are passed on the CLI
- 🔜 Finer group-docking; remember full window layout between runs

## Testing
- `cargo test` runs the suite (playlist ops, M3U/PLS parse, skin colours,
  classic window geometry/hit-testing, preferences, EQ/balance DSP math,
  visualizer). Pure-logic and cross-platform (Windows + Linux).
- **CI** (GitHub Actions): every push/PR builds and tests on `ubuntu-latest`
  and `windows-latest`; tags (`v*`) and manual runs also upload release
  binaries for both platforms. Linux needs `libasound2-dev` (installed in CI).

## Planned / Backlog
Big-ticket items; the full, current backlog is in
[GitHub Issues](https://github.com/cjjulius/linamp/issues).
- 🔜 Crossfade, gapless, ReplayGain
- 🔜 True "modern" (Winamp 3/5 `.wal` freeform) skin engine

## Keyboard shortcuts
See the [README](README.md#keyboard-shortcuts) for the full list. Remaining
Winamp 2.x shortcuts to fill in are tracked in #144.
