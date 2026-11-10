<#
.SYNOPSIS
  Build libvgmstream as a static library for the `vgmstream` feature on Windows (streamed game-engine
  audio: GameCube/Wii DSP, PS-ADPCM VAG, CRI HCA, Nintendo BRSTM/BCSTM/BFSTM, FMOD FSB and many more,
  via crate::vgmstream which binds the C API directly - no shim). With -Ext it builds the external-codec
  tier for the `vgmstream-ext` feature (adds Wwise/FSB Vorbis + XMA/ATRAC3/WMA).

.DESCRIPTION
  The Windows counterpart to packaging/linux/build-vgmstream-min.sh. vgmstream has first-class CMake and
  MSVC support, so this is a straight CMake build of the STATIC libvgmstream target (linked into the exe,
  the same self-contained approach as AdPlug/SID; build.rs's `rustc-link-lib=dylib=vgmstream` consumes
  the static lib). src/vgmstream.rs declares the C API itself, so no headers are needed at link time.

  Two modes:
  * core-only (default): every external codec and front-end disabled. Fully permissive (ISC),
    dependency-free (libc/libm only). In-tree codecs (DSP/ADPCM/PS-ADPCM/HCA/ATRAC9/PCM) still cover the
    large majority of formats; a payload needing an external codec just fails to open and is skipped.
  * external tier (-Ext, the `vgmstream-ext` feature): USE_VORBIS + USE_FFMPEG on, so it also decodes
    Wwise .wem / FMOD FSB Vorbis and Xbox 360 XMA / PS3-PSP ATRAC3 / WMA. The Linux build gates this on
    VGM_EXT=1; here it is the -Ext switch, and a `.ext` marker records which codec set the installed lib
    has so a later call with the other mode rebuilds instead of skipping.

  How the externals link on Windows (the key design point): the STATIC libvgmstream target is set up by
  vgmstream's setup_target() with LINK=FALSE, so USE_VORBIS/USE_FFMPEG only switch on the VGM_USE_VORBIS/
  VGM_USE_FFMPEG compile defines and the include dirs - NO target_link_libraries, no ext_libs/*.lib. The
  vorbis and ffmpeg decoder sources compile (vorbis against vgmstream's bundled ext_includes/vorbis
  headers; ffmpeg against the GyanD headers found below), and every external symbol stays unresolved in
  vgmstream.lib until the final linamp.exe link. There, build.rs links libvorbis/libogg (static, from
  LIBVORBIS_DIR) for Vorbis, and the ffmpeg symbols resolve against the SAME avcodec/avformat/avutil the
  `ffmpeg` feature already links and ships (ffmpeg-next) - one ffmpeg, not a second copy.

  find_package(FFmpeg) is pointed at the existing GyanD shared dev build (FFMPEG_DIR) via
  CMAKE_PREFIX_PATH, so (a) it is not auto-disabled (vgmstream's ffmpeg.cmake turns USE_FFMPEG off on
  Windows if it finds nothing) and (b) the ffmpeg code compiles against the exact avcodec version
  (5.1 / avcodec 59) it links at the exe. libvorbis must be built first (build-libvorbis.ps1).

  Must run inside an MSVC developer environment (nmake + cl on PATH). CMake required. Pinned to vgmstream
  r2168. Idempotent per codec set (the `.ext` marker).

.PARAMETER VgmstreamDir
  Install prefix. Default: <repo>\build\vgmstream-min (the VGMSTREAM_DIR build.rs defaults to).
.PARAMETER Ext
  Build the external-codec tier (USE_VORBIS + USE_FFMPEG) for the vgmstream-ext feature.
.PARAMETER LibvorbisDir
  libvorbis prefix (lib\vorbis.lib + include\vorbis). Default: <repo>\build\libvorbis-min. -Ext only.
.PARAMETER FfmpegDir
  FFmpeg prefix with include\ + lib\ (the GyanD shared dev build). Default: $env:FFMPEG_DIR. -Ext only.
#>
[CmdletBinding()]
param(
    [string]$VgmstreamDir,
    [switch]$Ext,
    [string]$LibvorbisDir,
    [string]$FfmpegDir
)
$ErrorActionPreference = 'Stop'

# Download with retries. Portable across Windows PowerShell 5.1 and pwsh 7 (CI runs under pwsh, but a
# local dev shell may be 5.1, whose Invoke-WebRequest lacks -MaximumRetryCount).
function Invoke-Download([string]$Uri, [string]$OutFile) {
    for ($i = 1; $i -le 5; $i++) {
        try { Invoke-WebRequest -Uri $Uri -OutFile $OutFile; return }
        catch { if ($i -ge 5) { throw }; Start-Sleep -Seconds 5 }
    }
}

$VGMSTREAM_VERSION = 'r2168'
$VGMSTREAM_SHA256  = '6390ac616c4a248616f1b9bf6352fe949fcf2350a84844ba9416feac94f02225'

$repo = Resolve-Path (Join-Path $PSScriptRoot '..\..')
if (-not $VgmstreamDir)  { $VgmstreamDir  = Join-Path $repo 'build\vgmstream-min' }
if (-not $LibvorbisDir)  { $LibvorbisDir  = Join-Path $repo 'build\libvorbis-min' }
if (-not $FfmpegDir)     { $FfmpegDir     = $env:FFMPEG_DIR }

$extMarker = Join-Path $VgmstreamDir '.ext'
$haveLib = Test-Path (Join-Path $VgmstreamDir 'lib\vgmstream.lib')
$haveExt = Test-Path $extMarker
if ($haveLib -and ([bool]$Ext -eq $haveExt)) {
    Write-Host "==> libvgmstream already present at $VgmstreamDir (ext=$([bool]$Ext))"
    return
}
if ($haveLib) {
    Write-Host "==> rebuilding libvgmstream at $VgmstreamDir for a different codec set (ext=$([bool]$Ext))"
    Remove-Item (Join-Path $VgmstreamDir 'lib\vgmstream.lib') -ErrorAction SilentlyContinue
    Remove-Item $extMarker -ErrorAction SilentlyContinue
}

if ($Ext) {
    if (-not $FfmpegDir -or -not (Test-Path (Join-Path $FfmpegDir 'lib\avcodec.lib'))) {
        throw "FFmpeg not found (FfmpegDir=$FfmpegDir). -Ext needs the GyanD FFmpeg dev build (set FFMPEG_DIR)."
    }
    if (-not (Test-Path (Join-Path $LibvorbisDir 'lib\vorbis.lib'))) {
        throw "libvorbis not found at $LibvorbisDir - run packaging\windows\build-libvorbis.ps1 first."
    }
}

$gen = 'NMake Makefiles'
$work = New-Item -ItemType Directory -Force -Path (Join-Path ([IO.Path]::GetTempPath()) ("vgm-" + [Guid]::NewGuid()))
try {
    $tgz = Join-Path $work 'vgmstream.tar.gz'
    Write-Host "==> Fetching vgmstream $VGMSTREAM_VERSION"
    Invoke-Download "https://github.com/vgmstream/vgmstream/archive/refs/tags/$VGMSTREAM_VERSION.tar.gz" $tgz
    $got = (Get-FileHash -Algorithm SHA256 $tgz).Hash.ToLower()
    if ($got -ne $VGMSTREAM_SHA256.ToLower()) {
        throw "SHA256 mismatch for vgmstream`n  expected $VGMSTREAM_SHA256`n  got      $got"
    }
    # Windows' own bsdtar (not a Git GNU tar that may be first on PATH, which reads a C:\ path as a host).
    & "$env:SystemRoot\System32\tar.exe" -xf $tgz -C $work
    if ($LASTEXITCODE) { throw "extract vgmstream failed" }
    $src = Join-Path $work "vgmstream-$VGMSTREAM_VERSION"

    # Common flags: STATIC libvgmstream, no front-ends. BUILD_WINAMP/BUILD_FB2K default ON on WIN32;
    # fb2k hard-requires the foobar2000 SDK, so disable both. CMAKE_POLICY_VERSION_MINIMUM lets a modern
    # CMake configure a tree whose minimum predates 3.5.
    $flags = @(
        "-DCMAKE_BUILD_TYPE=Release",
        "-DBUILD_CLI=OFF", "-DBUILD_V123=OFF", "-DBUILD_AUDACIOUS=OFF",
        "-DBUILD_WINAMP=OFF", "-DBUILD_FB2K=OFF",
        "-DUSE_MPEG=OFF", "-DUSE_CELT=OFF", "-DUSE_SPEEX=OFF",
        "-DCMAKE_POLICY_VERSION_MINIMUM=3.5"
    )
    if ($Ext) {
        Write-Host "==> Building libvgmstream (external tier: Vorbis + FFmpeg, static)"
        # USE_VORBIS compiles the Vorbis code against vgmstream's bundled ext_includes/vorbis headers
        # (links libvorbis at the exe). USE_FFMPEG: CMAKE_PREFIX_PATH points find_package(FFmpeg) at the
        # GyanD build so it is found (not auto-disabled) and compiles against avcodec 59 - the ffmpeg
        # symbols link at the exe against the same ffmpeg the `ffmpeg` feature ships.
        $flags += @(
            "-DUSE_VORBIS=ON", "-DUSE_FFMPEG=ON",
            "-DCMAKE_PREFIX_PATH=$FfmpegDir;$LibvorbisDir",
            # vgmstream's FindFFmpeg fills <comp>_VERSION only from pkg-config, which is absent on
            # Windows, so its guard `if(${AVCODEC_VERSION} VERSION_LESS 57)` chokes on an empty value even
            # though the libs are found. Pre-seed the cache (FindFFmpeg's own set is not FORCE, so it
            # won't overwrite these) with the pinned GyanD FFmpeg 5.1 major versions.
            "-DAVCODEC_VERSION=59", "-DAVUTIL_VERSION=57", "-DSWRESAMPLE_VERSION=4", "-DAVFORMAT_VERSION=59"
        )
    } else {
        Write-Host "==> Building libvgmstream (core-only, static)"
        $flags += @("-DUSE_VORBIS=OFF", "-DUSE_FFMPEG=OFF")
    }

    cmake -S $src -B (Join-Path $work 'bld') -G $gen @flags
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
    Copy-Item (Join-Path $src 'src\libvgmstream.h') $inc -Force
    Copy-Item (Join-Path $src 'src\libvgmstream_streamfile.h') $inc -Force
    if ($Ext) { New-Item -ItemType File -Force -Path $extMarker | Out-Null }
} finally {
    Remove-Item -Recurse -Force $work -ErrorAction SilentlyContinue
}

if (-not (Test-Path (Join-Path $VgmstreamDir 'lib\vgmstream.lib'))) {
    throw "vgmstream build incomplete: missing lib\vgmstream.lib under $VgmstreamDir"
}
Write-Host "==> Done. libvgmstream at $VgmstreamDir (ext=$([bool]$Ext))"
