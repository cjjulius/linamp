#!/usr/bin/env sh
# Build libgsf as a self-contained shared library for the `gsf` feature (Game Boy Advance GSF music,
# via crate::gsf + src/gsf_shim.c).
#
# libgsf (chrg, MPL-2.0) is a small GSF player library that statically bundles a trimmed mGBA (also
# MPL-2.0) and exposes a clean high-level C API (gsf_new/gsf_load_file/gsf_play/gsf_seek/...). It plays
# ROM-less: mGBA's built-in HLE BIOS stands in for the GBA BIOS (useBios=false), so no copyrighted
# Nintendo BIOS is needed or shipped; the game's audio program lives inside the .gsf/.minigsf file, which
# libgsf parses itself (it does NOT use psflib - it resolves the `_lib` overlay chain and PSF tags
# internally). Unlike the other *SF cores, this one has NO standalone source list we can hand-compile:
# mGBA is consumed through its own CMake, so this build is CMake-driven (and needs a C++20 compiler),
# the one build-tooling difference in the *SF family. build.rs compiles the C shim and links this
# (LIBGSF_DIR, default build/libgsf-min). No psflib, no shared-core dependency.
#
# Pinned by git commit (libgsf) + the mGBA submodule gitlink libgsf locks (so the mGBA commit is
# reproducible from the libgsf checkout). The upstream is a git repo with a submodule, so this fetches
# with git rather than a release tarball.
#
# Needs git, cmake (>= 3.15), a C++20 toolchain, zlib, and network access. Idempotent: skips when present.
set -eu

# chrg/libgsf pinned commit, and the mGBA commit its submodule gitlink points at (asserted below).
LIBGSF_COMMIT=df71d7142c79775e3df2e2d0416779177e9509f0
MGBA_COMMIT=11e16d1ad9fe8b7140312475afee60d4c7aa9742
OUT="${1:-$PWD/build/libgsf-min}"

if ls "$OUT"/lib/libgsf.so* >/dev/null 2>&1; then
  echo "==> libgsf already present at $OUT" >&2
  exit 0
fi

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
cd "$work"

echo "==> Fetching libgsf (codeberg.org/chrg/libgsf @ ${LIBGSF_COMMIT})" >&2
git clone --quiet https://codeberg.org/chrg/libgsf.git src
git -C src checkout --quiet "$LIBGSF_COMMIT"
echo "==> Fetching the pinned mGBA submodule" >&2
git -C src submodule update --init --depth 1 external/mgba
got_mgba="$(git -C src/external/mgba rev-parse HEAD)"
if [ "$got_mgba" != "$MGBA_COMMIT" ]; then
  echo "!!  mGBA submodule is $got_mgba, expected $MGBA_COMMIT" >&2
  exit 1
fi

echo "==> Building libgsf (CMake, shared, ROM-less HLE-BIOS GBA) -> $OUT" >&2
# SHARED library by default; BUILD_EXAMPLES=OFF drops the SDL2 example so only zlib is needed.
cmake -S src -B src/build -DCMAKE_BUILD_TYPE=Release -DBUILD_EXAMPLES=OFF >/dev/null
cmake --build src/build -j"$(nproc 2>/dev/null || echo 2)" >/dev/null

mkdir -p "$OUT/lib" "$OUT/include"
cp src/build/libgsf.so "$OUT/lib/libgsf.so"
cp src/include/gsf.h "$OUT/include/gsf.h"
echo "==> libgsf installed at $OUT ($(du -sh "$OUT/lib" | cut -f1))" >&2
