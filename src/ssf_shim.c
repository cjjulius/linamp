/* C shim over Highly_Theoretical (GPL-3) + psflib (MIT) for the `ssf` feature: Sega Saturn SSF and
 * Dreamcast DSF program music (src/ssf.rs). Highly_Theoretical emulates only the Saturn SCSP and
 * Dreamcast AICA sound chips (ROM-less - no console BIOS or game ROM; the driver program and samples
 * come from inside the .ssf/.dsf file). psflib parses the zlib-compressed PSF-family container and
 * resolves .minissf/.minidsf `_lib` chains. This keeps the Rust side a thin FFI over a handful of
 * functions, mirroring src/psf_shim.c.
 *
 * The load/merge sequence is ported from the Kodi audiodecoder.ssf reference (src/SSFCodec.cpp):
 * psf_load with a load callback that overlays the exe sections at their Sega load addresses, then
 * sega_upload_program into a cleared emulator state. The tune's length comes from its PSF `length`
 * tag, which (as with PSF1) we parse on the Rust side rather than in C. */
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include "psflib.h"
#include "sega.h"

/* --- little-endian helpers --- */
static uint32_t get_le32(const void *p) {
    const unsigned char *b = (const unsigned char *) p;
    return (uint32_t) b[0] | ((uint32_t) b[1] << 8) | ((uint32_t) b[2] << 16) | ((uint32_t) b[3] << 24);
}
static void put_le32(void *p, uint32_t v) {
    unsigned char *b = (unsigned char *) p;
    b[0] = v & 0xff; b[1] = (v >> 8) & 0xff; b[2] = (v >> 16) & 0xff; b[3] = (v >> 24) & 0xff;
}

/* --- stdio file callbacks for psflib --- */
static void *ssf_fopen(void *ctx, const char *path) { (void) ctx; return fopen(path, "rb"); }
static size_t ssf_fread(void *buf, size_t sz, size_t n, void *h) { return fread(buf, sz, n, (FILE *) h); }
static int ssf_fseek(void *h, int64_t off, int whence) { return fseek((FILE *) h, (long) off, whence); }
static int ssf_fclose(void *h) { return fclose((FILE *) h); }
static long ssf_ftell(void *h) { return ftell((FILE *) h); }
static const psf_file_callbacks SSF_IO = {
    "\\/", NULL, ssf_fopen, ssf_fread, ssf_fseek, ssf_fclose, ssf_ftell,
};

/* --- growable program buffer + the SSF/DSF exe-overlay load callback (ported from SSFCodec.cpp) --- */
typedef struct {
    uint8_t *data;
    size_t size;
} growbuf;

static int sdsf_load(void *context, const uint8_t *exe, size_t exe_size,
                     const uint8_t *reserved, size_t reserved_size) {
    growbuf *gb = (growbuf *) context;
    (void) reserved; (void) reserved_size;
    if (exe_size < 4)
        return -1;

    if (gb->size < 4) {
        uint8_t *nd = (uint8_t *) malloc(exe_size);
        if (!nd) return -1;
        memcpy(nd, exe, exe_size);
        gb->data = nd;
        gb->size = exe_size;
        return 0;
    }

    uint32_t dst_start = get_le32(gb->data) & 0x7FFFFF;
    uint32_t src_start = get_le32(exe) & 0x7FFFFF;
    size_t dst_len = gb->size - 4;
    size_t src_len = exe_size - 4;
    if (dst_len > 0x800000) dst_len = 0x800000;
    if (src_len > 0x800000) src_len = 0x800000;

    if (src_start < dst_start) {
        size_t diff = dst_start - src_start;
        uint8_t *nd = (uint8_t *) realloc(gb->data, dst_len + 4 + diff);
        if (!nd) return -1;
        gb->data = nd;
        memmove(nd + 4 + diff, nd + 4, dst_len);
        memset(nd + 4, 0, diff);
        dst_len += diff;
        dst_start = src_start;
        put_le32(nd, dst_start);
    }
    if ((src_start + src_len) > (dst_start + dst_len)) {
        size_t diff = (src_start + src_len) - (dst_start + dst_len);
        uint8_t *nd = (uint8_t *) realloc(gb->data, dst_len + 4 + diff);
        if (!nd) return -1;
        gb->data = nd;
        memset(nd + 4 + dst_len, 0, diff);
        dst_len += diff;
    }

    memcpy(gb->data + 4 + (src_start - dst_start), exe + 4, src_len);
    gb->size = dst_len + 4;
    return 0;
}

