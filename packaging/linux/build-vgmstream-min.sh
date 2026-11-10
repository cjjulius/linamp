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

# Optional external-codec tier (the `vgmstream-ext` feature). VGM_EXT=1 builds libvgmstream with
# USE_VORBIS (Wwise/FSB Vorbis via libvorbis, BSD) + USE_FFMPEG (XMA/ATRAC3/WMA via the LGPL ffmpeg
# LinAmp already builds). It then DT_NEEDs libvorbis/libogg (from $LIBVORBIS_DIR) and the same
# libavcodec/format/util sonames the `ffmpeg` feature bundles (from $FFMPEG_DIR), resolved via the
# binary's $ORIGIN/lib rpath. An `.ext` marker records that the installed lib is the extended build so
# a later core-only call does not treat it as satisfied (and vice versa).
VGM_EXT="${VGM_EXT:-0}"
LIBVORBIS_DIR="${LIBVORBIS_DIR:-$PWD/build/libvorbis-min}"
FFMPEG_DIR="${FFMPEG_DIR:-$PWD/build/ffmpeg-min}"

if ls "$OUT"/lib/libvgmstream.so* >/dev/null 2>&1; then
  if { [ "$VGM_EXT" = 1 ] && [ -f "$OUT/.ext" ]; } || { [ "$VGM_EXT" != 1 ] && [ ! -f "$OUT/.ext" ]; }; then
    echo "==> libvgmstream already present at $OUT (ext=$VGM_EXT)" >&2
    exit 0
  fi
  echo "==> rebuilding libvgmstream at $OUT for a different codec set (ext=$VGM_EXT)" >&2
  rm -f "$OUT"/lib/libvgmstream.so* "$OUT/.ext"
fi

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
cd "$work"

tarball="vgmstream-${VGMSTREAM_VERSION}.tar.gz"
echo "==> Fetching vgmstream $VGMSTREAM_VERSION source" >&2
curl -fsSL --retry 5 --retry-delay 5 --retry-all-errors -o "$tarball" \
  "https://github.com/vgmstream/vgmstream/archive/refs/tags/${VGMSTREAM_VERSION}.tar.gz"
echo "$VGMSTREAM_SHA256  $tarball" | sha256sum -c -
tar xzf "$tarball"
src="$work/vgmstream-${VGMSTREAM_VERSION}"

# CMAKE_POLICY_VERSION_MINIMUM lets CMake 4.x configure a tree whose minimum predates 3.5 (ignored by
# older CMake). Core-only disables every external codec; VGM_EXT=1 turns on Vorbis + FFmpeg.
if [ "$VGM_EXT" = 1 ]; then
  echo "==> Building libvgmstream (external tier: Vorbis + FFmpeg, shared) -> $OUT" >&2
  EXT_FLAGS="-DUSE_VORBIS=ON -DUSE_FFMPEG=ON"
  export PKG_CONFIG_PATH="$FFMPEG_DIR/lib/pkgconfig:$LIBVORBIS_DIR/lib/pkgconfig:${PKG_CONFIG_PATH:-}"
  # vgmstream's vorbis.cmake links bare -lvorbis/-lvorbisfile/-logg; put their lib dir on the linker
  # search path (ffmpeg is found with full paths via pkg-config, but add it too for good measure).
  export LIBRARY_PATH="$LIBVORBIS_DIR/lib:$FFMPEG_DIR/lib:${LIBRARY_PATH:-}"
  PREFIX_PATH="$FFMPEG_DIR;$LIBVORBIS_DIR"
else
  echo "==> Building libvgmstream (core-only, shared) -> $OUT" >&2
  EXT_FLAGS="-DUSE_VORBIS=OFF -DUSE_FFMPEG=OFF"
  PREFIX_PATH=""
fi
# shellcheck disable=SC2086
cmake -S "$src" -B "$work/build" \
  -DCMAKE_BUILD_TYPE=Release \
  -DBUILD_SHARED_LIBS=ON \
  -DBUILD_CLI=OFF -DBUILD_V123=OFF -DBUILD_AUDACIOUS=OFF \
  $EXT_FLAGS -DUSE_MPEG=OFF -DUSE_CELT=OFF -DUSE_SPEEX=OFF \
  -DCMAKE_PREFIX_PATH="$PREFIX_PATH" \
  -DCMAKE_POLICY_VERSION_MINIMUM=3.5
cmake --build "$work/build" --target libvgmstream_shared -j"$(nproc 2>/dev/null || echo 2)"

mkdir -p "$OUT/lib" "$OUT/include"
cp "$work/build/src/libvgmstream.so" "$OUT/lib/"
cp "$src/src/libvgmstream.h" "$src/src/libvgmstream_streamfile.h" "$OUT/include/"
[ "$VGM_EXT" = 1 ] && touch "$OUT/.ext" || true
echo "==> libvgmstream installed at $OUT ($(du -sh "$OUT/lib" | cut -f1))" >&2
