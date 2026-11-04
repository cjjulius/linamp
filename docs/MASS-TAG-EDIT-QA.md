# Mass Tag Editing: cross-format QA and test plan

Branch: feat/mass-tag-edit. This covers the field-by-format write vet the mass
editor needed, so no field is silently dropped on save. It pairs with the design
in [MASS-TAG-EDIT-PLAN.md](MASS-TAG-EDIT-PLAN.md).

## The problem

A mass edit writes the same changed fields into every selected file through each
file's own tag writer. lofty maps some frames only for ID3v2, so on a non-MP3 save
those fields vanished: the write returned Ok, but the value was gone on reload.
This looked like random "whack-a-mole" breakage because it depended on which
format each file used, not which field you typed.

## The vet

Rather than test combinations by hand, the write path is exercised against one
real sample of every editable format and each field is read back. A file set is
better than reasoning about lofty's maps, because serialization differs from the
in-memory tag (example: MP4 compilation and APE disc round-trip in memory but not
through a real file).

The automated version is the test
`tags::tests::mass_edit_round_trips_every_field_across_formats`. It copies each
sample, writes the full field set twice with different values ("set everything to
one set, then another, confirm it sticks"), and asserts every field reads back.
Samples live in `qa-music/format-samples/` (gitignored), so a format with no
local sample is skipped; the bundled MP3 always runs. To run the full matrix
locally:

```
cargo test --bin linamp mass_edit_round_trips_every_field_across_formats
```

## What was dropped, before the fix

Real-file round-trip, write through the mass path then read back:

| Format        | Fields dropped before the fix          |
|---------------|----------------------------------------|
| MP3 / MP2     | none                                   |
| FLAC          | Original Artist, URL                   |
| Ogg Vorbis    | Original Artist, URL                   |
| Opus          | Original Artist, URL                   |
| M4A / MP4     | Original Artist, URL, Compilation      |
| APE           | Original Artist, URL, Disc, BPM        |

Composer, Copyright and Encoded by survived everywhere, so the user's hunch was
right on Original Artist, URL and BPM, and the vet caught Disc and Compilation too.

## The fix

lofty maps Original Artist (TOPE) and URL (WOAS) only for ID3v2, and MP4 `cpil`
plus APE disc/bpm do not round-trip through its typed writers. The fix writes each
field where mainstream players actually read it, working around lofty where needed.
See `apply_v2`, `fields_from_tag`, `fallback_key`, `write_mp4_native_atoms` and
`read_mp4_bpm` in [src/tags.rs](../src/tags.rs).

- ID3v2 is untouched: it keeps its standard frames (TOPE, WOAS, TCMP, TPOS, TBPM),
  so MP3 tags stay clean and interoperable.
- Vorbis comments (FLAC/Ogg/Opus) and APEv2 take the community-convention key names
  (example: `ORIGINALARTIST`, `URL`, `DISCNUMBER`, `BPM`, `COMPILATION`). These are
  the same keys MusicBrainz Picard, foobar2000, Kid3 and ffmpeg use, so other tools
  read them. The write uses `insert_unchecked`, because `Tag::insert` re-maps the
  key and silently drops an `ItemKey::Unknown`.
- MP4 compilation and BPM are written as the native `cpil` and `tmpo` atoms, the
  ones iTunes/Apple Music/foobar/ffmpeg read. lofty's generic writer drops both on
  a file save (it keeps them only in memory), so `write_mp4_native_atoms` sets them
  on the concrete `Ilst` after the main write (`cpil` as the 1-byte iTunes boolean,
  `tmpo` as a signed-int atom), and `read_mp4_bpm` reads `tmpo` back (lofty's generic
  read drops it). MP4 Original Artist and URL have no native atom anywhere, so they
  use a freeform `----:com.apple.iTunes:<NAME>` atom, the iTunes-namespace form that
  foobar/Picard/Kid3/ffmpeg still read.

After the fix the matrix is clean: every field round-trips for every format, and
the values land where other players read them.

### Interop, verified

Checked the written files with mutagen and ffmpeg (ffprobe), which read like other
mainstream taggers:

- FLAC / Ogg / Opus: every field is a standard Vorbis comment (`originalartist`,
  `url`, `compilation`, `bpm`, `discnumber`, …).
- APE: every field is a plain APEv2 key (`ORIGINALARTIST`, `URL`, `BPM`,
  `DISCNUMBER`, `Compilation`, …).
- MP4: native atoms for all the standard fields (`cpil`, `tmpo`, `disk`, `trkn`,
  `cprt`, the `©` atoms); Original Artist and URL are iTunes-namespace freeform
  atoms (ffprobe surfaces them as `ORIGINALARTIST` and `URL`). The only field with
  no native home anywhere is Original Artist, which is inherently non-standard.

MP3 is unaffected throughout (standard ID3v2 frames).

## Manual QA checklist (hands-on)

The automated test covers the write/read matrix. These still want an eye:

1. Multi-select a mix (example: several MP3 plus one FLAC and one M4A), open Mass
   Edit Tags (Alt+3, or the context menu), set Original Artist, URL, BPM, Disc and
   Compilation, Update All. Reopen each file in the single editor and confirm the
   values are there.
2. Confirm an unedited field is left alone per file: set only Album across files
   with different Titles, and confirm each Title is unchanged.
3. Basic Info: the Formats row lists editable formats, then any uneditable ones
   (MIDI, tracker modules) in parentheses, and the counts sum to the selection,
   example: `8 MP3, 1 FLAC (2 MIDI, 1 Tracker module)`.
4. Dialog layout and skin parity: load a no-GEN skin and confirm the mass editor
   reads the same as the single-file editor.
