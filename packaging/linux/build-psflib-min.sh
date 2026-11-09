#!/usr/bin/env sh
# Build psflib as a self-contained shared library. psflib (kode54, MIT) is the shared PSF-family
# container/tag parser used by the SSF/DSF, QSF and NCSF decoders (it unpacks the zlib-compressed
# .ssf/.qsf/.ncsf wrapper and reads the [TAG] block); the per-core emulator libs link against it.
#
# It has no standalone release, so we build it from the Kodi audiodecoder.ssf source tree (which
# vendors lib/psflib) pinned to a commit, the same way build-upse-min.sh builds libupse. One shared
# libpsflib.so is produced and bundled beside the binary (resolved via an $ORIGIN/lib rpath); each
# core shim links -lpsflib. build.rs points at the prefix this installs to (PSFLIB_DIR, default
# build/psflib-min).
#
# Needs a C toolchain, zlib (system libz), and curl. Idempotent: skips when present.
set -eu

# audiodecoder.ssf pinned commit (vendors lib/psflib and lib/Highly_Theoretical).
SSF_COMMIT=692ace7752e84dedfd76a741266fbe778df845ef
SSF_SHA256=8e362a15b9a1620c9bccfe9ec62fa08f570a76ae626c55e2c07de8b1ad8377e5
OUT="${1:-$PWD/build/psflib-min}"

if ls "$OUT"/lib/libpsflib.so* >/dev/null 2>&1; then
  echo "==> psflib already present at $OUT" >&2
  exit 0
fi

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
cd "$work"

tarball="audiodecoder.ssf-${SSF_COMMIT}.tar.gz"
echo "==> Fetching psflib (audiodecoder.ssf @ ${SSF_COMMIT})" >&2
curl -fsSL --retry 5 --retry-delay 5 --retry-all-errors -o "$tarball" \
  "https://github.com/xbmc/audiodecoder.ssf/archive/${SSF_COMMIT}.tar.gz"
echo "$SSF_SHA256  $tarball" | sha256sum -c -
tar xzf "$tarball"
lib="$work/audiodecoder.ssf-${SSF_COMMIT}/lib/psflib"

echo "==> Building psflib (shared) -> $OUT" >&2
# Source list from lib/psflib/CMakeLists.txt.
objs=""
for s in psf2fs psflib; do
  o="$work/$s.o"
  cc -c -O2 -fPIC -I"$lib" "$lib/$s.c" -o "$o"
  objs="$objs $o"
done

mkdir -p "$OUT/lib" "$OUT/include"
# shellcheck disable=SC2086
cc -shared -o "$OUT/lib/libpsflib.so" $objs -lz
cp "$lib"/*.h "$OUT/include/"
echo "==> psflib installed at $OUT ($(du -sh "$OUT/lib" | cut -f1))" >&2
