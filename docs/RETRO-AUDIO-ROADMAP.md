# Retro Audio Roadmap

Goal: make LinAmp a one-stop destination for audio. Drop in anything, past or present,
and get sound out. This document is the plan for widening format coverage, with special
attention to the retro and chiptune world, and to how we get or generate a test file for
every new format so both the Linux and Windows machines can QA it.

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
- Deferred within this feature: the FFmpeg-backed codecs (XMA / ATRAC3 / WMA - a lot of Xbox 360 and
  PS3 content) and the Vorbis/CELT/Speex-payload variants (Wwise WEM Vorbis, FSB Vorbis). They need
  an external-codec build tier; a file that needs one simply fails to open today and is skipped. Add
  as a second tier later (LGPL FFmpeg, no `--enable-gpl`, same posture as the existing gme/adplug LGPL
  libs) if the demand is there.

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
malformed input. Routes `.psf`/`.minipsf` only. Linux build (`build-upse-min.sh`),
CI, and tarball bundling wired; Windows build/bundling (`build-upse.ps1` + Windows CI) pending on the
Windows box.

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
  - SSF/DSF: Highly Theoretical (GPL-2, MAME-free).  USF: lazyusf2 (GPL-2).
  - GSF: prefer **lazygsf + mGBA (MIT + MPL-2.0)** to keep it off GPL, else viogsf (GPL-2).
  - QSF: Highly Quixotic (GPL-2).  NCSF: SSEQPlayer (GPL-2/BSD, lightweight - no full emulator).
  - 2SF: vio2sf (GPL-2; drags in the full DeSmuME core - the heaviest of the set).
- License reality: enabling any `*SF` feature makes that build **GPL** (GPL-2, or GPL-3 if HE is used
  for PSF) - acceptable under mantra rule 5, exactly the status quo of the existing `sid` feature.
- **Wall: SNSF** (SNES). The only core is snsf9x, which bundles snes9x under a **non-commercial /
  personal-use-only** license. Skip it and document; the practical cost is nil, because SNES music is
  already covered as **SPC via the existing gme feature**. Also avoid the old monolithic `nmlgc/aosdk`
  SDK as a source - it mixes in MAME's non-commercial clause; use the kode54/Kodi split-cores instead.
- No pure-Rust option exists for any subformat (confirmed on crates.io); C/C++ is the floor.
- Suggested order when it is taken up: PSF1 first (biggest corpus; done), then SSF+DSF and USF, then
  GSF/QSF/NCSF, then 2SF. One tier (or a couple) per branch, like the 2a-2d work.

## Phase 3 - Modern and ideal-fit

- QOA (Quite OK Audio, `.qoa`): a 2023 format, dead simple, MIT/public-domain, with a
  pure-Rust decoder already available (`qoaudio`). This is the only candidate that satisfies
  every principle at once. Low effort, good signal that we track new formats.
- Tagging: `lofty` 0.21 is already a dependency. Audit which of the new formats it can tag
  and widen read/write coverage as formats land (principle 1 covers tagging, not just decode).

## Phase 4 - Non-codec audio sources (the "drop anything" glue)

- CUE sheets (`.cue`): split a single-file album (with `.flac`/`.ape`/`.wv`) into real
  tracks. Large UX win for ripped albums; no new codec, just index + seek.
