<#
.SYNOPSIS
  Build Highly_Theoretical as a static library for the `ssf` feature on Windows (Sega Saturn SSF and
  Dreamcast DSF program music, via crate::ssf + src/ssf_shim.c). Mirrors
  packaging/linux/build-highly-theoretical-min.sh.

.DESCRIPTION
  Highly_Theoretical (kode54, GPL-3) emulates only the Saturn SCSP and Dreamcast AICA sound chips
  (shared yam.c) with a Musashi 68000 core (Saturn) and an ARM7 core (Dreamcast). No console BIOS or
  game ROM: it plays ROM-less, the same principle as the PSF1/libupse path. The driver program and
  samples live inside the .ssf/.dsf file itself (parsed by the shared psflib, built separately by
  build-psflib.ps1). It has no standalone release, so we build it from the Kodi audiodecoder.ssf
  source tree, pinned to the same commit as the Linux build.

  The Core carries first-class MSVC support (foobar2000 heritage): emuconfig.h's "WIN32 native"
  branch gives native `__fastcall`/`__int64` types, and the DSP dynarec (the only VirtualProtect /
  inline-asm path) is `#undef`'d on `_WIN64`, so x64 builds a pure interpreter. We define WIN32 so
  every Core file takes that native path uniformly, and omit the Linux-only HAVE_MPROTECT (moot with
  the dynarec disabled on x64). Source list and the other defines mirror the Linux build and
  lib/Highly_Theoretical/CMakeLists.txt: Musashi 68000 (USE_M68K, with the pre-generated m68kops.c -
  no codegen step), ARM7, the shared SCSP/AICA core (yam.c), and the Saturn/Dreamcast front-ends.

  Built STATIC and linked into the exe (nothing ships beside it), the same approach as
  AdPlug/SID/vgmstream/upse. On MSVC a static .lib is consumed the same as an import lib, so
  build.rs's `rustc-link-lib=dylib=highlytheoretical` links it unchanged. build.rs compiles the C
  shim (src/ssf_shim.c) against the installed headers and the psflib headers. Install layout
  (HIGHLYTHEORETICAL_DIR, the prefix build.rs defaults to):

    <HtDir>\lib\highlytheoretical.lib    (static: SCSP/AICA core + Musashi/ARM CPUs)
    <HtDir>\include\*.h                   (Core headers; the shim includes <sega.h>)

  Must run inside an MSVC developer environment (cl + lib on PATH). zlib is not needed here (psflib
  owns the zlib dependency). Idempotent: skips when present.

.PARAMETER HtDir
  Install prefix. Default: <repo>\build\highlytheoretical-min (the HIGHLYTHEORETICAL_DIR default).
#>
[CmdletBinding()]
param(
    [string]$HtDir
)
$ErrorActionPreference = 'Stop'

# audiodecoder.ssf pinned commit (vendors lib/Highly_Theoretical and lib/psflib), matching the Linux
# build. The same tarball feeds build-psflib.ps1.
$SSF_COMMIT = '692ace7752e84dedfd76a741266fbe778df845ef'
$SSF_SHA256 = '8e362a15b9a1620c9bccfe9ec62fa08f570a76ae626c55e2c07de8b1ad8377e5'

$repo = Resolve-Path (Join-Path $PSScriptRoot '..\..')
if (-not $HtDir) { $HtDir = Join-Path $repo 'build\highlytheoretical-min' }

if (Test-Path (Join-Path $HtDir 'lib\highlytheoretical.lib')) {
    Write-Host "==> Highly_Theoretical already present at $HtDir"
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

# Source list + defines from lib/Highly_Theoretical/CMakeLists.txt (mirrored in
# build-highly-theoretical-min.sh). m68k/* is the Musashi core; m68kops.c is pre-generated.
$SRCS = @('sega', 'm68k/m68kops', 'm68k/m68kcpu', 'arm', 'dcsound', 'yam', 'satsound')

$work = New-Item -ItemType Directory -Force -Path (Join-Path ([IO.Path]::GetTempPath()) ("ht-" + [Guid]::NewGuid()))
try {
    $tgz = Join-Path $work 'ssf.tar.gz'
    Write-Host "==> Fetching Highly_Theoretical (audiodecoder.ssf @ $SSF_COMMIT)"
    Invoke-Download "https://github.com/xbmc/audiodecoder.ssf/archive/$SSF_COMMIT.tar.gz" $tgz
    $got = (Get-FileHash -Algorithm SHA256 $tgz).Hash.ToLower()
    if ($got -ne $SSF_SHA256.ToLower()) {
        throw "SHA256 mismatch for audiodecoder.ssf`n  expected $SSF_SHA256`n  got      $got"
    }
    tar -xf $tgz -C $work
    if ($LASTEXITCODE) { throw "extract audiodecoder.ssf failed" }
    $core = Join-Path $work "audiodecoder.ssf-$SSF_COMMIT\lib\Highly_Theoretical\Core"

    $objDir = Join-Path $work 'obj'
    New-Item -ItemType Directory -Force -Path $objDir | Out-Null
    Write-Host "==> Compiling Highly_Theoretical ($($SRCS.Count) sources, MSVC static, ROM-less SCSP/AICA)"
    $objs = @()
    foreach ($s in $SRCS) {
        $src = Join-Path $core ($s.Replace('/', '\') + '.c')
        $obj = Join-Path $objDir (($s.Replace('/', '_')) + '.obj')
        # EMU_COMPILE/EMU_LITTLE_ENDIAN select the little-endian emulator config; USE_M68K + LSB_FIRST
        # pick the Musashi 68000 core; WIN32 takes emuconfig.h's native MSVC path. HAVE_MPROTECT is
        # omitted (Linux-only; the dynarec that would use it is disabled on x64). /MD matches Rust's CRT.
        & cl /nologo /c /O2 /MD /DEMU_COMPILE /DEMU_LITTLE_ENDIAN /DUSE_M68K /DLSB_FIRST /DWIN32 /D_CRT_SECURE_NO_WARNINGS `
            /I"$core" /I"$core\m68k" "$src" /Fo"$obj"
        if ($LASTEXITCODE) { throw "cl failed on $s" }
        $objs += $obj
    }

    $outLib = Join-Path $HtDir 'lib'
    $outInc = Join-Path $HtDir 'include'
    New-Item -ItemType Directory -Force -Path $outLib, $outInc | Out-Null
    & lib /nologo "/OUT:$(Join-Path $outLib 'highlytheoretical.lib')" @objs
    if ($LASTEXITCODE) { throw "lib.exe (highlytheoretical) failed" }
    # Top-level Core headers only (the shim's <sega.h> and its public transitive includes), matching
    # the Linux build's `cp "$core"/*.h`.
    Copy-Item (Join-Path $core '*.h') $outInc -Force
} finally {
    Remove-Item -Recurse -Force $work -ErrorAction SilentlyContinue
}

foreach ($rel in @('lib\highlytheoretical.lib', 'include\sega.h')) {
    if (-not (Test-Path (Join-Path $HtDir $rel))) {
        throw "Highly_Theoretical build incomplete: missing $rel under $HtDir"
    }
}
Write-Host "==> Done. Highly_Theoretical at $HtDir"
