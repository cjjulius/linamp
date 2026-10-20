<div align="center">
<img src="assets/linamp_logo_256.png" alt="LinAmp" width="128">

# LinAmp

**A Winamp-inspired audio player for Linux and Windows, in Rust.**

Pixel-perfect classic skin · 10-band EQ · live analyzer · playlist · Winamp keyboard shortcuts
</div>

---

LinAmp recreates the classic Winamp feel — it loads real `.wsz` skins and
composites the main, equalizer and playlist windows at exact Winamp sprite
coordinates — on a modern cross-platform stack (`eframe`/`egui` UI,
`rodio`/`symphonia` audio, `lofty` tags).

## Features

- **Classic skin UI** — loads real Winamp `.wsz` skins; main + EQ + playlist as
  separate windows that dock and snap together, with window-shade mode.
- **Playback** — MP3, FLAC, OGG/Vorbis, WAV, AAC/M4A, Opus, WavPack, AIFF; seek,
  volume, stereo balance, repeat/shuffle, stop-after/stop-with-fade.
- **10-band equalizer** with presets, preamp and an auto-gain mode (live biquad DSP).
- **Visualizer** — live FFT spectrum analyzer and oscilloscope, skin-coloured.
- **Playlist** — add files/folders, drag-and-drop, reorder, sort, search, M3U/PLS.
- **Tags & library** — read/edit ID3 tags; SQLite-backed media library.
- **Keyboard shortcuts** matching Winamp — see [SHORTCUTS.md](SHORTCUTS.md).

Full feature list and roadmap: [FEATURES.md](FEATURES.md). The backlog lives in
[GitHub Issues](https://github.com/cjjulius/linamp/issues).

## Build & run

Install [Rust](https://rustup.rs), then:

```bash
cargo build --release
./target/release/linamp            # or target\release\linamp.exe on Windows
```

Pass files or folders to open them: `linamp song.flac album/`.

- **Windows** builds with no extra setup.
- **Linux** needs a few system libraries (ALSA, GTK, X11/Wayland) — see
  [LINUX.md](LINUX.md). Audio targets PipeWire via ALSA.

## Skins

LinAmp auto-discovers `.wsz` skins in `./skins` (several are bundled). Load any
Winamp classic skin from the options menu → **Load .wsz…**, or drop one in that
folder. Headless preview: `linamp --render-all skin.wsz out.png`.

## Project layout

| File | Responsibility |
|------|----------------|
| `app.rs` | egui app, window management, input, menus, playlist/library UI |
| `classic.rs` | classic-skin compositor (main / EQ / playlist) |
| `dock.rs` | window docking + magnetic-snap geometry |
| `player.rs` | playback (rodio) + device following |
| `eq.rs` · `balance.rs` · `viz.rs` | equalizer DSP, stereo balance, analyzer FFT |
| `skin.rs` · `text.rs` | `.wsz` theming, LCD text helpers |
| `playlist.rs` · `tags.rs` · `library.rs` · `config.rs` | playlist, metadata, media library, settings |

`cargo test` runs the suite (pure, cross-platform); CI builds and tests on Linux
and Windows for every push and PR.

## License

[Apache-2.0](LICENSE).
