# Releasing LinAmp

LinAmp ships per-user installers (no admin / sudo) for Windows 10/11 and
Ubuntu-based Linux (Pop!_OS, Mint). The official installers are built by CI from
a `vX.Y.Z` tag, but the tag is only pushed after the Linux machine signs off
on the same build.

Each installer bundles FFmpeg (for WMA and other formats with no permissive pure-Rust
decoder) and the retro/codec libraries (libgme, libopenmpt, AdPlug, libsidplayfp, vgmstream,
libupse and the `*SF` cores), so one installer per platform carries everything with no extra
downloads or separate DLLs. This is the default release configuration (all Cargo features
on); see Licensing below.

## Artifacts

| Platform | Package | Installs to | How |
|---|---|---|---|
| Windows | `LinAmp-<ver>-Setup.exe` (Inno Setup) | `%LOCALAPPDATA%\Programs\LinAmp` | run it, no UAC |
| Linux | `linamp-<ver>-linux-x86_64.tar.gz` | `~/.local` | extract, `./install.sh` |

User data (library DB, config, session) always lives under
`%LOCALAPPDATA%\LinAmp` (Windows) / `~/.local/share/LinAmp` (Linux) and is left
intact by uninstalls.

## Building the FFmpeg build locally (Windows)

FFmpeg is the shipping configuration, so a local candidate must be built with
`--features ffmpeg`. That needs three things, mostly because `ffmpeg-sys-next`
5.1.1 uses an old bindgen (0.59.2):

- FFmpeg 5.1.x "shared" dev build (matching `ffmpeg-next` 5.1.1), for example
  gyan's `ffmpeg-5.1.2-full_build-shared`. Set `FFMPEG_DIR` to its root (it holds
  `include/`, `lib/`, `bin/`). Its `bin` DLLs are what the installer bundles next
  to the exe, and what a dev build needs on `PATH` (or copied beside `linamp.exe`)
  at runtime.
- LLVM 14 for libclang. A newer libclang makes bindgen 0.59.2 panic (a `LexError`);
  14 works. The `libclang==14.0.1` pip wheel is a portable source: unzip it and
  point `LIBCLANG_PATH` at the folder holding `libclang.dll`.
- The MSVC C++ toolset (VC Tools), from an install that actually has it (Build
  Tools, or a VS install with "Desktop development with C++"; a bare Community
  install may not). bindgen 0.59.2 does not auto-find the MSVC headers, so after
  entering the dev shell, feed every `INCLUDE` dir to it:
  `BINDGEN_EXTRA_CLANG_ARGS` = each `INCLUDE` entry as `-I"..."`.

CI does all of this (`.github/workflows/ci.yml`); this section is only for
reproducing the release build on a dev box.

## Gated release flow

1. Branch & version. Work on `release/vX.Y.Z`. Set `version` in `Cargo.toml`.
2. Windows candidate. The authoritative installer is the one CI builds; it
   always bundles FFmpeg. For a local candidate that matches it, from a
   "Developer PowerShell for VS" (so the MSVC headers are on `INCLUDE`):
   ```powershell
   packaging\windows\build-installer.ps1 -FfmpegBin "<FFmpeg 5.1.x shared build>\bin"
   ```
   That builds the WMA-bundled installer. A bare `build-installer.ps1` builds a
   dev installer without WMA, fine for quick UI iteration but not for shipping.
   The `--features ffmpeg` build also needs LLVM for bindgen (the pinned bindgen
   wants LLVM 14; set `LIBCLANG_PATH`). Requires Inno Setup 6.3+
   (`choco install innosetup`). Install the resulting
   `dist\LinAmp-<ver>-Setup.exe` and smoke-test it, WMA playback included.
