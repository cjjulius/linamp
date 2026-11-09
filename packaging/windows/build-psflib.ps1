<#
.SYNOPSIS
  Build psflib as a static library for the PSF-family features on Windows (the shared container/tag
  parser behind `ssf` now, and `qsf`/`ncsf` later). Mirrors packaging/linux/build-psflib-min.sh.

.DESCRIPTION
  psflib (kode54, MIT) unpacks the zlib-compressed .ssf/.dsf/.qsf/.ncsf wrapper and reads its [TAG]
  block; every PSF-family emulator shim links it. It has no standalone release, so we build it from
  the Kodi audiodecoder.ssf source tree (which vendors lib/psflib), pinned to the same commit as the
  Linux build. psflib carries a clean MSVC path (`_MSC_VER`: strcasecmp -> _stricmp, snprintf ->
  sprintf_s), so we compile its two sources directly with cl.

  Unlike Linux (shared libpsflib.so + system -lz), Windows builds STATIC and links into the exe, the
  same self-contained approach as AdPlug/SID/vgmstream/upse. psflib needs zlib, so we also install a
  copy of the STATIC zlib that build-gme.ps1 produces (build/zlib/lib/zlibstatic.lib) as z.lib. That
  satisfies build.rs's unconditional `rustc-link-lib=dylib=z` (the same directive the Linux build
  resolves against system libz) without any platform branch in build.rs. On MSVC a static .lib is
  consumed the same as an import lib, so build.rs's `dylib=psflib` / `dylib=z` link these unchanged.
  Install layout (PSFLIB_DIR, the prefix build.rs defaults to):

    <PsflibDir>\lib\psflib.lib    (static: psf2fs + psflib)
    <PsflibDir>\lib\z.lib         (static zlib, for dylib=z)
    <PsflibDir>\include\*.h       (psflib.h / psf2fs.h; the shim includes <psflib.h>)

  Shared across ssf/qsf/ncsf: building psflib once covers all three. Must run inside an MSVC
  developer environment (cl + lib on PATH), AFTER build-gme.ps1 (for zlib). Idempotent: skips when
  present.

.PARAMETER PsflibDir
  Install prefix. Default: <repo>\build\psflib-min (the PSFLIB_DIR build.rs defaults to).
.PARAMETER ZlibDir
  zlib prefix with lib\zlibstatic.lib + include\zlib.h. Default: <repo>\build\zlib (from build-gme.ps1).
#>
[CmdletBinding()]
param(
    [string]$PsflibDir,
    [string]$ZlibDir
)
$ErrorActionPreference = 'Stop'

# audiodecoder.ssf pinned commit (vendors lib/psflib and lib/Highly_Theoretical), matching the Linux
# build. The same tarball feeds build-highly-theoretical.ps1.
$SSF_COMMIT = '692ace7752e84dedfd76a741266fbe778df845ef'
$SSF_SHA256 = '8e362a15b9a1620c9bccfe9ec62fa08f570a76ae626c55e2c07de8b1ad8377e5'

$repo = Resolve-Path (Join-Path $PSScriptRoot '..\..')
if (-not $PsflibDir) { $PsflibDir = Join-Path $repo 'build\psflib-min' }
if (-not $ZlibDir)   { $ZlibDir   = Join-Path $repo 'build\zlib' }

if (Test-Path (Join-Path $PsflibDir 'lib\psflib.lib')) {
    Write-Host "==> psflib already present at $PsflibDir"
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

$zlibStatic = Join-Path $ZlibDir 'lib\zlibstatic.lib'
$zlibInc    = Join-Path $ZlibDir 'include'
if (-not (Test-Path $zlibStatic)) {
    throw "Static zlib not found at $zlibStatic - run packaging\windows\build-gme.ps1 first (it builds zlib)."
}

# Source list from lib/psflib/CMakeLists.txt (mirrored in build-psflib-min.sh).
$SRCS = @('psf2fs', 'psflib')

$work = New-Item -ItemType Directory -Force -Path (Join-Path ([IO.Path]::GetTempPath()) ("psflib-" + [Guid]::NewGuid()))
try {
    $tgz = Join-Path $work 'ssf.tar.gz'
    Write-Host "==> Fetching psflib (audiodecoder.ssf @ $SSF_COMMIT)"
    Invoke-Download "https://github.com/xbmc/audiodecoder.ssf/archive/$SSF_COMMIT.tar.gz" $tgz
    $got = (Get-FileHash -Algorithm SHA256 $tgz).Hash.ToLower()
    if ($got -ne $SSF_SHA256.ToLower()) {
        throw "SHA256 mismatch for audiodecoder.ssf`n  expected $SSF_SHA256`n  got      $got"
    }
    tar -xf $tgz -C $work
    if ($LASTEXITCODE) { throw "extract audiodecoder.ssf failed" }
    $lib = Join-Path $work "audiodecoder.ssf-$SSF_COMMIT\lib\psflib"

    $objDir = Join-Path $work 'obj'
    New-Item -ItemType Directory -Force -Path $objDir | Out-Null
    Write-Host "==> Compiling psflib ($($SRCS.Count) sources, MSVC static)"
    $objs = @()
    foreach ($s in $SRCS) {
        $src = Join-Path $lib ($s + '.c')
        $obj = Join-Path $objDir ($s + '.obj')
        # /MD: dynamic CRT, to match Rust (and the cc-crate-compiled shim) so the FFI shares one CRT
        # heap. zlib include is needed because psflib.c includes <zlib.h>.
        & cl /nologo /c /O2 /MD /D_CRT_SECURE_NO_WARNINGS /I"$lib" /I"$zlibInc" "$src" /Fo"$obj"
        if ($LASTEXITCODE) { throw "cl failed on $s" }
        $objs += $obj
    }

    $outLib = Join-Path $PsflibDir 'lib'
    $outInc = Join-Path $PsflibDir 'include'
    New-Item -ItemType Directory -Force -Path $outLib, $outInc | Out-Null
    & lib /nologo "/OUT:$(Join-Path $outLib 'psflib.lib')" @objs
    if ($LASTEXITCODE) { throw "lib.exe (psflib) failed" }
    # Install the static zlib as z.lib so build.rs's `dylib=z` resolves it on the PSFLIB_DIR\lib
    # search path (the Windows stand-in for the Linux system -lz).
    Copy-Item $zlibStatic (Join-Path $outLib 'z.lib') -Force
    Copy-Item (Join-Path $lib '*.h') $outInc -Force
} finally {
    Remove-Item -Recurse -Force $work -ErrorAction SilentlyContinue
}

foreach ($rel in @('lib\psflib.lib', 'lib\z.lib', 'include\psflib.h')) {
    if (-not (Test-Path (Join-Path $PsflibDir $rel))) {
        throw "psflib build incomplete: missing $rel under $PsflibDir"
    }
}
Write-Host "==> Done. psflib at $PsflibDir"
