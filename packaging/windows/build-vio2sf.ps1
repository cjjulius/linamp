<#
.SYNOPSIS
  Build vio2sf as a static library for the `twosf` feature on Windows (Nintendo DS 2SF music, via
  crate::twosf + src/twosf_shim.cpp). Mirrors packaging/linux/build-vio2sf-min.sh.

.DESCRIPTION
  vio2sf (kode54, GPL-2.0-or-later) is a DeSmuME-derived Nintendo DS emulator cut down to just what 2SF
  playback needs: the ARM7 + ARM9 CPUs (pure interpreters), the MMU, and the SPU. It runs the game's
  actual ARM sound driver, so it covers DS titles whose audio cannot be cleanly extracted to NCSF. It
  plays ROM-less - DeSmuME's HLE BIOS SWI tables stand in for the DS BIOS and a blank in-memory
  firmware-settings block is synthesized, so no copyrighted DS BIOS or firmware image is ever needed or
  shipped; the game ROM + save state live inside the .2sf/.mini2sf file, parsed by the shared psflib
  (which resolves `_lib` chains). psflib is built separately by build-psflib.ps1 and shared across the
  SSF/QSF/NCSF/USF/2SF features. It has no standalone release, so we build it from the Kodi
  audiodecoder.2sf source tree, pinned to the same commit as the Linux build.

  The desmume sources carry first-class MSVC support: types.h has an `_MSC_VER` path (INLINE -> _inline)
  and no `__attribute__`/`__builtin_` is used in the compiled files, so they build with cl unchanged. We
  compile the 15 .c files as C (they rely on C linkage - the shim includes <vio2sf/state.h> under
  `extern "C"`) and SPU.cpp as C++. resampler.c hardcodes `#define RESAMPLER_SSE` and includes
  <xmmintrin.h>, so its SSE path is always taken; on MSVC x64 SSE2 is on by default and needs no flag
  (the Linux build's `-msse` is a GCC-only switch to enable the same intrinsics). Source list mirrors
  lib/vio2sf/CMakeLists.txt and build-vio2sf-min.sh.

  Built STATIC and linked into the exe (nothing ships beside it), the same approach as the other native
  features. On MSVC a static .lib is consumed the same as an import lib, so build.rs's
  `rustc-link-lib=dylib=vio2sf` links it unchanged. build.rs compiles the C++ shim (src/twosf_shim.cpp)
  against the installed headers and psflib's. The shim also includes <zlib.h> (it zlib-inflates the 2SF
  save-state overlays) and links `dylib=z`, so we install the static zlib's zlib.h/zconf.h next to the
  vio2sf headers (the z symbols themselves come from psflib's z.lib at the final link). The desmume
  headers install under a vio2sf\ subdir because the shim includes <vio2sf/state.h> (which pulls in its
  siblings). Install layout (VIO2SF_DIR, the prefix build.rs defaults to):

    <Vio2sfDir>\lib\vio2sf.lib          (static: ARM7/ARM9 + MMU + SPU)
    <Vio2sfDir>\include\vio2sf\*.h       (the shim includes <vio2sf/state.h>)
    <Vio2sfDir>\include\zlib.h           (+ zconf.h; for the shim's <zlib.h>)

  Must run inside an MSVC developer environment (cl + lib on PATH), AFTER build-gme.ps1 (for the static
  zlib's headers). Idempotent: skips when present.

.PARAMETER Vio2sfDir
  Install prefix. Default: <repo>\build\vio2sf-min (the VIO2SF_DIR default).
.PARAMETER ZlibDir
  zlib prefix with include\zlib.h + zconf.h. Default: <repo>\build\zlib (from build-gme.ps1).
#>
[CmdletBinding()]
param(
    [string]$Vio2sfDir,
    [string]$ZlibDir
)
$ErrorActionPreference = 'Stop'

# xbmc/audiodecoder.2sf pinned commit (vendors lib/vio2sf and lib/psflib), matching the Linux build.
$TSF_COMMIT = '039eeb7de76bf28d20c64ab4ef778982e70c639d'
$TSF_SHA256 = '2860e418295be02f5bc0619b7d5b3ea652971e84f681c3a843e871d6ef71addb'

$repo = Resolve-Path (Join-Path $PSScriptRoot '..\..')
if (-not $Vio2sfDir) { $Vio2sfDir = Join-Path $repo 'build\vio2sf-min' }
if (-not $ZlibDir)   { $ZlibDir   = Join-Path $repo 'build\zlib' }

if (Test-Path (Join-Path $Vio2sfDir 'lib\vio2sf.lib')) {
    Write-Host "==> vio2sf already present at $Vio2sfDir"
    return
}

# Download with retries. Portable across Windows PowerShell 5.1 and pwsh 7 (CI runs this step under
# pwsh, but a local dev shell may be 5.1, whose Invoke-WebRequest lacks -MaximumRetryCount).
function Invoke-Download([string]$Uri, [string]$OutFile) {
    for ($i = 1; $i -le 5; $i++) {
        try { Invoke-WebRequest -Uri $Uri -OutFile $OutFile; return }
        catch { if ($i -ge 5) { throw }; Start-Sleep -Seconds 5 }
    }
}

$zlibInc = Join-Path $ZlibDir 'include'
if (-not (Test-Path (Join-Path $zlibInc 'zlib.h'))) {
    throw "zlib.h not found at $zlibInc - run packaging\windows\build-gme.ps1 first (it builds zlib)."
}

# C source list from lib/vio2sf/CMakeLists.txt (SPU.cpp built separately as C++). Mirrors
# build-vio2sf-min.sh's CSRCS.
$CSRCS = @('armcpu', 'arm_instructions', 'barray', 'bios', 'cp15', 'FIFO', 'GPU', 'isqrt', 'matrix',
           'mc', 'MMU', 'NDSSystem', 'resampler', 'state', 'thumb_instructions')

$work = New-Item -ItemType Directory -Force -Path (Join-Path ([IO.Path]::GetTempPath()) ("vio2sf-" + [Guid]::NewGuid()))
try {
    $tgz = Join-Path $work '2sf.tar.gz'
    Write-Host "==> Fetching vio2sf (audiodecoder.2sf @ $TSF_COMMIT)"
    Invoke-Download "https://github.com/xbmc/audiodecoder.2sf/archive/$TSF_COMMIT.tar.gz" $tgz
    $got = (Get-FileHash -Algorithm SHA256 $tgz).Hash.ToLower()
    if ($got -ne $TSF_SHA256.ToLower()) {
        throw "SHA256 mismatch for audiodecoder.2sf`n  expected $TSF_SHA256`n  got      $got"
    }
    # Windows' own bsdtar (not a GNU tar that a Git install may put first on PATH, which reads a
    # `C:\...` path as a remote host). Present on windows-latest and Windows 10 1803+.
    & "$env:SystemRoot\System32\tar.exe" -xf $tgz -C $work
    if ($LASTEXITCODE) { throw "extract audiodecoder.2sf failed" }
    $dm = Join-Path $work "audiodecoder.2sf-$TSF_COMMIT\lib\vio2sf\src\vio2sf\desmume"

    $objDir = Join-Path $work 'obj'
    New-Item -ItemType Directory -Force -Path $objDir | Out-Null
    Write-Host "==> Compiling vio2sf ($($CSRCS.Count) C sources + SPU.cpp, MSVC static, ROM-less DS emulator)"
    $objs = @()
    # desmume's isqrt.c exports a generic `isqrt32`, which also lives in the statically linked Opus lib
    # (opusic-sys). On Linux vio2sf is a shared object with its own symbol namespace, so the two never
    # meet; on Windows every static lib folds into one exe link unit, so they collide (LNK2005). Rename
    # it uniformly across the vio2sf TUs (definition in isqrt.c, declaration in isqrt.h, the one caller in
    # bios.c) via the preprocessor, so the symbol becomes vio2sf_isqrt32. Nothing outside vio2sf calls it
    # (the shim uses only state_* functions), so the installed header keeping the old spelling is moot.
    $isqrtDef = '/Disqrt32=vio2sf_isqrt32'
    foreach ($s in $CSRCS) {
        $srcFile = Join-Path $dm ($s + '.c')
        $obj = Join-Path $objDir ($s + '.obj')
        # /MD matches Rust's CRT. resampler.c's RESAMPLER_SSE path uses <xmmintrin.h>, available on MSVC
        # x64 with no flag. Compiled as C so the functions keep C linkage for SPU.cpp and the extern "C" shim.
        & cl /nologo /c /O2 /MD $isqrtDef /D_CRT_SECURE_NO_WARNINGS /I"$dm" "$srcFile" /Fo"$obj"
        if ($LASTEXITCODE) { throw "cl failed on $s" }
        $objs += $obj
    }
    # SPU.cpp is the one C++ translation unit.
    $spuObj = Join-Path $objDir 'SPU.obj'
    & cl /nologo /c /O2 /MD /EHsc $isqrtDef /D_CRT_SECURE_NO_WARNINGS /I"$dm" "$(Join-Path $dm 'SPU.cpp')" /Fo"$spuObj"
    if ($LASTEXITCODE) { throw "cl failed on SPU.cpp" }
    $objs += $spuObj

    $outLib = Join-Path $Vio2sfDir 'lib'
    $outInc = Join-Path $Vio2sfDir 'include\vio2sf'
    New-Item -ItemType Directory -Force -Path $outLib, $outInc | Out-Null
    & lib /nologo "/OUT:$(Join-Path $outLib 'vio2sf.lib')" @objs
    if ($LASTEXITCODE) { throw "lib.exe (vio2sf) failed" }
    # All desmume headers flat under include\vio2sf\ so the shim's <vio2sf/state.h> and its siblings
    # resolve, matching the Linux build's include/vio2sf layout.
    Copy-Item (Join-Path $dm '*.h') $outInc -Force
    # zlib.h/zconf.h next to the vio2sf include root, for the C++ shim's <zlib.h> (it inflates the 2SF
    # save-state overlays). build.rs puts VIO2SF_DIR\include on the shim's include path.
    Copy-Item (Join-Path $zlibInc 'zlib.h')  (Join-Path $Vio2sfDir 'include') -Force
    Copy-Item (Join-Path $zlibInc 'zconf.h') (Join-Path $Vio2sfDir 'include') -Force
} finally {
    Remove-Item -Recurse -Force $work -ErrorAction SilentlyContinue
}

foreach ($rel in @('lib\vio2sf.lib', 'include\vio2sf\state.h', 'include\zlib.h')) {
    if (-not (Test-Path (Join-Path $Vio2sfDir $rel))) {
        throw "vio2sf build incomplete: missing $rel under $Vio2sfDir"
    }
}
Write-Host "==> Done. vio2sf at $Vio2sfDir"
