#!/usr/bin/env sh
# Build the per-user Linux tarball: dist/linamp-<version>-linux-x86_64.tar.gz
# Run on Linux (or in CI). Requires the usual build deps (libasound2-dev,
# libgtk-3-dev, pkg-config) plus the Rust toolchain.
set -eu

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

VERSION="$(grep -m1 '^version' Cargo.toml | sed -E 's/.*"([^"]+)".*/\1/')"

# Optional Cargo features. Set FEATURES to build a variant, example:
#   FEATURES=ffmpeg ./packaging/linux/build-tarball.sh   # adds WMA (links libav*)
# The default (no FEATURES) is the pure-Rust build. A featured build gets a name
# suffix so it never overwrites the default tarball. A build that links FFmpeg (the
# ffmpeg feature) also bundles the FFmpeg shared libraries below, so the install is
# self-contained and needs no system FFmpeg at runtime.
FEATURES="${FEATURES:-}"
if [ -n "$FEATURES" ]; then
  SUFFIX="-$(echo "$FEATURES" | tr ', ' '--')"
  FEATURE_ARGS="--features $FEATURES"
else
  SUFFIX=""
  FEATURE_ARGS=""
fi
case ",$FEATURES," in
  *,ffmpeg,*) BUNDLE_FFMPEG=1 ;;
  *) BUNDLE_FFMPEG=0 ;;
esac
# The gme feature (chiptune/console music) links libgme, which is bundled the same way as
# FFmpeg so the install is self-contained. Pass FEATURES=ffmpeg,gme for the full build.
case ",$FEATURES," in
  *,gme,*) BUNDLE_GME=1 ;;
  *) BUNDLE_GME=0 ;;
esac
# The openmpt feature (the tracker "zoo") links libopenmpt, bundled the same self-contained way.
# Pass FEATURES=ffmpeg,gme,openmpt for the full build.
case ",$FEATURES," in
  *,openmpt,*) BUNDLE_OPENMPT=1 ;;
  *) BUNDLE_OPENMPT=0 ;;
esac
# The adplug feature (AdLib/OPL FM music) links libadplug + libbinio, bundled the same self-contained
# way. Pass FEATURES=ffmpeg,gme,openmpt,adplug for the full build.
case ",$FEATURES," in
  *,adplug,*) BUNDLE_ADPLUG=1 ;;
  *) BUNDLE_ADPLUG=0 ;;
esac
# The sid feature (Commodore 64 SID music) links libsidplayfp, bundled the same self-contained way.
case ",$FEATURES," in
  *,sid,*) BUNDLE_SID=1 ;;
  *) BUNDLE_SID=0 ;;
esac
# The vgmstream feature (game-engine streams) links libvgmstream (core-only), bundled the same way.
case ",$FEATURES," in
  *,vgmstream,*) BUNDLE_VGMSTREAM=1 ;;
  *) BUNDLE_VGMSTREAM=0 ;;
esac
# The psf feature (PlayStation PSF1/PSF2) links libupse (ROM-less), bundled the same way.
case ",$FEATURES," in
  *,psf,*) BUNDLE_PSF=1 ;;
  *) BUNDLE_PSF=0 ;;
esac
NAME="linamp-${VERSION}-linux-x86_64${SUFFIX}"
STAGE="dist/$NAME"

# A bundled-FFmpeg build links WMA/DTS against a self-contained FFmpeg built from
# source (packaging/linux/build-ffmpeg-min.sh), not the distro's libavcodec: the
# distro build drags ~120 MB of external codec/network/crypto libraries, whereas the
# from-source one keeps the native codecs but links no external libraries, so its
# libav* depend only on libc/libm and the bundle is ~15 MB. FFMPEG_DIR points
# ffmpeg-sys-next at it. The $ORIGIN/lib rpath (DT_RPATH via --disable-new-dtags, so
# it also covers the libraries' own siblings) makes the binary load the bundled
# libav* from next to itself at runtime, so no system FFmpeg is needed.
if [ "$BUNDLE_FFMPEG" = 1 ]; then
  FFMPEG_MIN="$ROOT/build/ffmpeg-min"
  sh "$ROOT/packaging/linux/build-ffmpeg-min.sh" "$FFMPEG_MIN"
  export FFMPEG_DIR="$FFMPEG_MIN"
  export PKG_CONFIG_PATH="$FFMPEG_MIN/lib/pkgconfig${PKG_CONFIG_PATH:+:$PKG_CONFIG_PATH}"
  export RUSTFLAGS="${RUSTFLAGS:-} -C link-arg=-Wl,-rpath,\$ORIGIN/lib -C link-arg=-Wl,--disable-new-dtags"
