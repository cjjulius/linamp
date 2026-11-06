<#
Build the LinAmp per-user Windows installer.

  1. Stops any running linamp.exe (so the release build can overwrite it).
  2. Builds the optimized release binary (with the embedded exe icon).
  3. Compiles packaging\windows\linamp.iss with Inno Setup (ISCC).

The shipped installer bundles FFmpeg (for WMA and other formats with no
permissive pure-Rust decoder), so the single installer carries everything with
no extra downloads. To build that release-equivalent installer, point
-FfmpegBin at the bin folder of an FFmpeg 5.1.x "shared" build (5.1.x is the
API ffmpeg-next 5.1.1 tracks), or set FFMPEG_DIR and the script uses
$FFMPEG_DIR\bin. Building --features ffmpeg also needs LLVM for bindgen (set
LIBCLANG_PATH if clang is not on PATH; the pinned bindgen wants LLVM 14) and the
MSVC headers (run from a "Developer PowerShell for VS").

Without an FFmpeg build the script still produces a working installer, but a
DEV one without WMA. The official release installers are built by CI from a
vX.Y.Z tag and always bundle FFmpeg, so a no-WMA build is for local iteration,
not for shipping.

Prerequisite: Inno Setup 6.3+  (https://jrsoftware.org/isdl.php, or
`choco install innosetup`). Output: dist\LinAmp-<version>-Setup.exe
#>
param(
    # The FFmpeg shared build's bin dir. Defaults to $FFMPEG_DIR\bin when set.
    # Leave empty to build a dev installer without WMA.
    [string]$FfmpegBin = $(if ($env:FFMPEG_DIR) { Join-Path $env:FFMPEG_DIR 'bin' } else { '' })
)
$ErrorActionPreference = "Stop"

# Repo root = two levels up from this script.
$root = Resolve-Path (Join-Path $PSScriptRoot "..\..")
Set-Location $root

$withFfmpeg = [bool]$FfmpegBin -and (Test-Path $FfmpegBin)
if ($FfmpegBin -and -not $withFfmpeg) {
    throw "FfmpegBin '$FfmpegBin' does not exist. Pass the FFmpeg shared build's bin dir, or leave it empty to build a dev (no-WMA) installer."
}

Write-Host "==> Stopping any running linamp.exe" -ForegroundColor Cyan
Get-Process linamp -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue

if ($withFfmpeg) {
    Write-Host "==> cargo build --release --features ffmpeg  (WMA/FFmpeg bundled)" -ForegroundColor Cyan
    cargo build --release --features ffmpeg
} else {
    Write-Host "==> cargo build --release  (DEV build, no WMA)" -ForegroundColor Yellow
    Write-Host "    Release installers bundle FFmpeg; pass -FfmpegBin <FFmpeg 5.1.x shared build>\bin for the shipping config." -ForegroundColor Yellow
    cargo build --release
}
if ($LASTEXITCODE -ne 0) { throw "cargo build failed" }

# Locate the Inno Setup compiler.
$iscc = (Get-Command iscc -ErrorAction SilentlyContinue).Source
if (-not $iscc) {
    foreach ($p in @(
        "${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe",
        "${env:ProgramFiles}\Inno Setup 6\ISCC.exe",
        "${env:LOCALAPPDATA}\Programs\Inno Setup 6\ISCC.exe"
    )) {
        if (Test-Path $p) { $iscc = $p; break }
    }
}
if (-not $iscc) {
    throw "Inno Setup (ISCC.exe) not found. Install Inno Setup 6.3+ (choco install innosetup) and retry."
}

# Single source of truth for the version: Cargo.toml (passed to ISCC so the
# installer name/version can never drift from the crate's).
$ver = ([regex]::Match((Get-Content (Join-Path $root "Cargo.toml") -Raw), '(?m)^version\s*=\s*"([^"]+)"')).Groups[1].Value
if (-not $ver) { throw "Could not read version from Cargo.toml" }

# /DWithFfmpeg + /DFfmpegBin tell the installer to bundle the FFmpeg DLLs from
# the shared build's bin dir next to the exe (one installer, everything in it).
$isccArgs = @("/DMyAppVersion=$ver")
if ($withFfmpeg) { $isccArgs += @("/DWithFfmpeg", "/DFfmpegBin=$FfmpegBin") }
$isccArgs += (Join-Path $PSScriptRoot "linamp.iss")

$label = if ($withFfmpeg) { " (WMA bundled)" } else { " (dev, no WMA)" }
Write-Host "==> Compiling installer v$ver$label with $iscc" -ForegroundColor Cyan
& $iscc @isccArgs
if ($LASTEXITCODE -ne 0) { throw "ISCC failed" }

$setup = Get-ChildItem (Join-Path $root "dist\LinAmp-*-Setup.exe") | Sort-Object LastWriteTime | Select-Object -Last 1
Write-Host "==> Done: $($setup.FullName)" -ForegroundColor Green
