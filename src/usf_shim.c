/* C shim over lazyusf2 (GPL-2.0-or-later) + psflib (MIT) for the `usf` feature: Nintendo 64 USF music
 * (src/usf.rs). lazyusf2 is a Mupen64Plus-derived N64 emulator that plays ROM-less - a USF carries a
 * Project64 save state captured after boot plus the game's audio program, so no copyrighted N64 PIF/boot
 * ROM is involved; audio is produced by High-Level Emulation of the RSP microcode. The program + save
 * state live inside the .usf/.miniusf file (its data is in the PSF `reserved` section, not the exe),
 * parsed by the shared psflib (which resolves `_lib` chains). Driving ported from the Kodi
 * audiodecoder.usf reference (src/USFCodec.cpp). Output is pinned to 44.1 kHz s16 stereo via
 * usf_render_resampled. Exposes a C API, like src/qsf_shim.c. */
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>

#include "psflib.h"
#include "usf.h"

#define USF_OUT_RATE 44100

/* stdio file callbacks for psflib */
static void* usf_fopen(void* ctx, const char* path) { (void)ctx; return fopen(path, "rb"); }
static size_t usf_fread(void* buf, size_t sz, size_t n, void* h) { return fread(buf, sz, n, (FILE*)h); }
static int usf_fseek(void* h, int64_t off, int whence) { return fseek((FILE*)h, (long)off, whence); }
static int usf_fclose(void* h) { return fclose((FILE*)h); }
static long usf_ftell(void* h) { return ftell((FILE*)h); }
static const psf_file_callbacks USF_IO = {
    "\\/", NULL, usf_fopen, usf_fread, usf_fseek, usf_fclose, usf_ftell,
};

/* psf_load callback: USF stores its data (ROM + Project64 save state) in the reserved section; the exe
 * section is unused and rejected. psflib calls this for each file in the `_lib` chain (base first), and
 * usf_upload_section merges them. (USFCodec.cpp::usf_load) */
static int usf_load(void* ctx, const uint8_t* exe, size_t exe_size,
                    const uint8_t* reserved, size_t reserved_size) {
    (void)exe;
    if (exe_size > 0)
        return -1;
    return usf_upload_section(ctx, reserved, reserved_size);
}

/* Reject anything not a PSF-family file of version 0x21 (USF). Returns 1 for USF, else 0. */
int usf_shim_is_usf(const char* path) {
    unsigned char hd[4];
    FILE* f = fopen(path, "rb");
    if (!f)
        return 0;
    size_t got = fread(hd, 1, 4, f);
    fclose(f);
    return got == 4 && hd[0] == 'P' && hd[1] == 'S' && hd[2] == 'F' && hd[3] == 0x21;
}

/* The handle is the lazyusf2 state buffer (opaque). */
void* usf_shim_open(const char* path) {
    if (!usf_shim_is_usf(path))
        return NULL;
    void* state = malloc(usf_get_state_size());
    if (!state)
        return NULL;
    usf_clear(state);
    if (psf_load(path, &USF_IO, 0x21, usf_load, state, NULL, NULL, 0, NULL, NULL) < 0) {
        free(state); /* cleared but never rendered: free without usf_shutdown is correct */
        return NULL;
    }
    usf_set_compare(state, 0);
    usf_set_fifo_full(state, 0);
    usf_set_hle_audio(state, 1);
    return state;
}

/* Render up to `frames` interleaved stereo s16 frames at 44.1 kHz into buf. Returns frames produced
 * (0 on error/end). */
int usf_shim_render(void* handle, int16_t* buf, int frames) {
    if (!handle || frames <= 0)
        return 0;
    const char* err = usf_render_resampled(handle, buf, (size_t)frames, USF_OUT_RATE);
    return err ? 0 : frames;
}

/* Seek-by-reset: reload the uploaded program + save state and discard buffered samples. */
void usf_shim_reset(void* handle) {
    if (handle)
        usf_restart(handle);
}

void usf_shim_close(void* handle) {
    if (handle) {
        usf_shutdown(handle); /* required after any render before freeing, or memory leaks */
        free(handle);
    }
}
