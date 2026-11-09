#!/usr/bin/env sh
# Build lazyusf2 as a self-contained shared library for the `usf` feature (Nintendo 64 USF music,
# via crate::usf + src/usf_shim.c).
#
# lazyusf2 (kode54, GPL-2.0-or-later) is a Mupen64Plus-derived N64 emulator cut down to just what USF
# playback needs: the R4300 CPU (cached interpreter), the RSP, and the RSP audio HLE/LLE. It plays
# ROM-less - a USF carries a Project64 save state captured after boot plus the game's audio program, so
# no copyrighted N64 PIF/boot ROM is ever needed or shipped; audio is produced by High-Level Emulation
# of the RSP microcode. The program/save-state lives inside the .usf/.miniusf file, which the shared
# psflib unpacks (resolving `_lib` chains). psflib is built separately by build-psflib-min.sh and shared
# across the SSF/QSF/NCSF/USF features. build.rs compiles the C shim and links this plus libpsflib
# (LAZYUSF_DIR / PSFLIB_DIR, defaults build/lazyusf2-min and build/psflib-min).
#
# IMPORTANT (licensing): we build from kode54/lazyusf2 (via the derselbst fork), NOT the Kodi
# xbmc/audiodecoder.usf tree. That tree vendors the ORIGINAL lazyusf, whose core CPU files carry
# zilmar's Project64 "freeware for PERSONAL USE only / non-commercial" license - a field-of-use
# restriction we never ship. lazyusf2 replaced that core with Mupen64Plus's GPL R4300, so it is cleanly
# GPL-2.0-or-later (same copyleft posture as the Highly_Theoretical/Highly_Quixotic cores).
#
# Built with the cached interpreter (no x86 dynarec): a music player does not need the JIT, and the
# interpreter is one portable recipe across x86_64/arm64 with no runtime code generation. Only
# rsp_lle/rsp.c takes a SIMD define (SSE2 on x86_64, NEON on arm64).
#
# Needs a C toolchain and curl. Idempotent: skips when present.
set -eu

# derselbst/lazyusf2 pinned commit (maintained fork of kode54/lazyusf2; adds arm64 build support).
USF_COMMIT=00d19c9b1e0ae4924d395310844a94d871743c89
USF_SHA256=c2e838c0c690fada630ece1ce2fcf6a4b90537a46377f2092e4f8f22cc0a7609
OUT="${1:-$PWD/build/lazyusf2-min}"

if ls "$OUT"/lib/liblazyusf.so* >/dev/null 2>&1; then
  echo "==> lazyusf2 already present at $OUT" >&2
  exit 0
fi

: "${CC:=cc}"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
cd "$work"

tarball="lazyusf2-${USF_COMMIT}.tar.gz"
echo "==> Fetching lazyusf2 (derselbst/lazyusf2 @ ${USF_COMMIT})" >&2
curl -fsSL --retry 5 --retry-delay 5 --retry-all-errors -o "$tarball" \
  "https://github.com/derselbst/lazyusf2/archive/${USF_COMMIT}.tar.gz"
echo "$USF_SHA256  $tarball" | sha256sum -c -
tar xzf "$tarball"
src="$work/lazyusf2-${USF_COMMIT}"

# SIMD define for the low-level RSP (rsp_lle/rsp.c), matching the upstream Makefile's ROPTS.
case "$(uname -m)" in
  x86_64|amd64) RSP_SIMD="-DARCH_MIN_SSE2" ;;
  aarch64|arm64) RSP_SIMD="-DARCH_MIN_ARM_NEON" ;;
  *) RSP_SIMD="" ;;
esac

echo "==> Building lazyusf2 (shared, ROM-less, cached interpreter) -> $OUT" >&2
# Upstream Makefile OBJS minus the x86 recompiler, plus r4300/empty_dynarec.c (the no-dynarec stubs).
SRCS="ai/ai_controller api/callbacks debugger/dbg_decoder main/main main/rom main/savestates \
main/util memory/memory pi/cart_rom pi/pi_controller r4300/cached_interp r4300/cp0 r4300/cp1 \
r4300/empty_dynarec r4300/exception r4300/interupt r4300/mi_controller r4300/pure_interp r4300/r4300 \
r4300/r4300_core r4300/recomp r4300/reset r4300/tlb rdp/rdp_core ri/rdram ri/rdram_detection_hack \
ri/ri_controller rsp/rsp_core rsp_hle/alist rsp_hle/alist_audio rsp_hle/alist_naudio \
rsp_hle/alist_nead rsp_hle/audio rsp_hle/cicx105 rsp_hle/hle rsp_hle/hvqm rsp_hle/jpeg \
rsp_hle/memory rsp_hle/mp3 rsp_hle/musyx rsp_hle/plugin rsp_hle/re2 si/cic si/game_controller \
si/n64_cic_nus_6105 si/pif si/si_controller usf/usf usf/barray usf/resampler vi/vi_controller"

objs=""
for s in $SRCS; do
  o="$work/$(echo "$s" | tr '/' '_').o"
  "$CC" -c -O2 -g0 -fPIC -I"$src" "$src/$s.c" -o "$o"
  objs="$objs $o"
done
# rsp_lle/rsp.c needs the SIMD define.
o="$work/rsp_lle_rsp.o"
# shellcheck disable=SC2086
"$CC" -c -O2 -g0 -fPIC $RSP_SIMD -I"$src" "$src/rsp_lle/rsp.c" -o "$o"
objs="$objs $o"

mkdir -p "$OUT/lib" "$OUT/include"
# shellcheck disable=SC2086
"$CC" -shared -o "$OUT/lib/liblazyusf.so" $objs -lm
cp "$src/usf/usf.h" "$OUT/include/usf.h"
echo "==> lazyusf2 installed at $OUT ($(du -sh "$OUT/lib" | cut -f1))" >&2
