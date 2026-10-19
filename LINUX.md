# LinAmp on Linux — build & testing notes

This document briefs a contributor (human or AI assistant) picking up **Linux
build and runtime testing** for LinAmp. Development and testing have so far been
done on **Windows**, which is the known-good platform. The code is written to be
cross-platform and has **no OS-specific branches** yet, so most of the work here
is making it build and run cleanly on Linux and fixing whatever platform gaps
turn up — without regressing the working Windows build.

## What LinAmp is

A Winamp-inspired audio player in Rust that reproduces the **classic skin** look
and feel (main window, equalizer, playlist) and Winamp's keyboard-shortcut set.
Key crates:

- **eframe / egui 0.29** — borderless multi-viewport windows (main / EQ /
  playlist are separate OS windows drawn from composited skin bitmaps).
- **rodio 0.20** (`symphonia-all`) — decoding + playback. Audio reaches the OS
  through rodio → **cpal**.
- **lofty 0.21** — tag read/write. **rustfft** — the analyzer. **image 0.25**,
  **zip** — `.wsz` skin loading (skins are zipped Winamp BMP sets).
- **rusqlite** (`bundled`) — the media library DB (compiles its own SQLite).
- **rfd** — native file/folder dialogs.

Skin compositing lives in `src/classic.rs` (blits sprites from `.wsz` BMPs into
exact-geometry windows). Playback is in `src/player.rs`; the egui app and
keyboard shortcuts in `src/app.rs`.

## Audio on Linux

cpal uses the **ALSA** backend on Linux. On the target distros audio is served
by **PipeWire**, which exposes an ALSA-compatible device via `pipewire-alsa`, so
cpal talks to PipeWire through that bridge — no LinAmp code change needed. If
there's no sound, check that `pipewire` and `pipewire-alsa` (or `wireplumber`)
are running and that a default sink exists (`pactl info`, `pavucontrol`).

`src/player.rs` already rebuilds the output stream when the default device
changes (`poll_device`) — worth testing on Linux when switching sinks.

**Target distros:** Pop!_OS and Linux Mint, latest stock releases (both
Ubuntu/Debian-based). Commands below assume `apt`.

## Build prerequisites

Install Rust via [rustup](https://rustup.rs), then the system dev libraries:

```bash
sudo apt update
sudo apt install -y \
  build-essential pkg-config \
  libasound2-dev \
  libgtk-3-dev \
  libxcb-render0-dev libxcb-shape0-dev libxcb-xfixes0-dev \
  libxkbcommon-dev libwayland-dev
```

- `build-essential`, `pkg-config` — C toolchain for the `rusqlite` bundled
  SQLite and `-sys` build scripts.
- `libasound2-dev` — ALSA headers for cpal (audio).
- `libgtk-3-dev` — native file dialogs (rfd).
- `libxcb-*`, `libxkbcommon-dev`, `libwayland-dev` — winit/egui windowing on
  X11 and Wayland.

If a `cargo build` fails inside a `*-sys` crate, the error names the missing
library — install its matching `-dev` package and retry. Record any additions
in this file.

## Build, run, verify

```bash
cargo build --release          # debug build also fine for iterating
./target/release/linamp        # launches the player (needs a desktop session)
cargo test                     # unit tests (headless, no audio/display needed)
```

`build.rs` copies `skins/` next to the built binary, so bundled skins are found
at runtime.

**Headless skin verification** (no display or audio required) — renders a skin's
windows to a PNG so you can check compositing over SSH or in CI:

```bash
cargo run --release -- --render-all skins/linamp_base_skin.wsz /tmp/out.png
# or --render-skin for just the main window
```

## What to watch for on Linux

These are untested on Linux and the most likely sources of platform bugs:

- **Borderless windows** — the three windows are undecorated and manually
  positioned/dragged. Behaviour differs between **X11 and Wayland** (Wayland
  restricts client-side window positioning). Test both session types.
- **File dialogs** (rfd) — opening files/folders, adding to the playlist.
- **Audio** — playback, device switching, no-xruns under PipeWire.
- **HiDPI / fractional scaling** — the skin is pixel-exact; check it isn't
  blurred or mis-scaled.
- **Fonts/paths** — config/DB live under `dirs`-provided paths (XDG dirs on
  Linux); confirm the library DB is created in the right place.

## Working conventions

- **All Linux work stays on the `linux-testing` branch.** Keep it a single
  long-lived branch so the working Windows branches/`main` aren't disturbed.
  Prefer genuinely cross-platform fixes; when something must differ by OS, gate
  it with `#[cfg(...)]` rather than breaking Windows. Merge back to `main` only
  once a change is confirmed good on both platforms.
- **Commits** are authored as `cjjulius`. Match the existing history: a plain
  subject + body, **no AI/assistant attribution or `Co-Authored-By` lines.**
- **GitHub Issues are the backlog** (repo `cjjulius/linamp`, private). File new
  Linux findings as issues; reference them from commits/PRs.
- Don't commit build artifacts or local junk; `skins/` is bundled automatically.

## Repo map

| Path | What |
|------|------|
| `src/main.rs` | entry point; the `--render-skin` / `--render-all` headless flags |
| `src/app.rs` | egui app, window management, keyboard shortcuts, playlist/library UI |
| `src/classic.rs` | classic-skin compositor (main / EQ / playlist windows) |
| `src/player.rs` | rodio playback, device polling, fades |
| `src/viz.rs`, `src/eq.rs`, `src/balance.rs` | analyzer FFT, equalizer DSP, stereo balance |
| `src/skin.rs` | `.wsz` parsing, VISCOLOR/colour themes |
| `src/playlist.rs` | playlist model, track metadata, durations |
| `skins/` | bundled `.wsz` skins (copied next to the binary by `build.rs`) |
