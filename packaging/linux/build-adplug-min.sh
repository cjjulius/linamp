#!/usr/bin/env sh
# Build AdPlug (and its libbinio dependency) as self-contained shared libraries for the `adplug`
# feature (AdLib / OPL2 / OPL3 FM-synthesis music: DRO, CMF, RAD, HSC, ROL-less set, Doom MUS, and
# dozens more via crate::adplug + src/adplug_shim.cpp).
#
# AdPlug is LGPL-2.1+ and depends only on libbinio (also LGPL-2.1+) plus libstdc++, so we build both
# from source and bundle libadplug.so + libbinio.so beside the binary (resolved via an $ORIGIN/lib
# rpath), exactly like build-gme-min.sh / build-openmpt-min.sh. build.rs compiles the C++ shim and
# links against the prefix this installs to (ADPLUG_DIR, default build/adplug-min), then copies the
# .so files into the target dir for dev and release runs.
#
# Pinned to AdPlug 2.4 / libbinio 1.5. Needs a C++ toolchain and pkg-config; no system AdPlug.
# Idempotent: skips the build when the prefix already holds the library.
set -eu

ADPLUG_VERSION=2.4
ADPLUG_SHA256=de18463bf7c0cb639a3228ad47e69eb7f78a5a197802d325f3a5ed7e1c56d57f
BINIO_VERSION=1.5
BINIO_SHA256=398b2468e7838d2274d1f62dbc112e7e043433812f7ae63ef29f5cb31dc6defd
OUT="${1:-$PWD/build/adplug-min}"

if ls "$OUT"/lib/libadplug*.so* >/dev/null 2>&1; then
  echo "==> AdPlug already present at $OUT" >&2
  exit 0
fi

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
cd "$work"

echo "==> Fetching libbinio $BINIO_VERSION and AdPlug $ADPLUG_VERSION" >&2
curl -fsSL -o libbinio.tar.bz2 \
  "https://github.com/adplug/libbinio/releases/download/libbinio-${BINIO_VERSION}/libbinio-${BINIO_VERSION}.tar.bz2"
echo "$BINIO_SHA256  libbinio.tar.bz2" | sha256sum -c -
curl -fsSL -o adplug.tar.bz2 \
  "https://github.com/adplug/adplug/releases/download/adplug-${ADPLUG_VERSION}/adplug-${ADPLUG_VERSION}.tar.bz2"
echo "$ADPLUG_SHA256  adplug.tar.bz2" | sha256sum -c -

# libbinio first: AdPlug finds it through pkg-config.
echo "==> Building libbinio (shared) -> $OUT" >&2
tar xjf libbinio.tar.bz2
(cd "libbinio-${BINIO_VERSION}" && ./configure --prefix="$OUT" --disable-static --enable-shared && \
  make -j"$(nproc 2>/dev/null || echo 2)" && make install)

echo "==> Building AdPlug (shared) -> $OUT" >&2
export PKG_CONFIG_PATH="$OUT/lib/pkgconfig${PKG_CONFIG_PATH:+:$PKG_CONFIG_PATH}"
tar xjf adplug.tar.bz2
(cd "adplug-${ADPLUG_VERSION}" && ./configure --prefix="$OUT" --disable-static --enable-shared && \
  make -j"$(nproc 2>/dev/null || echo 2)" && make install)

echo "==> AdPlug installed at $OUT ($(du -sh "$OUT/lib" | cut -f1))" >&2
