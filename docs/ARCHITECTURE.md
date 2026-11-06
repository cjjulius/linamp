# LinAmp architecture

A developer-oriented tour of how LinAmp is put together. It is a Winamp-style
audio player written in Rust, using `eframe`/`egui` for the UI, `rodio` +
`symphonia` for audio and `lofty` for tags. The goal is pixel-faithful classic
Winamp parity on both Windows and Linux.

## Module map

Core (`src/`):
- `main.rs`, entry point, CLI handling (`--check-skin`, `--render-skin`,
  `--render-all`, file arguments) and app launch.
- `player.rs`, audio engine: decode, play/pause/stop/seek, volume, device
  follow and the EQ/balance source chain.
- `decode.rs`, the single local decode path (symphonia with a format hint, plus a
  bundled libopus for Opus); also feeds streaming.
- `midi.rs`, MIDI synthesis (rustysynth + a bundled SoundFont); `tracker.rs`,
  tracker-module rendering (xmrs/xmrsplayer). Both expose the same player source API.
- Side-decoders for codecs symphonia lacks, each wrapping a crate (pure Rust unless
  noted) as a seekable player source like `midi.rs`/`tracker.rs`: `ape.rs` (Monkey's
  Audio), `wavpack.rs` (WavPack via wavicle), `tta.rs` (True Audio), `mpc.rs`
  (Musepack SV7/SV8), `ac3.rs` (AC-3/E-AC-3), `dsd.rs` (DSD .dsf/.dff via dsd-reader
  plus an original two-stage decimating FIR to PCM), `speex.rs` (Speex, Ogg-demuxed
  with the `ogg` crate and decoded by libspeex, the pure-Rust option being incorrect)
  and `amr.rs` (AMR .amr/.awb via the pure-Rust rvoip-codec-core).
  `ffdec.rs` is the optional FFmpeg path (WMA, DTS and Dolby TrueHD/MLP), behind the `ffmpeg`
  Cargo feature and off by default.
- Container-audio extractors that pull the audio stream out of a video/transport
  container and feed it back through the shared decode path (`decode::open_stream`):
  `ts.rs` (MPEG-TS), `avi.rs` (AVI), `flv.rs` (FLV) and `ps.rs` (MPEG program
  stream, with an MPEG-1 fallback scanner). They share one `ContainerAudio` source.
- `stream.rs`, HTTP(S)/ICY internet-radio streaming on a background thread.
- `cdda.rs`, audio-CD support: the TOC, the `cdda://` model, per-OS raw sector reads
  (the `CdDrive` trait), the background `CdWatcher`, online metadata
  (MusicBrainz/freedb) and eject.
- `encode.rs` + `convert_queue.rs`, audio conversion and CD ripping: the encoders
  (MP3/Ogg/Opus/WAV/AIFF), the shared decode-to-PCM core, and the serial background
  job queue that indexes finished files into the library.
- `eq.rs`, 10-band + preamp biquad DSP and presets; `eqf.rs`, Winamp `.eqf` preset
  import/export.
- `balance.rs`, stereo pan with centre snap.
- `viz.rs`, FFT spectrum / oscilloscope data.
- `playlist.rs`, track list, M3U/M3U8/PLS load/save, ordering, next/prev,
  shuffle.
- `selection.rs`, Winamp-style multi-selection logic (kept pure for testing).
- `tags.rs`, read/write tags: ID3 (MP3/MP2/MP1), Vorbis comments (FLAC/Ogg) and
  MP4 atoms (M4A); plus Basic Info (format, length, bitrate, …) for any format.
- `library.rs`, SQLite media library (scan, search).
- `config.rs`, on-disk preferences and session (skin, playlist, volume, …);
  `openq.rs`, the cross-instance open queue behind single-instance forwarding.
- `mediakeys.rs`, global media keys (Windows `RegisterHotKey`, Linux MPRIS);
  `tray.rs`, the system-tray icon and menu; `fileassoc.rs`, per-user "Open with"
  file associations (Windows registry, Linux `.desktop`).
- `skin.rs`, derives the modern-view colour theme from a skin.
- `classic.rs`, the classic skin renderer: decodes a `.wsz` and composites each
  window from its sprite sheets. The largest and most parity-critical module.
- `region.rs`, `cursor.rs`, `wincursor.rs`, skin window-shape masks and custom
  cursors.
- `dock.rs`, magnetic window snapping/docking geometry.
- `text.rs`, LCD time formatting and marquee scrolling.

App layer (`src/app/`):
- `app.rs`, the `eframe::App`: state, the per-frame update loop and the window
  focus / shutdown / double-size logic.
- `render.rs`, the per-window fingerprint cache and composite/upload pipeline.
- `window.rs`, classic per-window input (click/drag), docking, resize.
- `input.rs`, `menu.rs`, keyboard shortcuts and the options/right-click menus.
- `modern.rs`, the fallback egui "modern" view (used only when no skin loads).
- `library_ui.rs`, `tag_ui.rs`, the Media Library and tag-editor windows.
- `theme.rs`, egui styling.
- `platform_win.rs`, the OS FFI boundary (see below).

## Windowing: one app, several OS windows

The main player, equalizer, playlist and media library are each a real OS
window, shown as a single taskbar entry. This uses egui's multi-viewport model:
the main player is the root viewport, and the others are deferred viewports
created on demand. The classic windows are borderless (decorations off, fixed
size) and draw their own skin title bars; the user drags them by the skin title
bar, and they magnetically snap to each other and the screen edges (`dock.rs`).

