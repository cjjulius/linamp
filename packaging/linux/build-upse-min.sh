#!/usr/bin/env sh
# Build libupse as a self-contained shared library for the `psf` feature (PlayStation PSF1/PSF2 music,
# via crate::psf + src/psf_shim.c).
#
# libupse ("Unix PlayStation Sound Emulator", GPL) ships its own HLE "FakeBIOS", so it plays PSF
# ROM-less: no PlayStation BIOS (copyrighted, which we never ship) is needed. It has no standalone
# release, so we build it from the Kodi audiodecoder.upse source tree (which vendors libupse under
# lib/libupse) pinned to a commit. We compile the library sources directly into libupse.so and bundle
# it beside the binary (resolved via an $ORIGIN/lib rpath), the same way the other native features do.
# build.rs compiles the C shim and links against the prefix this installs to (UPSE_DIR, default
# build/upse-min).
#
# Needs a C toolchain, zlib (system libz), and curl. Idempotent: skips when present.
set -eu

# audiodecoder.upse pinned commit (vendors libupse under lib/libupse).
UPSE_COMMIT=221dc3985eb5721e324bd67b738debe6527ebcb3
UPSE_SHA256=4c79ffa062d8f70cb60f80c9f95adb5c3616d1741da52972381f3153a5aa3696
OUT="${1:-$PWD/build/upse-min}"

if ls "$OUT"/lib/libupse.so* >/dev/null 2>&1; then
  echo "==> libupse already present at $OUT" >&2
  exit 0
fi

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
cd "$work"

tarball="audiodecoder.upse-${UPSE_COMMIT}.tar.gz"
echo "==> Fetching libupse (audiodecoder.upse @ ${UPSE_COMMIT})" >&2
curl -fsSL -o "$tarball" \
  "https://github.com/xbmc/audiodecoder.upse/archive/${UPSE_COMMIT}.tar.gz"
echo "$UPSE_SHA256  $tarball" | sha256sum -c -
tar xzf "$tarball"
lib="$work/audiodecoder.upse-${UPSE_COMMIT}/lib/libupse"

echo "==> Building libupse (shared, ROM-less FakeBIOS) -> $OUT" >&2
# Source list and defines from the vendored lib/libupse/CMakeLists.txt. The Neill/ subdir holds the
# SPU core; EMU_COMPILE/EMU_LITTLE_ENDIAN select the emulator config for a little-endian host.
SRCS="upse_bios_manager upse_container_xsf upse_eventloop upse_filesystem upse_loader
      upse_loader_psf1 upse_loader_psf2 upse_module upse_ps1_bios_base upse_ps1_counters
      upse_ps1_dma_manager upse_ps1_executive upse_ps1_gpu upse_ps1_hal upse_ps1_memory_manager
      upse_ps1_spu_base upse_ps1_spu_dma_handler upse_ps1_spu_lowpass_filter
      upse_ps1_spu_nyquist_filter upse_ps1_spu_register_io upse_ps2_iop_base upse_r3000_abstract
      upse_r3000_bytecode_evaluator upse_string upse_util Neill/spu Neill/spucore"
objs=""
for s in $SRCS; do
  o="$work/$(echo "$s" | tr '/' '_').o"
  cc -c -O2 -fPIC -DEMU_COMPILE -DEMU_LITTLE_ENDIAN -I"$lib" -I"$lib/Neill" "$lib/$s.c" -o "$o"
  objs="$objs $o"
done

mkdir -p "$OUT/lib" "$OUT/include"
# shellcheck disable=SC2086
cc -shared -o "$OUT/lib/libupse.so" $objs -lz -lm
cp "$lib"/*.h "$OUT/include/"
echo "==> libupse installed at $OUT ($(du -sh "$OUT/lib" | cut -f1))" >&2
