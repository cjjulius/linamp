<#
Build the LinAmp per-user Windows installer.

  1. Stops any running linamp.exe (so the release build can overwrite it).
  2. Builds the optimized release binary (with the embedded exe icon).
  3. Compiles packaging\windows\linamp.iss with Inno Setup (ISCC).

Prerequisite: Inno Setup 6.3+  (https://jrsoftware.org/isdl.php, or
`choco install innosetup`). Output: dist\LinAmp-<version>-Setup.exe
#>
$ErrorActionPreference = "Stop"

# Repo root = two levels up from this script.
$root = Resolve-Path (Join-Path $PSScriptRoot "..\..")
Set-Location $root

Write-Host "==> Stopping any running linamp.exe" -ForegroundColor Cyan
Get-Process linamp -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue

Write-Host "==> cargo build --release" -ForegroundColor Cyan
cargo build --release
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

Write-Host "==> Compiling installer v$ver with $iscc" -ForegroundColor Cyan
& $iscc "/DMyAppVersion=$ver" (Join-Path $PSScriptRoot "linamp.iss")
if ($LASTEXITCODE -ne 0) { throw "ISCC failed" }

$setup = Get-ChildItem (Join-Path $root "dist\LinAmp-*-Setup.exe") | Sort-Object LastWriteTime | Select-Object -Last 1
Write-Host "==> Done: $($setup.FullName)" -ForegroundColor Green