3. Linux candidate (Linux box). Pull the branch, then:
   ```sh
   sh packaging/linux/build-tarball.sh
   tar xzf dist/linamp-<ver>-linux-x86_64.tar.gz && cd linamp-<ver>-linux-x86_64
   ./install.sh
   ```
   Launch from the app menu; verify skins, PipeWire audio, file dialogs, docking.
   Runtime deps (present by default on Mint/Pop!_OS): GTK 3 (file dialogs),
   `libasound2` + `pipewire-alsa` (audio).
4. Linux thumbs-up is the gate. Do not cut the release until the Linux
   machine confirms the candidate is good.
5. Tag, then CI publishes. Push the tag:
   ```sh
   git tag vX.Y.Z && git push origin vX.Y.Z
   ```
   CI (`.github/workflows/ci.yml`) builds both installers and creates the GitHub
   Release with them attached. (A `workflow_dispatch` run builds the installers
   as artifacts only, a dry run with no Release.)
6. Merge back into `main`, required, same day as the tag. This is the
   final, non-optional step; a release is not finished until it is done:
   ```sh
   git switch main && git pull
   git merge --no-ff release/vX.Y.Z && git push origin main
   ```
   Do it before cutting any new branch. The tag does not advance `main`:
   if the merge-back is skipped, `main` stays behind the release, and every
   branch cut from it afterward silently lacks the released fixes.

   > This is exactly how 0.3.0 got tangled. 0.2.0 was tagged and published
   > from `release/v0.2.0` but never merged back, so `main` stayed on 0.1.0.
   > `feat/feature-set` then branched from that stale `main` and silently lacked
   > v0.2.0's Linux popup + packaging fixes, they had to be re-integrated into
   > 0.3.0 to avoid regressing them. No wrong branch, no merge conflict: just a
   > skipped merge-back.

## Simpler alternative: tag on `main`

A `release/*` branch exists only to hold release-candidate fixes during QA
without blocking other work on `main`. If a release needs no such isolation,
skip the branch entirely: bump `version` on `main`, push, run the Windows/Linux
candidates from `main`, then tag that `main` commit. With nothing to merge back,
`main` can't drift. Use a `release/*` branch only when you genuinely need it,
and when you do, step 6 is mandatory.

## Licensing

LinAmp's own source is Apache-2.0. The default pure-Rust build is permissive throughout:
Apache/MIT decoders, symphonia (MPL-2.0), and the statically linked BSD libopus and LGPL
LAME used for Opus/MP3 encoding (LGPL static linking is satisfied because the project is
open source and therefore relinkable). The full-featured release additionally bundles
several copyleft decoders, so the shipped binary as a whole is distributed under GPLv3:
libsidplayfp (GPL-2+), Highly_Theoretical and Highly_Quixotic (GPL-3), lazyusf2 and vio2sf
(GPL-2+), and libupse (GPL). GSF (mGBA/libgsf, MPL-2.0) and NCSF (SSEQPlayer, WTFPL) are
permissive. FFmpeg is built LGPL-only, with no GPL components (x264/libass/etc.); do not use
a GPL FFmpeg configuration, which would force the whole binary to GPL regardless. No
proprietary code and no console BIOS or ROM is bundled anywhere.

The retro and codec libraries (FFmpeg, libgme, libopenmpt, AdPlug, libsidplayfp, vgmstream,
libupse and the `*SF` cores, libvorbis) are built trimmed from source and bundled next to the
binary by the `packaging/{linux,windows}/build-*` scripts; each script pins its version and
documents its build gotchas inline (for example libsidplayfp is pinned to the 2.x API, and
libupse is compiled `/MD` on Windows to share the dynamic CRT).

## Notes

- The Windows exe embeds `assets/linamp.ico` (via `build.rs` + `winresource`);
  regenerate the `.ico` from `assets/linamp_logo_256.png` if the logo changes.
- Both installers place the `skins/` folder next to the binary, the app
  discovers skins relative to itself, so keep that layout.
- Deferred: Flatpak and `.deb`. The tarball covers the per-user requirement for
  the current target distros.
