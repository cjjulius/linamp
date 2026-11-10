<div align="center">

<h1><img src="assets/linamp_logo_64.png" alt="" height="30" align="absmiddle"> &nbsp;LinAmp</h1>

A Winamp-inspired audio player for Linux and Windows, in Rust.

**Plays 185 audio formats** - from everyday MP3 and FLAC to the chip music of three decades of consoles, computers and arcades.

Pixel-perfect classic skin with `.wsz` skin support - 10-band EQ - live analyzer - playlist - audio conversion & CD ripping - full keyboard shortcut support

<img src="assets/screenshot.png" alt="LinAmp in the classic skin, main player, equalizer and playlist" width="380">

</div>

---

LinAmp recreates the classic Winamp feel, it loads real `.wsz` skins and
composites the main, equalizer and playlist windows at exact Winamp sprite
coordinates, on a modern cross-platform stack (`eframe`/`egui` UI,
`rodio`/`symphonia` audio, `lofty` tags).

## What it plays

**185 file formats**, the full list is in [docs/SUPPORTED-FORMATS.md](docs/SUPPORTED-FORMATS.md).

- Everyday audio: MP3, FLAC, Ogg Vorbis, Opus, AAC/M4A, WAV, AIFF, Monkey's Audio, WavPack,
  True Audio, Musepack, DSD, Speex, AMR, AC-3 and QOA.
- The audio track of video files: MP4, MKV, WebM, AVI, FLV, MPEG-TS and MPEG program streams.
- MIDI (via a bundled SoundFont) and tracker modules (MOD/XM/S3M/IT and ~28 more).
- Retro console and computer chip music: NES, SNES, Game Boy, Sega Genesis/Mega Drive and
  Master System, PC Engine, Atari 8-bit, ZX Spectrum, Amstrad CPC, MSX, Commodore 64, plus
  the Amiga and DOS AdLib/OPL scene.
- Emulated console program music, all ROM-less (the `*SF` family): PlayStation, Nintendo 64,
  Nintendo DS, Game Boy Advance, Sega Saturn and Dreamcast, and Capcom QSound arcade.
- Game-engine streams (vgmstream): GameCube/Wii/3DS/Switch, PlayStation, Xbox and more.
- Windows Media Audio (WMA), DTS, Dolby TrueHD, RealAudio and Sony ATRAC.

## Features

- Classic skin UI, loads real Winamp `.wsz` skins; main + EQ + playlist as
  separate windows that dock and snap together, with window-shade mode.