/* --- live decoder handle --- */
typedef struct {
    uint8_t version;    /* 0x11 SSF, 0x12 DSF */
    void *sega_state;   /* emulator state buffer */
    uint8_t *program;   /* merged program kept for reset-and-seek */
    size_t program_len;
} ssf_handle;

static int g_inited = 0;

/* Reject anything that is not a PSF-family file of version 0x11 (SSF) or 0x12 (DSF). Content-sniff,
 * since `.dsf` is ambiguous with DSD Stream File (handled elsewhere). Returns the version or 0. */
int ssf_shim_version(const char *path) {
    unsigned char h[4];
    FILE *f = fopen(path, "rb");
    if (!f) return 0;
    size_t got = fread(h, 1, 4, f);
    fclose(f);
    if (got != 4 || h[0] != 'P' || h[1] != 'S' || h[2] != 'F') return 0;
    return (h[3] == 0x11 || h[3] == 0x12) ? h[3] : 0;
}

/* Load an SSF/DSF tune: parse + overlay the program, init a cleared emulator state (dry output, no
 * DSP dynarec), and upload the program. Returns a handle, or NULL on failure. */
void *ssf_shim_open(const char *path) {
    int version = ssf_shim_version(path);
    if (version != 0x11 && version != 0x12)
        return NULL;

    if (!g_inited) {
        if (sega_init())
            return NULL;
        g_inited = 1;
    }

    growbuf gb = {NULL, 0};
    int ret = psf_load(path, &SSF_IO, (uint8_t) version, sdsf_load, &gb, NULL, NULL, 0, NULL, NULL);
    if (ret < 0 || gb.size < 4) {
        free(gb.data);
        return NULL;
    }

    ssf_handle *hnd = (ssf_handle *) calloc(1, sizeof(ssf_handle));
    if (!hnd) { free(gb.data); return NULL; }
    hnd->version = (uint8_t) version;
    hnd->program = gb.data;
    hnd->program_len = gb.size;
    hnd->sega_state = malloc(sega_get_state_size((uint8_t) (version - 0x10)));
    if (!hnd->sega_state) { free(gb.data); free(hnd); return NULL; }

    /* clear + configure + upload; factored so seek can reset without re-parsing the file. */
    uint32_t start = get_le32(hnd->program) & 0x7FFFFF;
    size_t length = hnd->program_len;
    size_t max_length = (version == 0x12) ? 0x800000 : 0x80000;
    if ((size_t) start + (length - 4) > max_length)
        length = max_length - start + 4;

    sega_clear_state(hnd->sega_state, (uint8_t) (version - 0x10));
    sega_enable_dry(hnd->sega_state, 1);
    sega_enable_dsp(hnd->sega_state, 0);
    sega_enable_dsp_dynarec(hnd->sega_state, 0);
    sega_upload_program(hnd->sega_state, hnd->program, (uint32_t) length);
    return hnd;
}

/* Reset to the start (clear state + re-upload the kept program) so the Rust side can seek backward by
 * resetting and rendering forward. Fast: no file re-read. */
void ssf_shim_reset(void *handle) {
    ssf_handle *hnd = (ssf_handle *) handle;
    if (!hnd) return;
    uint32_t start = get_le32(hnd->program) & 0x7FFFFF;
    size_t length = hnd->program_len;
    size_t max_length = (hnd->version == 0x12) ? 0x800000 : 0x80000;
    if ((size_t) start + (length - 4) > max_length)
        length = max_length - start + 4;
    sega_clear_state(hnd->sega_state, (uint8_t) (hnd->version - 0x10));
    sega_enable_dry(hnd->sega_state, 1);
    sega_enable_dsp(hnd->sega_state, 0);
    sega_enable_dsp_dynarec(hnd->sega_state, 0);
    sega_upload_program(hnd->sega_state, hnd->program, (uint32_t) length);
}

/* Render up to `frames` interleaved stereo s16 frames into buf. Returns frames produced, 0 on error.
 * The tune loops forever; the Rust side caps total playback by the length tag. */
int ssf_shim_render(void *handle, int16_t *buf, int frames) {
    ssf_handle *hnd = (ssf_handle *) handle;
    if (!hnd || frames <= 0) return 0;
    uint32_t n = (uint32_t) frames;
    if (sega_execute(hnd->sega_state, 0x7FFFFFFF, buf, &n) < 0)
        return 0;
    return (int) n;
}

void ssf_shim_close(void *handle) {
    ssf_handle *hnd = (ssf_handle *) handle;
    if (!hnd) return;
    free(hnd->sega_state);
    free(hnd->program);
    free(hnd);
}
