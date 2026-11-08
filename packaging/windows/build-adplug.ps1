<#
.SYNOPSIS
  Build AdPlug (and its libbinio dependency) as shared libraries for the `adplug` feature on
  Windows (AdLib / OPL FM-synthesis music: DRO, CMF, RAD, HSC, LAA, Doom MUS and ~30 more via
  crate::adplug + src/adplug_shim.cpp).

.DESCRIPTION
  The Windows counterpart to packaging/linux/build-adplug-min.sh. Unlike Linux (which bundles
  shared libadplug.so + libbinio.so), Windows builds both as STATIC libraries and links them into
  the exe. The reason is specific to MSVC: AdPlug exposes its player registry as a static data
  member (CAdPlug::players), which factory() takes as a default argument. Referencing exported DLL
  *data* across a module boundary needs __declspec(dllimport), which AdPlug's headers do not carry,
  so a shared build leaves that symbol unresolved in the shim. Static linking sidesteps it entirely
  (the symbol resolves directly), makes the Windows build self-contained (no DLLs to ship), and is
  the conventional way to consume AdPlug on MSVC. Two wrinkles still have to be handled:

    * libbinio 1.5 ships no CMake/MSVC build (only autotools), so this generates its one configured
      header (binio.h, from binio.h.in) and a small CMakeLists that builds it as a static library,
      installed with a CMake config package exporting `libbinio::libbinio` (the target AdPlug links).
    * AdPlug 2.4 has native CMake support and finds libbinio through that config package.

  Everything installs into one prefix ($AdplugDir, default build\adplug-min) laid out as build.rs
  expects (on MSVC, `dylib=adplug`/`dylib=binio` consume a static .lib the same as an import lib):

    <AdplugDir>\lib\adplug.lib, lib\binio.lib   (static libraries: dylib=adplug, dylib=binio)
    <AdplugDir>\include\adplug\*.h              (the shim includes <adplug/adplug.h>)
    <AdplugDir>\include\libbinio\binio.h        (AdPlug headers include <binio.h>)

  build.rs compiles the C++ shim (cc crate, MSVC) and links the two static libraries straight into
  linamp.exe; nothing extra ships beside the exe (no installer bundling, unlike gme/openmpt). Must
  run inside an MSVC developer environment (nmake + cl on PATH), the same shell the FFmpeg/GME
  builds use. CMake is required. Pinned to AdPlug 2.4 / libbinio 1.5. Idempotent: skips when present.

.PARAMETER AdplugDir
  Install prefix for AdPlug + libbinio. Default: <repo>\build\adplug-min (the ADPLUG_DIR default).
#>
[CmdletBinding()]
param(
    [string]$AdplugDir
)
$ErrorActionPreference = 'Stop'

# Pinned to AdPlug 2.4 / libbinio 1.5 (the versions the Linux build uses). libbinio uses its
# release dist tarball (it carries src/ and binio.h.in, all we need). AdPlug, however, must come
# from the git source archive, not its release .tar.bz2: the release dist is an autotools `make
# dist` bundle that omits CMakeLists.txt, and we need AdPlug's CMake build on MSVC.
$BINIO_VERSION  = '1.5'
$BINIO_SHA256   = '398b2468e7838d2274d1f62dbc112e7e043433812f7ae63ef29f5cb31dc6defd'
$ADPLUG_VERSION = '2.4'
$ADPLUG_SHA256  = 'ebeaf8e2aacd3d19d756d4034cccaccfd2da5be465e633d1f463e3fc55baa065'

$repo = Resolve-Path (Join-Path $PSScriptRoot '..\..')
if (-not $AdplugDir) { $AdplugDir = Join-Path $repo 'build\adplug-min' }

if (Test-Path (Join-Path $AdplugDir 'lib\adplug.lib')) {
    Write-Host "==> AdPlug already present at $AdplugDir"
    return
}

