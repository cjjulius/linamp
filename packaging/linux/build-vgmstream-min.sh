#!/usr/bin/env sh
# Build libvgmstream as a self-contained shared library for the `vgmstream` feature (streamed
# game-engine audio: GameCube/Wii DSP, PS-ADPCM VAG, CRI HCA, Nintendo BRSTM/BCSTM/BFSTM, FMOD FSB,
# and many more, via crate::vgmstream which binds the C API directly - no shim).
#
# We build the CORE-ONLY configuration: no ffmpeg/vorbis/mpg123/celt/speex externals. That keeps the
# library fully permissive (vgmstream is ISC; its in-tree decoders are BSD/ISC/public-domain) and
# self-contained - the resulting libvgmstream.so depends only on libc/libm. The core already decodes
# the large majority of formats (their payloads are in-tree codecs: DSP/ADPCM, PS-ADPCM, HCA,
# ATRAC9, PCM). Formats whose payload needs an external codec (e.g. Vorbis-encoded FSB) just fail to
# open and the file is skipped; a later build tier can add those codecs.
#
# Bundled beside the binary (resolved via an $ORIGIN/lib rpath), the same way the other native
# features do. build.rs links against the prefix this installs to (VGMSTREAM_DIR, default
# build/vgmstream-min) and copies the .so into the target dir for dev and release runs.
#
# Pinned to vgmstream r2168. Needs cmake, a C toolchain, and curl. Idempotent: skips when present.
set -eu

VGMSTREAM_VERSION=r2168
VGMSTREAM_SHA256=6390ac616c4a248616f1b9bf6352fe949fcf2350a84844ba9416feac94f02225
OUT="${1:-$PWD/build/vgmstream-min}"

if ls "$OUT"/lib/libvgmstream.so* >/dev/null 2>&1; then
  echo "==> libvgmstream already present at $OUT" >&2
  exit 0
fi

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
cd "$work"

tarball="vgmstream-${VGMSTREAM_VERSION}.tar.gz"
echo "==> Fetching vgmstream $VGMSTREAM_VERSION source" >&2
curl -fsSL -o "$tarball" \
  "https://github.com/vgmstream/vgmstream/archive/refs/tags/${VGMSTREAM_VERSION}.tar.gz"
echo "$VGMSTREAM_SHA256  $tarball" | sha256sum -c -
tar xzf "$tarball"
src="$work/vgmstream-${VGMSTREAM_VERSION}"

echo "==> Building libvgmstream (core-only, shared) -> $OUT" >&2
# Core-only: disable every external codec and every front-end (CLI/plugins). CMAKE_POLICY_VERSION_MINIMUM
# lets CMake 4.x configure a tree whose minimum predates 3.5 (ignored by older CMake).
cmake -S "$src" -B "$work/build" \
  -DCMAKE_BUILD_TYPE=Release \
  -DBUILD_SHARED_LIBS=ON \
  -DBUILD_CLI=OFF -DBUILD_V123=OFF -DBUILD_AUDACIOUS=OFF \
  -DUSE_VORBIS=OFF -DUSE_MPEG=OFF -DUSE_FFMPEG=OFF -DUSE_CELT=OFF -DUSE_SPEEX=OFF \
  -DCMAKE_POLICY_VERSION_MINIMUM=3.5
cmake --build "$work/build" --target libvgmstream_shared -j"$(nproc 2>/dev/null || echo 2)"

mkdir -p "$OUT/lib" "$OUT/include"
cp "$work/build/src/libvgmstream.so" "$OUT/lib/"
cp "$src/src/libvgmstream.h" "$src/src/libvgmstream_streamfile.h" "$OUT/include/"
echo "==> libvgmstream installed at $OUT ($(du -sh "$OUT/lib" | cut -f1))" >&2