- Plays 185 audio formats, from everyday MP3/FLAC to a deep retro catalog (see
  [What it plays](#what-it-plays)); seek, volume, stereo balance, repeat/shuffle,
  stop-after/stop-with-fade.
- Audio CD, play and rip (TOC and titles looked up online); rip or convert any
  track to MP3, Ogg Vorbis, Opus, FLAC, WAV or AIFF through a background queue.
- 10-band equalizer with presets, preamp and an auto-gain mode (live biquad DSP).
- Visualizer, live FFT spectrum analyzer and oscilloscope, skin-coloured.
- Playlist, add files/folders, drag-and-drop, reorder, sort, search; loads
  M3U/M3U8/PLS/XSPF/ASX/RAM playlists and CUE sheets (single-file albums split into tracks).
- Tags & library, read/edit tags (ID3v1/ID3v2 for MP3, Vorbis for FLAC/Ogg/Opus,
  MP4 for M4A, APEv2), one file at a time or many at once with the mass editor;
  SQLite-backed media library.
- System integration, global media keys, a system-tray mode and "Open with" file
  associations (per-user, Windows and Linux).

Full feature list and roadmap: [FEATURES.md](FEATURES.md). The backlog lives in
[GitHub Issues](https://github.com/cjjulius/linamp/issues).

## Keyboard shortcuts

Matching classic Winamp. (Up/Down move the selection when the playlist window is
focused, otherwise they change the volume.) In the playlist, selection is
Winamp-style: click selects one row, Ctrl+click toggles a row, Shift+click
or Shift+Up/Down selects a range and Home/End jump to the first/last track.

| Keys | Action | | Keys | Action |
|------|--------|---|------|--------|
| `X` | Play | | `L` / `Shift+L` | Open file(s) / folder |
| `C` | Pause | | `J` / `Ctrl+J` | Jump to file / to time |
| `V` | Stop | | `Alt+G` / `Alt+E` | Toggle equalizer / playlist |
| `Shift+V` | Stop with fade-out | | `Ctrl+D` | Toggle double size |
| `Ctrl+V` | Stop after current (toggle) | | `Ctrl+P` | Preferences |
| `B` / `Z` | Next / previous track | | `Ctrl+T` | Time elapsed / remaining |
| `Space` | Play/pause toggle | | `Alt+3` | Edit file tags |
| `Left` / `Right` | Seek -5s / +5s | | `Enter` / `Delete` | Play / remove selected |
| `Up` / `Down` | Volume up / down | | `Ctrl+A` / `Insert` | Select all / add files |
| `R` / `S` | Repeat / shuffle | | | |

## Install

Grab the latest installer from the
[Releases page](https://github.com/cjjulius/linamp/releases/latest). Both are
per-user installs, no admin or `sudo` and keep your data on uninstall.

### Windows (10/11, 64-bit)

Download and run `LinAmp-<version>-Setup.exe`. It installs just for your user
(no UAC prompt) under `%LOCALAPPDATA%\Programs\LinAmp`, adds a Start-Menu and
optional desktop shortcut and can be removed from Settings, Apps, LinAmp.
Your library, config and session live in `%LOCALAPPDATA%\LinAmp` and are kept on
uninstall.

### Linux (per-user)

Extract the tarball (`linamp-<version>-linux-x86_64.tar.gz`) and run
`./install.sh`, no sudo. Launch LinAmp from your app menu or run
`~/.local/bin/linamp`. `./uninstall.sh` removes it. Files land in the standard
per-user locations:

| Path | What |
|------|------|
| `~/.local/bin/linamp` | launcher (symlink) |
| `~/.local/lib/linamp/` | the app, binary + bundled skins |
| `~/.local/share/LinAmp/` | your data, library, config, session |
| `~/.local/share/applications/linamp.desktop`, `…/icons/hicolor/*/apps/linamp.*` | menu entry + icon |

Uninstalling removes everything except `~/.local/share/LinAmp/`, your library
and settings are kept; delete that folder too for a full wipe. Target distros are
Ubuntu-based (Pop!_OS, Mint); audio goes to PipeWire via ALSA. See
[LINUX.md](LINUX.md) for details.

## Build from source

Install [Rust](https://rustup.rs) and [CMake](https://cmake.org/download/) (CMake
builds the bundled libopus for Opus decoding), then:

```bash
cargo build --release
./target/release/linamp            # or target\release\linamp.exe on Windows
```

Pass files or folders to open them: `linamp song.flac album/`.

- Windows needs CMake on PATH (for example `winget install Kitware.CMake`); no
  other setup.
- Linux needs CMake plus a few system libraries (ALSA, GTK, X11/Wayland), see
  [LINUX.md](LINUX.md). Audio targets PipeWire via ALSA.

### Optional formats (build features)

A plain `cargo build` compiles the pure-Rust core: everyday audio, the audio track of video
containers, MIDI and the four core tracker formats. The FFmpeg-backed formats (WMA, DTS,
Dolby TrueHD, RealAudio, Sony ATRAC) and the retro catalog (chiptunes, the tracker zoo,
AdLib/OPL, SID, game-engine streams and the `*SF` family) are behind Cargo features, most of
which link a native library built from source. The shipped installers turn every feature on;
building that full 185-format set yourself, with the exact prerequisites and the per-library
build scripts, is documented in [packaging/RELEASE.md](packaging/RELEASE.md) and
[LINUX.md](LINUX.md).

## Skins

Several Winamp classic skins are bundled and show up in the skin picker. To use
any other classic `.wsz` skin, load it from the options menu (Load .wsz…); the
last skin you loaded is remembered across runs.

## Documentation

- [docs/SUPPORTED-FORMATS.md](docs/SUPPORTED-FORMATS.md), every format LinAmp plays.
- [FEATURES.md](FEATURES.md), the full feature list and Winamp 2.666 parity status.
- [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md), the module map, multi-window model,
  fingerprint-cached render pipeline, platform FFI boundary, and how to add a format.
- [docs/SHORTCUTS.md](docs/SHORTCUTS.md), the keyboard and mouse shortcuts.
- [LINUX.md](LINUX.md) and [packaging/RELEASE.md](packaging/RELEASE.md), build and release.

`cargo test` runs the suite (pure, cross-platform); CI builds and tests on Linux
and Windows for every push and PR.

## License

[Apache-2.0](LICENSE).
