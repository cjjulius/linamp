#!/usr/bin/env sh
# Build SSEQPlayer as a self-contained shared library for the `ncsf` feature (Nintendo DS NCSF music,
# via crate::ncsf + src/ncsf_shim.cpp).
#
# SSEQPlayer (kode54, WTFPL - permissive) is a Nitro SDAT/SSEQ soundbank sequencer: it reads the
# embedded SDAT (sequences + instrument banks + samples) and renders it with a software synth. It is
# NOT an emulator - no Nintendo DS BIOS or firmware is involved, so it is fully ROM-less. The SDAT lives
# inside the .ncsf/.minincsf file, which the shared psflib unpacks (resolving `_lib` chains). It has no
# standalone release, so we build it from the Kodi audiodecoder.ncsf source tree pinned to a commit.
# psflib is built separately by build-psflib-min.sh and shared across the SSF/QSF/NCSF features.
# build.rs compiles the C++ shim and links this plus libpsflib (SSEQPLAYER_DIR / PSFLIB_DIR, defaults
# build/sseqplayer-min and build/psflib-min).
#
# Needs a C++ toolchain, zlib (system libz), and curl. Idempotent: skips when present.
set -eu

# audiodecoder.ncsf pinned commit (vendors lib/SSEQPlayer and lib/psflib).
NCSF_COMMIT=c3c545a71255a7edceacc0d193d626f6ec83690a
NCSF_SHA256=7c537c06a8f32b1a3af2045d1893f5293c4ffc9ec5ac0b5b38b9fdd3dcef2599
OUT="${1:-$PWD/build/sseqplayer-min}"

if ls "$OUT"/lib/libsseqplayer.so* >/dev/null 2>&1; then
  echo "==> SSEQPlayer already present at $OUT" >&2
  exit 0
fi

: "${CXX:=c++}"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
cd "$work"

tarball="audiodecoder.ncsf-${NCSF_COMMIT}.tar.gz"
echo "==> Fetching SSEQPlayer (audiodecoder.ncsf @ ${NCSF_COMMIT})" >&2
curl -fsSL --retry 5 --retry-delay 5 --retry-all-errors -o "$tarball" \
  "https://github.com/xbmc/audiodecoder.ncsf/archive/${NCSF_COMMIT}.tar.gz"
echo "$NCSF_SHA256  $tarball" | sha256sum -c -
tar xzf "$tarball"
lib="$work/audiodecoder.ncsf-${NCSF_COMMIT}/lib/SSEQPlayer"

# The CMake defines -D_LIBCPP_VERSION when <codecvt> is available, to select SSEQPlayer's std::codecvt
# path for SDAT symbol-name conversion. <codecvt> is present (deprecated) on libstdc++, so match that.
CODECVT=""
if printf '#include <codecvt>\nint main(){return 0;}\n' | "$CXX" -std=c++11 -x c++ - -o /dev/null 2>/dev/null; then
  CODECVT="-D_LIBCPP_VERSION"
fi

echo "==> Building SSEQPlayer (shared, ROM-less SDAT sequencer) -> $OUT" >&2
# Source list from lib/SSEQPlayer/CMakeLists.txt.
SRCS="Channel FATSection INFOEntry INFOSection NDSStdHeader Player SBNK SDAT SSEQ SWAR SWAV SYMBSection Track"
objs=""
for s in $SRCS; do
  o="$work/$s.o"
  "$CXX" -c -O2 -fPIC -std=c++11 $CODECVT -I"$lib" "$lib/$s.cpp" -o "$o"
  objs="$objs $o"
done

mkdir -p "$OUT/lib" "$OUT/include/SSEQPlayer"
# shellcheck disable=SC2086
"$CXX" -shared -o "$OUT/lib/libsseqplayer.so" $objs -lm
cp "$lib"/*.h "$OUT/include/SSEQPlayer/"
echo "==> SSEQPlayer installed at $OUT ($(du -sh "$OUT/lib" | cut -f1))" >&2
