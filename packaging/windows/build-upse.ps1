<#
.SYNOPSIS
  Build libupse as a static library for the `psf` feature on Windows (PlayStation PSF1 music via
  crate::psf + src/psf_shim.c).

.DESCRIPTION
  The Windows counterpart to packaging/linux/build-upse-min.sh. libupse ("Unix PlayStation Sound
  Emulator", GPL) ships its own HLE FakeBIOS, so PSF plays ROM-less - no PlayStation BIOS (which we
  never ship) is needed. It has no standalone release, so we build it from the Kodi audiodecoder.upse
  source tree (which vendors libupse under lib/libupse), pinned to the same commit as the Linux build.

  libupse's headers already carry an MSVC path (`WIN32_MSC`: native integer typedefs, <windows.h>
  instead of <unistd.h>, stricmp mapping), so we compile the vendored sources directly with cl and
  the same defines the Linux build uses (EMU_COMPILE / EMU_LITTLE_ENDIAN). PSF program sections are
  zlib-compressed, so libupse needs zlib; we link the STATIC zlib that build-gme.ps1 already produces
  (build/zlib/lib/zlibstatic.lib) and combine it into upse.lib so the one library is self-contained.

  Built STATIC and linked into the exe (nothing ships beside it), the same approach as AdPlug/SID/
  vgmstream. On MSVC a static .lib is consumed the same as an import lib, so build.rs's
  `rustc-link-lib=dylib=upse` links it unchanged. build.rs compiles the C shim (src/psf_shim.c)
  against the installed headers. Install layout:

    <UpseDir>\lib\upse.lib        (static: libupse + zlib)
    <UpseDir>\include\*.h         (libupse headers; the shim includes them)

  Must run inside an MSVC developer environment (cl + lib on PATH), AFTER build-gme.ps1 (for zlib).
  Idempotent: skips when present.

.PARAMETER UpseDir
  Install prefix. Default: <repo>\build\upse-min (the UPSE_DIR build.rs defaults to).
.PARAMETER ZlibDir
  zlib prefix with lib\zlibstatic.lib + include\zlib.h. Default: <repo>\build\zlib (from build-gme.ps1).
#>
[CmdletBinding()]
param(
    [string]$UpseDir,
    [string]$ZlibDir
)
$ErrorActionPreference = 'Stop'

# audiodecoder.upse pinned commit (vendors libupse under lib/libupse), matching the Linux build.
$UPSE_COMMIT = '221dc3985eb5721e324bd67b738debe6527ebcb3'
$UPSE_SHA256 = '4c79ffa062d8f70cb60f80c9f95adb5c3616d1741da52972381f3153a5aa3696'

$repo = Resolve-Path (Join-Path $PSScriptRoot '..\..')
if (-not $UpseDir)  { $UpseDir  = Join-Path $repo 'build\upse-min' }
if (-not $ZlibDir)  { $ZlibDir  = Join-Path $repo 'build\zlib' }

if (Test-Path (Join-Path $UpseDir 'lib\upse.lib')) {
    Write-Host "==> libupse already present at $UpseDir"
    return
}

$zlibStatic = Join-Path $ZlibDir 'lib\zlibstatic.lib'
$zlibInc    = Join-Path $ZlibDir 'include'
if (-not (Test-Path $zlibStatic)) {
    throw "Static zlib not found at $zlibStatic - run packaging\windows\build-gme.ps1 first (it builds zlib)."
}

# The source list + defines come from the vendored lib/libupse/CMakeLists.txt (mirrored in the Linux
# build-upse-min.sh). Neill/ holds the SPU core.
$SRCS = @(
    'upse_bios_manager','upse_container_xsf','upse_eventloop','upse_filesystem','upse_loader',
    'upse_loader_psf1','upse_loader_psf2','upse_module','upse_ps1_bios_base','upse_ps1_counters',
    'upse_ps1_dma_manager','upse_ps1_executive','upse_ps1_gpu','upse_ps1_hal','upse_ps1_memory_manager',
    'upse_ps1_spu_base','upse_ps1_spu_dma_handler','upse_ps1_spu_lowpass_filter',
    'upse_ps1_spu_nyquist_filter','upse_ps1_spu_register_io','upse_ps2_iop_base','upse_r3000_abstract',
    'upse_r3000_bytecode_evaluator','upse_string','upse_util','Neill/spu','Neill/spucore'
)

$work = New-Item -ItemType Directory -Force -Path (Join-Path ([IO.Path]::GetTempPath()) ("upse-" + [Guid]::NewGuid()))
try {
    $tgz = Join-Path $work 'upse.tar.gz'
    Write-Host "==> Fetching libupse (audiodecoder.upse @ $UPSE_COMMIT)"
    Invoke-WebRequest -Uri "https://github.com/xbmc/audiodecoder.upse/archive/$UPSE_COMMIT.tar.gz" -OutFile $tgz
    $got = (Get-FileHash -Algorithm SHA256 $tgz).Hash.ToLower()
    if ($got -ne $UPSE_SHA256.ToLower()) {
        throw "SHA256 mismatch for libupse`n  expected $UPSE_SHA256`n  got      $got"
    }
    tar -xf $tgz -C $work
    if ($LASTEXITCODE) { throw "extract libupse failed" }
    $lib = Join-Path $work "audiodecoder.upse-$UPSE_COMMIT\lib\libupse"

    $objDir = Join-Path $work 'obj'
    New-Item -ItemType Directory -Force -Path $objDir | Out-Null
    Write-Host "==> Compiling libupse ($($SRCS.Count) sources, MSVC static)"
    $objs = @()
    foreach ($s in $SRCS) {
        $src = Join-Path $lib ($s.Replace('/', '\') + '.c')
        $obj = Join-Path $objDir (($s.Replace('/', '_')) + '.obj')
        # WIN32_MSC selects libupse's MSVC typedefs/headers; EMU_* pick the little-endian emulator
        # config; _CRT_SECURE_NO_WARNINGS quiets the CRT deprecation noise.
        # /MD: dynamic CRT, to match Rust (and the cc-crate-compiled shim) so the FFI shares one CRT
        # heap - a static-CRT libupse triggers LNK4098 and risks cross-CRT alloc/free bugs.
        & cl /nologo /c /O2 /MD /DEMU_COMPILE /DEMU_LITTLE_ENDIAN /DWIN32_MSC /D_CRT_SECURE_NO_WARNINGS `
            /I"$lib" /I"$lib\Neill" /I"$zlibInc" "$src" /Fo"$obj"
        if ($LASTEXITCODE) { throw "cl failed on $s" }
        $objs += $obj
    }

    $outLib = Join-Path $UpseDir 'lib'
    $outInc = Join-Path $UpseDir 'include'
    New-Item -ItemType Directory -Force -Path $outLib, $outInc | Out-Null
    # One static lib carrying libupse + zlib, so build.rs's dylib=upse resolves everything.
    & lib /nologo "/OUT:$(Join-Path $outLib 'upse.lib')" @objs $zlibStatic
    if ($LASTEXITCODE) { throw "lib.exe combine failed" }
    Copy-Item (Join-Path $lib '*.h') $outInc -Force
} finally {
    Remove-Item -Recurse -Force $work -ErrorAction SilentlyContinue
}

if (-not (Test-Path (Join-Path $UpseDir 'lib\upse.lib'))) {
    throw "libupse build incomplete: missing lib\upse.lib under $UpseDir"
}
Write-Host "==> Done. libupse at $UpseDir"
