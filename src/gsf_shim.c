/* C shim over libgsf (MPL-2.0) for the `gsf` feature: Game Boy Advance GSF music (src/gsf.rs). libgsf
 * bundles a trimmed mGBA and plays GSF ROM-less (mGBA's built-in HLE BIOS, useBios=false; no Nintendo
 * BIOS). Unlike the other *SF cores it does NOT use psflib: libgsf parses the PSF container, the `_lib`
 * overlay chain, and the PSF tags itself, and it exposes native seeking. This is a thin wrapper over
 * its high-level C API (gsf_new/gsf_load_file/gsf_play/gsf_seek_samples), like src/sid_shim.cpp. Output
 * is 44.1 kHz s16 stereo. */
#include <stdint.h>
#include <stdio.h>

#include "gsf.h"

#define GSF_OUT_RATE 44100

/* Reject anything not a PSF-family file of version 0x22 (GSF). Returns 1 for GSF, else 0. */
int gsf_shim_is_gsf(const char* path) {
    unsigned char hd[4];
    FILE* f = fopen(path, "rb");
    if (!f)
        return 0;
    size_t got = fread(hd, 1, 4, f);
    fclose(f);
    return got == 4 && hd[0] == 'P' && hd[1] == 'S' && hd[2] == 'F' && hd[3] == 0x22;
}

/* The handle is the GsfEmu* (opaque). */
void* gsf_shim_open(const char* path) {
    if (!gsf_shim_is_gsf(path))
        return NULL;
    GsfEmu* emu = NULL;
    if (gsf_new(&emu, GSF_OUT_RATE, 0).code != 0 || !emu)
        return NULL;
    if (gsf_load_file(emu, path).code != 0) {
        gsf_delete(emu);
        return NULL;
    }
    return emu;
}

/* Render `frames` interleaved stereo s16 frames at 44.1 kHz into buf. gsf_play takes a count of 16-bit
 * samples (2 per stereo frame) and zeroes the buffer itself. Returns frames produced. */
int gsf_shim_render(void* handle, int16_t* buf, int frames) {
    if (!handle || frames <= 0)
        return 0;
    gsf_play((GsfEmu*)handle, buf, (long)frames * 2);
    return frames;
}

/* Native seek to an absolute sample (frame) position. libgsf handles backward seeks by resetting
 * internally, so this is the seek primitive (no manual reset + render-forward needed). */
void gsf_shim_seek(void* handle, long frame) {
    if (handle)
        gsf_seek_samples((GsfEmu*)handle, frame);
}

void gsf_shim_close(void* handle) {
    if (handle)
        gsf_delete((GsfEmu*)handle);
}
