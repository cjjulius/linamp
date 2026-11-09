#!/usr/bin/env sh
# Build vio2sf as a self-contained shared library for the `twosf` feature (Nintendo DS 2SF music,
# via crate::twosf + src/twosf_shim.cpp).
#
# vio2sf (kode54, GPL-2.0-or-later) is a DeSmuME-derived Nintendo DS emulator cut down to just what 2SF
# playback needs: the ARM7 + ARM9 CPUs (pure interpreters), the MMU, and the SPU. It runs the game's
# actual ARM sound driver, so it covers DS titles whose audio cannot be cleanly extracted to NCSF. It
# plays ROM-less - DeSmuME's HLE BIOS SWI tables stand in for the DS BIOS and a blank in-memory
# firmware-settings block is synthesized, so no copyrighted DS BIOS or firmware image is ever needed or
# shipped; the game ROM + save state live inside the .2sf/.mini2sf file, which the shared psflib unpacks
# (resolving `_lib` chains). psflib is built separately by build-psflib-min.sh and shared across the
# SSF/QSF/NCSF/USF/2SF features. build.rs compiles the C++ shim and links this plus libpsflib
# (VIO2SF_DIR / PSFLIB_DIR, defaults build/vio2sf-min and build/psflib-min).
#
# This is the heaviest core (full DS CPU + SPU emulation). It is a pure interpreter (no x86 dynarec), so
# one portable recipe builds on x86_64/arm64; only resampler.c takes an SSE define on x86.
#
# Needs a C and C++ toolchain and curl. Idempotent: skips when present.
set -eu

# xbmc/audiodecoder.2sf pinned commit (vendors lib/vio2sf and lib/psflib).
TSF_COMMIT=039eeb7de76bf28d20c64ab4ef778982e70c639d
TSF_SHA256=2860e418295be02f5bc0619b7d5b3ea652971e84f681c3a843e871d6ef71addb
OUT="${1:-$PWD/build/vio2sf-min}"

if ls "$OUT"/lib/libvio2sf.so* >/dev/null 2>&1; then
  echo "==> vio2sf already present at $OUT" >&2
  exit 0
fi

: "${CC:=cc}"
: "${CXX:=c++}"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
cd "$work"

tarball="audiodecoder.2sf-${TSF_COMMIT}.tar.gz"
echo "==> Fetching vio2sf (audiodecoder.2sf @ ${TSF_COMMIT})" >&2
curl -fsSL --retry 5 --retry-delay 5 --retry-all-errors -o "$tarball" \
  "https://github.com/xbmc/audiodecoder.2sf/archive/${TSF_COMMIT}.tar.gz"
echo "$TSF_SHA256  $tarball" | sha256sum -c -
tar xzf "$tarball"
dm="$work/audiodecoder.2sf-${TSF_COMMIT}/lib/vio2sf/src/vio2sf/desmume"

# SSE for the resampler on x86 (matches the CMake's check_cxx_symbol_exists(__SSE__) gate).
case "$(uname -m)" in
  x86_64|amd64) SSE="-msse" ;;
  *) SSE="" ;;
esac

echo "==> Building vio2sf (shared, ROM-less DS emulator) -> $OUT" >&2
# C source list from lib/vio2sf/CMakeLists.txt (SPU.cpp built separately as C++).
CSRCS="armcpu arm_instructions barray bios cp15 FIFO GPU isqrt matrix mc MMU NDSSystem resampler state thumb_instructions"
objs=""
for s in $CSRCS; do
  o="$work/$s.o"
  simd=""
  [ "$s" = "resampler" ] && simd="$SSE"
  # shellcheck disable=SC2086
  "$CC" -c -O2 -g0 -fPIC $simd -I"$dm" "$dm/$s.c" -o "$o"
  objs="$objs $o"
done
"$CXX" -c -O2 -g0 -fPIC -I"$dm" "$dm/SPU.cpp" -o "$work/SPU.o"
objs="$objs $work/SPU.o"

mkdir -p "$OUT/lib" "$OUT/include/vio2sf"
# Link with the C++ driver so libstdc++ is pulled in for SPU.cpp.
# shellcheck disable=SC2086
"$CXX" -shared -o "$OUT/lib/libvio2sf.so" $objs -lm
# The shim includes "state.h", which pulls in the other desmume headers; install them all (flat).
cp "$dm"/*.h "$OUT/include/vio2sf/"
echo "==> vio2sf installed at $OUT ($(du -sh "$OUT/lib" | cut -f1))" >&2
