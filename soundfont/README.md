# SoundFont

LinAmp plays MIDI (`.mid`/`.midi`) by synthesizing it with a General MIDI
SoundFont, the way classic Winamp used a built-in softsynth. Drop one `.sf2`
file in this folder and MIDI playback uses it; the build copies this folder next
to the executable, and the app also looks for `soundfont/` in the working
directory.

If no `.sf2` is present, MIDI files report that a SoundFont is needed instead of
playing (everything else is unaffected).

Use a SoundFont whose licence permits redistribution if you bundle it with a
release — for example GeneralUser GS, which is free to distribute. Larger
SoundFonts sound fuller but make the download bigger.
