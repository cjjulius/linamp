<#
.SYNOPSIS
  Build libgsf as a shared library for the `gsf` feature on Windows (Game Boy Advance GSF music, via
  crate::gsf + src/gsf_shim.c). Mirrors packaging/linux/build-libgsf-min.sh.

.DESCRIPTION
  libgsf (chrg, MPL-2.0) is a small GSF player library that statically bundles a trimmed mGBA (also
  MPL-2.0) and exposes a clean high-level C API (gsf_new/gsf_load_file/gsf_play/gsf_seek_samples/...). It
  plays ROM-less: mGBA's built-in HLE BIOS stands in for the GBA BIOS (useBios=false), so no copyrighted
  Nintendo BIOS is needed or shipped; the game's audio program lives inside the .gsf/.minigsf file, which
  libgsf parses itself (it does NOT use psflib - it resolves the `_lib` overlay chain and PSF tags
  internally). Unlike the other *SF cores, there is no hand-compilable source list: mGBA is consumed
  through its own CMake, so this build is CMake-driven and needs a C++20 compiler - the one build-tooling
  difference in the *SF family.

  The upstream CMake has first-class MSVC support: external/CMakeLists.txt globs the Windows platform
  sources, defines _WIN32_WINNT, sets CMAKE_WINDOWS_EXPORT_ALL_SYMBOLS (so the DLL exports its API and an
  import lib is produced), adds NOMINMAX/WIN32_LEAN_AND_MEAN/_UNICODE under MSVC, and links ws2_32 +
  shlwapi. All the mGBA sources compile straight into the one libgsf target, so the only outputs are
  gsf.dll + its import lib. The target is named `libgsf` with OUTPUT_NAME `gsf`, so on Windows CMake emits
  `gsf.dll` + `gsf.lib` (no `lib` prefix), exactly the name build.rs's `rustc-link-lib=dylib=gsf` wants.

  Unlike the static *SF cores, libgsf ships as a DLL (the gme/openmpt pattern): build.rs links the import
  lib gsf.lib and the runtime loads gsf.dll, which must sit beside the exe (CI puts LIBGSF_DIR\bin on PATH
  for the tests; the installer bundles gsf.dll via /DWithGsf). libgsf depends on zlib: find_package(ZLIB)
  is pointed at the static-zlib build from build-gme.ps1 and linked dynamically, so gsf.dll imports
  zlib.dll - which is already bundled (gme.dll needs it too) and already on PATH. We also install the
  static zlib as z.lib so build.rs's unconditional `dylib=z` resolves on the LIBGSF_DIR\lib search path,
  the same stand-in for system -lz that build-psflib.ps1 uses. Install layout (LIBGSF_DIR, the prefix
  build.rs defaults to):

    <LibgsfDir>\lib\gsf.lib    (import lib: GSF player + trimmed mGBA)
    <LibgsfDir>\lib\z.lib      (static zlib, for dylib=z)
    <LibgsfDir>\bin\gsf.dll    (runtime; bundled beside the exe)
    <LibgsfDir>\include\gsf.h  (the shim includes <gsf.h>)

  Pinned by git commit (libgsf) + the mGBA submodule commit libgsf locks (asserted). Must run inside an
  MSVC developer environment (cl + nmake on PATH), AFTER build-gme.ps1 (for the static zlib + zlib.dll).
  Needs git + cmake (>= 3.15). Idempotent: skips when present.

.PARAMETER LibgsfDir
  Install prefix. Default: <repo>\build\libgsf-min (the LIBGSF_DIR default).
.PARAMETER ZlibDir
  zlib prefix with lib\zlib.lib + lib\zlibstatic.lib + include\zlib.h. Default: <repo>\build\zlib.
#>
[CmdletBinding()]
param(
    [string]$LibgsfDir,
    [string]$ZlibDir
)
$ErrorActionPreference = 'Stop'

# chrg/libgsf pinned commit, and the mGBA commit its submodule gitlink points at (asserted below),
# matching the Linux build.
$LIBGSF_COMMIT = 'df71d7142c79775e3df2e2d0416779177e9509f0'
$MGBA_COMMIT   = '11e16d1ad9fe8b7140312475afee60d4c7aa9742'

$repo = Resolve-Path (Join-Path $PSScriptRoot '..\..')
if (-not $LibgsfDir) { $LibgsfDir = Join-Path $repo 'build\libgsf-min' }
if (-not $ZlibDir)   { $ZlibDir   = Join-Path $repo 'build\zlib' }

if (Test-Path (Join-Path $LibgsfDir 'lib\gsf.lib')) {
    Write-Host "==> libgsf already present at $LibgsfDir"
    return
}

