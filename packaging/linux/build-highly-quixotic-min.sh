#!/usr/bin/env sh
# Build Highly_Quixotic as a self-contained shared library for the `qsf` feature (Capcom QSound QSF
# arcade music, via crate::qsf + src/qsf_shim.c).
#
# Highly_Quixotic (kode54, GPL-3) emulates the Capcom Z80 + QSound DSP from a reverse-engineered,
# reimplemented-in-C algorithm - no copyrighted QSound DSP ROM (dl-1425.bin) is needed or shipped, so
# it plays ROM-less; the Z80 program and sample ROM come from inside the .qsf/.miniqsf file (parsed by
# the shared psflib). It has no standalone release, so we build it from the Kodi audiodecoder.qsf
# source tree pinned to a commit. psflib is built separately by build-psflib-min.sh and shared across
# the SSF/QSF/NCSF features. build.rs compiles the C shim and links this plus libpsflib
# (HIGHLYQUIXOTIC_DIR / PSFLIB_DIR, defaults build/highlyquixotic-min and build/psflib-min).
#
# Needs a C toolchain, zlib (system libz), and curl. Idempotent: skips when present.
set -eu

# audiodecoder.qsf pinned commit (vendors lib/highly_quixotic and lib/psflib).
QSF_COMMIT=ed24a15f9ed01cfdafe1f051895d11c5286a442f
QSF_SHA256=2b1aa52bc7b112c083affdc5649e33b6a28cd6d0bec53e755722020548e786da
OUT="${1:-$PWD/build/highlyquixotic-min}"

if ls "$OUT"/lib/libhighlyquixotic.so* >/dev/null 2>&1; then
  echo "==> Highly_Quixotic already present at $OUT" >&2
  exit 0
fi

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
cd "$work"

tarball="audiodecoder.qsf-${QSF_COMMIT}.tar.gz"
echo "==> Fetching Highly_Quixotic (audiodecoder.qsf @ ${QSF_COMMIT})" >&2
curl -fsSL --retry 5 --retry-delay 5 --retry-all-errors -o "$tarball" \
  "https://github.com/xbmc/audiodecoder.qsf/archive/${QSF_COMMIT}.tar.gz"
echo "$QSF_SHA256  $tarball" | sha256sum -c -
tar xzf "$tarball"
core="$work/audiodecoder.qsf-${QSF_COMMIT}/lib/highly_quixotic/Core"

echo "==> Building Highly_Quixotic (shared, ROM-less QSound) -> $OUT" >&2
# Source list and defines from lib/highly_quixotic/CMakeLists.txt.
SRCS="kabuki qmix qsound qsound_ctr z80"
objs=""
for s in $SRCS; do
  o="$work/$s.o"
  cc -c -O2 -fPIC -DEMU_COMPILE -DEMU_LITTLE_ENDIAN -I"$core" "$core/$s.c" -o "$o"
  objs="$objs $o"
done

mkdir -p "$OUT/lib" "$OUT/include"
# shellcheck disable=SC2086
cc -shared -o "$OUT/lib/libhighlyquixotic.so" $objs -lm
cp "$core"/*.h "$OUT/include/"
echo "==> Highly_Quixotic installed at $OUT ($(du -sh "$OUT/lib" | cut -f1))" >&2