- Playlists: `.m3u`/`.m3u8`, `.pls`, `.xspf`, `.asx`, and `.ram` (RealAudio metafile).
- Internet radio: SHOUTcast/Icecast streaming (existing issue #146, the minibrowser pane).

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

Phase 2e (vgmstream):
- Generate (preferred, reproducible): a GENH (`.genh`) wrapping raw PCM16 is mintable from any WAV
  with a short script (no deps) - see the cookbook; this is the committed baseline sample and
  exercises the full open/render/seek path. The real game ADPCM codecs are generated with VGAudio
  (`VGAudioCli in.wav out.dsp` / `.brstm` / `.bcstm` / `.bfstm` / `.hca`) on a box with a .NET runtime
  (the Linux dev box has none; the Windows box can). ADX is mintable with ffmpeg but ADX stays on the
  ffdec path, so it is not a vgmstream sample.
- Procure (no open encoder): FMOD FSB, Microsoft XWB, Sony XVAG, Koei KTSS - from freely-licensed /
  homebrew rips and the sample files attached to vgmstream GitHub issues/PRs. Local QA only (gitignored).
- Oracle: `vgmstream-cli -i -o ref.wav <file>` decodes with looping disabled as a reference; cross-check
  metadata with `vgmstream-cli -m <file>`. libvgmstream and the CLI share the decode core, so for the
  single-decoder formats the committed test is a non-silent render smoke test, like the chiptunes.

Phase 2f (*SF family), when taken up:
- Procure only: Zophar's Domain per-format music sets (`zophar.net/music/<fmt>`). These are copyrighted
  commercial game rips - local QA only, never commit (matches the per-box gitignored-samples rule).
  Generation (USFMaker, Zophar's PSF utilities) still needs a copyrighted ROM/EXE input, so it is not
  truly license-free. Pick a set whose subsong 0 has audio and is long enough to scrub.
- Oracle: each format's library is the sole decoder, so a non-silent render smoke test is the fallback.

Phase 3:
- QOA: generate directly - the reference `qoaconv` tool converts a wav to `.qoa`. Oracle:
  decode back with `qoaconv` or the reference decoder.

### Briefing the Windows box

The Windows machine keeps its own copy of `qa-music/format-samples/` (gitignored, so it is
never in git and must be built locally on each box). After we implement a format here, post
to the coordination Slack (the self-DM, messages marked as from the Linux box) with:

- The format, the extension(s), and the commit.
- The exact sample recipe: the generator command or the procurement URL, verbatim.
- The oracle command.
- Any platform caveat.

All the generator tools named above (ffmpeg, openmpt123, Furnace, FamiStudio, Game Music Emu
CLI, sidplayfp, qoaconv) run on Windows as well, so the recipes are cross-platform; note any
path or packaging differences when they come up. The canonical, always-current copy of each
recipe lives in `qa-music/format-samples/README.md`; this roadmap is the plan, the README is
the cookbook.

## Status tracker

Phase 1 - FFmpeg freebies
- [ ] Shorten (.shn)
- [ ] MPEG-4 ALS (.als)
- [ ] TwinVQ (.vqf)
- [ ] CRI ADX (.adx)
- [ ] Amiga 8SVX (.8svx/.iff)
- [ ] Creative VOC (.voc), Sun AU (.au/.snd), Wave64 (.w64)
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
  rendered to both channels, default length + fade, subsong 0. `.sid`/`.psid`/`.rsid`. Windows
  build/bundling pending on the Windows box.

Phase 2 stretch
- [x] vgmstream (game-engine streams: DSP/VAG/HCA/BRSTM/BCSTM/BFSTM/BFWAV/BWAV/AT9/XVAG/FSB/XWB/HPS/
  KTSS/IDSP/SVAG/GENH) - `vgmstream` feature, src/vgmstream.rs (direct C-API FFI, no shim). Core-only
  libvgmstream (ISC, permissive, libc/libm only); streaming + native seek + f32 out; allowlist routing,
  runs last. Linux build (`build-vgmstream-min.sh`, CMake, pinned r2168), CI, and tarball bundling wired.
  Windows build/bundling (`build-vgmstream.ps1` + Windows CI feature add) pending on the Windows box.
  FFmpeg-backed codecs (XMA/ATRAC3/WMA) and Vorbis/CELT-payload variants deferred to a later build tier.
- [x] `*SF` family, PSF1 - `psf` feature, src/psf.rs + src/psf_shim.c (libupse, GPL). ROM-less
  (libupse's HLE FakeBIOS; no PlayStation BIOS, not HE which needs the real BIOS). Native seek, stereo.
  `.psf`/`.minipsf` only (PSF2 dropped - libupse IOP HLE is a stub -> silence; see 2f-psf). Linux
  build/CI/tarball wired; Windows build pending on the Windows box. Reproducible CC0 QA sample
  (hand-authored minimal PSF, recipe in the cookbook).
- [ ] `*SF` family, the rest (GSF/USF/2SF/SSF/DSF/QSF/NCSF) - planned (see "Phase 2 stretch" above);
  GPL per-core. BIOS/ROM-check each core before adopting. SNSF is a wall (snes9x non-commercial).

Phase 3 - Modern/ideal
- [ ] QOA (.qoa)
- [ ] Tagging coverage audit (lofty)

Phase 4 - Non-codec sources
- [ ] CUE sheets
- [ ] Playlists (m3u/pls/xspf/asx/ram)
- [ ] Internet radio (#146)

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
libvgmstream (ISC) plays hundreds of console/engine stream formats. We build it core-only (no
ffmpeg/vorbis/mpg123 externals) so it is fully permissive and depends only on libc/libm; that core
decodes the headline formats (DSP/ADPCM, PS-ADPCM, HCA, ATRAC9, PCM). One build plays all routed
extensions. Routing is an allowlist (`VGMSTREAM_EXTS`), so it never claims generic container names.
No oracle for the single-decoder formats: the test is a render smoke test (non-silent audio), like
the chiptunes. Pick a sample long enough to scrub; most game streams loop, and LinAmp plays them once
with looping disabled.

Linux library: `packaging/linux/build-vgmstream-min.sh` (CMake, pinned vgmstream r2168, core-only,
shared). Windows: `packaging/windows/build-vgmstream.ps1` builds the core-only `libvgmstream` STATIC
target via CMake and links it into the exe (nothing to bundle, like AdPlug/SID). It disables the
Windows front-ends (`BUILD_WINAMP`/`BUILD_FB2K`, the latter needs the foobar2000 SDK) alongside the
external codecs, and installs the static lib as `vgmstream.lib` to match `build.rs`'s `dylib=vgmstream`.

VGAudio sample set (Windows box, needs .NET): `VGAudioCli in.wav out.<ext>` cleanly generates `dsp`,
`idsp`, `hca`, `brstm`, `bcstm`, `bfstm`, and `hps` (it has no `bwav` encoder and emits an empty
`bfwav`, so skip those two). The rest of the allowlist (`vag`, `svag`, `at9`, `xvag`, `fsb`, `xwb`,
`ktss`) has no free encoder and is procure-only from real game rips (local QA, never committed).

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
(GameCube DSP), `out.brstm` / `out.bcstm` / `out.bfstm` (Nintendo stream), `out.hca` (CRI HCA). Save
as `<ext>-sample.<ext>`.

ADPCM decoders without .NET - a GENH can also wrap ADPCM that ffmpeg produces, which exercises real
ADPCM decoders (not just PCM) on a box with no VGAudio. Encode to an ADPCM WAV, then wrap the raw
`data` chunk in a GENH with `interleave = nBlockAlign` (MS-ADPCM sets frame_size, MS-IMA sets block
size; both need a nonzero interleave). Codecs used for the local QA samples: MS-ADPCM
(`ffmpeg -c:a adpcm_ms`, GENH codec 11), MS-IMA (`-c:a adpcm_ima_wav`, codec 15), and AICA/Yamaha
(`-c:a adpcm_yamaha -ac 1`, codec 10, interleave 0). These are GENH-container QA aids, not native
`.dsp`/`.hca` files, so they stay local (gitignored) and do not replace the VGAudio/procured samples.

Procure (no open encoder) - FSB/XWB/XVAG/KTSS from freely-licensed or homebrew rips and vgmstream
issue/PR attachments; local QA only, never commit. Oracle/cross-check if vgmstream-cli is installed:
`vgmstream-cli -i -o ref.wav <file>` (decode, looping disabled) and `vgmstream-cli -m <file>` (metadata).

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

Windows: `packaging/windows/build-upse.ps1` (to be written by the Windows box) builds libupse as a
static lib or DLL from the same audiodecoder.upse source and links it, like the AdPlug/SID handoff.
The Linux side (`build-upse-min.sh`) compiles the vendored lib/libupse sources into libupse.so.

### Oracles
The `oracles/*.f32` and `*.s16` files are headerless raw-PCM decode references for the correlation
tests. They are not a playable format and are deliberately absent from SUPPORTED_EXTS.
