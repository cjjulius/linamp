#!/usr/bin/env sh
# Build libopenmpt as a self-contained shared library for the `openmpt` feature (the tracker
# "zoo": ~30 module formats beyond xmrs's mod/xm/s3m/it, via crate::openmpt).
#
# libopenmpt is BSD-3-Clause and self-contained for module playback, so we build it from source
# and bundle libopenmpt.so beside the binary (resolved via an $ORIGIN/lib rpath), exactly like
# build-gme-min.sh does for libgme. build.rs links against the prefix this installs to
# (OPENMPT_DIR, default build/openmpt-min) and copies the .so into the target dir for dev and
# release runs.
#
# Pinned to 0.8.9. The optional decoders (MP3/Ogg/Vorbis sample support, the openmpt123 player,
# the examples and the test suite) are all disabled: module playback needs none of them, and
# this keeps the build dependency-free (just a C++ toolchain). Idempotent: skips the build when
# the prefix already holds the library.
set -eu

OPENMPT_VERSION=0.8.9
OPENMPT_SHA256=d7ce84fd05d686c4bcf66af40eae857afa371442db60eeda3f874bd6cf6fc318
OUT="${1:-$PWD/build/openmpt-min}"

if ls "$OUT"/lib/libopenmpt.so* >/dev/null 2>&1; then
  echo "==> libopenmpt already present at $OUT" >&2
  exit 0
fi

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
cd "$work"

tarball="libopenmpt-${OPENMPT_VERSION}+release.autotools.tar.gz"
echo "==> Fetching libopenmpt $OPENMPT_VERSION source" >&2
curl -fsSL -o "$tarball" "https://lib.openmpt.org/files/libopenmpt/src/${tarball}"
echo "$OPENMPT_SHA256  $tarball" | sha256sum -c -
tar xzf "$tarball"
cd "libopenmpt-${OPENMPT_VERSION}+release.autotools"

echo "==> Configuring libopenmpt (shared, module playback only) -> $OUT" >&2
./configure --prefix="$OUT" \
  --disable-openmpt123 --disable-examples --disable-tests --disable-doxygen \
  --without-mpg123 --without-ogg --without-vorbis --without-vorbisfile \
  --without-flac --without-sndfile --without-portaudio --without-portaudiocpp --without-zlib

echo "==> Building" >&2
make -j"$(nproc 2>/dev/null || echo 2)"
make install
echo "==> libopenmpt installed at $OUT ($(du -sh "$OUT/lib" | cut -f1))" >&2
