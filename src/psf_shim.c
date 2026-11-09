/*
 * Thin C shim over libupse (GPL) for the `psf` feature: PlayStation PSF1/PSF2 music.
 *
 * libupse already exposes a C API, but opening a module needs an I/O-callback struct and a one-time
 * global init, and PSF seeking/length live behind a couple of calls. This shim wires stdio into
 * libupse's iofuncs and presents the handful of functions src/psf.rs binds, keeping the Rust side a
 * plain FFI like the gme/openmpt features.
 *
 * ROM-less: libupse ships its own HLE "FakeBIOS", so no PlayStation BIOS is needed or used. We never
 * call upse_set_custom_bios, so no proprietary BIOS is ever loaded.
 */
#include <stdint.h>
#include <stdio.h>
#include "upse.h"

/* libupse reads the PSF (and any _lib base files) through these callbacks; plain stdio is all it
 * needs. The casts adapt stdio's FILE* to libupse's void* file handle. */
static upse_iofuncs_t PSF_IO = {
    (void *(*)(const char *, const char *)) fopen,
    (size_t (*)(void *, size_t, size_t, void *)) fread,
    (int (*)(void *, long, int)) fseek,
    (int (*)(void *)) fclose,
    (long (*)(void *)) ftell,
};

static int g_inited = 0;

/* libupse crashes on malformed input, so gate every entry point on the PSF signature: the bytes
 * "PSF" followed by a version 0x01 (PSF1) or 0x02 (PSF2). A renamed or truncated file is rejected
 * here instead of faulting inside the emulator. Returns 1 when the file looks like a PSF1/PSF2. */
static int psf_shim_has_signature(const char *path) {
    unsigned char h[4];
    FILE *f = fopen(path, "rb");
    if (!f) {
        return 0;
    }
    size_t got = fread(h, 1, 4, f);
    fclose(f);
    /* PSF1 (version 0x01) only. PSF2 (0x02) is rejected: libupse's PS2 IOP HLE is a stub, so PSF2
     * decodes to silence, and correct PSF2 playback would require the copyrighted PS2 BIOS. */
    return got == 4 && h[0] == 'P' && h[1] == 'S' && h[2] == 'F' && h[3] == 0x01;
}

/* Open a PSF file and return its module, or NULL on failure. Callers serialize access (libupse has
 * process-global emulator state), so the one-time init guard is race-free. */
upse_module_t *psf_shim_open(const char *path) {
    if (!psf_shim_has_signature(path)) {
        return NULL;
    }
    if (!g_inited) {
        upse_module_init();
        g_inited = 1;
    }
    return upse_module_open(path, &PSF_IO);
}

/* Render the next batch of interleaved stereo s16 frames. Returns the frame count (0 at end of
 * stream, respecting the tune's length/fade tags); *out points at libupse's buffer, valid until the
 * next render call. */
int psf_shim_render(upse_module_t *mod, int16_t **out) {
    return upse_eventloop_render(mod, (s16 **) out);
}

/* Seek to an absolute position in milliseconds. Returns nonzero on success. */
int psf_shim_seek(upse_module_t *mod, uint32_t ms) {
    return upse_eventloop_seek(mod, ms);
}

/* Declared length of the currently-open module in milliseconds (0 when the tune carries no length
 * tag). */
uint32_t psf_shim_length_ms(upse_module_t *mod) {
    return (mod && mod->metadata) ? mod->metadata->length : 0;
}

/* Length in milliseconds read from the PSF tags without loading/emulating the module (cheap, for the
 * playlist). 0 when unknown or unreadable. */
uint32_t psf_shim_metadata_length_ms(const char *path) {
    if (!psf_shim_has_signature(path)) {
        return 0;
    }
    upse_psf_t *m = upse_get_psf_metadata(path, &PSF_IO);
    uint32_t len = m ? m->length : 0;
    if (m) {
        upse_free_psf_metadata(m);
    }
    return len;
}

void psf_shim_close(upse_module_t *mod) {
    if (mod) {
        upse_module_close(mod);
    }
}
