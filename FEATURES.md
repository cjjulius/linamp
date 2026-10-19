# LinAmp — Feature Roadmap

A Winamp-inspired audio player for Linux and Windows, written in Rust
(`eframe`/`egui` for UI, `rodio`/`symphonia` for audio, `lofty` for tags).

Legend: ✅ done in v0.1 · 🔜 planned · 🧪 partial

> Planned work and known shortfalls are tracked as **GitHub issues** in
> [cjjulius/linamp](https://github.com/cjjulius/linamp/issues) — that's the
> backlog of record. The 🔜 items below are kept only as a quick overview.

## Core playback
- ✅ Decode MP3, FLAC, OGG/Vorbis, WAV, AAC/M4A, Opus, WavPack, AIFF
- ✅ Play / Pause / Stop / Next / Previous
- ✅ Seek bar (scrub) with position + total time
- ✅ Volume control
- ✅ Repeat-all / Repeat-one / Shuffle
- ✅ Auto-advance on track end
- 🔜 Balance control, crossfade, gapless, ReplayGain
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
- 🔜 Jump-to-file, multi-select operations

## Metadata / Library
- ✅ Read ID3v2 / Vorbis / FLAC tags (title/artist/album)
- ✅ Audio properties (kbps / kHz / channels)
- ✅ SQLite media library (scan folders, search, add to playlist)
  — stored at `%LOCALAPPDATA%/LinAmp/library.db` (Win) / `~/.local/share/LinAmp/` (Linux)
- 🔜 Album art, smart views / playlists

## UI / UX (Winamp feel)
- ✅ Dark skin, LCD-style display, scrolling marquee title
- ✅ LinAmp logo (green lightning bolt on a disc) as the app/window + taskbar
  icon; vector source kept in `assets/linamp_logo.svg` for reuse
- ✅ Main player + Equalizer + Playlist in one window
- ✅ Keyboard shortcuts matching Winamp (see below)
- 🔜 Window shade / mini mode, docking, always-on-top

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
- ✅ Upper-left **options menu** (and right-click): open file/folder, toggle
  EQ/Playlist, load skin, modern view, exit
- ✅ Title bars rebranded to LinAmp (main / equalizer / playlist)
- ✅ `--render-all` headless preview (main + EQ + playlist stacked)
- ✅ Playlist bottom bar wired: ADD (file/folder), REM (remove selected / crop /
  remove all), SEL (all/none/invert), MISC (sort title/artist/album/duration),
  LIST (new/save/load) — click-to-select, double-click-to-play, wheel scroll
- 🔜 Window-shade polish, live spectrum in the classic viz area
- 🔜 True "modern" (Winamp 3/5 `.wal` freeform) skin engine — large separate phase

## Equalizer
- ✅ 10-band + preamp UI, on/off, reset
- ✅ Per-band biquad DSP (RBJ peaking filters) — the sliders and preamp shape
  the sound live; applied as a rodio `Source` ahead of the visualizer
- 🔜 EQ presets (Rock, Pop, etc.) and per-track auto-EQ

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
  classic window geometry/hit-testing, preferences, visualizer). Pure-logic and
  cross-platform (Windows + Linux).

## Planned / Backlog
- 🔜 **Options / Preferences window** (Ctrl+P): 
  - ID3 tag loading mode: **on load** (scan all up front) vs **on view** (lazy,
    as rows scroll into view) — matches Winamp
  - Drag-drop behaviour (append vs. insert)
  - **Sizing**: 1x pixel-for-pixel vs 2x scaled-for-modern-screens (Ctrl+D)
  - persisted to a config file
  - opens in its own window (never clipped by the small main window)
- 🔜 Lazy/background ID3 scanning to support the "on view" mode
- 🔜 Full Winamp keyboard-shortcut support — see [SHORTCUTS.md](SHORTCUTS.md)
  (implies: ID3 tag editor Alt+3, jump-to-file J, jump-to-time Ctrl+J,
  time elapsed/remaining Ctrl+T, open URL/stream Ctrl+L)
- 🔜 Always-on-top, window-shade polish
- 🔜 Live spectrum/oscilloscope inside the classic viz area
- 🔜 True "modern" (Winamp 3/5 `.wal` freeform) skin engine

## Keyboard shortcuts (Winamp-compatible)
| Key | Action |
|-----|--------|
| X | Play |
| C | Pause |
| V | Stop |
| B | Next track |
| Z | Previous track |
| Space | Play/Pause toggle |
| L | Open file(s) |
| R | Toggle repeat |
| S | Toggle shuffle |
| ↑ / ↓ | Volume up / down |
| ← / → | Seek -5s / +5s |
