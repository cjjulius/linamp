# Retro Audio Roadmap and QA Sample Cookbook

Goal: make LinAmp a one-stop destination for audio. Drop in anything, past or present, and
get sound out. Most of this roadmap is now delivered: as of v1.2.0 the retro and chiptune
coverage is broad (console chiptunes, the tracker zoo, AdLib/OPL, SID, game-engine streams,
QOA, and the full `*SF` program-music family). The two living parts of this document are the
short list of formats still to come (and the walls we will not implement), and the
per-format QA sample cookbook in the appendix, which is the canonical, in-git recipe for
building the gitignored `qa-music/format-samples/` corpus so both the Linux and Windows
boxes QA against the same samples.

## Guiding principles (the mantra)

1. Support as many audio decoding and tagging options as possible.
2. Rust is the ideal implementation.
3. Permissive licenses (MIT/BSD/Apache/MPL/ISC/Zlib) are ideal.
4. Failing 2, a low-level language (C, and by extension C++) is acceptable.
5. Failing 3, a copyleft or attribution license (GPL/LGPL, CC-BY) is acceptable.
6. Failing all the above: never a proprietary license, and never a high-level language
   such as Python.

Decision order for any new format: pure-Rust permissive crate first; then a C/C++ library
(permissive, else GPL/LGPL); never a proprietary SDK. If the only decoder is proprietary,
it is a wall: file a wontfix issue and move on (see Walls).

## How a decoder plugs in

Every format reaches playback through one of two shapes, both ending as a seekable
`rodio::Source`:

- Side-decoder crate: a module in `src/` (see `mpc.rs`, `dsd.rs`, `amr.rs`, `tracker.rs`,
  `tta.rs`, `wavpack.rs`, `ape.rs`) that wraps a Rust crate or an FFI binding and exposes an
  `is_<fmt>` predicate plus an `open()` returning a `Source`.
- FFmpeg path: `src/ffdec.rs`, behind the `ffmpeg` feature, for anything FFmpeg already
  decodes. Adding a format here is usually just extending the `is_ffmpeg_audio` extension
  list.

Wiring, in all cases:
- `src/<mod>.rs` or `src/ffdec.rs`: the predicate and decoder.
- `src/playlist.rs`: add the extension(s) to `SUPPORTED_EXTS` and to the router table; the
  `supported_exts_match_the_router` guard test keeps the predicate, the list and the router
  in agreement.
- `src/player.rs`: route the file to the decoder (the ffmpeg path is already wired; a new
  side-crate needs a branch here).
- `src/fileassoc.rs`: a MIME type and a `mime_package_xml` entry so the desktop offers
  LinAmp for the new extension.
- A decode test plus a QA sample and oracle (see QA strategy below).

Native C/C++ libraries follow the minimal-FFmpeg precedent (`packaging/linux/build-ffmpeg-min.sh`):
build a trimmed shared library from source, bundle it next to the binary with an
`$ORIGIN/lib` rpath, so the single installer stays self-contained. Prefer a maintained Rust
binding crate over hand-rolled FFI where one exists.

## Phase 1 - FFmpeg freebies (route-only, zero new dependency)

Decoders already compiled into the FFmpeg we bundle, not yet routed. Each is an
extension-list change plus a sample and oracle. Confirmed present in the bundled
`libavcodec`/`libavformat`:

- Shorten (`.shn`) - classic lossless, large in the live-recording trading scene.
- MPEG-4 ALS (`.als`) - lossless.
- TwinVQ (`.vqf`) - Yamaha/NTT late-90s lossy.
- CRI ADX (`.adx`) - very common game audio.
- Amiga 8SVX / IFF (`.8svx`, `.iff`).
- Creative Voice (`.voc`), Sun/NeXT (`.au`, `.snd`), Sony Wave64 (`.w64`), RF64.
- QuickTime QDesign (`.qdm2` content inside `.mov`, already reachable), Nellymoser (Flash;
  also enables demuxing `.swf` for embedded audio).
- Classic Mac MACE (`.mace`), TrueSpeech, MetaSound.
- Telephony/speech: GSM (`.gsm`), G.729, QCELP, EVRC, iLBC.
- Game FMV audio: Smacker, Interplay, RoQ, PAF, and the broad ADPCM/DPCM family.

Approach: these are lossy or lossless codecs FFmpeg already decodes, so they route through
`ffdec`. Group them into one branch. Skip anything we cannot get a sample for (same rule we
used to hold back ATRAC9).

## Phase 2 - Retro multipliers (the marquee)

Each library below adds a whole family of formats. This is what turns LinAmp into a
retro audio museum. All are C/C++ (principle 4) with acceptable licenses (principle 3 or 5).
Integrate each as a new side-crate wrapping a Rust binding (preferred) or thin FFI, bundled
like the minimal FFmpeg.

### 2a. Game Music Emu (console chiptunes)
- Adds: NSF/NSFE (NES), SPC (SNES), GBS (Game Boy), VGM/VGZ (Genesis, Master System,
  arcade), AY (ZX Spectrum, Amstrad CPC), GYM (Genesis), HES (PC Engine), KSS (MSX),
  SAP (Atari 8-bit).
- Language/License: C++ / LGPL-2.1 (principle 5). Rust bindings exist; evaluate the `gme`
  crate family, else FFI to `libgme`.
- Notes: these are emulated sound chips driven by a register/music log. Many have multiple
  subsongs per file, which the player UI will eventually need to expose (track index). Start
  with first-subsong playback.

### 2b. libopenmpt (the tracker zoo)
- Adds: roughly 30 module formats beyond our current xmrs set (mod/xm/s3m/it). Includes
  `.mptm`, `.med` (OctaMED), `.669`, `.far`, `.okt`, `.dbm`, `.psm`, `.mt2`, `.ult`,
  `.ams`, `.dmf`, `.dsm`, `.gdm`, `.imf`, `.j2b`, `.mdl`, `.mtm`, `.ptm`, `.stm`, and more.
- Language/License: C++ / BSD-3-Clause (principle 3, ideal among the native options).
- Notes: libopenmpt would supersede the pure-Rust `xmrs` path for breadth. Decide whether to
  replace xmrs or route only the extra formats to libopenmpt and keep xmrs for the four it
  already handles. Replacing is cleaner long-term.

### 2c. AdPlug / libADLMIDI (AdLib / OPL FM synthesis)
- Adds: DRO (DOSBox OPL capture), IMF (id Software), RAD, and game MIDI wrappers HMI/HMP/XMI,
  plus Doom-style MUS. The AdLib/OPL FM sound of early-90s PC games.
- Language/License: C++ / LGPL (AdPlug), mixed MIT/LGPL (libADLMIDI). Verify bank licensing.
- Notes: AdPlug alone supports dozens of AdLib container formats. libADLMIDI adds MIDI-to-OPL
  synthesis. Consider AdPlug first for file coverage.

### 2d. libsidplayfp (Commodore 64 SID)
- Adds: `.sid` (PSID/RSID) - the entire C64 music catalog (HVSC).
- Language/License: C++ / GPL-2 (principle 5).
- Notes: cycle-exact 6502 + SID emulation. Subsongs as in GME. GPL is acceptable but note it
  is the most restrictive license we would take; keep it isolated behind a feature so the
  license surface is explicit.

### Phase 2 stretch: emulated-console and game-engine formats
Two separate bodies of work, researched in depth (Nov 2026). They are nothing alike under the hood,
so they are planned and shipped separately.

#### 2e. vgmstream (game-engine streamed audio) - DONE
Hundreds of console/engine stream formats: GameCube/Wii DSP, PS-ADPCM VAG, CRI HCA, Nintendo
BRSTM/BCSTM/BFSTM, FMOD FSB, Microsoft XWB, Sony XVAG/ATRAC9, Koei KTSS, and more.

- Library/license: libvgmstream, **ISC** (permissive - mantra tier 3, the best of any native option).
  The roadmap's old "murkier licensing" worry is **obsolete**: upstream internalized the two
  problematic externals (LibAtrac9 and the unknown-license G.719 decoder) as in-tree ISC decoders in
  Oct 2026, so the **core-only** build (no ffmpeg/vorbis/mpg123 externals) is fully permissive and
  self-contained (the `.so` depends only on libc/libm). That core already decodes the large majority
  of formats, whose payloads are in-tree codecs (DSP/ADPCM, PS-ADPCM, HCA, ATRAC9, PCM).
- Shape: libvgmstream exposes a C API, so there is no shim (unlike AdPlug/SID). `src/vgmstream.rs`
  binds it via hand-rolled FFI, like gme/openmpt. It streams on demand and seeks natively
  (sample-accurate), outputs f32 directly (`force_sfmt = FLOAT`), caps multichannel to stereo
  (`auto_downmix_channels = 2`), duplicates mono to both channels, and plays once with looping
  disabled (`ignore_loop`) so every track has a finite length.
- Routing: an **explicit allowlist** (`VGMSTREAM_EXTS`), not "ask the library." vgmstream's own
  extension table is huge and aggressively claims generic names (.ogg/.wav/.aac/...); we route only
  game-specific extensions nothing else in LinAmp handles and run vgmstream **last** in the dispatch
  chain, so the pure-Rust and ffmpeg paths always win a tie. `.adx`/`.at3`/`.oma` stay on ffdec;
  `.awb` stays with AMR-WB. Pinned to vgmstream r2168.
