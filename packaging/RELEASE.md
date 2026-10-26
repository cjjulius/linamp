# Releasing LinAmp

LinAmp ships per-user installers (no admin / sudo) for Windows 10/11 and
Ubuntu-based Linux (Pop!_OS, Mint). The official installers are built by CI from
a `vX.Y.Z` tag, but the tag is only pushed after the Linux machine signs off
on the same build.

## Artifacts

| Platform | Package | Installs to | How |
|---|---|---|---|
| Windows | `LinAmp-<ver>-Setup.exe` (Inno Setup) | `%LOCALAPPDATA%\Programs\LinAmp` | run it, no UAC |
| Linux | `linamp-<ver>-linux-x86_64.tar.gz` | `~/.local` | extract, `./install.sh` |

User data (library DB, config, session) always lives under
`%LOCALAPPDATA%\LinAmp` (Windows) / `~/.local/share/LinAmp` (Linux) and is left
intact by uninstalls.

## Gated release flow

1. Branch & version. Work on `release/vX.Y.Z`. Set `version` in `Cargo.toml`.
2. Windows candidate (dev box).
   ```powershell
   packaging\windows\build-installer.ps1
   ```
   Requires Inno Setup 6.3+ (`choco install innosetup`). Install the resulting
   `dist\LinAmp-<ver>-Setup.exe` and smoke-test it.
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

## Notes

- The Windows exe embeds `assets/linamp.ico` (via `build.rs` + `winresource`);
  regenerate the `.ico` from `assets/linamp_logo_256.png` if the logo changes.
- Both installers place the `skins/` folder next to the binary, the app
  discovers skins relative to itself, so keep that layout.
- Deferred: Flatpak and `.deb`. The tarball covers the per-user requirement for
  the current target distros.
