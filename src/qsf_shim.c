/* C shim over Highly_Quixotic (GPL-3) + psflib (MIT) for the `qsf` feature: Capcom QSound QSF arcade
 * music (src/qsf.rs). Highly_Quixotic emulates the Capcom Z80 + QSound DSP from a reverse-engineered,
 * reimplemented-in-C algorithm, so it is ROM-less: no copyrighted QSound DSP ROM is needed or shipped
 * (the Z80 program and sample ROM come from inside the .qsf/.miniqsf file, parsed by the shared
 * psflib). Ported from the Kodi audiodecoder.qsf reference (src/QSFCodec.cpp + src/QSoundRom.h): the
 * loader sorts tagged sections (KEY / Z80 / SMP) into separate ROM buffers, then sets the kabuki key
 * and the Z80 + sample ROMs into a cleared emulator state. Output is 24038 Hz s16 stereo (QSound's
 * native rate); the Rust side reports that and lets the pipeline resample. Mirrors src/ssf_shim.c. */
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include "psflib.h"
#include "qsound.h"

static uint32_t get_le32(const void *p) {
    const unsigned char *b = (const unsigned char *) p;
    return (uint32_t) b[0] | ((uint32_t) b[1] << 8) | ((uint32_t) b[2] << 16) | ((uint32_t) b[3] << 24);
}
static uint32_t get_le16(const void *p) {
    const unsigned char *b = (const unsigned char *) p;
    return (uint32_t) b[0] | ((uint32_t) b[1] << 8);
}
static uint32_t endian_swap32(uint32_t x) {
    return (x << 24) | ((x << 8) & 0x00FF0000) | ((x >> 8) & 0x0000FF00) | (x >> 24);
}

/* --- stdio file callbacks for psflib --- */
static void *qsf_fopen(void *ctx, const char *path) { (void) ctx; return fopen(path, "rb"); }
static size_t qsf_fread(void *buf, size_t sz, size_t n, void *h) { return fread(buf, sz, n, (FILE *) h); }
static int qsf_fseek(void *h, int64_t off, int whence) { return fseek((FILE *) h, (long) off, whence); }
static int qsf_fclose(void *h) { return fclose((FILE *) h); }
static long qsf_ftell(void *h) { return ftell((FILE *) h); }
static const psf_file_callbacks QSF_IO = {
    "\\/", NULL, qsf_fopen, qsf_fread, qsf_fseek, qsf_fclose, qsf_ftell,
};

/* --- the three QSound ROM regions, grown and overlaid as sections arrive (QSoundRom.h) --- */
typedef struct {
    uint8_t *d;
    size_t n;
} rbuf;
typedef struct {
    rbuf key, z80, smp;
} qrom;

/* Overlay one tagged section into the right ROM buffer at `start` (grows + zero-fills gaps). */
static void upload_section(qrom *r, const char *tag, uint32_t start, const uint8_t *data, uint32_t size) {
    rbuf *b;
    uint32_t maxsize = 0x7FFFFFFF;
    if (!strcmp(tag, "KEY")) { b = &r->key; maxsize = 11; }
    else if (!strcmp(tag, "Z80")) { b = &r->z80; }
    else if (!strcmp(tag, "SMP")) { b = &r->smp; }
    else return;
    if (start + size < start) return;
    uint32_t newsize = start + size;
    if (newsize > maxsize) return;
    if (newsize > b->n) {
        uint8_t *nd = (uint8_t *) realloc(b->d, newsize);
        if (!nd) return;
        memset(nd + b->n, 0, newsize - b->n);
        b->d = nd;
        b->n = newsize;
    }
    memcpy(b->d + start, data, size);
}

/* psflib load callback: parse the QSF exe body into tagged sections (3-char tag, LE u32 offset, LE u32
 * size, data). Called once per file in the _lib chain; overlaying merges them. (QSFCodec.cpp) */
