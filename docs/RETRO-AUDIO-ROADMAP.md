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

### Phase 2 stretch: emulated-console song formats
The `*SF` family (PSF/PSF2 PlayStation, GSF Game Boy Advance, 2SF DS, USF N64, SSF Saturn)
and vgmstream (hundreds of game-engine audio formats: HCA, FSB, Wwise WEM, GameCube DSP,
PS VAG, Nintendo BRSTM/BCSTM). High coverage, but heavier and with murkier licensing
(vgmstream bundles several libraries; audit before adopting). Defer until 2a-2d land.

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
- [ ] Game Music Emu (NSF/SPC/GBS/VGM/AY/GYM/HES/KSS/SAP)
- [ ] libopenmpt (the tracker zoo)
- [ ] AdPlug / libADLMIDI (OPL FM, DRO/IMF/RAD, Doom MUS)
- [ ] libsidplayfp (.sid)

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
