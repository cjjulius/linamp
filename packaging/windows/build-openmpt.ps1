<#
.SYNOPSIS
  Provision libopenmpt for the `openmpt` feature (the tracker "zoo": ~28 module formats via
  crate::openmpt) on Windows.

.DESCRIPTION
  The Windows counterpart to packaging/linux/build-openmpt-min.sh. libopenmpt has no MSVC
  build from the autotools source the Linux script uses, and the project ships an official,
  MSVC-built development package, so on Windows we consume that prebuilt the same way CI
  consumes the prebuilt FFmpeg shared build rather than compiling from source. It lays the
  package out as an OPENMPT_DIR prefix:

    <OutDir>\lib\openmpt.lib          (the import library, renamed from libopenmpt.lib)
    <OutDir>\bin\libopenmpt.dll       (+ its codec dependency DLLs, see below)
    <OutDir>\licenses\...             (the bundled component licenses)

  build.rs links `openmpt` from <OutDir>\lib; an import library records the DLL name it was
  built against internally, so renaming the file to openmpt.lib still makes the exe load
  libopenmpt.dll. The official DLL imports its codec helpers (openmpt-mpg123/ogg/vorbis/zlib)
  at load time, so all of them ship beside the exe even though the routed module formats use
  only PCM samples. (The Linux build is dependency-free; this is the one platform difference,
  and it is functionally identical for every format LinAmp routes to libopenmpt.)

  Pinned to 0.8.9 to match the Linux build. Idempotent: skips when the prefix already holds
  the import library.

.PARAMETER OutDir
  Install prefix. Default: <repo>\build\openmpt-min (the OPENMPT_DIR build.rs defaults to).
#>
[CmdletBinding()]
param(
    [string]$OutDir
)
$ErrorActionPreference = 'Stop'

$OPENMPT_VERSION = '0.8.9'
$ZIP = "libopenmpt-$OPENMPT_VERSION+release.dev.windows.vs2022.zip"
$ZIP_SHA256 = 'a23e375e576e3a6997ffeeb6cf34488e9020e2a5d8226856b02628b6a87c5e8e'
# Architecture subfolder inside the dev package (x64).
$ARCH = 'amd64'
# The DLLs that must ship beside the exe: libopenmpt.dll plus the codec helpers it imports.
$DLLS = @('libopenmpt.dll', 'openmpt-mpg123.dll', 'openmpt-ogg.dll', 'openmpt-vorbis.dll', 'openmpt-zlib.dll')

$repo = Resolve-Path (Join-Path $PSScriptRoot '..\..')
if (-not $OutDir) { $OutDir = Join-Path $repo 'build\openmpt-min' }

if (Test-Path (Join-Path $OutDir 'lib\openmpt.lib')) {
    Write-Host "==> libopenmpt already present at $OutDir"
    return
}

$work = New-Item -ItemType Directory -Force -Path (Join-Path ([IO.Path]::GetTempPath()) ("openmpt-" + [Guid]::NewGuid()))
try {
    $zipPath = Join-Path $work $ZIP
    Write-Host "==> Fetching libopenmpt $OPENMPT_VERSION dev package"
    Invoke-WebRequest -Uri "https://lib.openmpt.org/files/libopenmpt/dev/$ZIP" -OutFile $zipPath
    $got = (Get-FileHash -Algorithm SHA256 $zipPath).Hash.ToLower()
    if ($got -ne $ZIP_SHA256) {
        throw "SHA256 mismatch for $ZIP`n  expected $ZIP_SHA256`n  got      $got"
    }
    $ex = Join-Path $work 'ex'
    Expand-Archive -Path $zipPath -DestinationPath $ex -Force

    $lib = Join-Path $OutDir 'lib'
    $bin = Join-Path $OutDir 'bin'
    $lic = Join-Path $OutDir 'licenses'
    New-Item -ItemType Directory -Force -Path $lib, $bin, $lic | Out-Null

    # Rename the import library to openmpt.lib so build.rs's `dylib=openmpt` finds it; the DLL
    # name it references (libopenmpt.dll) is recorded inside the lib and is unaffected.
    Copy-Item (Join-Path $ex "lib\$ARCH\libopenmpt.lib") (Join-Path $lib 'openmpt.lib') -Force
    foreach ($d in $DLLS) {
        Copy-Item (Join-Path $ex "bin\$ARCH\$d") (Join-Path $bin $d) -Force
    }
    if (Test-Path (Join-Path $ex 'Licenses')) {
        Copy-Item (Join-Path $ex 'Licenses\*') $lic -Recurse -Force
    }
    Write-Host "==> libopenmpt laid out at $OutDir"
} finally {
    Remove-Item -Recurse -Force $work -ErrorAction SilentlyContinue
}
