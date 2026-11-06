# WMA / FFmpeg build on Windows

WMA playback is an opt-in build behind the `ffmpeg` Cargo feature (the `ffdec`
module), which links the system FFmpeg libraries (LGPL). The default build is
pure Rust with no FFmpeg, so `cargo build` and `cargo test` and the normal
installer are unchanged. This note covers building the WMA variant on Windows.

## What you need

1. MSVC toolchain and the Windows SDK (Visual Studio 2022 or the Build Tools,
   "Desktop development with C++"). The SDK provides `stdint.h` and the rest of
   the system headers that bindgen needs.
2. An FFmpeg shared dev build (headers, import `.lib`, and the DLLs) for a major
   version `ffmpeg-next` 5.1.1 supports. FFmpeg 4.x through 6.x work. Verified
   with gyan.dev's `ffmpeg-6.1.1-full_build-shared` (its `ffmpeg-sys-next` probe
   reports `ffmpeg_5_1`). The `.zip` has `bin/`, `include/`, and `lib/`.
   https://github.com/GyanD/codexffmpeg/releases/download/6.1.1/ffmpeg-6.1.1-full_build-shared.zip
3. LLVM/`libclang`, because `ffmpeg-sys-next` runs bindgen. A normal LLVM install
   (for example `winget install LLVM.LLVM`, which lands in `C:\Program Files\LLVM`)
   gives a `libclang.dll` that `clang-sys` loads reliably.

## Building locally

Build from a Visual Studio developer shell so the MSVC headers are on `INCLUDE`
(a plain shell fails bindgen with "'stdint.h' file not found"). In a
"x64 Native Tools Command Prompt for VS 2022", or after running
`VsDevCmd.bat -arch=x64 -host_arch=x64`:

```
set FFMPEG_DIR=C:\path\to\ffmpeg-6.1.1-full_build-shared
set INCLUDE=%FFMPEG_DIR%\include;%INCLUDE%
set LIB=%FFMPEG_DIR%\lib;%LIB%
set PATH=%FFMPEG_DIR%\bin;%PATH%
set LIBCLANG_PATH=C:\Program Files\LLVM\bin
cargo build --release --features ffmpeg
```

`FFMPEG_DIR` lets `ffmpeg-sys-next` find the libs and headers; adding
`include`/`lib` to `INCLUDE`/`LIB` also satisfies the build's `check.c` probe and
the linker, and `bin` on `PATH` lets the linked binary load the DLLs at run time.

To run the WMA-enabled binary outside that shell, keep the FFmpeg DLLs next to
`linamp.exe` (or on `PATH`).

## CI

`.github/workflows/ci.yml` has an optional `ffmpeg-build` job (both OSes,
`continue-on-error`, so it never gates a merge). On Windows it uses
`ilammy/msvc-dev-cmd` for the MSVC environment, downloads the FFmpeg 6.1.1 shared
build, sets `FFMPEG_DIR`/`INCLUDE`/`LIB`/`PATH`, and points `LIBCLANG_PATH` at the
runner's bundled LLVM. On Linux it installs the `libav*-dev` and `libclang-dev`
packages. The job has something to build once the `ffmpeg` feature is on the
branch.

## Installer

`packaging\windows\linamp.iss` bundles the FFmpeg DLLs next to the exe only when
built for WMA. A normal installer build is unchanged; for a WMA-enabled installer
pass both defines:

```
ISCC /DMyAppVersion=<x.y.z> /DWithFfmpeg /DFfmpegBin="C:\path\to\ffmpeg\bin" packaging\windows\linamp.iss
```

## Note for this machine

The Windows QA box currently has no admin rights, so LLVM is not installed there
(the `ffmpeg-build` CI job and any machine with LLVM are unaffected). The FFmpeg
6.1.1 shared build and the FFmpeg version compatibility with `ffmpeg-next` 5.1.1
are confirmed; a full local WMA build on that box is pending either an IT-provided
LLVM install or running the build through CI.
