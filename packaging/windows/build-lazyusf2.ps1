<#
.SYNOPSIS
  Build lazyusf2 as a static library for the `usf` feature on Windows (Nintendo 64 USF music, via
  crate::usf + src/usf_shim.c). Mirrors packaging/linux/build-lazyusf2-min.sh.

.DESCRIPTION
  lazyusf2 (kode54, GPL-2.0-or-later) is a Mupen64Plus-derived N64 emulator cut down to just what USF
  playback needs: the R4300 CPU (cached interpreter), the RSP, and the RSP audio HLE/LLE. It plays
  ROM-less - a USF carries a Project64 save state captured after boot plus the game's audio program, so
  no copyrighted N64 PIF/boot ROM is ever needed or shipped; audio is High-Level Emulation of the RSP
  microcode. The program/save-state lives inside the .usf/.miniusf file, which the shared psflib unpacks
  (resolving `_lib` chains). psflib is built separately by build-psflib.ps1 and shared across the
  SSF/QSF/NCSF/USF features. It has no standalone release, so we build it from the derselbst/lazyusf2
  source tree, pinned to the same commit as the Linux build.

  IMPORTANT (licensing): we build from derselbst/lazyusf2 (a maintained fork of kode54/lazyusf2), NOT
  the Kodi xbmc/audiodecoder.usf tree. That tree vendors the ORIGINAL lazyusf, whose core CPU files
  carry zilmar's Project64 "freeware for PERSONAL USE only / non-commercial" license - a field-of-use
  restriction we never ship. lazyusf2 replaced that core with Mupen64Plus's GPL R4300, so it is cleanly
  GPL-2.0-or-later (same copyleft posture as the Highly_Theoretical/Highly_Quixotic cores).

  Built with the cached interpreter (no x86 dynarec), the same choice as the Linux build: a music player
  does not need the JIT, and the interpreter is one portable recipe. We compile the upstream Makefile's
  OBJS minus the x86/x86_64 recompiler dirs, plus r4300/empty_dynarec.c (the no-dynarec stubs), and do
  NOT define DYNAREC - so r4300.c's `#if defined(DYNAREC)` paths are all skipped and only the pure
  interpreter runs. The GCC-isms upstream carries are all guarded for non-GCC compilers: recomp.c's
  POSIX includes (<unistd.h>, <sys/mman.h>) are under `#if defined(__GNUC__)`, memory.h's `sl()` byte
  swap falls back from `__builtin_bswap32` to a portable shift macro, and the `__attribute__((used))`
  prototypes in cached_interp.c/pure_interp.c are `#if defined(__GNUC__)` only. We define WIN32 +
  __x86_64__ + ARCH_MIN_SSE2 to match upstream's own MSVC project (prj/msvc/lazyusf2.vcxproj); __x86_64__
  only affects the (skipped) dynarec blocks and is what GCC predefines on the Linux build anyway. The
  interpreter def includes <zlib.h> for adler32(), so we add the static zlib's include dir (the adler32
  symbol itself is resolved at the final link by psflib's z.lib, via build.rs's `dylib=z`).

  Built STATIC and linked into the exe (nothing ships beside it), the same approach as the other native
  features. On MSVC a static .lib is consumed the same as an import lib, so build.rs's
  `rustc-link-lib=dylib=lazyusf` links it unchanged. build.rs compiles the C shim (src/usf_shim.c)
  against the installed headers and psflib's; the shared z.lib (installed by build-psflib.ps1) satisfies
  `dylib=z`. Install layout (LAZYUSF_DIR, the prefix build.rs defaults to):

    <LazyusfDir>\lib\lazyusf.lib   (static: R4300 interpreter + RSP + RSP audio HLE)
    <LazyusfDir>\include\usf.h      (the shim includes <usf.h>)

  Must run inside an MSVC developer environment (cl + lib on PATH), AFTER build-gme.ps1 (for the static
  zlib's zlib.h). Idempotent: skips when present.

.PARAMETER LazyusfDir
  Install prefix. Default: <repo>\build\lazyusf2-min (the LAZYUSF_DIR default).
.PARAMETER ZlibDir
  zlib prefix with include\zlib.h. Default: <repo>\build\zlib (from build-gme.ps1).
#>
[CmdletBinding()]
param(
    [string]$LazyusfDir,
    [string]$ZlibDir
)
$ErrorActionPreference = 'Stop'

# derselbst/lazyusf2 pinned commit (maintained fork of kode54/lazyusf2), matching the Linux build.
$USF_COMMIT = '00d19c9b1e0ae4924d395310844a94d871743c89'
$USF_SHA256 = 'c2e838c0c690fada630ece1ce2fcf6a4b90537a46377f2092e4f8f22cc0a7609'

$repo = Resolve-Path (Join-Path $PSScriptRoot '..\..')
if (-not $LazyusfDir) { $LazyusfDir = Join-Path $repo 'build\lazyusf2-min' }
if (-not $ZlibDir)    { $ZlibDir    = Join-Path $repo 'build\zlib' }

if (Test-Path (Join-Path $LazyusfDir 'lib\lazyusf.lib')) {
    Write-Host "==> lazyusf2 already present at $LazyusfDir"
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

# Upstream Makefile OBJS minus the x86/x86_64 recompiler, plus r4300/empty_dynarec (the no-dynarec
# stubs) and rsp_lle/rsp (the low-level RSP). Mirrors build-lazyusf2-min.sh's SRCS.
$SRCS = @(
    'ai/ai_controller', 'api/callbacks', 'debugger/dbg_decoder', 'main/main', 'main/rom',
    'main/savestates', 'main/util', 'memory/memory', 'pi/cart_rom', 'pi/pi_controller',
    'r4300/cached_interp', 'r4300/cp0', 'r4300/cp1', 'r4300/empty_dynarec', 'r4300/exception',
    'r4300/interupt', 'r4300/mi_controller', 'r4300/pure_interp', 'r4300/r4300', 'r4300/r4300_core',
    'r4300/recomp', 'r4300/reset', 'r4300/tlb', 'rdp/rdp_core', 'ri/rdram', 'ri/rdram_detection_hack',
    'ri/ri_controller', 'rsp/rsp_core', 'rsp_hle/alist', 'rsp_hle/alist_audio', 'rsp_hle/alist_naudio',
    'rsp_hle/alist_nead', 'rsp_hle/audio', 'rsp_hle/cicx105', 'rsp_hle/hle', 'rsp_hle/hvqm',
    'rsp_hle/jpeg', 'rsp_hle/memory', 'rsp_hle/mp3', 'rsp_hle/musyx', 'rsp_hle/plugin', 'rsp_hle/re2',
    'rsp_lle/rsp', 'si/cic', 'si/game_controller', 'si/n64_cic_nus_6105', 'si/pif', 'si/si_controller',
    'usf/usf', 'usf/barray', 'usf/resampler', 'vi/vi_controller'
)

$work = New-Item -ItemType Directory -Force -Path (Join-Path ([IO.Path]::GetTempPath()) ("lazyusf-" + [Guid]::NewGuid()))
try {
    $tgz = Join-Path $work 'lazyusf2.tar.gz'
    Write-Host "==> Fetching lazyusf2 (derselbst/lazyusf2 @ $USF_COMMIT)"
    Invoke-Download "https://github.com/derselbst/lazyusf2/archive/$USF_COMMIT.tar.gz" $tgz
    $got = (Get-FileHash -Algorithm SHA256 $tgz).Hash.ToLower()
    if ($got -ne $USF_SHA256.ToLower()) {
        throw "SHA256 mismatch for lazyusf2`n  expected $USF_SHA256`n  got      $got"
    }
    # Windows' own bsdtar (not a GNU tar that a Git install may put first on PATH, which reads a
    # `C:\...` path as a remote host). Present on windows-latest and Windows 10 1803+.
    & "$env:SystemRoot\System32\tar.exe" -xf $tgz -C $work
    if ($LASTEXITCODE) { throw "extract lazyusf2 failed" }
    $src = Join-Path $work "lazyusf2-$USF_COMMIT"
    $msvcCompat = Join-Path $src 'rsp_hle\msvc-compat'

    $objDir = Join-Path $work 'obj'
    New-Item -ItemType Directory -Force -Path $objDir | Out-Null
    Write-Host "==> Compiling lazyusf2 ($($SRCS.Count) sources, MSVC static, ROM-less, cached interpreter)"
    $objs = @()
    foreach ($s in $SRCS) {
        $srcFile = Join-Path $src ($s.Replace('/', '\') + '.c')
        $obj = Join-Path $objDir (($s.Replace('/', '_')) + '.obj')
        # WIN32 + __x86_64__ + ARCH_MIN_SSE2 match upstream's MSVC project; NO DYNAREC, so r4300.c runs
        # the pure interpreter and empty_dynarec.c stubs the JIT. rsp_hle/msvc-compat supplies our-stdbool.h;
        # the zlib include is for the interpreter def's adler32 prototype. /MD matches Rust's CRT.
        & cl /nologo /c /O2 /MD /DWIN32 /D__x86_64__ /DARCH_MIN_SSE2 /DNDEBUG /D_LIB /D_CRT_SECURE_NO_WARNINGS `
            /I"$src" /I"$msvcCompat" /I"$zlibInc" "$srcFile" /Fo"$obj"
        if ($LASTEXITCODE) { throw "cl failed on $s" }
        $objs += $obj
    }

    $outLib = Join-Path $LazyusfDir 'lib'
    $outInc = Join-Path $LazyusfDir 'include'
    New-Item -ItemType Directory -Force -Path $outLib, $outInc | Out-Null
    & lib /nologo "/OUT:$(Join-Path $outLib 'lazyusf.lib')" @objs
    if ($LASTEXITCODE) { throw "lib.exe (lazyusf) failed" }
    Copy-Item (Join-Path $src 'usf\usf.h') $outInc -Force
} finally {
    Remove-Item -Recurse -Force $work -ErrorAction SilentlyContinue
}

foreach ($rel in @('lib\lazyusf.lib', 'include\usf.h')) {
    if (-not (Test-Path (Join-Path $LazyusfDir $rel))) {
        throw "lazyusf2 build incomplete: missing $rel under $LazyusfDir"
    }
}
Write-Host "==> Done. lazyusf2 at $LazyusfDir"