- External-codec tier - DONE (the `vgmstream-ext` feature). Rebuilds libvgmstream with `USE_VORBIS`
  (Wwise `.wem` + FSB Vorbis via libvorbis/libogg, BSD) and `USE_FFMPEG` (Xbox 360 XMA, PS3/PSP
  ATRAC3, WMA-in-container) pointed at the same LGPL `ffmpeg-min` the `ffmpeg` feature builds (no
  `--enable-gpl`, no static link, so it DT_NEEDs the identical `libavcodec.so.58` and shares one copy -
  no duplicate-symbol clash). Routing adds `.wem`/`.xma`; previously-routed containers (`.fsb`/`.xwb`/
  `.xvag`) now decode their Vorbis/XMA payloads instead of skipping. build-vgmstream-min.sh takes
  `VGM_EXT=1`; build-libvorbis-min.sh supplies libvorbis. Combined license stays GPLv3. (CELT/Speex/
  mpg123 payloads remain off - niche; add later if wanted.)

#### 2f. The `*SF` family (emulated-program music) - PSF1 DONE (PSF2 dropped), rest planned
PSF (PlayStation), GSF (GBA), 2SF (NDS), USF (N64), SSF/DSF (Saturn/Dreamcast), QSF (Capcom
QSound), NCSF (NDS). Each file is a compressed program + driver for a specific console, played by a
system-specific emulator core - so unlike vgmstream there is **no single library**: it is one shared
container plus several emulator cores. PSF1 shipped first (the `psf` feature); the rest are per-
format Cargo features behind their own cores, like the AdPlug/SID shims.

**2f-psf (PSF1) - DONE.** `psf` feature, src/psf.rs + src/psf_shim.c (C shim via cc). Decoder:
**libupse (GPL)**, which ships its own HLE "FakeBIOS" and so plays PSF **ROM-less** - no PlayStation
BIOS is needed or shipped. (This is why libupse, not Highly Experimental: HE requires the real 512 KB
Sony BIOS, which is a shipping wall - confirmed hands-on.) libupse decodes to stereo s16 at 44.1 kHz
and seeks natively; the shim gates every call on the PSF signature because libupse crashes on
malformed input. Routes `.psf`/`.minipsf` only. Linux build (`build-upse-min.sh`), CI, and tarball
bundling wired. Windows: `build-upse.ps1` compiles the vendored libupse sources with cl (the headers
carry a WIN32_MSC path) into a static lib, combining the static zlib from build-gme.ps1 (PSF program
sections are zlib-compressed) so one `upse.lib` is self-contained; it is linked into the exe (no DLL),
and Windows CI builds `--features ...,psf`. Compile libupse with /MD so it shares the dynamic CRT with
the Rust side and the cc-compiled shim (a static-CRT build trips LNK4098 and risks cross-CRT heap bugs).

**PSF2 dropped (not shippable ROM-less).** libupse's PS2 IOP HLE is a stub - `upse_ps2_iop_call()`
in `upse_ps2_iop_base.c` is empty, so no IOP/SPU2 syscall is serviced: real PSF2 tunes load but
decode to pure silence (confirmed hands-on on Chess Challenger and Dark Cloud). The only engine that
plays PSF2 correctly is Highly Experimental, which needs the copyrighted 512 KB PS2 BIOS - the same
shipping wall as PSF1's HE path, and banned. So `.psf2`/`.minipsf2` are rejected at the signature
check (psf_shim.c accepts version 0x01 only) and are not in any routing/MIME/ext list. Revisit only
if a ROM-less PS2 IOP HLE that actually drives the SPU2 appears.

A critical lesson for the remaining cores: **BIOS/ROM-check each core before adopting it** (as done for
PSF). Several of the others HLE and should be fine, but it must be verified - we ship no console ROMs.

- Decoders (all actively maintained; the Kodi `audiodecoder.*` binary-addons are the cleanest uniform
  source - one shared `psflib`, all CMake, all build on Linux + Windows):
  - PSF1: **libupse (GPL)** - DONE (the `psf` feature, see 2f-psf below). NOT Highly Experimental:
    HE needs the real 512 KB PlayStation BIOS (a shipping wall; verified hands-on), while libupse ships
    its own HLE "FakeBIOS" and plays PSF1 ROM-less. PSF2 dropped - libupse's IOP HLE is a stub
    (silence) and HE's PS2 path needs the banned PS2 BIOS (see 2f-psf).
  (Decoder picks below were deep-researched against primary sources 2026-11-09; each is ROM-less
  confirmed - none needs a console BIOS/ROM - so none is a wall. Several roadmap guesses were wrong;
  corrected here.)
  - SSF/DSF: **Highly_Theoretical** (Saturn SCSP + Dreamcast AICA, one core for both; emulates only the
    sound chip, no BIOS). License **GPL-3.0** (not GPL-2), psflib MIT. MAME-free confirmed. Vendor from
    the Kodi `xbmc/audiodecoder.ssf` split-core (`lib/Highly_Theoretical` + `lib/psflib` + zlib).
    CAVEAT: `.dsf` also means DSD Stream File (we already decode that in src/dsd.rs) - route SSF/DSF by
    content (PSF signature + version byte 0x11=SSF / 0x12=DSF), never by extension.
  - USF: **lazyusf2** (mupen64plus-derived; ROM-less via the USF's embedded Project64 save-state, no PIF
    boot ROM). License **GPL-2.0-or-later** (verified from source headers: 79 GPL-2-or-later + 2 CC0,
    no non-commercial code). SOURCE CORRECTION: do NOT use the Kodi `xbmc/audiodecoder.usf` tree - it
    vendors the ORIGINAL lazyusf, whose core CPU files carry zilmar's Project64 "PERSONAL USE only /
    non-commercial" license (field-of-use = effectively proprietary, mantra tier 6). lazyusf2 replaced
    that core with mupen64plus's GPL R4300, so it is clean; build from **derselbst/lazyusf2** (a
    maintained fork of kode54's, adds arm64) pinned. Built with the **cached interpreter, no dynarec**
    (one portable recipe x86_64/arm64; a music player does not need the JIT). Audio is pinned to 44.1 kHz
    via lazyusf2's own resampler. DONE.
  - GSF: **mGBA + libgsf** (MPL-2.0). mGBA's built-in HLE BIOS plays GSF ROM-less (`useBios=false`); no
    Nintendo BIOS. Corrections: lazygsf (jprjr) is **deleted from GitHub** - use **libgsf (chrg127,
    MPL-2.0)** from Codeberg (`codeberg.org/chrg/libgsf`, pin a copy); and viogsf is built on **VBA-M
    (GPL-2)**, not mGBA. CAVEAT: mGBA HLE stubs the BIOS sound-driver SWIs (0x1A-0x1E), so a small
    long tail of rips that drive sound through the BIOS driver may be imperfect - acceptable under the
    no-BIOS rule (every HLE GSF player has this). Build uses CMake (mGBA) - its own build shape. DONE:
    pin **codeberg.org/chrg/libgsf @ df71d714** (username is `chrg`, not `chrg127`; GitHub mirror is
    archived read-only), bundling **mGBA @ 11e16d1a** (submodule gitlink; `expected` is vendored, not a
    submodule). No psflib - libgsf parses the container, `_lib` chain and tags itself, and seeks
    natively. Version byte 0x22. Built + render-verified on libgsf's bundled test GSFs.
  - QSF: **Highly_Quixotic** (Capcom QSound; reimplemented DSP, no copyrighted `dl-1425.bin`). License
    **GPL-3.0** (not GPL-2), psflib MIT. Kodi `xbmc/audiodecoder.qsf`. Light.
  - NCSF: **SSEQPlayer** (Nitro SDAT/SSEQ soundbank sequencer - no emulator, no BIOS at all). License
    **WTFPL** (permissive; or BSD-3 via CyberBotX in_xsf) - NOT GPL. Kodi `xbmc/audiodecoder.ncsf`
    bundles the WTFPL engine + psflib. Lightest of the set.
  - 2SF: **vio2sf** (DeSmuME-derived; ROM-less via DeSmuME's HLE BIOS - `bios.c`, no `bios.bin`, plus a
    synthesized blank firmware-settings block). License **GPL-2.0-or-later** (verified from the DeSmuME
    source headers - the "or later" confirmed, so not the GPLv2-only blocker). Kodi `xbmc/audiodecoder.2sf`
    (vendors `lib/vio2sf` + `lib/psflib`; its `src/2SFCodec.cpp` loader was ported into `src/twosf_shim.cpp`).
    HEAVIEST core (dual ARM7/ARM9 interpreters, MMU, SPU; one C++ TU -> libstdc++, like NCSF). The Kodi
    silence-suppression/fade machinery is dropped - LinAmp plays what is there. 44.1 kHz native. DONE.
