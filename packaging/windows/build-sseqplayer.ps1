<#
.SYNOPSIS
  Build SSEQPlayer as a static library for the `ncsf` feature on Windows (Nintendo DS NCSF music, via
  crate::ncsf + src/ncsf_shim.cpp). Mirrors packaging/linux/build-sseqplayer-min.sh.

.DESCRIPTION
  SSEQPlayer (kode54, WTFPL - permissive) is a Nitro SDAT/SSEQ soundbank sequencer: it reads the
  embedded SDAT (sequences + instrument banks + samples) and renders it with a software synth. It is
  NOT an emulator (no Nintendo DS BIOS or firmware), so it is fully ROM-less; the SDAT lives inside the
  .ncsf/.minincsf file, which the shared psflib unpacks. It has no standalone release, so we build it
  from the Kodi audiodecoder.ncsf source tree, pinned to the same commit as the Linux build. psflib is
  built separately by build-psflib.ps1 and shared across ssf/qsf/ncsf.

  This is C++ (unlike the C emulator cores). The one subtlety is wide-string conversion: SSEQPlayer's
  convert.h selects its char16/char32 codecvt path with
  `#if (defined(__GNUC__) || defined(__clang__)) && !defined(_LIBCPP_VERSION)` - the bundled codecvt,
  else `<codecvt>`. On MSVC neither __GNUC__ nor __clang__ is defined, so it already takes the
  `<codecvt>` std path and the bundled codecvt.h/wstring_convert.h self-guard to empty. We still define
  _LIBCPP_VERSION here, as build.rs does for the C++ shim, so the library and the shim agree on the
  codecvt ABI they share across the boundary. `<codecvt>` is deprecated from C++17, so we silence that
  (we build at the default C++14, where it is live) and pass /EHsc for convert.h's exceptions.

  Built STATIC and linked into the exe (nothing ships beside it). On MSVC a static .lib is consumed the
  same as an import lib, so build.rs's `rustc-link-lib=dylib=sseqplayer` links it unchanged. build.rs
  compiles the C++ shim (src/ncsf_shim.cpp, also _LIBCPP_VERSION-defined) against the installed headers
  and psflib's; the shared z.lib (installed by build-psflib.ps1) satisfies `dylib=z`. The headers
  install under an SSEQPlayer\ subdir because the shim includes <SSEQPlayer/Player.h>. Install layout
  (SSEQPLAYER_DIR, the prefix build.rs defaults to):

    <SseqDir>\lib\sseqplayer.lib          (static: the SDAT sequencer + synth)
    <SseqDir>\include\SSEQPlayer\*.h       (the shim includes <SSEQPlayer/Player.h>)

  Must run inside an MSVC developer environment (cl + lib on PATH). Idempotent: skips when present.

.PARAMETER SseqDir
  Install prefix. Default: <repo>\build\sseqplayer-min (the SSEQPLAYER_DIR default).
#>
[CmdletBinding()]
param(
    [string]$SseqDir
)
$ErrorActionPreference = 'Stop'

# audiodecoder.ncsf pinned commit (vendors lib/SSEQPlayer and lib/psflib), matching the Linux build.
$NCSF_COMMIT = 'c3c545a71255a7edceacc0d193d626f6ec83690a'
$NCSF_SHA256 = '7c537c06a8f32b1a3af2045d1893f5293c4ffc9ec5ac0b5b38b9fdd3dcef2599'

$repo = Resolve-Path (Join-Path $PSScriptRoot '..\..')
if (-not $SseqDir) { $SseqDir = Join-Path $repo 'build\sseqplayer-min' }

if (Test-Path (Join-Path $SseqDir 'lib\sseqplayer.lib')) {
    Write-Host "==> SSEQPlayer already present at $SseqDir"
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

# Source list from lib/SSEQPlayer/CMakeLists.txt (mirrored in build-sseqplayer-min.sh).
$SRCS = @('Channel', 'FATSection', 'INFOEntry', 'INFOSection', 'NDSStdHeader', 'Player', 'SBNK',
          'SDAT', 'SSEQ', 'SWAR', 'SWAV', 'SYMBSection', 'Track')

$work = New-Item -ItemType Directory -Force -Path (Join-Path ([IO.Path]::GetTempPath()) ("sseq-" + [Guid]::NewGuid()))
try {
    $tgz = Join-Path $work 'ncsf.tar.gz'
    Write-Host "==> Fetching SSEQPlayer (audiodecoder.ncsf @ $NCSF_COMMIT)"
    Invoke-Download "https://github.com/xbmc/audiodecoder.ncsf/archive/$NCSF_COMMIT.tar.gz" $tgz
    $got = (Get-FileHash -Algorithm SHA256 $tgz).Hash.ToLower()
    if ($got -ne $NCSF_SHA256.ToLower()) {
        throw "SHA256 mismatch for audiodecoder.ncsf`n  expected $NCSF_SHA256`n  got      $got"
    }
    tar -xf $tgz -C $work
    if ($LASTEXITCODE) { throw "extract audiodecoder.ncsf failed" }
    $lib = Join-Path $work "audiodecoder.ncsf-$NCSF_COMMIT\lib\SSEQPlayer"

    $objDir = Join-Path $work 'obj'
    New-Item -ItemType Directory -Force -Path $objDir | Out-Null
    Write-Host "==> Compiling SSEQPlayer ($($SRCS.Count) sources, MSVC static C++, ROM-less SDAT sequencer)"
    $objs = @()
    foreach ($s in $SRCS) {
        $src = Join-Path $lib ($s + '.cpp')
        $obj = Join-Path $objDir ($s + '.obj')
        # /std:c++14 (default; superset of SSEQPlayer's C++11, keeps <codecvt> live rather than
        # deprecated as in C++17), with the codecvt deprecation silenced as insurance. _LIBCPP_VERSION
        # keeps the lib and the cc-compiled shim on the same std::codecvt path. /EHsc for convert.h's
        # exceptions; /MD matches Rust's CRT.
        & cl /nologo /c /O2 /MD /EHsc /std:c++14 /D_LIBCPP_VERSION `
            /D_SILENCE_CXX17_CODECVT_HEADER_DEPRECATION_WARNING /D_CRT_SECURE_NO_WARNINGS `
            /I"$lib" "$src" /Fo"$obj"
        if ($LASTEXITCODE) { throw "cl failed on $s" }
        $objs += $obj
    }

    $outLib = Join-Path $SseqDir 'lib'
    $outInc = Join-Path $SseqDir 'include\SSEQPlayer'
    New-Item -ItemType Directory -Force -Path $outLib, $outInc | Out-Null
    & lib /nologo "/OUT:$(Join-Path $outLib 'sseqplayer.lib')" @objs
    if ($LASTEXITCODE) { throw "lib.exe (sseqplayer) failed" }
    # Headers under an SSEQPlayer\ subdir so the shim's <SSEQPlayer/Player.h> resolves from
    # <SseqDir>\include, matching the Linux build's include/SSEQPlayer layout.
    Copy-Item (Join-Path $lib '*.h') $outInc -Force
} finally {
    Remove-Item -Recurse -Force $work -ErrorAction SilentlyContinue
}

foreach ($rel in @('lib\sseqplayer.lib', 'include\SSEQPlayer\Player.h')) {
    if (-not (Test-Path (Join-Path $SseqDir $rel))) {
        throw "SSEQPlayer build incomplete: missing $rel under $SseqDir"
    }
}
Write-Host "==> Done. SSEQPlayer at $SseqDir"
