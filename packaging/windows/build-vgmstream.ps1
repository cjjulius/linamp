<#
.SYNOPSIS
  Build libvgmstream (core-only) as a static library for the `vgmstream` feature on Windows (streamed
  game-engine audio: GameCube/Wii DSP, PS-ADPCM VAG, CRI HCA, Nintendo BRSTM/BCSTM/BFSTM, FMOD FSB and
  many more, via crate::vgmstream which binds the C API directly - no shim).

.DESCRIPTION
  The Windows counterpart to packaging/linux/build-vgmstream-min.sh. vgmstream has first-class CMake
  and MSVC support, so this is a straight CMake build. CORE-ONLY, like the Linux build: every external
  codec (ffmpeg/vorbis/mpeg/celt/speex) and every front-end (CLI/plugins) is disabled, which keeps the
  library fully permissive (ISC) and dependency-free (libc/libm only). The in-tree codecs
  (DSP/ADPCM/PS-ADPCM/HCA/ATRAC9/PCM) still cover the large majority of formats; a file whose payload
  needs an external codec just fails to open and is skipped.

  Built STATIC and linked into the exe, the same self-contained approach as AdPlug/SID - nothing ships
  beside the exe. On MSVC a static .lib is consumed the same as an import lib, so build.rs's
  `rustc-link-lib=dylib=vgmstream` links it unchanged (the static target's output, libvgmstream.lib, is
  installed as vgmstream.lib to match that name). build.rs links $VGMSTREAM_DIR\lib and compiles no
  shim; src/vgmstream.rs declares the C API itself, so no headers are needed at link time.

  Must run inside an MSVC developer environment (nmake + cl on PATH). CMake is required. Pinned to
  vgmstream r2168. Idempotent: skips when present.

.PARAMETER VgmstreamDir
  Install prefix. Default: <repo>\build\vgmstream-min (the VGMSTREAM_DIR build.rs defaults to).
#>
[CmdletBinding()]
param(
    [string]$VgmstreamDir
)
$ErrorActionPreference = 'Stop'

$VGMSTREAM_VERSION = 'r2168'
$VGMSTREAM_SHA256  = '6390ac616c4a248616f1b9bf6352fe949fcf2350a84844ba9416feac94f02225'

$repo = Resolve-Path (Join-Path $PSScriptRoot '..\..')
if (-not $VgmstreamDir) { $VgmstreamDir = Join-Path $repo 'build\vgmstream-min' }

if (Test-Path (Join-Path $VgmstreamDir 'lib\vgmstream.lib')) {
    Write-Host "==> libvgmstream already present at $VgmstreamDir"
    return
}

$gen = 'NMake Makefiles'
$work = New-Item -ItemType Directory -Force -Path (Join-Path ([IO.Path]::GetTempPath()) ("vgm-" + [Guid]::NewGuid()))
try {
    $tgz = Join-Path $work 'vgmstream.tar.gz'
    Write-Host "==> Fetching vgmstream $VGMSTREAM_VERSION"
    Invoke-WebRequest -Uri "https://github.com/vgmstream/vgmstream/archive/refs/tags/$VGMSTREAM_VERSION.tar.gz" -OutFile $tgz
    $got = (Get-FileHash -Algorithm SHA256 $tgz).Hash.ToLower()
    if ($got -ne $VGMSTREAM_SHA256.ToLower()) {
        throw "SHA256 mismatch for vgmstream`n  expected $VGMSTREAM_SHA256`n  got      $got"
    }
    tar -xf $tgz -C $work
    if ($LASTEXITCODE) { throw "extract vgmstream failed" }
    $src = Join-Path $work "vgmstream-$VGMSTREAM_VERSION"

    Write-Host "==> Building libvgmstream (core-only, static)"
    # Core-only: disable every external codec and front-end. We build the STATIC target
    # (add_library(libvgmstream STATIC ...) in src/CMakeLists.txt), so no export macros or DLLs are
    # involved. CMAKE_POLICY_VERSION_MINIMUM lets a modern CMake configure a tree whose minimum predates 3.5.
    # BUILD_WINAMP/BUILD_FB2K are the Windows front-ends (default ON on WIN32); fb2k hard-requires the
    # foobar2000 SDK, so both are disabled here just as the Linux build disables its CLI/plugins.
    cmake -S $src -B (Join-Path $work 'bld') -G $gen `
        "-DCMAKE_BUILD_TYPE=Release" `
        "-DBUILD_CLI=OFF" "-DBUILD_V123=OFF" "-DBUILD_AUDACIOUS=OFF" `
        "-DBUILD_WINAMP=OFF" "-DBUILD_FB2K=OFF" `
        "-DUSE_VORBIS=OFF" "-DUSE_MPEG=OFF" "-DUSE_FFMPEG=OFF" "-DUSE_CELT=OFF" "-DUSE_SPEEX=OFF" `
        "-DCMAKE_POLICY_VERSION_MINIMUM=3.5"
    if ($LASTEXITCODE) { throw "cmake configure failed" }
    cmake --build (Join-Path $work 'bld') --target libvgmstream
    if ($LASTEXITCODE) { throw "cmake build failed" }

    # Find the built static lib (libvgmstream.lib) wherever the generator placed it.
    $built = Get-ChildItem -Path (Join-Path $work 'bld') -Recurse -Filter 'libvgmstream.lib' | Select-Object -First 1
    if (-not $built) { throw "libvgmstream.lib not produced" }

    $lib = Join-Path $VgmstreamDir 'lib'
    $inc = Join-Path $VgmstreamDir 'include'
    New-Item -ItemType Directory -Force -Path $lib, $inc | Out-Null
    # Install as vgmstream.lib so build.rs's dylib=vgmstream links it.
    Copy-Item $built.FullName (Join-Path $lib 'vgmstream.lib') -Force
    # Headers are not needed at link time (the FFI is pure Rust), but install them for parity/reference.
    Copy-Item (Join-Path $src 'src\libvgmstream.h') $inc -Force
    Copy-Item (Join-Path $src 'src\libvgmstream_streamfile.h') $inc -Force
} finally {
    Remove-Item -Recurse -Force $work -ErrorAction SilentlyContinue
}

if (-not (Test-Path (Join-Path $VgmstreamDir 'lib\vgmstream.lib'))) {
    throw "vgmstream build incomplete: missing lib\vgmstream.lib under $VgmstreamDir"
}
Write-Host "==> Done. libvgmstream at $VgmstreamDir"