Focus is tracked app-wide, not per window. Each window's own focus is recorded
(`foc_root`, `foc_eq`, …) and OR'd each frame into `app_focused`. All windows
show their active title bar together and only switch to the inactive title bar
when the whole app loses focus to another application, matching Winamp, rather
than one LinAmp window dimming when you click another.

"Always on top" and 1x/2x "double size" are app-wide toggles (clutterbar or
Preferences). Always-on-top is pushed to the windows only when it actually
changes (the last pushed state is remembered) to avoid spamming the window
manager. Double size rescales every window's native pixel size.

## Render pipeline: fingerprint-cached compositing

Each classic window is drawn by compositing skin sprites into an off-screen
`RgbaImage`, then uploading that as a nearest-neighbour GPU texture. Recompositing
every frame would be wasteful, so `render.rs` computes a cheap per-window
fingerprint (`window_fp`) hashing everything a window draws from, playback
state, theme, focus, toggles, volume/balance, marquee offset, the current
track's fields, visualizer bars and so on. The expensive recomposite + GPU
re-upload runs only when that fingerprint changes; otherwise the cached texture
is reused, so a paused, idle player costs almost nothing. The update loop also
throttles repaints: ~30 fps while something is animating, and a slow wake
otherwise (egui still repaints immediately on input).

When a skin omits optional bitmaps, windows fall back to sane defaults rather
than blank areas (example: the collapsed EQ draws a solid bar, and the main volume
and balance sliders synthesize a groove and thumb).

## Classic skin system

A `.wsz` skin is a ZIP of BMP sprite sheets (`MAIN`, `CBUTTONS`, `TITLEBAR`,
`SHUFREP`, `POSBAR`, `VOLUME`, `BALANCE`, `NUMBERS`, `TEXT`, `EQMAIN`, `PLEDIT`,
…). `classic.rs` decodes them and blits exact sprite rectangles (coordinates
taken from the webamp project) into the 275-wide classic windows, including the
bitmap-font LCD/marquee, custom cursors and `REGION.TXT` window-shape masks.
Headless previews (`--render-skin` / `--render-all`) use the same compositing
code, which makes skins straightforward to vet without launching the UI.

## Audio pipeline, CD and conversion

Playback routes a track by kind: MIDI through `midi.rs`, tracker modules through
`tracker.rs`, the side-decoders (`ape`/`wavpack`/`tta`/`mpc`/`ac3`/`dsd`/`speex`/`amr`, and `ffdec` for
WMA/DTS/TrueHD when built with `--features ffmpeg`) for codecs symphonia lacks, the
container-audio extractors (`ts`/`avi`/`flv`/`ps`) for video/transport containers,
and everything else through `decode.rs` (symphonia, plus libopus for Opus), with
`stream.rs` for HTTP(S) sources. The decoded `rodio::Source` runs
through the EQ and balance stages before the output device, with the analyzer
tapping the same stream.

An audio CD is handled entirely in `cdda.rs`: a background `CdWatcher` polls the
drives, reads each disc's TOC once and looks its titles up online, while playback
and ripping read raw CD sectors through the per-OS `CdDrive`. Conversion and
ripping share one path, `encode.rs` decodes to PCM (a file via `decode.rs`, or a CD
track via `cdda.rs`) and encodes to MP3/Ogg/Opus/WAV/AIFF, and `convert_queue.rs`
runs the jobs one at a time off the UI thread and indexes each finished file into
the library.

## System integration

`mediakeys.rs`, `tray.rs` and `fileassoc.rs` each isolate their OS specifics behind
a small cross-platform surface: global media keys use `RegisterHotKey` on Windows
and MPRIS over D-Bus on Linux; the tray uses the native shell tray on Windows and
`ksni` on Linux; file associations are per-user only (the Windows registry under
`HKCU`, a Linux `.desktop` plus `xdg-mime`), and never seize the default handler.

## Platform FFI boundary

Everything OS-specific is isolated in `app/platform_win.rs` behind small helpers,
so the rest of the code stays portable:
- Windows: Win32 calls for the global cursor position and for applying
  always-on-top to the real `HWND`.
- Linux/X11: Xlib calls to skip the taskbar for child windows, apply window-shape
  masks and load custom skin cursors, each wrapped so a missing library or a
  failed call degrades gracefully rather than crashing.

## Graceful shutdown

When the window manager asks LinAmp to close, it does not tear down immediately.
A short countdown lets the child viewports (EQ/playlist/library) close before the
root window, because dropping the root while child viewports are still alive can
crash `winit` (an unwrap on a TranslateCoordinates query against the dying
window). The root closes last once the children are gone.

## Persistence

`config.rs` stores preferences and the last session (skin, playlist contents and
current track, playlist-window size, volume and balance) under the OS data
directory, restored on launch unless files are passed on the command line.

## Testing and CI

`cargo test` covers the pure logic: playlist operations and M3U/PLS parsing,
skin colour extraction, classic-window geometry and hit-testing, preferences,
EQ/balance DSP math, selection logic and the visualizer. Tests are
cross-platform. GitHub Actions builds and tests on Ubuntu and Windows for every
push; version tags additionally build and upload release binaries.
