# Releasing LinAmp

LinAmp ships **per-user installers** (no admin / sudo) for Windows 10/11 and
Ubuntu-based Linux (Pop!_OS, Mint). The official installers are built by CI from
a `vX.Y.Z` tag — but the tag is **only pushed after the Linux machine signs off**
on the same build.

## Artifacts

| Platform | Package | Installs to | How |
|---|---|---|---|
| Windows | `LinAmp-<ver>-Setup.exe` (Inno Setup) | `%LOCALAPPDATA%\Programs\LinAmp` | run it — no UAC |
| Linux | `linamp-<ver>-linux-x86_64.tar.gz` | `~/.local` | extract, `./install.sh` |

User data (library DB, config, session) always lives under
`%LOCALAPPDATA%\LinAmp` (Windows) / `~/.local/share/LinAmp` (Linux) and is left
intact by uninstalls.

## Gated release flow

1. **Branch & version.** Work on `release/vX.Y.Z`. Set `version` in `Cargo.toml`.
2. **Windows candidate (dev box).**
   ```powershell
   packaging\windows\build-installer.ps1
   ```
   Requires Inno Setup 6.3+ (`choco install innosetup`). Install the resulting
   `dist\LinAmp-<ver>-Setup.exe` and smoke-test it.
3. **Linux candidate (Linux box).** Pull the branch, then:
   ```sh
   sh packaging/linux/build-tarball.sh
   tar xzf dist/linamp-<ver>-linux-x86_64.tar.gz && cd linamp-<ver>-linux-x86_64
   ./install.sh
   ```
   Launch from the app menu; verify skins, PipeWire audio, file dialogs, docking.
   Runtime deps (present by default on Mint/Pop!_OS): GTK 3 (file dialogs),
   `libasound2` + `pipewire-alsa` (audio).
4. **Linux thumbs-up is the gate.** Do **not** cut the release until the Linux
   machine confirms the candidate is good.
5. **Tag → CI publishes.** Push the tag:
   ```sh
   git tag vX.Y.Z && git push origin vX.Y.Z
   ```
   CI (`.github/workflows/ci.yml`) builds both installers and creates the GitHub
   Release with them attached. (A `workflow_dispatch` run builds the installers
   as artifacts only — a dry run with no Release.)
6. **Merge.** Merge `release/vX.Y.Z` into `main`.

## Notes

- The Windows exe embeds `assets/linamp.ico` (via `build.rs` + `winresource`);
  regenerate the `.ico` from `assets/linamp_logo_256.png` if the logo changes.
- Both installers place the `skins/` folder next to the binary — the app
  discovers skins relative to itself, so keep that layout.
- Deferred: Flatpak and `.deb`. The tarball covers the per-user requirement for
  the current target distros.
