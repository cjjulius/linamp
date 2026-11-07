#!/usr/bin/env sh
# Build Game Music Emu (libgme) as a self-contained shared library for the `gme` feature
# (chiptune/console music: NSF, GBS, VGM, SPC, AY, SAP, KSS, HES, GYM via crate::gme).
#
# libgme is small (~400 KB) and LGPL-2.1+, with no external dependencies beyond libc/libstdc++,
# so we build it from source and bundle libgme.so beside the binary (resolved via an
# $ORIGIN/lib rpath), exactly like build-ffmpeg-min.sh does for libav*. build.rs links against
# the prefix this installs to (GME_DIR, default build/gme-min) and copies the .so into the
# target dir for dev/release runs.
#
# Pinned to 0.6.3 to match what has been validated. Needs a C/C++ toolchain and cmake; no
# system libgme. Idempotent: skips the build if the prefix already holds the library.
set -eu

GME_VERSION=0.6.3
GME_SHA256=626e8a104e0dadd10ef6519a67aca880c7b40f81471659f1935b61754e12fc7b
OUT="${1:-$PWD/build/gme-min}"

if ls "$OUT"/lib/libgme.so* >/dev/null 2>&1; then
  echo "==> libgme already present at $OUT" >&2
  exit 0
fi

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
cd "$work"

echo "==> Fetching game-music-emu $GME_VERSION source" >&2
curl -fsSL -o gme.tar.gz "https://bitbucket.org/mpyne/game-music-emu/downloads/game-music-emu-${GME_VERSION}.tar.gz"
echo "$GME_SHA256  gme.tar.gz" | sha256sum -c -
tar xzf gme.tar.gz
cd "game-music-emu-${GME_VERSION}"

echo "==> Configuring libgme (shared, release) -> $OUT" >&2
# CMAKE_POLICY_VERSION_MINIMUM=3.5: libgme 0.6.3's cmake_minimum_required predates modern
# CMake, which otherwise refuses to configure. Build the library only (no SDL player demo).
cmake -S . -B bld \
  -DCMAKE_BUILD_TYPE=Release \
  -DBUILD_SHARED_LIBS=ON \
  -DENABLE_UBSAN=OFF \
  -DCMAKE_POLICY_VERSION_MINIMUM=3.5 \
  -DCMAKE_INSTALL_PREFIX="$OUT"

echo "==> Building" >&2
cmake --build bld -j"$(nproc 2>/dev/null || echo 2)"
cmake --install bld
echo "==> libgme installed at $OUT ($(du -sh "$OUT/lib" | cut -f1))" >&2
