<#
.SYNOPSIS
  Build libogg + libvorbis as static libraries for the `vgmstream-ext` feature on Windows: they let the
  extended libvgmstream decode Vorbis-payload game audio (Wwise .wem Vorbis, FMOD FSB Vorbis), which the
  core-only vgmstream build skips. Mirrors packaging/linux/build-libvorbis-min.sh.

.DESCRIPTION
  libogg and libvorbis are Xiph.Org, BSD-3-Clause (fully permissive). vgmstream (built with USE_VORBIS=ON
  by build-vgmstream.ps1 -Ext) feeds raw Vorbis packets to libvorbis's low-level API; the decode code is
  compiled into vgmstream.lib against vgmstream's own bundled ext_includes headers, and the vorbis/ogg
  symbols are resolved at the final linamp.exe link. So, like the other Windows native deps, we build
  these STATIC (ogg.lib, vorbis.lib, vorbisfile.lib) and build.rs links them into the exe - no DLLs to
  ship (unlike the Linux build, which ships the .so files). CMake builds each cleanly on MSVC.

  libogg is built first and installed, then libvorbis is built against it (CMAKE_PREFIX_PATH points at
  this same prefix so libvorbis's find_package(Ogg) resolves). Install layout (LIBVORBIS_DIR, the prefix
  build.rs defaults to for the vgmstream-ext bundling, and which build-vgmstream.ps1 points cmake at):

    <LibvorbisDir>\lib\ogg.lib + vorbis.lib + vorbisfile.lib   (static)
    <LibvorbisDir>\include\ogg\*.h + vorbis\*.h                 (for vgmstream's find + our link)

  Must run inside an MSVC developer environment (cl + nmake on PATH). CMake required. Idempotent: skips
  when present.

.PARAMETER LibvorbisDir
  Install prefix. Default: <repo>\build\libvorbis-min (the LIBVORBIS_DIR default).
#>
[CmdletBinding()]
param(
    [string]$LibvorbisDir
)
$ErrorActionPreference = 'Stop'

$OGG_VER     = '1.3.5'
$OGG_SHA256  = '0eb4b4b9420a0f51db142ba3f9c64b333f826532dc0f48c6410ae51f4799b664'
$VORBIS_VER  = '1.3.7'
$VORBIS_SHA256 = '0e982409a9c3fc82ee06e08205b1355e5c6aa4c36bca58146ef399621b0ce5ab'

$repo = Resolve-Path (Join-Path $PSScriptRoot '..\..')
if (-not $LibvorbisDir) { $LibvorbisDir = Join-Path $repo 'build\libvorbis-min' }

if (Test-Path (Join-Path $LibvorbisDir 'lib\vorbis.lib')) {
    Write-Host "==> libvorbis already present at $LibvorbisDir"
    return
}

# Download with retries. Portable across Windows PowerShell 5.1 and pwsh 7.
function Invoke-Download([string]$Uri, [string]$OutFile) {
    for ($i = 1; $i -le 5; $i++) {
        try { Invoke-WebRequest -Uri $Uri -OutFile $OutFile; return }
        catch { if ($i -ge 5) { throw }; Start-Sleep -Seconds 5 }
    }
}

$gen  = 'NMake Makefiles'
$work = New-Item -ItemType Directory -Force -Path (Join-Path ([IO.Path]::GetTempPath()) ("vorbis-" + [Guid]::NewGuid()))
try {
    $oggTgz = Join-Path $work 'ogg.tar.gz'
    $vorbisTgz = Join-Path $work 'vorbis.tar.gz'
    Write-Host "==> Fetching libogg $OGG_VER + libvorbis $VORBIS_VER (Xiph, BSD)"
    Invoke-Download "https://downloads.xiph.org/releases/ogg/libogg-$OGG_VER.tar.gz" $oggTgz
    if (((Get-FileHash -Algorithm SHA256 $oggTgz).Hash.ToLower()) -ne $OGG_SHA256) { throw "SHA256 mismatch for libogg" }
    Invoke-Download "https://downloads.xiph.org/releases/vorbis/libvorbis-$VORBIS_VER.tar.gz" $vorbisTgz
    if (((Get-FileHash -Algorithm SHA256 $vorbisTgz).Hash.ToLower()) -ne $VORBIS_SHA256) { throw "SHA256 mismatch for libvorbis" }
    & "$env:SystemRoot\System32\tar.exe" -xf $oggTgz -C $work
    if ($LASTEXITCODE) { throw "extract libogg failed" }
    & "$env:SystemRoot\System32\tar.exe" -xf $vorbisTgz -C $work
    if ($LASTEXITCODE) { throw "extract libvorbis failed" }
    $oggSrc    = Join-Path $work "libogg-$OGG_VER"
    $vorbisSrc = Join-Path $work "libvorbis-$VORBIS_VER"

    Write-Host "==> Building libogg (static) -> $LibvorbisDir"
    cmake -S $oggSrc -B (Join-Path $work 'b-ogg') -G $gen `
        "-DCMAKE_BUILD_TYPE=Release" "-DBUILD_SHARED_LIBS=OFF" `
        "-DCMAKE_INSTALL_PREFIX=$LibvorbisDir" "-DINSTALL_DOCS=OFF" `
        "-DCMAKE_POLICY_VERSION_MINIMUM=3.5"
    if ($LASTEXITCODE) { throw "cmake configure (ogg) failed" }
    cmake --build (Join-Path $work 'b-ogg')
    if ($LASTEXITCODE) { throw "cmake build (ogg) failed" }
    cmake --install (Join-Path $work 'b-ogg')
    if ($LASTEXITCODE) { throw "cmake install (ogg) failed" }

    Write-Host "==> Building libvorbis (static, against the libogg above) -> $LibvorbisDir"
    cmake -S $vorbisSrc -B (Join-Path $work 'b-vorbis') -G $gen `
        "-DCMAKE_BUILD_TYPE=Release" "-DBUILD_SHARED_LIBS=OFF" `
        "-DCMAKE_INSTALL_PREFIX=$LibvorbisDir" "-DCMAKE_PREFIX_PATH=$LibvorbisDir" `
        "-DCMAKE_POLICY_VERSION_MINIMUM=3.5"
    if ($LASTEXITCODE) { throw "cmake configure (vorbis) failed" }
    cmake --build (Join-Path $work 'b-vorbis')
    if ($LASTEXITCODE) { throw "cmake build (vorbis) failed" }
    cmake --install (Join-Path $work 'b-vorbis')
    if ($LASTEXITCODE) { throw "cmake install (vorbis) failed" }
} finally {
    Remove-Item -Recurse -Force $work -ErrorAction SilentlyContinue
}

foreach ($rel in @('lib\ogg.lib', 'lib\vorbis.lib', 'lib\vorbisfile.lib', 'include\vorbis\vorbisfile.h')) {
    if (-not (Test-Path (Join-Path $LibvorbisDir $rel))) {
        throw "libvorbis build incomplete: missing $rel under $LibvorbisDir"
    }
}
Write-Host "==> Done. libogg + libvorbis at $LibvorbisDir"
