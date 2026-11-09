<#
.SYNOPSIS
  Build Game Music Emu (libgme) and its zlib dependency as shared libraries for the
  `gme` feature on Windows (chiptune/console music: NSF, GBS, VGM/VGZ, SPC, AY, SAP,
  KSS, HES, GYM via crate::gme).

.DESCRIPTION
  The Windows counterpart to packaging/linux/build-gme-min.sh. On Linux libgme links the
  system zlib; Windows has none, so this builds zlib from source first and points libgme
  at it, which is what turns on the gzipped VGM (.vgz) format. It produces two prefixes:

    <ZlibDir>\{bin\zlib.dll, lib\zlib.lib, include\...}
    <GmeDir>\{bin\gme.dll,  lib\gme.lib,  include\gme\gme.h}

  build.rs links the import library from $GME_DIR\lib; the installer (linamp.iss, via
  /DWithGme /DGmeBin /DZlibBin) ships gme.dll and zlib.dll next to the exe. gme.dll imports
  zlib.dll at load time, so both must travel together.

  Must run inside an MSVC developer environment (nmake + cl on PATH), the same shell the
  FFmpeg build uses. CMake is required. Idempotent: skips a library already present.

.PARAMETER ZlibDir
  Install prefix for zlib. Default: <repo>\build\zlib.

.PARAMETER GmeDir
  Install prefix for libgme. Default: <repo>\build\gme-min.
#>
[CmdletBinding()]
param(
    [string]$ZlibDir,
    [string]$GmeDir
)
$ErrorActionPreference = 'Stop'

# Pinned to match what has been validated (and the Linux build for libgme).
$ZLIB_VERSION = '1.3.1'
$ZLIB_SHA256  = '9a93b2b7dfdac77ceba5a558a580e74667dd6fede4585b91eefb60f03b72df23'
$GME_VERSION  = '0.6.3'
$GME_SHA256   = '626e8a104e0dadd10ef6519a67aca880c7b40f81471659f1935b61754e12fc7b'

$repo = Resolve-Path (Join-Path $PSScriptRoot '..\..')
if (-not $ZlibDir) { $ZlibDir = Join-Path $repo 'build\zlib' }
if (-not $GmeDir)  { $GmeDir  = Join-Path $repo 'build\gme-min' }

function Get-Verified([string]$url, [string]$outFile, [string]$sha256) {
    Write-Host "==> Fetching $url"
    Invoke-WebRequest -Uri $url -OutFile $outFile -MaximumRetryCount 5 -RetryIntervalSec 5
    $got = (Get-FileHash -Algorithm SHA256 $outFile).Hash.ToLower()
    if ($got -ne $sha256.ToLower()) {
        throw "SHA256 mismatch for $outFile`n  expected $sha256`n  got      $got"
    }
}

# A plain NMake build keeps this single-config and free of the Visual Studio generator's
# multi-config layout; nmake/cl come from the MSVC developer environment the caller sets up.
$gen = 'NMake Makefiles'

# ---- zlib ----
if (Test-Path (Join-Path $ZlibDir 'lib\zlib.lib')) {
    Write-Host "==> zlib already present at $ZlibDir"
} else {
    $work = New-Item -ItemType Directory -Force -Path (Join-Path ([IO.Path]::GetTempPath()) ("zlib-" + [Guid]::NewGuid()))
    try {
        $tgz = Join-Path $work 'zlib.tar.gz'
        Get-Verified "https://github.com/madler/zlib/releases/download/v$ZLIB_VERSION/zlib-$ZLIB_VERSION.tar.gz" $tgz $ZLIB_SHA256
        tar -xzf $tgz -C $work
        $src = Join-Path $work "zlib-$ZLIB_VERSION"
        Write-Host "==> Configuring zlib (shared, release) -> $ZlibDir"
        cmake -S $src -B (Join-Path $work 'bld') -G $gen `
            "-DCMAKE_BUILD_TYPE=Release" `
            "-DCMAKE_INSTALL_PREFIX=$ZlibDir"
        if ($LASTEXITCODE) { throw "cmake configure (zlib) failed" }
        cmake --build (Join-Path $work 'bld')
        if ($LASTEXITCODE) { throw "cmake build (zlib) failed" }
        cmake --install (Join-Path $work 'bld')
        if ($LASTEXITCODE) { throw "cmake install (zlib) failed" }
    } finally {
        Remove-Item -Recurse -Force $work -ErrorAction SilentlyContinue
    }
}

# ---- libgme ----
if (Test-Path (Join-Path $GmeDir 'lib\gme.lib')) {
    Write-Host "==> libgme already present at $GmeDir"
} else {
    $work = New-Item -ItemType Directory -Force -Path (Join-Path ([IO.Path]::GetTempPath()) ("gme-" + [Guid]::NewGuid()))
    try {
        $tgz = Join-Path $work 'gme.tar.gz'
        Get-Verified "https://bitbucket.org/mpyne/game-music-emu/downloads/game-music-emu-$GME_VERSION.tar.gz" $tgz $GME_SHA256
        tar -xzf $tgz -C $work
        $src = Join-Path $work "game-music-emu-$GME_VERSION"
        Write-Host "==> Configuring libgme (shared, release, with zlib) -> $GmeDir"
        # CMAKE_POLICY_VERSION_MINIMUM=3.5: libgme 0.6.3's cmake_minimum_required predates
        # modern CMake, which otherwise refuses to configure. CMAKE_PREFIX_PATH points at the
        # zlib prefix so the configure step finds it and enables VGZ (gzipped VGM).
        cmake -S $src -B (Join-Path $work 'bld') -G $gen `
            "-DCMAKE_BUILD_TYPE=Release" `
            "-DBUILD_SHARED_LIBS=ON" `
            "-DENABLE_UBSAN=OFF" `
            "-DCMAKE_POLICY_VERSION_MINIMUM=3.5" `
            "-DCMAKE_PREFIX_PATH=$ZlibDir" `
            "-DCMAKE_INSTALL_PREFIX=$GmeDir"
        if ($LASTEXITCODE) { throw "cmake configure (libgme) failed" }
        cmake --build (Join-Path $work 'bld')
        if ($LASTEXITCODE) { throw "cmake build (libgme) failed" }
        cmake --install (Join-Path $work 'bld')
        if ($LASTEXITCODE) { throw "cmake install (libgme) failed" }
    } finally {
        Remove-Item -Recurse -Force $work -ErrorAction SilentlyContinue
    }
}

Write-Host "==> Done. zlib: $ZlibDir  libgme: $GmeDir"
