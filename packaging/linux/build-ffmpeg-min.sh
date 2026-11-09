#!/usr/bin/env sh
# Build a self-contained FFmpeg (shared) from source into a prefix, for the `ffmpeg`
# feature (WMA via crate::ffdec, and DTS once wired).
#
# Why from source: the distro's libavcodec is built against the full external stack
# (x264/x265/vpx/aom, GnuTLS/Kerberos, VA-API/VDPAU, librsvg, ...). Bundling that for a
# release is ~120 MB and a large ABI/security surface. This build keeps FFmpeg's own
# native codecs (so it decodes WMA, DTS and more) but links NO external libraries, so
# libavcodec depends only on libc and libm and the whole set is ~15 MB.
#
# The version is pinned to 4.4.x on purpose: our ffmpeg-next 5.1.1 does not compile
# against FFmpeg 6.x (removed fields / new enum variants), and 4.4 is the ABI we have
# validated WMA against. Keep this in sync with the Cargo ffmpeg-next version.
#
# Usage: build-ffmpeg-min.sh [PREFIX]   (default PREFIX: $PWD/build/ffmpeg-min)
# Idempotent: skips the build if the prefix already holds the libraries. Needs a C
# toolchain, make and curl; no nasm (x86 asm is disabled), no system FFmpeg.
set -eu

FFMPEG_VERSION=4.4.5
FFMPEG_SHA256=f9514e0d3515aee5a271283df71636e1d1ff7274b15853bcd84e144be416ab07
OUT="${1:-$PWD/build/ffmpeg-min}"

if ls "$OUT"/lib/libavcodec.so.* >/dev/null 2>&1; then
  echo "==> minimal FFmpeg already present at $OUT" >&2
  exit 0
fi

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
cd "$work"

echo "==> Fetching FFmpeg $FFMPEG_VERSION source" >&2
curl -fsSL --retry 5 --retry-delay 5 --retry-all-errors -o ffmpeg.tar.xz "https://ffmpeg.org/releases/ffmpeg-${FFMPEG_VERSION}.tar.xz"
echo "$FFMPEG_SHA256  ffmpeg.tar.xz" | sha256sum -c -
tar xf ffmpeg.tar.xz
cd "ffmpeg-${FFMPEG_VERSION}"

echo "==> Configuring self-contained FFmpeg (native codecs, no external libs) -> $OUT" >&2
# --disable-autodetect: never link an external library, even if its -dev headers are
#   present on the build host. Keeps libavcodec's deps to libc/libm.
# Drop the sub-libraries we do not use (avdevice/avfilter/swscale/postproc) and the
# programs/docs. The native codec set (WMA, DCA/DTS, ...) and all public headers stay,
# so ffmpeg-sys-next's bindgen has the headers it expects.
./configure --prefix="$OUT" \
  --disable-autodetect --disable-programs --disable-doc --disable-htmlpages --disable-manpages \
  --disable-network --disable-avdevice --disable-avfilter --disable-swscale --disable-postproc \
  --disable-x86asm --disable-debug --disable-static --enable-shared

echo "==> Building" >&2
make -j"$(nproc 2>/dev/null || echo 2)"
make install
echo "==> FFmpeg installed at $OUT ($(du -sh "$OUT/lib" | cut -f1))" >&2