fi

# libgme (chiptune/console music) is small (~400 KB, LGPL) and built from source the same way;
# build.rs links GME_DIR and emits the $ORIGIN/lib rpath itself, so it is self-contained even in
# a gme-only build. We bundle its .so into $STAGE/lib below.
if [ "$BUNDLE_GME" = 1 ]; then
  GME_MIN="$ROOT/build/gme-min"
  sh "$ROOT/packaging/linux/build-gme-min.sh" "$GME_MIN"
  export GME_DIR="$GME_MIN"
fi

# libopenmpt (the tracker "zoo") is built from source the same way (BSD-3-Clause, self-contained
# for module playback); build.rs links OPENMPT_DIR and emits the $ORIGIN/lib rpath itself. We
# bundle its .so into $STAGE/lib below.
if [ "$BUNDLE_OPENMPT" = 1 ]; then
  OPENMPT_MIN="$ROOT/build/openmpt-min"
  sh "$ROOT/packaging/linux/build-openmpt-min.sh" "$OPENMPT_MIN"
  export OPENMPT_DIR="$OPENMPT_MIN"
fi

# AdPlug (AdLib/OPL FM music) + its libbinio dependency, built from source the same way; build.rs
# compiles the C++ shim, links ADPLUG_DIR, and emits the $ORIGIN/lib rpath itself. We bundle both
# .so files into $STAGE/lib below.
if [ "$BUNDLE_ADPLUG" = 1 ]; then
  ADPLUG_MIN="$ROOT/build/adplug-min"
  sh "$ROOT/packaging/linux/build-adplug-min.sh" "$ADPLUG_MIN"
  export ADPLUG_DIR="$ADPLUG_MIN"
fi

# libsidplayfp (C64 SID) built from source the same way; build.rs compiles the shim, links
# SIDPLAYFP_DIR, and emits the $ORIGIN/lib rpath. Bundled into $STAGE/lib below.
if [ "$BUNDLE_SID" = 1 ]; then
  SIDPLAYFP_MIN="$ROOT/build/sidplayfp-min"
  sh "$ROOT/packaging/linux/build-sidplayfp-min.sh" "$SIDPLAYFP_MIN"
  export SIDPLAYFP_DIR="$SIDPLAYFP_MIN"
fi

# libvgmstream (game-engine streams) built from source (core-only) the same way; build.rs links
# VGMSTREAM_DIR and emits the $ORIGIN/lib rpath. Bundled into $STAGE/lib below.
if [ "$BUNDLE_VGMSTREAM" = 1 ]; then
  VGMSTREAM_MIN="$ROOT/build/vgmstream-min"
  sh "$ROOT/packaging/linux/build-vgmstream-min.sh" "$VGMSTREAM_MIN"
  export VGMSTREAM_DIR="$VGMSTREAM_MIN"
fi

# libupse (PlayStation PSF) built from source the same way; ROM-less (its own HLE FakeBIOS, no PS1
# BIOS). build.rs compiles the shim, links UPSE_DIR, emits the $ORIGIN/lib rpath. Bundled below.
if [ "$BUNDLE_PSF" = 1 ]; then
  UPSE_MIN="$ROOT/build/upse-min"
  sh "$ROOT/packaging/linux/build-upse-min.sh" "$UPSE_MIN"
  export UPSE_DIR="$UPSE_MIN"
fi

echo "==> cargo build --release $FEATURE_ARGS"
# shellcheck disable=SC2086
cargo build --release $FEATURE_ARGS

echo "==> Staging $STAGE"
rm -rf "$STAGE"; mkdir -p "$STAGE"
install -m 0755 target/release/linamp "$STAGE/linamp"

