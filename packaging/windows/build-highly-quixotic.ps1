<#
.SYNOPSIS
  Build Highly_Quixotic as a static library for the `qsf` feature on Windows (Capcom QSound QSF arcade
  music, via crate::qsf + src/qsf_shim.c). Mirrors packaging/linux/build-highly-quixotic-min.sh.

.DESCRIPTION
  Highly_Quixotic (kode54, GPL-3) emulates the Capcom Z80 + QSound DSP from a reverse-engineered,
  reimplemented-in-C algorithm. No copyrighted QSound DSP ROM (dl-1425.bin) is needed or shipped, so
  it plays ROM-less, the same principle as the PSF1/libupse and SSF paths; the Z80 program and sample
  ROM live inside the .qsf/.miniqsf file (parsed by the shared psflib, built separately by
  build-psflib.ps1). It has no standalone release, so we build it from the Kodi audiodecoder.qsf
  source tree, pinned to the same commit as the Linux build.

  Like Highly_Theoretical, the Core uses emuconfig.h's "WIN32 native" branch (we define WIN32) for
  native MSVC types, and qsound_ctr.c carries an _MSC_VER path; there is no inline asm or POSIX call,
  so it compiles clean with cl. Source list and defines mirror the Linux build and
  lib/highly_quixotic/CMakeLists.txt.

  Built STATIC and linked into the exe (nothing ships beside it), the same approach as the other
  native features. On MSVC a static .lib is consumed the same as an import lib, so build.rs's
  `rustc-link-lib=dylib=highlyquixotic` links it unchanged. build.rs compiles the C shim
  (src/qsf_shim.c) against the installed headers and psflib's; the shared z.lib (installed by
  build-psflib.ps1) satisfies `dylib=z`. Install layout (HIGHLYQUIXOTIC_DIR, the prefix build.rs
  defaults to):

    <HqDir>\lib\highlyquixotic.lib   (static: Z80 + QSound DSP)
    <HqDir>\include\*.h              (Core headers; the shim includes <qsound.h>)

  Must run inside an MSVC developer environment (cl + lib on PATH). Idempotent: skips when present.

.PARAMETER HqDir
  Install prefix. Default: <repo>\build\highlyquixotic-min (the HIGHLYQUIXOTIC_DIR default).
#>
[CmdletBinding()]
param(
    [string]$HqDir
)
$ErrorActionPreference = 'Stop'

# audiodecoder.qsf pinned commit (vendors lib/highly_quixotic and lib/psflib), matching the Linux build.
$QSF_COMMIT = 'ed24a15f9ed01cfdafe1f051895d11c5286a442f'
$QSF_SHA256 = '2b1aa52bc7b112c083affdc5649e33b6a28cd6d0bec53e755722020548e786da'

$repo = Resolve-Path (Join-Path $PSScriptRoot '..\..')
if (-not $HqDir) { $HqDir = Join-Path $repo 'build\highlyquixotic-min' }

if (Test-Path (Join-Path $HqDir 'lib\highlyquixotic.lib')) {
    Write-Host "==> Highly_Quixotic already present at $HqDir"
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

# Source list from lib/highly_quixotic/CMakeLists.txt (mirrored in build-highly-quixotic-min.sh).
$SRCS = @('kabuki', 'qmix', 'qsound', 'qsound_ctr', 'z80')

$work = New-Item -ItemType Directory -Force -Path (Join-Path ([IO.Path]::GetTempPath()) ("hq-" + [Guid]::NewGuid()))
try {
    $tgz = Join-Path $work 'qsf.tar.gz'
    Write-Host "==> Fetching Highly_Quixotic (audiodecoder.qsf @ $QSF_COMMIT)"
    Invoke-Download "https://github.com/xbmc/audiodecoder.qsf/archive/$QSF_COMMIT.tar.gz" $tgz
    $got = (Get-FileHash -Algorithm SHA256 $tgz).Hash.ToLower()
    if ($got -ne $QSF_SHA256.ToLower()) {
        throw "SHA256 mismatch for audiodecoder.qsf`n  expected $QSF_SHA256`n  got      $got"
    }
    tar -xf $tgz -C $work
    if ($LASTEXITCODE) { throw "extract audiodecoder.qsf failed" }
    $core = Join-Path $work "audiodecoder.qsf-$QSF_COMMIT\lib\highly_quixotic\Core"

    $objDir = Join-Path $work 'obj'
    New-Item -ItemType Directory -Force -Path $objDir | Out-Null
    Write-Host "==> Compiling Highly_Quixotic ($($SRCS.Count) sources, MSVC static, ROM-less QSound)"
    $objs = @()
    foreach ($s in $SRCS) {
        $src = Join-Path $core ($s + '.c')
        $obj = Join-Path $objDir ($s + '.obj')
        # EMU_COMPILE/EMU_LITTLE_ENDIAN select the little-endian emulator config; WIN32 takes
        # emuconfig.h's native MSVC path. /MD matches Rust's CRT.
        & cl /nologo /c /O2 /MD /DEMU_COMPILE /DEMU_LITTLE_ENDIAN /DWIN32 /D_CRT_SECURE_NO_WARNINGS `
            /I"$core" "$src" /Fo"$obj"
        if ($LASTEXITCODE) { throw "cl failed on $s" }
        $objs += $obj
    }

    $outLib = Join-Path $HqDir 'lib'
    $outInc = Join-Path $HqDir 'include'
    New-Item -ItemType Directory -Force -Path $outLib, $outInc | Out-Null
    & lib /nologo "/OUT:$(Join-Path $outLib 'highlyquixotic.lib')" @objs
    if ($LASTEXITCODE) { throw "lib.exe (highlyquixotic) failed" }
    Copy-Item (Join-Path $core '*.h') $outInc -Force
} finally {
    Remove-Item -Recurse -Force $work -ErrorAction SilentlyContinue
}

foreach ($rel in @('lib\highlyquixotic.lib', 'include\qsound.h')) {
    if (-not (Test-Path (Join-Path $HqDir $rel))) {
        throw "Highly_Quixotic build incomplete: missing $rel under $HqDir"
    }
}
Write-Host "==> Done. Highly_Quixotic at $HqDir"