static int qsound_load(void *context, const uint8_t *exe, size_t exe_size,
                       const uint8_t *reserved, size_t reserved_size) {
    qrom *r = (qrom *) context;
    (void) reserved; (void) reserved_size;
    for (;;) {
        if (exe_size < 11) break;
        char tag[4];
        memcpy(tag, exe, 3); tag[3] = 0; exe += 3; exe_size -= 3;
        uint32_t dataofs = get_le32(exe); exe += 4; exe_size -= 4;
        uint32_t datasize = get_le32(exe); exe += 4; exe_size -= 4;
        if (datasize > exe_size) return -1;
        upload_section(r, tag, dataofs, exe, datasize);
        exe += datasize; exe_size -= datasize;
    }
    return 0;
}

typedef struct {
    void *qsound_state;
    qrom rom; /* kept so a seek can reset the state without re-parsing the file */
} qsf_handle;

static int g_inited = 0;

/* Apply the kabuki key + Z80 + sample ROMs to the (cleared) emulator state. */
static void apply_rom(qsf_handle *h) {
    if (h->rom.key.n == 11) {
        uint8_t *p = h->rom.key.d;
        qsound_set_kabuki_key(h->qsound_state,
                              endian_swap32(get_le32(p + 0)),
                              endian_swap32(get_le32(p + 4)),
                              endian_swap32(get_le16(p + 8)),
                              p[10]);
    } else {
        qsound_set_kabuki_key(h->qsound_state, 0, 0, 0, 0);
    }
    qsound_set_z80_rom(h->qsound_state, h->rom.z80.d, (uint32_t) h->rom.z80.n);
    qsound_set_sample_rom(h->qsound_state, h->rom.smp.d, (uint32_t) h->rom.smp.n);
}

/* Reject anything not a PSF-family file of version 0x41 (QSF). Returns 1 for QSF, else 0. */
int qsf_shim_is_qsf(const char *path) {
    unsigned char hd[4];
    FILE *f = fopen(path, "rb");
    if (!f) return 0;
    size_t got = fread(hd, 1, 4, f);
    fclose(f);
    return got == 4 && hd[0] == 'P' && hd[1] == 'S' && hd[2] == 'F' && hd[3] == 0x41;
}

void *qsf_shim_open(const char *path) {
    if (!qsf_shim_is_qsf(path))
        return NULL;
    if (!g_inited) {
        if (qsound_init())
            return NULL;
        g_inited = 1;
    }
    qsf_handle *h = (qsf_handle *) calloc(1, sizeof(qsf_handle));
    if (!h) return NULL;
    if (psf_load(path, &QSF_IO, 0x41, qsound_load, &h->rom, NULL, NULL, 0, NULL, NULL) < 0) {
        free(h->rom.key.d); free(h->rom.z80.d); free(h->rom.smp.d); free(h);
        return NULL;
    }
    h->qsound_state = malloc(qsound_get_state_size());
    if (!h->qsound_state) {
        free(h->rom.key.d); free(h->rom.z80.d); free(h->rom.smp.d); free(h);
        return NULL;
    }
    qsound_clear_state(h->qsound_state);
    apply_rom(h);
    return h;
}

void qsf_shim_reset(void *handle) {
    qsf_handle *h = (qsf_handle *) handle;
    if (!h) return;
    qsound_clear_state(h->qsound_state);
    apply_rom(h);
}

int qsf_shim_render(void *handle, int16_t *buf, int frames) {
    qsf_handle *h = (qsf_handle *) handle;
    if (!h || frames <= 0) return 0;
    uint32_t n = (uint32_t) frames;
    if (qsound_execute(h->qsound_state, 0x7FFFFFFF, buf, &n) < 0)
        return 0;
    return (int) n;
}

void qsf_shim_close(void *handle) {
    qsf_handle *h = (qsf_handle *) handle;
    if (!h) return;
    free(h->qsound_state);
    free(h->rom.key.d);
    free(h->rom.z80.d);
    free(h->rom.smp.d);
    free(h);
}
