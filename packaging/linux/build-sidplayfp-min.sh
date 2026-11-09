#!/usr/bin/env sh
# Build libsidplayfp as a self-contained shared library for the `sid` feature (Commodore 64 SID
# music, .sid PSID/RSID, via crate::sid + src/sid_shim.cpp).
#
# libsidplayfp is GPL-2.0+ and bundles the reSIDfp engine (no separate libresidfp), so one shared
# library plays everything. We build it from source and bundle libsidplayfp.so beside the binary
# (resolved via an $ORIGIN/lib rpath), the same way build-openmpt-min.sh / build-adplug-min.sh do.
# build.rs compiles the C++ shim and links against the prefix this installs to (SIDPLAYFP_DIR,
# default build/sidplayfp-min), then copies the .so into the target dir for dev and release runs.
#
# libsidplayfp optionally uses libgcrypt for tune MD5 hashing (only needed for the HVSC song-length
# database, which we do not ship). When libgcrypt-dev is absent at configure time it falls back to a
# built-in MD5, so built in a minimal environment the result depends only on libc/libstdc++. If the
# build host has libgcrypt, the resulting .so links the system libgcrypt/libgpg-error, which are
# near-universal base libraries; the bundle still ships only libsidplayfp.so and resolves those from
# the system (same as libc/libstdc++).
#
# Pinned to libsidplayfp 2.16.1. The 2.x series bundles the reSIDfp engine in-tree (one shared
# library, no separate libresidfp), and src/sid_shim.cpp targets the 2.x API. libsidplayfp 3.x split
# reSIDfp into a standalone libresidfp package and changed that API, so it neither links as a single
# library here nor compiles against the shim; do not bump to 3.x without updating the shim.
# Needs a C++ toolchain and pkg-config; no system libsidplayfp. Idempotent: skips when present.
set -eu

SIDPLAYFP_VERSION=2.16.1
SIDPLAYFP_SHA256=ace0f73c2ef8645ab069ce1b298b10e31e36af7b5996109983b2b67ad60ff3ca
OUT="${1:-$PWD/build/sidplayfp-min}"

if ls "$OUT"/lib/libsidplayfp.so* >/dev/null 2>&1; then
  echo "==> libsidplayfp already present at $OUT" >&2
  exit 0
fi

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
cd "$work"

tarball="libsidplayfp-${SIDPLAYFP_VERSION}.tar.gz"
echo "==> Fetching libsidplayfp $SIDPLAYFP_VERSION source" >&2
curl -fsSL --retry 5 --retry-delay 5 --retry-all-errors -o "$tarball" \
  "https://github.com/libsidplayfp/libsidplayfp/releases/download/v${SIDPLAYFP_VERSION}/${tarball}"
echo "$SIDPLAYFP_SHA256  $tarball" | sha256sum -c -
tar xzf "$tarball"
cd "libsidplayfp-${SIDPLAYFP_VERSION}"

echo "==> Building libsidplayfp (shared, reSIDfp) -> $OUT" >&2
# Shared library only; no tests, no hardware (exSID/HardSID) backends.
./configure --prefix="$OUT" --disable-static --enable-shared --disable-tests --without-exsid
make -j"$(nproc 2>/dev/null || echo 2)"
make install
echo "==> libsidplayfp installed at $OUT ($(du -sh "$OUT/lib" | cut -f1))" >&2
