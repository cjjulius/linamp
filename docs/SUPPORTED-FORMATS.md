<!-- Maintainers: this is the user-facing format list. Keep it current as codecs are added. -->

# What LinAmp Plays

LinAmp plays a lot: your everyday music, the audio hiding inside a video file, and a deep back
catalog of retro game and computer music. The rule of thumb is simple. If you can find your
file's extension in one of the tables below, LinAmp can open it and play it.

Formats are grouped by what they actually are, not by the machinery behind them:

- [Everyday audio](#everyday-audio) - MP3, FLAC, and the usual suspects
- [Audio from video](#audio-from-video) - pull the soundtrack out of a video file
- [Retro audio](#retro-audio) - console and computer music, sorted by system
- [Other](#other) - MIDI files and audio CDs

One thing about the retro formats: they are not "decoded" the way an MP3 is. LinAmp emulates the
original sound chip and renders it fresh at 44.1 kHz stereo, so there is no bitrate to quote. A
lot of these files also hold several tunes in one file, and LinAmp plays the first one.

---

## Everyday audio

The files you probably have the most of.

### Lossy (compressed)

| Extension | Format | Good to know |
|---|---|---|
| `.mp3` `.mp1` `.mp2` `.mpa` | MPEG audio (Layer I/II/III) | Every bitrate, CBR or VBR |
| `.aac` | AAC | The audio in most streaming and Apple files |
| `.m4a` | AAC or ALAC | Lossy AAC, or lossless ALAC (listed again below) |
| `.ogg` `.oga` | Ogg Vorbis | |
| `.opus` | Opus | |
| `.wma` | Windows Media Audio | Covers WMA 1, 2, Pro, and Lossless |
| `.mpc` `.mp+` `.mpp` | Musepack | |
| `.dts` | DTS | Including the DTS-HD extensions |
| `.ra` `.rm` | RealAudio / RealMedia | The old RealPlayer formats |
| `.oma` `.omg` `.aa3` `.at3` `.aea` | Sony ATRAC | MiniDisc and OpenMG (ATRAC1 / 3 / 3+) |
| `.amr` `.awb` | AMR | Voice recordings (narrowband and wideband) |
| `.spx` | Speex | Another speech codec |

### Lossless (nothing thrown away)

| Extension | Format | Good to know |
|---|---|---|
| `.flac` | FLAC | The popular lossless choice, up to 24-bit |
| `.wav` | WAV | Uncompressed PCM |
| `.aiff` `.aif` | AIFF | Uncompressed PCM, common on Mac |
| `.ape` | Monkey's Audio | |
| `.wv` | WavPack | |
| `.tta` | True Audio | |
| `.tak` | TAK | Up to 24-bit |
| `.shn` | Shorten | Common in live-concert archives |
| `.w64` | Sony Wave64 | A WAV variant for very long recordings |
| `.thd` `.mlp` | Dolby TrueHD / MLP | Multichannel, mixed down to stereo |
| `.dsf` `.dff` | DSD / SACD | The 1-bit Super Audio CD format |
| `.au` | Sun / NeXT audio | |
| `.caf` | Apple Core Audio | Plays whatever audio the file holds |

---

## Audio from video

Have a video but only want the sound? Drop it in. LinAmp reads the audio track straight out of
these containers and plays it like any other song.

| Extension | Container | Good to know |
|---|---|---|
| `.mp4` `.m4v` `.mov` `.m4b` `.3gp` | MPEG-4 / QuickTime | Plays the main audio track |
| `.mkv` `.webm` | Matroska / WebM | |
| `.ogv` | Ogg video | |
| `.avi` | AVI | |
| `.flv` | Flash Video | |
| `.mpg` `.mpeg` `.vob` `.m2p` | MPEG Program Stream | DVD-style discs and rips |
| `.ts` `.m2ts` `.mts` | MPEG Transport Stream | Blu-ray and broadcast recordings |
| `.ac3` `.eac3` `.ec3` | Dolby Digital / E-AC-3 | On its own, or lifted from a video |

---

## Retro audio

This is where LinAmp goes deep: chip music and tracker tunes from decades of consoles, home
computers, and PC games.

### Game consoles and home computers

Sound-chip music, grouped by the machine it came from.

#### Nintendo

| Extension | System | Good to know |
|---|---|---|
| `.nsf` `.nsfe` | NES / Famicom | `.nsfe` just adds extra track info |
| `.gbs` | Game Boy | |
| `.spc` | SNES / Super Famicom | |

#### Sega

| Extension | System | Good to know |
|---|---|---|
| `.vgm` `.vgz` | Genesis / Mega Drive, Master System, arcade | `.vgz` is the zipped version of `.vgm` |
| `.gym` | Genesis / Mega Drive | |

#### NEC

| Extension | System |
|---|---|
| `.hes` | PC Engine / TurboGrafx-16 |

#### Atari

| Extension | System |
|---|---|
| `.sap` | Atari 8-bit |

#### Sinclair / Amstrad

| Extension | System |
|---|---|
| `.ay` | ZX Spectrum, Amstrad CPC |

#### MSX

| Extension | System |
|---|---|
| `.kss` | MSX |

### PC: AdLib and Sound Blaster (OPL FM)

The bleepy FM-synth sound of early-90s DOS games, from the AdLib and Sound Blaster cards. Nearly
40 formats in all, from raw DOSBox captures to the music drivers of individual games. The common
ones:

| Extension | Format | Good to know |
|---|---|---|
| `.dro` | DOSBox Raw OPL | A recording of what a game sent to the OPL chip |
| `.cmf` | Creative Music File | Shipped with early Sound Blaster cards |
| `.rad` | Reality AdLib Tracker | |
| `.hsc` | HSC AdLib Composer | |
| `.laa` | LucasArts AdLib | LucasArts adventure games |
| `.d00` | EdLib | |
| `.a2m` | AdLib Tracker 2 | |
| `.adl` | Westwood ADL | Westwood Studios games |
| `.mus` | Doom DMX | id Software's OPL music |
| `.wlf` | Wolfenstein 3-D IMF | |
| `.lds` | Loudness | |
| `.amd` | AMUSIC | |
| `.cff` | BoomTracker | |
| `.dmo` | Twin TrackPlayer | |
| `.xad` | XAD | A wrapper around several AdLib sub-formats |
| `.bam` | Bob's AdLib Music | |
| `.bmf` | Easy AdLib | |
| `.mdi` | AdLib MIDI | |

Plus a long tail of game and tracker formats that work the same way:
`.got` `.agd` `.ha2` `.hsp` `.hsq` `.ims` `.jbm` `.mad` `.mkj` `.msc` `.rix` `.sa2` `.sat`
`.sdb` `.sng` `.sop` `.sqx` `.xsm` `.dfm`.

### Tracker modules

Pattern-based "module" music, born on the Amiga and everywhere in the demoscene since. These pack
their own instrument samples and a sequence of notes into one file.

The big four:

| Extension | Format | Good to know |
|---|---|---|
| `.mod` | ProTracker / SoundTracker | The original Amiga module |
| `.xm` | FastTracker 2 | |
| `.s3m` | ScreamTracker 3 | |
| `.it` | Impulse Tracker | |

And a whole zoo of about 28 more, for when you dig into the archives:

| Extension | Format |
|---|---|
| `.mptm` | OpenMPT |
| `.med` | OctaMED |
| `.far` | Farandole Composer |
| `.okt` | Oktalyzer |
| `.669` | Composer 669 |
| `.mtm` | MultiTracker |
| `.dbm` `.digi` | DigiBooster |

Plus: `.amf` `.ams` `.c67` `.dmf` `.dsm` `.dtm` `.gdm` `.imf` `.j2b` `.mdl` `.mt2` `.plm`
`.psm` `.ptm` `.stm` `.stp` `.stx` `.symmod` `.ult` `.umx` (Velvet Studio, X-Tracker, DSIK,
Imago Orpheus, MadTracker, Epic MASI, Symphonie, UltraTracker, Unreal, and more).

### Vintage sampled audio

A few older formats that store real recorded samples rather than chip instructions, so unlike
everything else in this section they are genuine recorded sound.

| Extension | Format | Good to know |
|---|---|---|
| `.8svx` | Amiga IFF 8SVX | 8-bit Amiga samples |
| `.voc` | Creative Voice | The Sound Blaster sample format |
| `.adx` | CRI ADX | Game audio (Dreamcast and arcade) |

---

## Other

| Source | What it does |
|---|---|
| MIDI (`.mid` `.midi`) | Played through a built-in SoundFont, so you do not need any external synth |
| Audio CD | Pops in an audio CD and plays it straight from the drive |

---

## On the roadmap

- Commodore 64 SID (`.sid`) is next in line.
- More AdLib MIDI-wrapper formats (XMI, HMI, HMP) may follow.
