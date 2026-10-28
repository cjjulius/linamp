<div align="center">

<h1><img src="assets/linamp_logo_64.png" alt="" height="30" align="absmiddle"> &nbsp;LinAmp</h1>

A Winamp-inspired audio player for Linux and Windows, in Rust.

Pixel-perfect classic skin with full `.wsz` skin support - 10-band EQ - live analyzer - playlist - full keyboard shortcut support

<img src="assets/screenshot.png" alt="LinAmp in the classic skin, main player, equalizer and playlist" width="380">

</div>

---

LinAmp recreates the classic Winamp feel, it loads real `.wsz` skins and
composites the main, equalizer and playlist windows at exact Winamp sprite
coordinates, on a modern cross-platform stack (`eframe`/`egui` UI,
`rodio`/`symphonia` audio, `lofty` tags).

## Features

- Classic skin UI, loads real Winamp `.wsz` skins; main + EQ + playlist as
  separate windows that dock and snap together, with window-shade mode.
- Playback, MP3, MPEG Layer I/II, FLAC, OGG/Vorbis, WAV, AAC/M4A, AIFF, tracker
  modules and MIDI; seek, volume, stereo balance, repeat/shuffle,
  stop-after/stop-with-fade.
- 10-band equalizer with presets, preamp and an auto-gain mode (live biquad DSP).
- Visualizer, live FFT spectrum analyzer and oscilloscope, skin-coloured.
- Playlist, add files/folders, drag-and-drop, reorder, sort, search, M3U/PLS.
- Tags & library, read/edit tags (ID3 for MP3/MP2, Vorbis for FLAC/Ogg, MP4 for
  M4A); SQLite-backed media library.

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

Install [Rust](https://rustup.rs), then:

```bash
cargo build --release
./target/release/linamp            # or target\release\linamp.exe on Windows
```

Pass files or folders to open them: `linamp song.flac album/`.

- Windows builds with no extra setup.
- Linux needs a few system libraries (ALSA, GTK, X11/Wayland), see
  [LINUX.md](LINUX.md). Audio targets PipeWire via ALSA.

Packaging the installers (Inno Setup / tarball) is documented in
[packaging/RELEASE.md](packaging/RELEASE.md).

## Skins

LinAmp auto-discovers `.wsz` skins in `./skins` (several are bundled). Load any
Winamp classic skin from the options menu (Load .wsz…), or drop one in that
folder. Headless preview: `linamp --render-all skin.wsz out.png`.

## Project layout

| File | Responsibility |
|------|----------------|
| `app.rs` + `app/` | app state and update loop, split into focused modules (`window`, `input`, `menu`, `render`, `library_ui`, `tag_ui`, `modern`, `theme`, `platform_win`) |
| `classic.rs` | classic-skin compositor (main / EQ / playlist) |
| `dock.rs` | window docking + magnetic-snap geometry |
| `player.rs` | playback (rodio) + device following |
| `eq.rs` - `balance.rs` - `viz.rs` | equalizer DSP, stereo balance, analyzer FFT |
| `skin.rs` - `text.rs` | `.wsz` theming, LCD text helpers |
| `playlist.rs` - `tags.rs` - `library.rs` - `config.rs` | playlist, metadata, media library, settings |

For how the pieces fit together, the multi-window model, the fingerprint-cached
render pipeline and the platform FFI boundary, see
[docs/ARCHITECTURE.md](docs/ARCHITECTURE.md). The keyboard and mouse shortcuts are
listed in [docs/SHORTCUTS.md](docs/SHORTCUTS.md), and the Winamp 2.666 parity
status is tracked in [docs/PARITY.md](docs/PARITY.md).

`cargo test` runs the suite (pure, cross-platform); CI builds and tests on Linux
and Windows for every push and PR.

> The screenshot is `assets/screenshot.png`, regenerated with
> `cargo run -- --render-all skins/linamp_base_skin.wsz` (then scaled 2×) when the
> UI changes substantially.

## License

[Apache-2.0](LICENSE).