if [ "$BUNDLE_FFMPEG" = 1 ]; then
  echo "==> Bundling the from-source FFmpeg libraries into $STAGE/lib (self-contained)"
  mkdir -p "$STAGE/lib"
  # The from-source FFmpeg (build-ffmpeg-min.sh) links only libc/libm, so the whole
  # runtime set is just its own libav*/libsw* libraries. Copy them straight from the
  # build prefix we linked against (not by ldd-walking the binary, which would resolve
  # against the host's full FFmpeg and drag its ~120 MB of external deps). The soname
  # symlinks are preserved; the binary's $ORIGIN/lib rpath loads them at runtime, and
  # the C runtime comes from the user's system.
  cp -a "$FFMPEG_DIR"/lib/libav*.so.* "$FFMPEG_DIR"/lib/libsw*.so.* "$STAGE/lib/" 2>/dev/null || true
  for f in "$STAGE"/lib/*.so.*; do [ -e "$f" ] && echo "    + $(basename "$f")"; done
  # Sanity check: the installed binary must resolve every libav* from the bundle.
  if ldd "$STAGE/linamp" 2>/dev/null | grep -E 'libav|libswresample' | grep -vq "$STAGE/lib"; then
    echo "!!  WARNING: a libav* still resolves outside $STAGE/lib; check the rpath" >&2
  fi
fi
if [ "$BUNDLE_GME" = 1 ]; then
  echo "==> Bundling libgme into $STAGE/lib (self-contained)"
  mkdir -p "$STAGE/lib"
  # libgme depends only on libc/libstdc++ (both from the user's system), so its own .so is the
  # whole runtime set. Copy it from the build prefix; the soname symlinks are preserved and the
  # binary's $ORIGIN/lib rpath (emitted by build.rs) loads it at runtime.
  cp -a "$GME_DIR"/lib/libgme.so* "$STAGE/lib/" 2>/dev/null || true
  for f in "$STAGE"/lib/libgme.so.*; do [ -e "$f" ] && echo "    + $(basename "$f")"; done
  if ldd "$STAGE/linamp" 2>/dev/null | grep -E 'libgme' | grep -vq "$STAGE/lib"; then
    echo "!!  WARNING: libgme still resolves outside $STAGE/lib; check the rpath" >&2
  fi
fi
if [ "$BUNDLE_OPENMPT" = 1 ]; then
  echo "==> Bundling libopenmpt into $STAGE/lib (self-contained)"
  mkdir -p "$STAGE/lib"
  # libopenmpt depends only on libc/libstdc++ (both from the user's system), so its own .so is
  # the whole runtime set. Copy it from the build prefix; the soname symlinks are preserved and
  # the binary's $ORIGIN/lib rpath (emitted by build.rs) loads it at runtime.
  cp -a "$OPENMPT_DIR"/lib/libopenmpt.so* "$STAGE/lib/" 2>/dev/null || true
  for f in "$STAGE"/lib/libopenmpt.so.*; do [ -e "$f" ] && echo "    + $(basename "$f")"; done
  if ldd "$STAGE/linamp" 2>/dev/null | grep -E 'libopenmpt' | grep -vq "$STAGE/lib"; then
    echo "!!  WARNING: libopenmpt still resolves outside $STAGE/lib; check the rpath" >&2
  fi
fi
if [ "$BUNDLE_ADPLUG" = 1 ]; then
  echo "==> Bundling libadplug + libbinio into $STAGE/lib (self-contained)"
  mkdir -p "$STAGE/lib"
  # Both depend only on libc/libstdc++ (from the user's system); the binary's $ORIGIN/lib rpath
  # (emitted by build.rs) loads them at runtime. Soname symlinks are preserved.
  cp -a "$ADPLUG_DIR"/lib/libadplug*.so* "$ADPLUG_DIR"/lib/libbinio*.so* "$STAGE/lib/" 2>/dev/null || true
  for f in "$STAGE"/lib/libadplug*.so.* "$STAGE"/lib/libbinio*.so.*; do [ -e "$f" ] && echo "    + $(basename "$f")"; done
  if ldd "$STAGE/linamp" 2>/dev/null | grep -E 'libadplug|libbinio' | grep -vq "$STAGE/lib"; then
    echo "!!  WARNING: libadplug/libbinio still resolves outside $STAGE/lib; check the rpath" >&2
  fi
fi
if [ "$BUNDLE_SID" = 1 ]; then
  echo "==> Bundling libsidplayfp into $STAGE/lib (self-contained)"
  mkdir -p "$STAGE/lib"
  # libsidplayfp's own .so; the $ORIGIN/lib rpath loads it at runtime. It may link the system
  # libgcrypt/libgpg-error (near-universal base libs, like libc/libstdc++), resolved from the system.
  cp -a "$SIDPLAYFP_DIR"/lib/libsidplayfp.so* "$STAGE/lib/" 2>/dev/null || true
  for f in "$STAGE"/lib/libsidplayfp.so.*; do [ -e "$f" ] && echo "    + $(basename "$f")"; done
  if ldd "$STAGE/linamp" 2>/dev/null | grep -E 'libsidplayfp' | grep -vq "$STAGE/lib"; then
    echo "!!  WARNING: libsidplayfp still resolves outside $STAGE/lib; check the rpath" >&2
  fi
fi
if [ "$BUNDLE_VGMSTREAM" = 1 ]; then
  echo "==> Bundling libvgmstream into $STAGE/lib (self-contained)"
  mkdir -p "$STAGE/lib"
  # The core-only libvgmstream depends only on libc/libm (from the user's system), so its own .so is
  # the whole runtime set. The binary's $ORIGIN/lib rpath (emitted by build.rs) loads it at runtime.
  cp -a "$VGMSTREAM_DIR"/lib/libvgmstream.so* "$STAGE/lib/" 2>/dev/null || true
  for f in "$STAGE"/lib/libvgmstream.so*; do [ -e "$f" ] && echo "    + $(basename "$f")"; done
  if ldd "$STAGE/linamp" 2>/dev/null | grep -E 'libvgmstream' | grep -vq "$STAGE/lib"; then
    echo "!!  WARNING: libvgmstream still resolves outside $STAGE/lib; check the rpath" >&2
  fi
fi
if [ "$BUNDLE_PSF" = 1 ]; then
  echo "==> Bundling libupse into $STAGE/lib (self-contained)"
  mkdir -p "$STAGE/lib"
  # libupse depends only on libc and the system libz, so its own .so is the whole runtime set. The
  # binary's $ORIGIN/lib rpath (emitted by build.rs) loads it at runtime.
  cp -a "$UPSE_DIR"/lib/libupse.so* "$STAGE/lib/" 2>/dev/null || true
  for f in "$STAGE"/lib/libupse.so*; do [ -e "$f" ] && echo "    + $(basename "$f")"; done
  if ldd "$STAGE/linamp" 2>/dev/null | grep -E 'libupse' | grep -vq "$STAGE/lib"; then
    echo "!!  WARNING: libupse still resolves outside $STAGE/lib; check the rpath" >&2
  fi
fi
cp -r skins "$STAGE/skins"
rm -rf "$STAGE/skins/backup"   # dev-only skin backups, not for release
cp -r soundfont "$STAGE/soundfont"   # MIDI SoundFont, the app finds it next to the binary
[ -d music ] && cp -r music "$STAGE/music" || true
cp packaging/linux/install.sh packaging/linux/uninstall.sh packaging/linux/linamp.desktop "$STAGE/"
chmod +x "$STAGE/install.sh" "$STAGE/uninstall.sh"
cp assets/linamp_logo_256.png "$STAGE/linamp_256.png"
cp assets/linamp_logo.svg "$STAGE/linamp.svg"
for d in README.md FEATURES.md SHORTCUTS.md LINUX.md LICENSE; do [ -f "$d" ] && cp "$d" "$STAGE/" || true; done

echo "==> Creating dist/$NAME.tar.gz"
( cd dist && tar czf "$NAME.tar.gz" "$NAME" )
echo "==> Done: dist/$NAME.tar.gz"