$zlibInc = Join-Path $ZlibDir 'include'
$zlibImportLib = Join-Path $ZlibDir 'lib\zlib.lib'
$zlibStatic    = Join-Path $ZlibDir 'lib\zlibstatic.lib'
if (-not (Test-Path $zlibImportLib) -or -not (Test-Path (Join-Path $zlibInc 'zlib.h'))) {
    throw "zlib not found at $ZlibDir - run packaging\windows\build-gme.ps1 first (it builds zlib)."
}

$gen  = 'NMake Makefiles'
$work = New-Item -ItemType Directory -Force -Path (Join-Path ([IO.Path]::GetTempPath()) ("libgsf-" + [Guid]::NewGuid()))
try {
    $src = Join-Path $work 'src'
    Write-Host "==> Fetching libgsf (codeberg.org/chrg/libgsf @ $LIBGSF_COMMIT)"
    git clone --quiet https://codeberg.org/chrg/libgsf.git $src
    if ($LASTEXITCODE) { throw "git clone libgsf failed" }
    git -C $src checkout --quiet $LIBGSF_COMMIT
    if ($LASTEXITCODE) { throw "git checkout libgsf failed" }
    Write-Host "==> Fetching the pinned mGBA submodule"
    git -C $src submodule update --init --depth 1 external/mgba
    if ($LASTEXITCODE) { throw "git submodule update (mgba) failed" }
    $gotMgba = (git -C (Join-Path $src 'external\mgba') rev-parse HEAD).Trim()
    if ($gotMgba -ne $MGBA_COMMIT) {
        throw "mGBA submodule is $gotMgba, expected $MGBA_COMMIT"
    }

    Write-Host "==> Building libgsf (CMake/$gen, shared, ROM-less HLE-BIOS GBA, C++20)"
    $bld = Join-Path $work 'bld'
    # SHARED by default; BUILD_EXAMPLES=OFF drops the SDL2 example so only zlib is needed. Point
    # find_package(ZLIB) at the build-gme.ps1 zlib (dynamic import lib -> gsf.dll imports zlib.dll, which
    # is already bundled). CMAKE_POLICY_VERSION_MINIMUM lets a modern CMake configure mGBA's older tree.
    cmake -S $src -B $bld -G $gen `
        "-DCMAKE_BUILD_TYPE=Release" `
        "-DBUILD_EXAMPLES=OFF" `
        "-DZLIB_INCLUDE_DIR=$zlibInc" `
        "-DZLIB_LIBRARY=$zlibImportLib" `
        "-DCMAKE_POLICY_VERSION_MINIMUM=3.5"
    if ($LASTEXITCODE) { throw "cmake configure failed" }
    cmake --build $bld
    if ($LASTEXITCODE) { throw "cmake build failed" }

    # Find the built DLL + import lib wherever the generator placed them.
    $dll = Get-ChildItem -Path $bld -Recurse -Filter 'gsf.dll' | Select-Object -First 1
    $implib = Get-ChildItem -Path $bld -Recurse -Filter 'gsf.lib' | Select-Object -First 1
    if (-not $dll)    { throw "gsf.dll not produced" }
    if (-not $implib) { throw "gsf.lib (import lib) not produced" }

    $outLib = Join-Path $LibgsfDir 'lib'
    $outBin = Join-Path $LibgsfDir 'bin'
    $outInc = Join-Path $LibgsfDir 'include'
    New-Item -ItemType Directory -Force -Path $outLib, $outBin, $outInc | Out-Null
    Copy-Item $implib.FullName (Join-Path $outLib 'gsf.lib') -Force
    Copy-Item $dll.FullName    (Join-Path $outBin 'gsf.dll') -Force
    Copy-Item (Join-Path $src 'include\gsf.h') $outInc -Force
    # Install the static zlib as z.lib so build.rs's `dylib=z` resolves on LIBGSF_DIR\lib (the Windows
    # stand-in for system -lz), the same trick as build-psflib.ps1.
    Copy-Item $zlibStatic (Join-Path $outLib 'z.lib') -Force
} finally {
    Remove-Item -Recurse -Force $work -ErrorAction SilentlyContinue
}

foreach ($rel in @('lib\gsf.lib', 'lib\z.lib', 'bin\gsf.dll', 'include\gsf.h')) {
    if (-not (Test-Path (Join-Path $LibgsfDir $rel))) {
        throw "libgsf build incomplete: missing $rel under $LibgsfDir"
    }
}
Write-Host "==> Done. libgsf at $LibgsfDir"
