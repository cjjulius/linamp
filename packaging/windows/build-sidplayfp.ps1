<#
.SYNOPSIS
  Build libsidplayfp as a static library for the `sid` feature on Windows (Commodore 64 SID music,
  .sid PSID/RSID via crate::sid + src/sid_shim.cpp).

.DESCRIPTION
  The Windows counterpart to packaging/linux/build-sidplayfp-min.sh. libsidplayfp 2.x bundles the
  reSIDfp engine in-tree (one library, no separate libresidfp), and src/sid_shim.cpp is written to
  the 2.x API (SidConfig::playback/MONO, SidInfo::maxsids(), ReSIDfpBuilder::create(n)/getStatus(),
  sidplayfp::play(short*, uint)). This is pinned to 2.16.1 to match that shim. (libsidplayfp 3.x
  split reSIDfp into a separate libresidfp package and changed that API, so it does not build against
  the shim.)

  libsidplayfp has no CMake/MSVC build upstream (autotools only), so this generates a small
  CMakeLists plus the configured headers (config.h, the reSIDfp siddefs-fp.h, sidversion.h). No
  build-time code generation is needed: the release tarball already ships the xa65-assembled PSID
  driver bytes (psiddrv.bin, sidplayer1/2.bin), and reSIDfp computes its tables at runtime. libgcrypt
  is not used (tune-MD5 for the HVSC song-length DB, which we do not ship); the bundled MD5 is built
  instead. The old resid engine and the hardware builders (exsid/usbsid/hardsid/sidlite) and STILview
  are not compiled.

  Built STATIC and linked into the exe, same self-contained approach as AdPlug (nothing ships beside
  the exe). On MSVC a static .lib is consumed the same as an import lib, so build.rs's
  `rustc-link-lib=dylib=sidplayfp` links it unchanged. Install layout:

    <SidplayfpDir>\lib\sidplayfp.lib
    <SidplayfpDir>\include\sidplayfp\*.h          (the shim includes <sidplayfp/sidplayfp.h> etc.)
    <SidplayfpDir>\include\sidplayfp\builders\residfp.h

  Must run inside an MSVC developer environment (nmake + cl on PATH). CMake is required.
  Idempotent: skips when present.

.PARAMETER SidplayfpDir
  Install prefix. Default: <repo>\build\sidplayfp-min (the SIDPLAYFP_DIR build.rs defaults to).
#>
[CmdletBinding()]
param(
    [string]$SidplayfpDir
)
$ErrorActionPreference = 'Stop'

$SIDPLAYFP_VERSION = '2.16.1'
$SIDPLAYFP_SHA256  = 'ace0f73c2ef8645ab069ce1b298b10e31e36af7b5996109983b2b67ad60ff3ca'

$repo = Resolve-Path (Join-Path $PSScriptRoot '..\..')
if (-not $SidplayfpDir) { $SidplayfpDir = Join-Path $repo 'build\sidplayfp-min' }

if (Test-Path (Join-Path $SidplayfpDir 'lib\sidplayfp.lib')) {
    Write-Host "==> libsidplayfp already present at $SidplayfpDir"
    return
}