- License reality: GSF (mGBA/libgsf **MPL-2.0**) and NCSF (SSEQPlayer **WTFPL**/BSD) are permissive
  (mantra tier 3); SSF/DSF and QSF are **GPL-3**, USF is **GPL-2-or-later**, 2SF is **GPL-2**. Enabling
  any copyleft `*SF` feature makes that build GPL - acceptable under mantra rule 5, the status quo of
  the existing `sid` feature.
- License-compat note (our host is **Apache-2.0**): Apache-2.0 is compatible with **GPL-3.0** and
  **GPL-2.0-or-later** (the feature build just becomes GPL) and with MPL/WTFPL/BSD - so SSF/DSF/QSF/USF/
  GSF/NCSF are all clean. The one risk is a strictly **GPL-2.0-only** core: Apache-2.0 + GPL-2.0-only is
  a real incompatibility. 2SF's DeSmuME is the one to check "or-later" on before shipping (it is last in
  the order regardless). The existing `sid`/libsidplayfp feature already sets the GPL-opt-in precedent.
- **Wall: SNSF** (SNES). The only core is snsf9x, which bundles snes9x under a **non-commercial /
  personal-use-only** license. Skip it and document; the practical cost is nil, because SNES music is
  already covered as **SPC via the existing gme feature**. Also avoid the old monolithic `nmlgc/aosdk`
  SDK as a source - it mixes in MAME's non-commercial clause; use the kode54/Kodi split-cores instead.
