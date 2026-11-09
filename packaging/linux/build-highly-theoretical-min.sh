#!/usr/bin/env sh
# Build Highly_Theoretical as a self-contained shared library for the `ssf` feature (Sega Saturn SSF
# and Dreamcast DSF music, via crate::ssf + src/ssf_shim.c).
#
# Highly_Theoretical (kode54, GPL-3) emulates only the Saturn SCSP and Dreamcast AICA sound chips
# (shared yam.c) with a Musashi 68000 core (Saturn) and an ARM7 core (Dreamcast) - no console BIOS or
# game ROM, so it plays ROM-less, the same principle as the PSF1/libupse path. The driver program and
# samples come from inside the .ssf/.dsf file itself (parsed by the shared psflib). It has no
# standalone release, so we build it from the Kodi audiodecoder.ssf source tree pinned to a commit.
# We compile only the emulator core (psflib is built separately by build-psflib-min.sh and shared
# across the SSF/QSF/NCSF features). build.rs compiles the C shim and links this plus libpsflib
# (HIGHLYTHEORETICAL_DIR / PSFLIB_DIR, defaults build/highlytheoretical-min and build/psflib-min).
#
# Needs a C toolchain, zlib (system libz), and curl. Idempotent: skips when present.
set -eu

# audiodecoder.ssf pinned commit (vendors lib/Highly_Theoretical and lib/psflib).
SSF_COMMIT=692ace7752e84dedfd76a741266fbe778df845ef
SSF_SHA256=8e362a15b9a1620c9bccfe9ec62fa08f570a76ae626c55e2c07de8b1ad8377e5
OUT="${1:-$PWD/build/highlytheoretical-min}"

if ls "$OUT"/lib/libhighlytheoretical.so* >/dev/null 2>&1; then
  echo "==> Highly_Theoretical already present at $OUT" >&2
  exit 0
fi

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
cd "$work"

tarball="audiodecoder.ssf-${SSF_COMMIT}.tar.gz"
echo "==> Fetching Highly_Theoretical (audiodecoder.ssf @ ${SSF_COMMIT})" >&2
curl -fsSL --retry 5 --retry-delay 5 --retry-all-errors -o "$tarball" \
  "https://github.com/xbmc/audiodecoder.ssf/archive/${SSF_COMMIT}.tar.gz"
echo "$SSF_SHA256  $tarball" | sha256sum -c -
tar xzf "$tarball"
core="$work/audiodecoder.ssf-${SSF_COMMIT}/lib/Highly_Theoretical/Core"

echo "==> Building Highly_Theoretical (shared, ROM-less SCSP/AICA) -> $OUT" >&2
# Source list and defines from lib/Highly_Theoretical/CMakeLists.txt: Musashi 68000 (USE_M68K, with
# the pre-generated m68kops.c - no codegen step), ARM7, the shared SCSP/AICA core (yam.c), and the
# Saturn/Dreamcast front-ends. EMU_COMPILE/EMU_LITTLE_ENDIAN select the little-endian emulator config.
SRCS="sega m68k/m68kops m68k/m68kcpu arm dcsound yam satsound"
objs=""
for s in $SRCS; do
  o="$work/$(echo "$s" | tr '/' '_').o"
  cc -c -O2 -fPIC -DEMU_COMPILE -DEMU_LITTLE_ENDIAN -DUSE_M68K -DLSB_FIRST -DHAVE_MPROTECT \
    -I"$core" -I"$core/m68k" "$core/$s.c" -o "$o"
  objs="$objs $o"
done

mkdir -p "$OUT/lib" "$OUT/include"
# shellcheck disable=SC2086
cc -shared -o "$OUT/lib/libhighlytheoretical.so" $objs -lm
cp "$core"/*.h "$OUT/include/"
echo "==> Highly_Theoretical installed at $OUT ($(du -sh "$OUT/lib" | cut -f1))" >&2