$gen = 'NMake Makefiles'
$work = New-Item -ItemType Directory -Force -Path (Join-Path ([IO.Path]::GetTempPath()) ("sid-" + [Guid]::NewGuid()))
try {
    $tgz = Join-Path $work 'libsidplayfp.tar.gz'
    Write-Host "==> Fetching libsidplayfp $SIDPLAYFP_VERSION"
    Invoke-WebRequest -Uri "https://github.com/libsidplayfp/libsidplayfp/releases/download/v$SIDPLAYFP_VERSION/libsidplayfp-$SIDPLAYFP_VERSION.tar.gz" -OutFile $tgz
    $got = (Get-FileHash -Algorithm SHA256 $tgz).Hash.ToLower()
    if ($got -ne $SIDPLAYFP_SHA256.ToLower()) {
        throw "SHA256 mismatch for libsidplayfp`n  expected $SIDPLAYFP_SHA256`n  got      $got"
    }
    tar -xf $tgz -C $work
    if ($LASTEXITCODE) { throw "extract libsidplayfp failed" }
    $src = Join-Path $work "libsidplayfp-$SIDPLAYFP_VERSION"

    # config.h: the autoconf defines, fixed for MSVC/x64. libgcrypt/strings.h/strcasecmp/unistd/
    # pthread/exsid/usbsid and SIMD RUNTIME_DISPATCH are left off (bundled MD5, stringutils.h takes
    # the _WIN32 path, no hardware backends, scalar reSIDfp).
    $configH = @'
#ifndef LIBSIDPLAYFP_CONFIG_H
#define LIBSIDPLAYFP_CONFIG_H
#define PACKAGE "libsidplayfp"
#define PACKAGE_NAME "libsidplayfp"
#define PACKAGE_TARNAME "libsidplayfp"
#define PACKAGE_VERSION "2.16.1"
#define PACKAGE_STRING "libsidplayfp 2.16.1"
#define PACKAGE_BUGREPORT ""
#define PACKAGE_URL ""
#define VERSION "2.16.1"
#define STDC_HEADERS 1
#define HAVE_STDIO_H 1
#define HAVE_STDLIB_H 1
#define HAVE_STRING_H 1
#define HAVE_STDINT_H 1
#define HAVE_INTTYPES_H 1
#define HAVE_SYS_TYPES_H 1
#define HAVE_SYS_STAT_H 1
#define SIZEOF_INT 4
#define SIZEOF_SHORT 2
#define HAVE_CXX11 1
#define HAVE_CXX14 1
#define HAVE_CXX17 1
#endif
'@
    Set-Content (Join-Path $src 'src\config.h') $configH -Encoding ascii

    # reSIDfp engine compiler knobs (siddefs-fp.h): no __builtin_expect on MSVC, inlining on.
    $rfh = Join-Path $src 'src\builders\residfp-builder\residfp\siddefs-fp.h'
    $sd = Get-Content ($rfh + '.in') -Raw
    $sd = $sd -replace '@RESID_BRANCH_HINTS@', '0' `
              -replace '@HAVE_BUILTIN_EXPECT@', '0' `
              -replace '@RESID_INLINING@', '1' `
              -replace '@RESID_INLINE@', 'inline' `
              -replace '@PACKAGE_VERSION@', $SIDPLAYFP_VERSION
    Set-Content $rfh $sd -Encoding ascii

    # sidversion.h (ABI version 2.16.1).
    $sv = Get-Content (Join-Path $src 'src\sidplayfp\sidversion.h.in') -Raw
    $sv = $sv -replace '@LIB_MAJOR@', '2' -replace '@LIB_MINOR@', '16' -replace '@LIB_LEVEL@', '1'
    Set-Content (Join-Path $src 'src\sidplayfp\sidversion.h') $sv -Encoding ascii

    # Single static lib: the core player + the in-tree reSIDfp engine. Exclude the old resid engine,
    # the hardware builders, STILview, and the resampler test program.
    $cmake = @'
cmake_minimum_required(VERSION 3.16)
project(sidplayfp CXX)
file(GLOB_RECURSE SRC "${CMAKE_CURRENT_SOURCE_DIR}/src/*.cpp")
list(FILTER SRC EXCLUDE REGEX "builders/(resid-builder|exsid-builder|usbsid-builder|hardsid-builder|sidlite-builder)/")
list(FILTER SRC EXCLUDE REGEX "utils/STILview/")
list(FILTER SRC EXCLUDE REGEX "resample/test\\.cpp")
add_library(sidplayfp STATIC ${SRC})
target_compile_definitions(sidplayfp PRIVATE HAVE_CONFIG_H)
target_include_directories(sidplayfp PRIVATE
  "${CMAKE_CURRENT_SOURCE_DIR}/src"
  "${CMAKE_CURRENT_SOURCE_DIR}/src/builders/residfp-builder"
  "${CMAKE_CURRENT_SOURCE_DIR}/src/builders/residfp-builder/residfp")
set_target_properties(sidplayfp PROPERTIES CXX_STANDARD 17 CXX_STANDARD_REQUIRED ON)
if(MSVC)
  # /Zc:__cplusplus so __cplusplus reports the real standard (siddefs-fp.h errors out otherwise).
  target_compile_options(sidplayfp PRIVATE /utf-8 /EHsc /Zc:__cplusplus)
endif()
'@
    Set-Content (Join-Path $src 'CMakeLists.txt') $cmake -Encoding ascii

    Write-Host "==> Building libsidplayfp (static, with bundled reSIDfp)"
    cmake -S $src -B (Join-Path $work 'bld') -G $gen "-DCMAKE_BUILD_TYPE=Release"
    if ($LASTEXITCODE) { throw "cmake configure failed" }
    cmake --build (Join-Path $work 'bld')
    if ($LASTEXITCODE) { throw "cmake build failed" }
    $built = Join-Path $work 'bld\sidplayfp.lib'
    if (-not (Test-Path $built)) { throw "sidplayfp.lib not produced" }

    # Install lib + the public headers the shim includes.
    $lib = Join-Path $SidplayfpDir 'lib'
    $incB = Join-Path $SidplayfpDir 'include\sidplayfp\builders'
    New-Item -ItemType Directory -Force -Path $lib, $incB | Out-Null
    Copy-Item $built (Join-Path $lib 'sidplayfp.lib') -Force
    Copy-Item (Join-Path $src 'src\sidplayfp\*.h') (Join-Path $SidplayfpDir 'include\sidplayfp') -Force
    Copy-Item (Join-Path $src 'src\builders\residfp-builder\residfp.h') (Join-Path $incB 'residfp.h') -Force
} finally {
    Remove-Item -Recurse -Force $work -ErrorAction SilentlyContinue
}

foreach ($rel in @('lib\sidplayfp.lib', 'include\sidplayfp\sidplayfp.h', 'include\sidplayfp\builders\residfp.h')) {
    if (-not (Test-Path (Join-Path $SidplayfpDir $rel))) {
        throw "libsidplayfp build incomplete: missing $rel under $SidplayfpDir"
    }
}
Write-Host "==> Done. libsidplayfp at $SidplayfpDir"