- No pure-Rust option exists for any subformat (confirmed on crates.io); C/C++ is the floor.
- Implementation order (finalized 2026-11-09, after the deep-research pass above):
  1. **SSF + DSF + QSF + NCSF** - DONE, merged (PR #241, v1.1.6). Three
     psflib-based cores (Highly_Theoretical for SSF+DSF and Highly_Quixotic for QSF are C; SSEQPlayer
     for NCSF is C++), all built the PSF1/libupse way (vendored sources + shared psflib + zlib, thin
     `cc` shim, Cargo feature). Includes the `.dsf`-vs-DSD content-routing fix. Validated on real
     Saturn/Dreamcast/Capcom/DS rips.
  2. **USF + 2SF** - DONE (pending QA/merge) on `feat/tier2f-usf-2sf`. Both psflib-based (`cc`/`c++`
     shim, shared psflib), same pattern as grouping 1: USF = lazyusf2 (C, cached interpreter - no
     dynarec), 2SF = vio2sf (C + one C++ TU, the heaviest core). Both GPL-2-or-later (-> GPLv3 combined,
     the same posture as the merged cores). Validated on real N64 + DS rips.
  3. **GSF** - DONE (pending QA/merge) on `feat/tier2f-gsf`. The odd one out: libgsf (which bundles a
     trimmed mGBA) parses the container itself (no psflib) and is built by its own CMake (needs C++20),
     the one build-tooling difference in the family. MPL-2.0 (lighter than the GPL cores). ROM-less via
     mGBA's HLE BIOS (useBios=false). libgsf supports native seeking, so no seek-by-reset. Validated by
     building + rendering libgsf's own bundled test GSFs. This completes the *SF family.
  PSF1 shipped first (biggest corpus; done). One grouping per branch, like the 2a-2d work. USF+2SF were
  bundled (same proven pattern, both GPL); GSF stood alone for its CMake/C++20 build.

## Phase 3 - Modern and ideal-fit

- QOA (Quite OK Audio, `.qoa`) - DONE (the `qoa` feature, crate::qoa). Pure-Rust `qoaudio` 0.7.0
  (MIT/Apache-2.0), no FFI, no bundled library - satisfies every principle at once. The decoder is an
  iterator of interleaved i16 samples; the fixed-mode header gives rate/channels/length. Seek-by-reset
  (re-create the decoder from the in-memory bytes + render forward). Validated against the crate's own
  reference `.qoa` fixture.
- Tagging: `lofty` 0.21 is already a dependency. Audit which of the new formats it can tag
  and widen read/write coverage as formats land (principle 1 covers tagging, not just decode).

## Phase 4 - Non-codec audio sources (the "drop anything" glue)

- CUE sheets (`.cue`) - DONE (crate::cue). Split a single-file album (`.flac`/`.ape`/`.wv`/`.wav`
  plus a `.cue`) into real tracks. Pure-Rust `cuna` 0.7.0 (MIT). Each track is a `cue://<start>:<len>/
  <audio>` pseudo-path (the `cdda://` trick), opened through the normal dispatch and wrapped in a
  `SubTrackSource` that seeks to the start, caps to the length, and re-bases `try_seek` for correct
  mid-track scrubbing. No new codec, just index + seek.
- Playlists - DONE. `.m3u`/`.m3u8`/`.pls` were already handled (parse_playlist_file); added `.xspf` and
  `.asx` (XML, via `quick-xml`) and `.ram` (line-based). A playlist expands into tracks on load; remote
  stream URLs are never auto-fetched (dropped by the `exists()` filter) to avoid SSRF/privacy exposure.
- Internet radio - the streaming ENGINE is already DONE (src/stream.rs: a full ICY/SHOUTcast client,
  Ctrl+L "Open location" UI). What remains (deferred) is an in-app station-directory UI; the engine
  works today via a stream URL. (#146)

## Walls (proprietary - do not implement)

File a short wontfix issue and move on. Known: AC-4 (#232), OptimFROG (#233), plus LA
(Lossless Audio), RKAU, MQA, LDAC, and the encoder side of Sony 360RA / MPEG-H.

## QA sample strategy (read this before implementing any format)

Every format needs a test file in `qa-music/format-samples/` (gitignored, per-box) and,
where possible, a decode oracle in `oracles/`. The decode test loads our output and compares
it against the oracle by correlation, exactly as the ffmpeg-backed tests already do. Keep
samples short (tens of seconds) so the debug test suite stays fast.

Two ways to get a sample:

- Generate it with an open-source tool (preferred: reproducible on both machines).
- Procure it from a known archive when no encoder exists (document the exact URL).

Oracle: decode the sample with the reference library's own CLI to raw f32, store that, and
compare. If no CLI oracle is practical, fall back to a smoke test (decodes to non-silent
audio of plausible length), as we do for the short DSD tone.

### Per-family acquisition and generation

Phase 1 (FFmpeg freebies):
- Oracle for all: `ffmpeg -i sample.<ext> -t 20 -f f32le -ac 2 -ar <rate> oracles/<fmt>-oracle.f32`.
- Shorten, ALS, TwinVQ, ADX, 8SVX, VOC, AU, Wave64: no encoder in our ffmpeg for most, so
  procure from `https://samples.ffmpeg.org/` (A-codecs and the format-named directories).
  A few (ADPCM, 8SVX, some PCM wrappers) can be minted with ffmpeg; check `ffmpeg -encoders`.

Phase 2a (Game Music Emu):
- Generate: Furnace (open source, cross-platform tracker) exports VGM and many chip formats
  directly, so we can author our own short test tunes. FamiStudio (open source) exports NSF
  (NES). These give us clean, freely-licensed, reproducible samples.
- Procure: freely/CC-licensed chiptunes and homebrew rips where generation is impractical
  (SPC in particular needs an SNES driver; prefer a known CC-licensed SPC or a smoke test).
- Oracle: Game Music Emu ships a CLI; decode to wav and convert to f32. Where subsongs exist,
  pin subsong 0.

Phase 2b (libopenmpt):
- Generate/procure: the OpenMPT project and ModArchive host many Creative-Commons and
  public-domain modules across the exotic formats. Pick small CC0/CC-BY files per format.
- Oracle: `openmpt123 --render sample.<ext>` (writes a wav), then convert to f32. openmpt123
  ships with libopenmpt on both platforms.

Phase 2c (AdPlug / libADLMIDI):
- Generate: DRO can be captured from DOSBox OPL output; IMF/RAD can be authored with open
  trackers. Simpler: use AdPlug's own test corpus.
- Oracle: AdPlug ships `adplay`; render to wav, convert to f32.

Phase 2d (libsidplayfp):
- Generate/procure: tiny public-domain `.sid` tunes, or author a minimal one. HVSC is not
  freely licensed, so prefer a known public-domain composer or a smoke test.
- Oracle: `sidplayfp` can write a wav; convert to f32.

Phase 2e (vgmstream): see the appendix cookbook for the full, validated recipes. In short:
- Generate (reproducible, no copyright): `genh` from a WAV with a short script; `vag`/`svag` with the
  committed pure-Python PS-ADPCM encoder; `dsp`/`idsp`/`hca`/`brstm`/`bcstm`/`bfstm`/`hps` with VGAudio
  (.NET - the Windows box runs it; the Linux dev box is sandbox-blocked). VGAudio has no ATRAC9 encoder.
- Procure real rips (local QA only, gitignored): `brstm`/`bcstm`/`bfstm`/`bfwav`/`bwav`/`hca` from
  GameBanana (login-free; the working source now that joshw.info is down and SCM needs a login).
- Still procure-only (no free encoder, not on GameBanana): `at9`, `xvag`, `fsb`, `xwb`, `ktss`.
- Oracle: `vgmstream-cli -i -o ref.wav <file>` decodes with looping disabled as a reference; cross-check
  metadata with `vgmstream-cli -m <file>`. libvgmstream and the CLI share the decode core, so for the
  single-decoder formats the committed test is a non-silent render smoke test, like the chiptunes.

vgmstream external tier (`vgmstream-ext`: `wem`/`xma`, and Vorbis/XMA/ATRAC3 payloads inside already-routed
containers):
- Procure only (no free encoder - these need proprietary tools or a game's own banks): `.wem` is
  extracted from a game's Wwise `.bnk`/`.pck` banks (or exported from Wwise with a Vorbis conversion);
  `.xma` from Xbox 360 rips; ATRAC3 `.fsb`/`.xvag` from PS3/PSP rips. Local QA only, gitignored.
- Verify with `vgmstream-cli -m <file>` (prints the codec: "Wwise Vorbis", "XMA2", "ATRAC3", ...) and
  `vgmstream-cli -i -o ref.wav <file>` for a reference decode. The committed render test is the
  non-silent smoke test (libvgmstream is the sole decoder).
- There is no license-free generated sample for these, so the render test simply skips when none is
  present; validate against a procured bank.

Phase 2f (*SF family), when taken up:
- Procure only: Zophar's Domain per-format music sets (`zophar.net/music/<fmt>`). These are copyrighted
  commercial game rips - local QA only, never commit (matches the per-box gitignored-samples rule).
  Generation (USFMaker, Zophar's PSF utilities) still needs a copyrighted ROM/EXE input, so it is not
  truly license-free. Pick a set whose subsong 0 has audio and is long enough to scrub.
- Oracle: each format's library is the sole decoder, so a non-silent render smoke test is the fallback.

Phase 3:
- QOA (`.qoa`): generate directly, license-free. Reference tool: `qoaconv` from phoboslab/qoa builds
  with `make` and converts `qoaconv in.wav out.qoa` (make the WAV with a short tone script, ffmpeg or
  sox). Pure-Rust alternative: a tiny program on `qoaudio`'s git `master` (which has `QoaEncoder`;
  the 0.7.0 crate release is decode-only) encodes an i16 tone to `.qoa`. The `qoaudio` crate also ships
  a reference `.qoa` fixture (`fixtures/`), used here for local decode validation. Oracle: decode back
  with `qoaconv`; the committed test is a non-silent render smoke test.

Phase 4 (non-codec glue), all generated license-free:
- CUE (`.cue`): a short script writes a multi-tone stereo WAV and a `.cue` with one `TRACK`/`INDEX 01`
  per tone (see `scratchpad/mkcue.py` in the dev notes); open the `.cue` and confirm it expands to the
  right tracks with the right lengths, and that each track plays its own tone and scrubs correctly.
- Playlists (`m3u`/`m3u8`/`pls`/`xspf`/`asx`/`ram`): hand-write a tiny playlist pointing at a local
  sample (for XSPF a `<location>file:///...</location>`, for ASX a `<REF HREF=.../>`, for RAM one path
  per line); load it and confirm the track is added. Include one remote `http(s)`/`rtsp` entry and
  confirm it is NOT fetched (dropped by the exists() filter).

### Briefing the Windows box

The Windows machine keeps its own copy of `qa-music/format-samples/` (gitignored, so it is
never in git and must be built locally on each box). After we implement a format here, post
to the coordination Slack (the self-DM, messages marked as from the Linux box) with:

- The format, the extension(s), and the commit.
- The exact sample recipe: the generator command or the procurement URL, verbatim.
- The oracle command.
- Any platform caveat.

All the generator tools named above (ffmpeg, openmpt123, Furnace, FamiStudio, Game Music Emu
CLI, sidplayfp, qoaconv, VGAudio, the committed Python encoders) run on Windows as well, so the
recipes are cross-platform; note any path or packaging differences when they come up. The canonical,
committed copy of every recipe is the **"Appendix: QA sample cookbook" below** - use that, since it
is in git and the same on both boxes. Each box's `qa-music/format-samples/README.md` is a local,
gitignored scratch note of what that box happened to build, not a source of truth.

## Status tracker

Phase 1 - FFmpeg freebies
- [x] Shorten (.shn) - `ffmpeg` feature
- [x] CRI ADX (.adx) - `ffmpeg` feature
- [x] Amiga 8SVX (.8svx) - `ffmpeg` feature
- [x] Creative VOC (.voc), Sun AU (.au), Wave64 (.w64) - `ffmpeg` feature
- [ ] MPEG-4 ALS (.als, ALS-in-mp4 only), TwinVQ (.vqf) - deferred
- [ ] Flash Nellymoser / .swf audio, QuickTime QDM2
- [ ] Speech: GSM (.gsm), G.729, QCELP, EVRC, iLBC (as useful)

Phase 2 - Retro multipliers
- [x] Game Music Emu (NSF/SPC/GBS/VGM/AY/GYM/HES/KSS/SAP) - merged to main.
- [x] libopenmpt (the tracker zoo) - `openmpt` feature, src/openmpt.rs (streaming FFI). All 28
  routed formats now have a QA sample and pass the visualizer test (recipes in the cookbook; `.stx`
  from libxmp's corpus, `.j2b` from Mod Archive, `.umx` from a Deus Ex GOG installer via
  innoextract). xmrs keeps mod/xm/s3m/it. Windows build/installer/CI wired by the Windows box.
- [x] AdPlug (OPL FM: DRO/CMF/RAD/HSC/LAA/Doom MUS/Wolf IMF and ~30 more) - `adplug` feature,
  src/adplug.rs + src/adplug_shim.cpp (C++ shim via the cc crate; AdPlug has no C API). 37
  self-contained formats routed and sampled from AdPlug's own test corpus; `.ksm`/`.rol`/`.sci`
  dropped (they need a companion instrument-bank file). libADLMIDI (XMI/HMI/HMP MIDI wrappers) is
  deferred - it is GPLv3 and its embedded banks are game rips, so it needs its own feature later.
- [x] libsidplayfp (.sid) - `sid` feature, src/sid.rs + src/sid_shim.cpp (C++ shim via cc). reSIDfp
  engine, ROM-less (ships no Commodore ROMs; plays most PSID and the majority of RSID), mono
  rendered to both channels, default length + fade, subsong 0. `.sid`/`.psid`/`.rsid`. Linux and
  Windows builds (`build-sidplayfp.ps1`) wired; CI green on both platforms.

Phase 2 stretch
- [x] vgmstream (game-engine streams: DSP/VAG/HCA/BRSTM/BCSTM/BFSTM/BFWAV/BWAV/AT9/XVAG/FSB/XWB/HPS/
  KTSS/IDSP/SVAG/GENH) - `vgmstream` feature, src/vgmstream.rs (direct C-API FFI, no shim). Core-only
  libvgmstream (ISC, permissive, libc/libm only); streaming + native seek + f32 out; allowlist routing,
  runs last. Linux build (`build-vgmstream-min.sh`, CMake, pinned r2168), CI, and tarball bundling wired.
  Windows build (`build-vgmstream.ps1`, static link) and CI feature-add wired by the Windows box; CI
  green on both platforms. FFmpeg-backed codecs (XMA/ATRAC3/WMA) and Vorbis/CELT-payload variants
  deferred to a later build tier.
- [x] `*SF` family, PSF1 - `psf` feature, src/psf.rs + src/psf_shim.c (libupse, GPL). ROM-less
  (libupse's HLE FakeBIOS; no PlayStation BIOS, not HE which needs the real BIOS). Native seek, stereo.
  `.psf`/`.minipsf` only (PSF2 dropped - libupse IOP HLE is a stub -> silence; see 2f-psf). Linux and
  Windows builds (`build-upse-min.sh` / `build-upse.ps1`), CI, and tarball wired; CI green on both
  platforms. Reproducible CC0 QA sample (hand-authored minimal PSF, recipe in the cookbook); for real
  PSF1 music, procure a tune from Zophar (local QA only).
- [x] `*SF` family, the rest - shipped in v1.2.0, all ROM-less (cores/licenses in "Phase 2
  stretch" above). SSF+DSF (Highly_Theoretical), QSF (Highly_Quixotic) and NCSF (SSEQPlayer)
  merged in PR #241 (v1.1.6); USF (lazyusf2) and 2SF (vio2sf) as the `usf`/`twosf` features;
  GSF (libgsf/mGBA, `gsf` feature, its own CMake build, no psflib). This completes the `*SF`
  family. GSF (MPL) and NCSF (WTFPL) are permissive; the rest are GPL. SNSF stays a wall
  (snes9x non-commercial).

Phase 3 - Modern/ideal
- [x] QOA (.qoa) - the `qoa` feature (qoaudio, pure Rust)
- [ ] Tagging coverage audit (lofty)

Phase 4 - Non-codec sources
- [x] CUE sheets - crate::cue (cuna; cue:// virtual tracks + SubTrackSource)
- [x] Playlists (m3u/m3u8/pls already done; added xspf/asx/ram)
- [~] Internet radio (#146) - streaming engine DONE (src/stream.rs); station-directory UI deferred

vgmstream external-codec tier
- [x] `vgmstream-ext` - Wwise/FSB Vorbis (libvorbis) + XMA/ATRAC3/WMA (bundled LGPL FFmpeg)

Done this cycle (context)
- [x] RealAudio/RealMedia, TAK, Sony ATRAC (ffdec), plus the corrupt-input decode-hang fix.
- [x] Walls documented: AC-4 (#232), OptimFROG (#233).

## Appendix: QA sample cookbook (every format)

The canonical per-format recipe for building the local QA corpus in `qa-music/format-samples/`.
That folder is gitignored (samples are large/per-box/often copyrighted), so each machine builds
its own set from these recipes - this appendix is how both the Linux and Windows boxes stay in
sync. Preference order per format: a real song transcoded from the CC0 clip "Hypno This" by Kevin
MacLeod (public domain); else generate; else procure (exact URL given). The `hypno-this.wav`
source clip is the shared base; `hypno-mono.wav` / `hypno-48k.wav` are variants.

Vet any new sample before relying on it with the all-codecs test (ignored/slow), which renders a
sample per extension and flags any that error or render silence:
`cargo test --features "ffmpeg gme" -- --ignored every_supported_codec_feeds_the_visualizer`
(Linux: `LD_LIBRARY_PATH=build/gme-min/lib`; Windows: libgme DLL on PATH).

### Core audio and containers (symphonia and pure-Rust side-decoders)
- Transcode from hypno-this.wav: `.mp3` (libmp3lame 192k), `.mp2`/`.mpa` (mp2), `.flac`, `.ogg`
  (Vorbis), `.opus`, `.m4a`/`.aac`, `.wav` (pcm_s16le), `.aiff` (pcm_s16be), `.eac3`/`.ec3` (E-AC-3),
  `.wv` (WavPack: stereo/mono/48k/24-bit/untagged variants), `.tta` (True Audio).
- Musepack `.mpc` (SV8): `mpcenc` (musepack-tools; ffmpeg has no MPC encoder). `.mp+`/`.mpp` are copies.
- Video-for-audio (a 320x240 test-pattern video + the audio track): `.mp4`/`.mov`/`.mkv` (H.264/AAC),
  `.webm` (VP8/Opus), `.ogv` (Theora/Vorbis), `.mov`. `.m4b`/`.m4v`/`.3gp` are MP4-family copies.
- Container-extract: MPEG-TS `.ts`/`.m2ts`/`.mts` (AAC/MP2/AC-3), AVI `.avi` (MP3/PCM/AC-3),
  FLV `.flv` (MP3/AAC), MPEG-PS `.mpg`/`.mpeg`/`.vob`/`.m2p` (MP2/AC-3).
- MP1 `.mp1`: ffmpeg has no Layer I encoder; make with mjpegtools `mp2enc -l 1` fed a real WAV on
  stdin (`apt-get download mjpegtools`, `dpkg -x`), or procure samples.ffmpeg.org/A-codecs/mp1-sample.mp1.
- MIDI `.mid`/`.midi`: a format-1 General MIDI song (exercises rustysynth + SoundFont).
- Trackers: `.mod`/`.s3m`/`.it` small valid modules (TagLib test data), `.xm` = xmrs examples/note.xm.
  Use real modules with audible notes, not empty templates (an empty module renders silence).

### QOA, CUE sheets and playlists (crate::qoa, crate::cue, playlist.rs; pure Rust)
- QOA `.qoa`: generate license-free with phoboslab/qoa `qoaconv in.wav out.qoa`, or a tiny program on
  `qoaudio` git master (`QoaEncoder`). The `qoaudio` 0.7.0 crate ships `fixtures/*.qoa` for local decode
  validation. Committed test: non-silent render smoke test (`qoa::tests::decodes_real_qoa_files`).
- CUE `.cue`: `scratchpad/mkcue.py` writes a 3-tone stereo WAV + a `.cue` with one `TRACK`/`INDEX 01`
  per tone. The committed test (`cue::tests::decodes_real_cue_album_tracks`) expands it and confirms
  each virtual track decodes its own non-silent audio (SubTrackSource seeks + caps correctly).
- Playlists `.xspf`/`.asx`/`.ram`: hand-write a tiny file pointing at a local sample; `.m3u`/`.pls`
  were already covered. Unit test (`playlist::tests::parse_xspf_asx_ram_extract_locations`) checks the
  parser, including that a remote `http`/`rtsp` entry is extracted but (via load_file's exists() filter)
  never fetched.

### FFmpeg-backed formats (crate::ffdec, `--features ffmpeg`)
Oracle for the decode tests: `ffmpeg -i <sample> -f f32le -ac 2 -ar <rate> oracles/<name>-oracle.f32`.
- Mint from hypno-this.wav: `.wma` (`-c:a wmav2`), `.dts` (`-c:a dca -strict -2`), `.thd`/`.mlp`
  (`-c:a truehd`/`mlp -strict -2`), `.ra` (`-ar 8000 -ac 1 -c:a real_144`, RA 1.0), `.adx`
  (`-c:a adpcm_adx`), `.voc`/`.w64` (`-c:a pcm_s16le`), `.au` (`-c:a pcm_s16be`).
- Procure (no encoder) from samples.ffmpeg.org:
  - Shorten `.shn`: `/A-codecs/lossless/luckynight.shn`
  - 8SVX `.8svx`: `/8svx/conect.8svx`
  - RealAudio Cook `.rm`: `/real/audiosig.rm` (audio-only), `/real/RLG2_19.rm` (cook + rv20 video)
  - TAK `.tak`: `/A-codecs/tak/lostchord-tak_2_2_0.tak` (2.x works; the 1.0 file is a negative
    fixture - our FFmpeg 4.4 demuxes but cannot decode it, and playlist::playable must reject it)
  - ATRAC: `.oma`/`.omg`/`.aa3` `/A-codecs/ATRAC3+/sonateno14op27-2.aa3` (one OpenMG/ATRAC3+
    container plays under all three extensions; use this audible sonata for every one. The
    other file there, at3p_sample1.oma, decodes correctly but is a very quiet test tone that
    sounds like wind on listening QA, so do not use it as the shown sample). `.at3`
    `/A-codecs/ATRAC3/SND0.AT3`, `.aea` `/A-codecs/ATRAC1/Christmas1970.aea`.
- AMR `.amr` samples.ffmpeg.org/A-codecs/amr/sample.amr; `.awb` = an AMR-WB file renamed (crate::amr
  dispatches by magic). DSD `.dsf` generated via sigma-delta; `.dff` tone procured.
- Not advertised (no sample): `.at9` (ATRAC9). Deferred: `.als` (only ALS-in-mp4), `.vqf` (TwinVQ).

### Chiptunes / console music (crate::gme, `--features gme`)
libgme emulates the sound hardware; no encoder, so procure from Modland
(modland.com/pub/modules/<Format>/<author>/...). One libgme build plays all. Pick a small file;
LinAmp plays subsong 0, so choose one whose first track has audio.
- NES `.nsf`: "Nintendo Sound Format" | Game Boy `.gbs`: "Gameboy Sound System" | PC Engine `.hes`:
  "HES" | Atari `.sap`: "Slight Atari Player" | SNES `.spc`: "Nintendo SPC" | Sega/arcade `.vgz`
  (gzipped VGM): "Video Game Music".
- `.vgm` (raw, uncompressed VGM): the same format as `.vgz` without the gzip wrapper, so just
  gunzip the `.vgz` sample into a `.vgm` one (both route to libgme, but each is its own supported
  extension, so cover both): `python3 -c 'import gzip,sys;
  open(sys.argv[2],"wb").write(gzip.decompress(open(sys.argv[1],"rb").read()))' vgz-sample.vgz vgm-sample.vgm`
- The four formats Modland does not serve directly (all now sampled; recipes below):
  - `.ay` (ZX Spectrum / Amstrad CPC): Modland's AY dirs hold `.emul`/`.amad`/`.strc`, not raw `.ay`.
    Pull a raw `.ay` (magic `ZXAYEMUL`) from the chris-y/AY mirror of Project AY. The `games`
    tree is split into one subdirectory per leading letter, so the path carries that letter:
    `https://raw.githubusercontent.com/chris-y/AY/main/music/ay/games/<letter>/<name>.ay`
    (URL-encode spaces and `[]`). `a/ATF(AdvancedTacticalFighter).ay` is single-track and loud
    (subsong 0 audible).
  - `.gym` (Sega Genesis): Modland's "Megadrive GYM" files are PACKED (magic `GYMX`), which libgme
    0.6.3 rejects with "Packed GYM file not supported". Unpack to a raw register dump libgme accepts:
    strip the 428-byte GYMX header, then zlib-decompress the body when the uint32 at header offset
    424 (compressed size) is nonzero. Write out just the decompressed stream (no GYMX header):
    `python3 -c 'import struct,zlib,sys; d=open(sys.argv[1],"rb").read(); assert d[:4]==b"GYMX";
    n=struct.unpack_from("<I",d,424)[0]; b=d[428:]; open(sys.argv[2],"wb").write(zlib.decompress(b) if n else b)'
    packed.gym gym-sample.gym`
    Pick an actual game soundtrack, not a utility: the sample in use is
    `Megadrive GYM/_unknown/Sonic 2 Time Zone/sonic 2 time zone - title screen theme.gym` (a clear
    ~61s melody that is easy to QA). The earlier "Art Alive" file was a drawing program's sparse,
    noise-like audio and hard to judge by ear.
  - `.kss` (MSX): most KSS rips have a silent subsong 0 and LinAmp plays subsong 0. Of the whole
    Modland "KSS" collection (17 authors), only `KSS/Airborne/nemesis gametune.kss` has an audible
    first track; use that until the subsong-selection feature lands.
  - `.nsfe` (NES extended): no sample located, so derive one from any `.nsf`. NSFe is a chunked
    (RIFF-style) container: `NSFE` magic then `<uint32 size><4cc><body>` chunks. Emit `INFO`
    (load/init/play addrs + region + chip + track count + 0-based start), a `BANK` chunk with the
    NSF header's 8 bankswitch bytes when any is nonzero (ours needs it), the `DATA` chunk (NSF bytes
    from 0x80), then an empty `NEND`. The result renders byte-identical audio to the source `.nsf`
    through libgme. Converter (`python3 nsf2nsfe.py nsf-sample.nsf nsfe-sample.nsfe`):

    ```python
    import struct, sys
    nsf = open(sys.argv[1], "rb").read()
    assert nsf[:5] == b"NESM\x1a"
    def chunk(cc, body): return struct.pack("<I", len(body)) + cc + body
    info = nsf[8:14] + bytes([nsf[0x7A], nsf[0x7B], nsf[6], max(0, nsf[7] - 1)])
    out = b"NSFE" + chunk(b"INFO", info)
    if any(nsf[0x70:0x78]): out += chunk(b"BANK", nsf[0x70:0x78])
    out += chunk(b"DATA", nsf[0x80:]) + chunk(b"NEND", b"")
    open(sys.argv[2], "wb").write(out)
    ```

### Tracker zoo (crate::openmpt, `--features openmpt`)
libopenmpt plays ~30 module formats beyond xmrs's mod/xm/s3m/it; one build plays all. There is no
encoder, so procure small modules per format from Modland (modland.com/pub/modules/<Format>/...),
one file per extension named `<ext>-sample.<ext>`. libopenmpt detects format by content, so the
on-disk extension only has to route the file to the decoder; a module stored under a different name
on Modland can be saved under the target extension. No oracle: libopenmpt is the only decoder, so
the test is a render smoke test (non-silent audio), same as the chiptunes. Pick a sample at least
~60s long, ideally one to three minutes (most Modland modules are full songs, which is fine - the
source streams, so length does not bloat the tests). Do NOT pick a very short module: a sub-30s
track can't be scrubbed for manual QA (seeking anywhere lands near the end, so it stops and
auto-advances before you hear much), and tracker seeks snap to the nearest pattern row, which is
only noticeable on a tiny track. This bit us with a 15s `.ult`; it was swapped for a ~186s one.
The same "pick something long enough to scrub" rule applies to every streamed format here and to
the chiptunes. Format -> Modland directory used:
- `669` Composer 669 | `amf` Asylum | `ams` Velvet Studio | `c67` Composer 670 (CDFM) |
  `dbm` Digibooster Pro | `digi` Digibooster | `dmf` X-Tracker | `dtm` Digital Tracker DTM |
  `far` Farandole Composer | `gdm` General DigiMusic | `imf` Imago Orpheus | `mdl` Digitrakker |
  `med` OctaMED MMD0 (files are `.mmd0`; save as `.med`) | `mptm` OpenMPT MPTM |
  `mt2` Mad Tracker 2 | `mtm` Multitracker | `okt` Oktalyzer (files are `.okta`; save as `.okt`) |
  `plm` Disorder Tracker 2 | `psm` Epic Megagames MASI | `ptm` Polytracker |
  `stm` Screamtracker 2 | `stp` Soundtracker Pro II | `symmod` Symphonie | `ult` Ultratracker.
- `dsm`: use "Digital Sound Interface Kit RIFF" (the RIFF DSM), not the plain "Digital Sound
  Interface Kit" dir - the older DSIK `.dsm` files fail to load in libopenmpt 0.6.1.
- `c67`: the "Composer 670 (CDFM)" dir holds both `.670` and `.c67` files; take a `.c67` one
  (save as `c67-sample.c67`). The `.670` variant does not load in libopenmpt (tested 0.8.9).
- `.j2b` (Galaxy / Jazz Jackrabbit 2): The Mod Archive has these (search by format J2B). The
  game's own tracks are there, e.g. `water.j2b` at
  `https://api.modarchive.org/downloads.php?moduleid=194172` (magic `MUSE`).
- `.stx` (STMIK, the STM-to-S3M transitional format): rare, but libxmp's test corpus has a real
  one: `https://raw.githubusercontent.com/libxmp/libxmp/master/test-dev/data/m/FutureBrain.stx`
  (magic `!Scream!` at offset 0x14).
- `.umx` (Unreal/UT package): no freely-downloadable raw sample (GitHub copies are 206-byte stubs;
  archive.org items are converted MP3/FLAC). A real one lives in a Deus Ex / Unreal / UT install's
  `Music/` folder. Extract it straight from a GOG offline installer, which is an Inno Setup archive,
  with `innoextract` (no Wine): `innoextract -I "Music/Title_Music.umx" setup_deus_ex_goty_*.exe`
  writes `Music/Title_Music.umx`; save it as `umx-sample.umx`. The Deus Ex GOTY installer carries 35
  `Music/*.umx` tracks; `Title_Music.umx` (the main theme by Alexander Brandon) is the one in use.
  A `.umx` is an Unreal package wrapping a tracker module, which libopenmpt plays directly.
  Verified on Linux; Windows is assumed to match (same libopenmpt 0.8.9 and shared decoder code).

### AdLib / OPL (crate::adplug, `--features adplug`)
AdPlug ships its own LGPL test corpus, which is the whole QA sample set in one place and is
reproducible on both boxes. Clone a pinned AdPlug and copy one working file per routed extension:

    git clone https://github.com/adplug/adplug   # or download a release tarball
    # from adplug/test/testmus/, copy one file per extension, named <ext>-sample.<ext>, e.g.:
    #   doofus.dro -> dro-sample.dro ; 2.CMF -> cmf-sample.cmf ; ALLOYRUN.RAD -> rad-sample.rad ...

One working testmus file per routed extension (all 37 verified by the all-codecs visualizer test):

    a2m MARIO.A2M   adl DUNE19.ADL   agd WORMINTR.AGD  amd TU_BLESS.AMD  bam ARAB.BAM
    bmf bmf1_1.bmf  cff SAILOR.CFF   cmf 2.CMF          d00 TheAlibi.d00  dfm ADAGIO.DFM
    dmo fdance03.dmo dro doofus.dro   got menu.got       ha2 NEWPAGA.HA2   hsc SMKEREM.HSC
    hsp SATNIGHT.HSP hsq MORNING.HSQ  ims go-_-go.ims    jbm DEMO4.JBM     laa mi2.laa
    lds loudness.lds mad TOCCATA.MAD  mdi Flying.mdi     mkj 2001.MKJ      msc blaster2.msc
    mus lines1.mus   rad ALLOYRUN.RAD rix RI051.RIX      sa2 SCALES.SA2    sat TUBES.SAT
    sdb ARRAKIS.SDB  sng PLAYMUS1.SNG sop GALWAY.SOP     sqx GORBI2.SQX    wlf WONDERIN.WLF
    xad bmf1_2.xad   xsm CHILD1.XSM

Windows: there is no DLL to bundle. `packaging/windows/build-adplug.ps1` builds AdPlug 2.4 and
libbinio 1.5 as static libraries and links them into the exe. (A shared MSVC build leaves AdPlug's
exported static data member `CAdPlug::players` unresolved in the shim, because AdPlug's headers carry
no dllimport; static linking sidesteps it.) libbinio has no upstream CMake/MSVC build, so the script
generates one, and AdPlug comes from its git source archive since the release dist tarball omits
CMakeLists.txt. The Linux side (`build-adplug-min.sh`) stays shared, as with gme/openmpt.

No oracle (AdPlug is the only decoder); the test is a render smoke test (non-silent audio), same
as the chiptunes. Notes that bit us, so they are worth repeating:
- Route only self-contained files. `.ksm`, `.rol`, and `.sci` load in the corpus only because their
  companion instrument-bank files (`.bnk`, patch data) sit beside them there; a lone file does not
  play, so those three extensions are not routed.
- A few corpus files are silent or fail to load on their own (e.g. `canonind.rad`, `GRABBAG.MID`);
  pick a file that the all-codecs visualizer test accepts. Avoid extensions that collide with other
  decoders (`.imf`/`.dtm` -> libopenmpt, `.s3m` -> xmrs, `.mid` -> MIDI, `.vgm` -> libgme) or that
  are too generic (`.raw`, `.dat`, `.m`, `.bnk`).

### Commodore 64 SID (crate::sid, `--features sid`)
libsidplayfp emulates the C64 + SID chip; no encoder, and the big archive (HVSC) is not freely
licensed, so do not commit HVSC tunes. Reproducible free options:
- Generate a CC0 tune with GoatTracker (GPL; its `.sng` exports to `.sid`); keep the `.sng`, commit
  the `.sid` as `sid-sample.sid`. This is the preferred nicer sample.
- Dependency-free fallback (what the committed sample uses): a minimal hand-authored PSID - a tiny
  ROM-less 6502 routine that sets SID voice 1 to a sustained triangle note. Build it with:

    ```python
    import struct
    code = bytes([0xA9,0x0F,0x8D,0x18,0xD4, 0xA9,0x1C,0x8D,0x00,0xD4, 0xA9,0x11,0x8D,0x01,0xD4,
                  0xA9,0x00,0x8D,0x05,0xD4, 0xA9,0xF0,0x8D,0x06,0xD4, 0xA9,0x11,0x8D,0x04,0xD4,
                  0x60, 0x60])  # init: vol/freq/ADSR/gate; play: rts
    data = struct.pack("<H", 0x1000) + code
    be = lambda v: struct.pack(">H", v)
    hdr = (b"PSID" + be(2) + be(0x7C) + be(0) + be(0x1000) + be(0x1000 + len(code) - 1)
           + be(1) + be(1) + struct.pack(">I", 0)
           + b"LinAmp SID test".ljust(32,b"\0") + b"LinAmp".ljust(32,b"\0") + b"2026 CC0".ljust(32,b"\0")
           + be(0) + bytes(4))
    open("sid-sample.sid","wb").write(hdr + data)
    ```
No oracle (libsidplayfp is the only decoder); the test is a render smoke test (non-silent audio),
ROM-less. Note: some RSID tunes need the real Commodore ROMs and will be quiet/wrong without them;
that is expected and we do not ship those ROMs.

Version: pin libsidplayfp 2.x (currently 2.16.1). The 2.x series bundles the reSIDfp engine in-tree
(one library) and matches src/sid_shim.cpp's API. libsidplayfp 3.x split reSIDfp into a separate
libresidfp package and changed the API (SidConfig::playback/MONO, SidInfo::maxsids, play signature),
so it neither builds as a single library nor compiles against the shim; do not bump to 3.x without
rewriting the shim.

Windows: there is no DLL to bundle. `packaging/windows/build-sidplayfp.ps1` builds libsidplayfp
2.16.1 (reSIDfp bundled) as a static library and links it into the exe. libsidplayfp has no upstream
CMake/MSVC build, so the script generates a CMakeLists and the configured headers; no 6502 assembler
is needed because the release tarball ships the pre-assembled PSID driver bytes. The Linux side
(`build-sidplayfp-min.sh`) stays shared, as with gme/openmpt.

### Game-engine streams (crate::vgmstream, `--features vgmstream`)
libvgmstream (ISC) plays hundreds of console/engine stream formats. The core build is permissive and
depends only on libc/libm; that core decodes the headline formats (DSP/ADPCM, PS-ADPCM, HCA, ATRAC9,
PCM). The optional `vgmstream-ext` build (VGM_EXT=1) adds libvorbis (BSD, Wwise `.wem`/FSB Vorbis) and
the bundled LGPL FFmpeg (XMA/ATRAC3/WMA), so those payloads decode instead of being skipped; it
DT_NEEDs the same `libavcodec.so.58` the `ffmpeg` feature bundles (shared, no second copy). One build
plays all routed extensions. Routing is an allowlist (`VGMSTREAM_EXTS`), so it never claims generic container names.
No oracle for the single-decoder formats: the test is a render smoke test (non-silent audio), like
the chiptunes. Pick a sample long enough to scrub; most game streams loop, and LinAmp plays them once
with looping disabled.

Linux library: `packaging/linux/build-vgmstream-min.sh` (CMake, pinned vgmstream r2168, core-only,
shared). Windows: `packaging/windows/build-vgmstream.ps1` builds the core-only `libvgmstream` STATIC
target via CMake and links it into the exe (nothing to bundle, like AdPlug/SID). It disables the
Windows front-ends (`BUILD_WINAMP`/`BUILD_FB2K`, the latter needs the foobar2000 SDK) alongside the
external codecs, and installs the static lib as `vgmstream.lib` to match `build.rs`'s `dylib=vgmstream`.

How each allowlist extension gets a sample (all validated hands-on; local QA, gitignored):
- `genh` - minted from any WAV with the script below (the committed baseline sample).
- `vag`, `svag` - the committed pure-Python PS-ADPCM encoder below (reproducible, no .NET, no copyright).
- `dsp`, `idsp`, `hca`, `brstm`, `bcstm`, `bfstm`, `hps` - VGAudio (`VGAudioCli in.wav out.<ext>`),
  needs .NET: the Windows box runs it natively; on the Linux dev box it is sandbox-blocked. `.at9`
  output throws (VGAudio decodes ATRAC9 but has no encoder) and `.bwav`/`.bfwav` come out empty -
  procure those two instead.
- `brstm`, `bcstm`, `bfstm`, `bfwav`, `bwav`, `hca` - also procurable as real game rips from
  GameBanana (method below); that is the only free source found for `bwav`/`bfwav`.
- `at9`, `xvag`, `fsb`, `xwb`, `ktss` - no free encoder and none on GameBanana; procure-only from
  real game rips (Sony `at9tool` makes `.at9` on Windows; otherwise rips).
Mirrors note (2026-11-08): joshw.info is unreachable and SmashCustomMusic gates downloads behind a
login, so GameBanana is the working procurement source for the rip-only formats.

Preferred committed sample - GENH wrapping raw PCM16 (reproducible, no deps, exercises the full
open/render/seek path). From any WAV (here a 60s cut of the shared `hypno-this.wav`):

    ffmpeg -i hypno-this.wav -t 60 -c:a pcm_s16le -ar 44100 -ac 2 hypno60.wav
    python3 genh_gen.py hypno60.wav genh-sample.genh    # script below

    ```python
    import struct, sys, wave
    w = wave.open(sys.argv[1], "rb"); ch, sr, n = w.getnchannels(), w.getframerate(), w.getnframes()
    assert w.getsampwidth() == 2, "need 16-bit PCM"
    pcm = w.readframes(n); w.close()
    START, HEADER_SIZE, PCM16LE, NO_LOOP, INTERLEAVE = 0x40, 0x24, 4, 0xFFFFFFFF, 2
    h = bytearray(START); h[0:4] = b"GENH"
    for off, val in [(0x04, ch), (0x08, INTERLEAVE), (0x0c, sr), (0x10, NO_LOOP),
                     (0x14, n), (0x18, PCM16LE), (0x1c, START), (0x20, HEADER_SIZE)]:
        struct.pack_into("<I", h, off, val)
    open(sys.argv[2], "wb").write(bytes(h) + pcm)
    ```

Real game ADPCM codecs - generate with VGAudio (open source, .NET) on a box that has a runtime
(`dotnet`/`mono`; the Linux dev box has neither, the Windows box can): `VGAudioCli in.wav out.dsp`
(GameCube DSP), `out.idsp`, `out.brstm` / `out.bcstm` / `out.bfstm` (Nintendo stream), `out.hca`
(CRI HCA), `out.hps` (HAL stream). Save as `<ext>-sample.<ext>`. VGAudio has no ATRAC9 encoder, so
`.at9` output throws; its `.bwav`/`.bfwav` output is empty - procure those three from GameBanana below.
On Linux, VGAudio's netcoreapp2.0 build runs under .NET 8 with `DOTNET_ROLL_FORWARD=LatestMajor`, but
executing it is sandbox-blocked on the dev box ("Code from External"); the Windows box has no such gate.

PS-ADPCM (`vag`, `svag`) - the committed pure-Python encoder below (no .NET, no copyright, cross-
platform). It mirrors vgmstream's `psx_decoder.c` exactly, so round-trips are faithful, and writes a
standard `VAGp` (meta/vag.c) and a Konami `Svag` (meta/svag_kcet.c), both mono. Run
`python3 mk_vag.py in.wav vag-sample.vag svag-sample.svag` (any 16-bit WAV; stereo is downmixed):

    ```python
    import struct, sys, wave
    F0 = [0, 60, 115, 98, 122]; F1 = [0, 0, -52, -55, -60]
    def clamp16(x): return 32767 if x > 32767 else -32768 if x < -32768 else x
    def read_wav_mono(path):
        w = wave.open(path, "rb"); assert w.getsampwidth() == 2, "need 16-bit PCM"
        ch, sr, n = w.getnchannels(), w.getframerate(), w.getnframes()
        s = list(struct.unpack("<%dh" % (n*ch), w.readframes(n))); w.close()
        if ch > 1: s = [clamp16(sum(s[i:i+ch])//ch) for i in range(0, len(s), ch)]
        return sr, s
    def encode_psx(samples):
        s = list(samples) + [0]*((28 - len(samples) % 28) % 28)
        out = bytearray(); h1 = h2 = 0; nf = len(s)//28
        for fi in range(nf):
            block = s[fi*28:(fi+1)*28]; best = None
            for filt in range(5):
                c0, c1 = F0[filt], F1[filt]
                for shift in range(13):
                    a, b = h1, h2; nibs = []; sse = 0
                    for x in block:
                        pred = (c0*a + c1*b) >> 6
                        t = int(round((x - pred) * (1 << shift) / 4096.0))
                        t = 7 if t > 7 else -8 if t < -8 else t
                        dec = clamp16(((t << 12) >> shift) + pred)
                        sse += (dec - x)**2; b, a = a, dec; nibs.append(t & 0x0F)
                    if best is None or sse < best[0]: best = (sse, filt, shift, nibs, a, b)
            _, filt, shift, nibs, h1, h2 = best
            out.append((filt << 4) | shift); out.append(0x01 if fi == nf-1 else 0x00)
            for i in range(0, 28, 2): out.append((nibs[i] & 0x0F) | ((nibs[i+1] & 0x0F) << 4))
        return bytes(out)
    sr, pcm = read_wav_mono(sys.argv[1]); adpcm = encode_psx(pcm); data = b"\x00"*16 + adpcm
    v = bytearray(0x30); v[0:4] = b"VAGp"            # VAGp, big-endian, mono
    struct.pack_into(">I", v, 0x04, 0x20); struct.pack_into(">I", v, 0x0C, len(data))
    struct.pack_into(">I", v, 0x10, sr); v[0x20:0x29] = b"linamp-qa"
    open(sys.argv[2], "wb").write(bytes(v) + data)
    k = bytearray(0x800); k[0:4] = b"Svag"           # Konami Svag, little-endian, data at 0x800
    struct.pack_into("<I", k, 0x04, len(data)); struct.pack_into("<I", k, 0x08, sr)
    struct.pack_into("<H", k, 0x0C, 1)               # channels=1 skips the stereo padding check
    open(sys.argv[3], "wb").write(bytes(k) + data)
    ```

Procure real rips - GameBanana (`gamebanana.com`) hosts "Sound" mods as individual, login-free
downloads, and is the working source for `brstm`, `bcstm`, `bfstm`, `bfwav`, `bwav`, and `hca`. Find
one via its API (no auth), download the mod archive, and extract the file with the extension you want:

    # search Sound mods for an extension, list each hit's download archive, pick a small one,
    # 7z-extract it, and copy out the first file that actually has that extension.
    curl -s "https://gamebanana.com/apiv11/Util/Search/Results?_sSearchString=brstm&_sModelName=Sound&_nPerpage=20&_nPage=1"
    curl -s "https://gamebanana.com/apiv11/Sound/<idRow>?_csvProperties=_aFiles"   # -> _sDownloadUrl, _nFilesize
    # then: curl -L <url> -o mod.zip && 7z x mod.zip && find . -iname '*.brstm'

Keep these local only (gitignored, never commit). GameBanana `.xwb` are XMA voice banks that the
core-only (no-ffmpeg) vgmstream build cannot decode, so `xwb` stays genuinely procure-only elsewhere.

ADPCM decoders without .NET - a GENH can also wrap ADPCM that ffmpeg produces, which exercises real
ADPCM decoders (not just PCM) on a box with no VGAudio. Encode to an ADPCM WAV, then wrap the raw
`data` chunk in a GENH with `interleave = nBlockAlign` (MS-ADPCM sets frame_size, MS-IMA sets block
size; both need a nonzero interleave). Codecs used for the local QA samples: MS-ADPCM
(`ffmpeg -c:a adpcm_ms`, GENH codec 11), MS-IMA (`-c:a adpcm_ima_wav`, codec 15), and AICA/Yamaha
(`-c:a adpcm_yamaha -ac 1`, codec 10, interleave 0). These are GENH-container QA aids, not native
`.dsp`/`.hca` files, so they stay local (gitignored) and do not replace the VGAudio/procured samples.

Still procure-only (no free encoder, not on GameBanana) - `at9` (Sony `at9tool`, Windows SDK),
`xvag`, `fsb`, `xwb`, `ktss`: real game rips or vgmstream issue/PR attachments; local QA only, never
commit. Oracle/cross-check if vgmstream-cli is installed: `vgmstream-cli -i -o ref.wav <file>` (decode,
looping disabled) and `vgmstream-cli -m <file>` (metadata).

Not routed on purpose: `.adx`/`.at3`/`.oma` (already on ffdec), `.awb` (AMR-WB owns it; `.acb` is only
its cue sheet), and vgmstream's generic claims (`.ogg`/`.wav`/`.aac`/...). Formats whose payload needs
an external codec (Vorbis FSB, XMA/ATRAC3/WMA) fail to open in the core-only build and are skipped
until a later FFmpeg/Vorbis build tier lands.

### PlayStation PSF (crate::psf, `--features psf`)
libupse emulates the PS1 sound hardware and runs the driver a PSF carries; it is ROM-less (its own
HLE FakeBIOS), so no PlayStation BIOS is needed or shipped. PSF1 only - PSF2 was dropped because
libupse's PS2 IOP HLE is a stub (`upse_ps2_iop_call` is empty) so PSF2 decodes to silence, and the
only working engine (Highly Experimental) needs the banned PS2 BIOS. No oracle (libupse is the only
decoder); the test is a render smoke test (non-silent audio), like the chiptunes.

Preferred committed sample - a minimal hand-authored CC0 PSF1 (reproducible, no copyright, exercises
the full open/render/seek path). It is a tiny PS-X EXE that sets SPU voice 0 to noise mode and keys it
on, producing continuous audible noise (the PS1 SPU is sample-based, so a noise voice is the simplest
sound that needs no ADPCM sample data). Note the I/O base is `0x1F80_0000`, not the `0xBF80` KSEG1
mirror - libupse maps I/O there. Build it with (`python3 mkpsf.py psf-sample.psf`):

    ```python
    import struct, zlib
    def I(op, rs=0, rt=0, imm=0): return (op<<26)|(rs<<21)|(rt<<16)|(imm&0xFFFF)
    LUI,ORI,SH,BEQ = 0x0F,0x0D,0x29,0x04; Z,T0,T1,T2 = 0,8,9,10
    c = [I(LUI,0,T0,0x1F80),                                   # t0 = 0x1F800000 (I/O base)
         I(ORI,Z,T1,0x1800), I(SH,T0,T1,0x1D80), I(SH,T0,T1,0x1D82),  # main vol L/R
         I(SH,T0,T1,0x1C00), I(SH,T0,T1,0x1C02),                      # voice0 vol L/R
         I(ORI,Z,T2,0x1000), I(SH,T0,T2,0x1C04),                      # voice0 pitch
         I(ORI,Z,T2,0x00FF), I(SH,T0,T2,0x1C08),                      # voice0 ADSR1 (fast attack, sustain max)
         I(ORI,Z,T2,0x0000), I(SH,T0,T2,0x1C0A),                      # voice0 ADSR2 (hold)
         I(ORI,Z,T1,0x0001), I(SH,T0,T1,0x1D94),                      # NON: voice0 noise mode
         I(ORI,Z,T2,0xFF00), I(SH,T0,T2,0x1DAA),                      # SPUCNT: enable+unmute+noise clock
         I(ORI,Z,T1,0x0001), I(SH,T0,T1,0x1D88),                      # KON voice0
         I(BEQ,Z,Z,0xFFFF), 0]                                        # b . ; nop
    text = b"".join(struct.pack("<I",w) for w in c); text = text.ljust((len(text)+2047)&~2047, b"\0")
    PC0 = 0x80010000
    hdr = bytearray(0x800); hdr[0:8] = b"PS-X EXE"
    struct.pack_into("<I",hdr,0x10,PC0); struct.pack_into("<I",hdr,0x18,PC0)
    struct.pack_into("<I",hdr,0x1C,len(text)); struct.pack_into("<I",hdr,0x30,0x801FFF00)
    comp = zlib.compress(bytes(hdr)+text, 9)
    psf = b"PSF"+bytes([1])+struct.pack("<I",0)+struct.pack("<I",len(comp))+struct.pack("<I",zlib.crc32(comp)&0xFFFFFFFF)+comp
    psf += b"[TAG]utf8=1\ntitle=LinAmp PSF test\nartist=LinAmp\ncopyright=2026 CC0\n"
    open("psf-sample.psf","wb").write(psf)
    ```

Real game tunes (for richer manual QA): procure PSF1 from Zophar's Domain
(`zophar.net/music/playstation-psf`). These are copyrighted game rips - local QA only, never commit.
A `.minipsf` needs its `.psflib` beside it; libupse resolves the `_lib` reference. Pick a tune long
enough to scrub. (PSF2 is not supported - see above - so there is no PSF2 QA sample.)

Windows: `packaging/windows/build-upse.ps1` (written by the Windows box) builds libupse from the same
audiodecoder.upse source and links it, like the AdPlug/SID handoff.
The Linux side (`build-upse-min.sh`) compiles the vendored lib/libupse sources into libupse.so.

### Oracles
The `oracles/*.f32` and `*.s16` files are headerless raw-PCM decode references for the correlation
tests. They are not a playable format and are deliberately absent from SUPPORTED_EXTS.