function Get-Verified([string]$url, [string]$outFile, [string]$sha256) {
    Write-Host "==> Fetching $url"
    Invoke-WebRequest -Uri $url -OutFile $outFile
    $got = (Get-FileHash -Algorithm SHA256 $outFile).Hash.ToLower()
    if ($got -ne $sha256.ToLower()) {
        throw "SHA256 mismatch for $outFile`n  expected $sha256`n  got      $got"
    }
}

# Plain NMake build: single-config, no Visual Studio multi-config layout. nmake/cl come from the
# MSVC developer environment the caller set up.
$gen = 'NMake Makefiles'

$work = New-Item -ItemType Directory -Force -Path (Join-Path ([IO.Path]::GetTempPath()) ("adplug-" + [Guid]::NewGuid()))
try {
    # ---- libbinio ----
    if (Test-Path (Join-Path $AdplugDir 'lib\binio.lib')) {
      Write-Host "==> libbinio already present, skipping"
    } else {
    $bz = Join-Path $work 'libbinio.tar.bz2'
    Get-Verified "https://github.com/adplug/libbinio/releases/download/libbinio-$BINIO_VERSION/libbinio-$BINIO_VERSION.tar.bz2" $bz $BINIO_SHA256
    tar -xf $bz -C $work
    if ($LASTEXITCODE) { throw "extract libbinio failed" }
    $binioSrc = Join-Path $work "libbinio-$BINIO_VERSION"

    # Generate the one configured header. On MSVC/x64 (little-endian, full STL, math.h present) the
    # autoconf substitutions are fixed values; Int is the widest integer and Float is double.
    $binioH = Get-Content (Join-Path $binioSrc 'src\binio.h.in') -Raw
    $binioH = $binioH -replace '@ENABLE_STRING@', '1' `
                      -replace '@ENABLE_IOSTREAM@', '1' `
                      -replace '@ISO_STDLIB@', '1' `
                      -replace '@WITH_MATH@', '1' `
                      -replace '@TYPE_INT@', 'long long' `
                      -replace '@TYPE_FLOAT@', 'double'
    Set-Content -Path (Join-Path $binioSrc 'src\binio.h') -Value $binioH -Encoding ascii

    # A minimal CMake build for libbinio: a static library whose .lib is named `binio` (so
    # build.rs's dylib=binio links it), exporting the `libbinio::libbinio` target AdPlug links.
    $binioCMake = @'
cmake_minimum_required(VERSION 3.16)
project(libbinio CXX)
include(GNUInstallDirs)
add_library(libbinio STATIC
  src/binfile.cpp src/binio.cpp src/binstr.cpp src/binwrap.cpp)
set_target_properties(libbinio PROPERTIES
  OUTPUT_NAME binio
  POSITION_INDEPENDENT_CODE ON
  CXX_STANDARD 17
  CXX_STANDARD_REQUIRED ON)
target_include_directories(libbinio PUBLIC
  "$<BUILD_INTERFACE:${CMAKE_CURRENT_SOURCE_DIR}/src>"
  "$<INSTALL_INTERFACE:${CMAKE_INSTALL_INCLUDEDIR}/libbinio>")
install(TARGETS libbinio EXPORT libbinioTargets
  RUNTIME DESTINATION ${CMAKE_INSTALL_BINDIR}
  ARCHIVE DESTINATION ${CMAKE_INSTALL_LIBDIR}
  LIBRARY DESTINATION ${CMAKE_INSTALL_LIBDIR})
install(FILES src/binio.h src/binfile.h src/binstr.h src/binwrap.h
  DESTINATION ${CMAKE_INSTALL_INCLUDEDIR}/libbinio)
install(EXPORT libbinioTargets
  NAMESPACE libbinio::
  FILE libbinioConfig.cmake
  DESTINATION ${CMAKE_INSTALL_LIBDIR}/cmake/libbinio)
'@
    Set-Content -Path (Join-Path $binioSrc 'CMakeLists.txt') -Value $binioCMake -Encoding ascii

    Write-Host "==> Building libbinio (shared) -> $AdplugDir"
    cmake -S $binioSrc -B (Join-Path $work 'binio-bld') -G $gen `
        "-DCMAKE_BUILD_TYPE=Release" `
        "-DCMAKE_INSTALL_PREFIX=$AdplugDir"
    if ($LASTEXITCODE) { throw "cmake configure (libbinio) failed" }
    cmake --build (Join-Path $work 'binio-bld')
    if ($LASTEXITCODE) { throw "cmake build (libbinio) failed" }
    cmake --install (Join-Path $work 'binio-bld')
    if ($LASTEXITCODE) { throw "cmake install (libbinio) failed" }
    }

    # ---- AdPlug ----
    # GitHub source archive (git tree) so CMakeLists.txt is present; it extracts to adplug-adplug-<v>.
    $atgz = Join-Path $work 'adplug.tar.gz'
    Get-Verified "https://github.com/adplug/adplug/archive/refs/tags/adplug-$ADPLUG_VERSION.tar.gz" $atgz $ADPLUG_SHA256
    tar -xf $atgz -C $work
    if ($LASTEXITCODE) { throw "extract adplug failed" }
    $adplugSrc = Join-Path $work "adplug-adplug-$ADPLUG_VERSION"

    $adplugBld = Join-Path $work 'adplug-bld'
    Write-Host "==> Building AdPlug (shared, against libbinio) -> $AdplugDir"
    # CMAKE_PREFIX_PATH points at the prefix so find_package(libbinio CONFIG) resolves the package
    # just installed above. Notes on the flags:
    #  * BUILD_SHARED_LIBS=OFF: static build (see the header comment) so CAdPlug::players resolves
    #    directly in the shim, no DLL data-export dance.
    #  * adplug_INCLUDE_PACKAGING=ON: AdPlug's install(TARGETS) rules live in packaging/ (via the
    #    reinking_package macro), so install does nothing with packaging off. include(CPack) in that
    #    subdir only defines cpack targets; `cmake --install` never runs them.
    #  * tests and precompiled headers off (we only need the library).
    cmake -S $adplugSrc -B $adplugBld -G $gen `
        "-DCMAKE_BUILD_TYPE=Release" `
        "-DBUILD_SHARED_LIBS=OFF" `
        "-Dadplug_INCLUDE_TEST=OFF" `
        "-Dadplug_INCLUDE_PACKAGING=ON" `
        "-DADPLUG_PRECOMPILED_HEADERS=OFF" `
        "-DCMAKE_PREFIX_PATH=$AdplugDir" `
        "-DCMAKE_INSTALL_PREFIX=$AdplugDir"
    if ($LASTEXITCODE) { throw "cmake configure (adplug) failed" }
    cmake --build $adplugBld
    if ($LASTEXITCODE) { throw "cmake build (adplug) failed" }
    cmake --install $adplugBld
    if ($LASTEXITCODE) { throw "cmake install (adplug) failed" }

    # AdPlug's install rules ship the library + CMake config but not the public headers (they are
    # only staged into the build tree for consumers that use it in-place). Copy the source headers
    # into the prefix so build.rs's shim can include <adplug/adplug.h>. Copying all of src/*.h is a
    # harmless superset of the public set; the shim only pulls in adplug.h and its transitive
    # includes (none of which need the generated version.h).
    $dstInc = Join-Path $AdplugDir 'include\adplug'
    New-Item -ItemType Directory -Force -Path $dstInc | Out-Null
    Copy-Item (Join-Path $adplugSrc 'src\*.h') $dstInc -Force
} finally {
    Remove-Item -Recurse -Force $work -ErrorAction SilentlyContinue
}

# Verify the prefix has what build.rs needs (static libs + headers; no DLLs for a static build).
foreach ($rel in @('lib\adplug.lib', 'lib\binio.lib', 'include\adplug\adplug.h', 'include\libbinio\binio.h')) {
    if (-not (Test-Path (Join-Path $AdplugDir $rel))) {
        throw "AdPlug build incomplete: missing $rel under $AdplugDir"
    }
}
Write-Host "==> Done. AdPlug + libbinio at $AdplugDir"
