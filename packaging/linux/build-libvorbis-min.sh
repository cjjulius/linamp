#!/usr/bin/env sh
# Build libogg + libvorbis as self-contained shared libraries, for the `vgmstream-ext` feature: they
# let the bundled libvgmstream decode Vorbis-payload game audio (Wwise .wem Vorbis, FMOD FSB Vorbis),
# which the core-only vgmstream build skips. vgmstream feeds raw Vorbis packets to libvorbis's low-level
# API (it does NOT go through FFmpeg for these), so USE_VORBIS=ON needs libvorbis + libogg present.
#
# libogg and libvorbis are Xiph.Org, BSD-3-Clause (fully permissive). The resulting .so files depend
# only on libc/libm and each other, and are bundled beside the binary via the $ORIGIN/lib rpath, like
# the other native deps. build-vgmstream-min.sh points cmake at $LIBVORBIS_DIR (this prefix) when
# VGM_EXT=1; build.rs bundles the three .so files when the vgmstream-ext feature is on.
#
# Needs cmake, a C toolchain, and curl. Idempotent: skips when present.
set -eu

OGG_VER=1.3.5
OGG_SHA256=0eb4b4b9420a0f51db142ba3f9c64b333f826532dc0f48c6410ae51f4799b664
VORBIS_VER=1.3.7
VORBIS_SHA256=0e982409a9c3fc82ee06e08205b1355e5c6aa4c36bca58146ef399621b0ce5ab
OUT="${1:-$PWD/build/libvorbis-min}"

if ls "$OUT"/lib/libvorbis.so* >/dev/null 2>&1; then
  echo "==> libvorbis already present at $OUT" >&2
  exit 0
fi

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
cd "$work"

echo "==> Fetching libogg $OGG_VER + libvorbis $VORBIS_VER (Xiph, BSD)" >&2
curl -fsSL --retry 5 --retry-delay 5 --retry-all-errors -o ogg.tgz \
  "https://downloads.xiph.org/releases/ogg/libogg-${OGG_VER}.tar.gz"
echo "$OGG_SHA256  ogg.tgz" | sha256sum -c -
curl -fsSL --retry 5 --retry-delay 5 --retry-all-errors -o vorbis.tgz \
  "https://downloads.xiph.org/releases/vorbis/libvorbis-${VORBIS_VER}.tar.gz"
echo "$VORBIS_SHA256  vorbis.tgz" | sha256sum -c -
tar xzf ogg.tgz
tar xzf vorbis.tgz

mkdir -p "$OUT"
echo "==> Building libogg (shared) -> $OUT" >&2
cmake -S "libogg-${OGG_VER}" -B b-ogg -DCMAKE_BUILD_TYPE=Release -DBUILD_SHARED_LIBS=ON \
  -DCMAKE_INSTALL_PREFIX="$OUT" -DINSTALL_DOCS=OFF -DCMAKE_POLICY_VERSION_MINIMUM=3.5 >/dev/null
cmake --build b-ogg -j"$(nproc 2>/dev/null || echo 2)" >/dev/null
cmake --install b-ogg >/dev/null

echo "==> Building libvorbis (shared, against the libogg above) -> $OUT" >&2
cmake -S "libvorbis-${VORBIS_VER}" -B b-vorbis -DCMAKE_BUILD_TYPE=Release -DBUILD_SHARED_LIBS=ON \
  -DCMAKE_INSTALL_PREFIX="$OUT" -DCMAKE_PREFIX_PATH="$OUT" -DCMAKE_POLICY_VERSION_MINIMUM=3.5 >/dev/null
cmake --build b-vorbis -j"$(nproc 2>/dev/null || echo 2)" >/dev/null
cmake --install b-vorbis >/dev/null

echo "==> libogg + libvorbis installed at $OUT ($(du -sh "$OUT/lib" | cut -f1))" >&2
