/* C++ shim over vio2sf (GPL-2.0-or-later) + psflib (MIT) for the `twosf` feature: Nintendo DS 2SF music
 * (src/twosf.rs). vio2sf is a DeSmuME-derived DS emulator that runs the game's actual ARM sound driver,
 * so it covers DS titles whose audio cannot be cleanly extracted to NCSF. It plays ROM-less - DeSmuME's
 * HLE BIOS SWI tables plus a synthesized blank firmware-settings block stand in for the DS BIOS and
 * firmware, so nothing copyrighted is shipped; the game ROM + save state live inside the .2sf/.mini2sf
 * file, parsed by the shared psflib (which resolves `_lib` chains and applies the diff/overlay maps).
 * The loader (twosf_loader/load_twosf_map/load_twosf_mapz/twosf_info) is ported from the Kodi
 * audiodecoder.2sf reference (src/2SFCodec.cpp); the Kodi silence-suppression/fade machinery is dropped
 * - LinAmp plays what is there. Output is 44.1 kHz s16 stereo. Exposes a C API, like src/ncsf_shim.cpp. */
#include <cstdint>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <new>

// MSVC has no POSIX strcasecmp; it spells the case-insensitive compare _stricmp (declared in <cstring>).
// Map it so twosf_info's tag matching below builds on Windows as it does on Linux/glibc.
#ifdef _MSC_VER
#define strcasecmp _stricmp
#endif

#include <zlib.h>

extern "C" {
#include "psflib.h"
#include "vio2sf/state.h"
}

namespace {

constexpr int TWOSF_RESAMPLE_QUALITY = 4; // sinc, matches the Kodi default

/* The parsed ROM + save state. Owns the blobs for the handle's lifetime: state_setrom stores the ROM
 * pointer (no copy), and they are reused across resets, so they must outlive the emulator. */
struct loader_state {
    uint8_t* rom = nullptr;
    uint8_t* state = nullptr;
    size_t rom_size = 0;
    size_t state_size = 0;
    int initial_frames = -1;
    int sync_type = 0;
    int clockdown = 0;
    int arm9_clockdown_level = 0;
    int arm7_clockdown_level = 0;
};

void* twosf_fopen(void*, const char* path) { return std::fopen(path, "rb"); }
size_t twosf_fread(void* buf, size_t sz, size_t n, void* h) { return std::fread(buf, sz, n, static_cast<FILE*>(h)); }
int twosf_fseek(void* h, int64_t off, int whence) { return std::fseek(static_cast<FILE*>(h), static_cast<long>(off), whence); }
int twosf_fclose(void* h) { return std::fclose(static_cast<FILE*>(h)); }
long twosf_ftell(void* h) { return std::ftell(static_cast<FILE*>(h)); }
const psf_file_callbacks TWOSF_IO = {
    "\\/", nullptr, twosf_fopen, twosf_fread, twosf_fseek, twosf_fclose, twosf_ftell,
};

unsigned get_le32(const void* p) {
    const unsigned char* b = static_cast<const unsigned char*>(p);
    return static_cast<unsigned>(b[3]) << 24 | static_cast<unsigned>(b[2]) << 16 |
           static_cast<unsigned>(b[1]) << 8 | static_cast<unsigned>(b[0]);
}

/* Overlay one ROM/save-state map chunk into the accumulating buffer (power-of-two padded for ROM so
 * state_setrom's size assertion holds). (2SFCodec.cpp::load_twosf_map) */
int load_twosf_map(loader_state* st, int issave, const unsigned char* udata, unsigned usize) {
    if (usize < 8)
        return -1;
    unsigned char* iptr;
    size_t isize;
    unsigned xsize = get_le32(udata + 4);
    unsigned xofs = get_le32(udata + 0);
    if (issave) { iptr = st->state; isize = st->state_size; st->state = nullptr; st->state_size = 0; }
    else { iptr = st->rom; isize = st->rom_size; st->rom = nullptr; st->rom_size = 0; }
    if (!iptr) {
        size_t rsize = xofs + xsize;
        if (!issave) { rsize -= 1; rsize |= rsize >> 1; rsize |= rsize >> 2; rsize |= rsize >> 4; rsize |= rsize >> 8; rsize |= rsize >> 16; rsize += 1; }
        iptr = static_cast<unsigned char*>(std::malloc(rsize + 10));
        if (!iptr)
            return -1;
        std::memset(iptr, 0, rsize + 10);
        isize = rsize;
    } else if (isize < xofs + xsize) {
        size_t rsize = xofs + xsize;
        if (!issave) { rsize -= 1; rsize |= rsize >> 1; rsize |= rsize >> 2; rsize |= rsize >> 4; rsize |= rsize >> 8; rsize |= rsize >> 16; rsize += 1; }
        unsigned char* xptr = static_cast<unsigned char*>(std::realloc(iptr, xofs + rsize + 10));
        if (!xptr) { std::free(iptr); return -1; }
        iptr = xptr;
        isize = rsize;
    }
    std::memcpy(iptr + xofs, udata + 8, xsize);
    if (issave) { st->state = iptr; st->state_size = isize; }
    else { st->rom = iptr; st->rom_size = isize; }
    return 0;
}

/* zlib-inflate a map chunk, then overlay it. (2SFCodec.cpp::load_twosf_mapz) */
int load_twosf_mapz(loader_state* st, int issave, const unsigned char* zdata, unsigned zsize, unsigned zcrc) {
    (void)zcrc;
    int zerr;
    uLongf usize = 8;
    uLongf rsize = usize;
    unsigned char* udata = static_cast<unsigned char*>(std::malloc(usize));
    if (!udata)
        return -1;
    while (Z_OK != (zerr = uncompress(udata, &usize, zdata, zsize))) {
        if (Z_MEM_ERROR != zerr && Z_BUF_ERROR != zerr) { std::free(udata); return -1; }
        if (usize >= 8) { usize = get_le32(udata + 4) + 8; if (usize < rsize) { rsize += rsize; usize = rsize; } else rsize = usize; }
        else { rsize += rsize; usize = rsize; }
        unsigned char* rdata = static_cast<unsigned char*>(std::realloc(udata, usize));
        if (!rdata) { std::free(udata); return -1; }
        udata = rdata;
    }
    unsigned char* rdata = static_cast<unsigned char*>(std::realloc(udata, usize));
    if (!rdata) { std::free(udata); return -1; }
    int ret = load_twosf_map(st, issave, rdata, static_cast<unsigned>(usize));
    std::free(rdata);
    return ret;
}

/* psf_load callback: the exe section carries the ROM map (through the `_lib` chain); the reserved
 * section holds optional "SAVE" (0x45564153) save-state blocks. (2SFCodec.cpp::twosf_loader) */
int twosf_loader(void* ctx, const uint8_t* exe, size_t exe_size, const uint8_t* reserved, size_t reserved_size) {
    loader_state* st = static_cast<loader_state*>(ctx);
    if (exe_size >= 8) {
        if (load_twosf_map(st, 0, exe, static_cast<unsigned>(exe_size)))
            return -1;
    }
    if (reserved_size) {
        size_t rp = 0;
        if (reserved_size < 16)
            return -1;
        while (rp + 12 < reserved_size) {
            unsigned save_size = get_le32(reserved + rp + 4);
            unsigned save_crc = get_le32(reserved + rp + 8);
            if (get_le32(reserved + rp + 0) == 0x45564153) {
                if (rp + 12 + save_size > reserved_size)
                    return -1;
                if (load_twosf_mapz(st, 1, reserved + rp + 12, save_size, save_crc))
                    return -1;
            }
            rp += 12 + save_size;
        }
    }
    return 0;
}

/* psf tag callback: the vio2sf driver tags. (2SFCodec.cpp::twosf_info) */
int twosf_info(void* ctx, const char* name, const char* value) {
    loader_state* st = static_cast<loader_state*>(ctx);
    char* end;
    if (!strcasecmp(name, "_frames")) st->initial_frames = std::strtol(value, &end, 10);
    else if (!strcasecmp(name, "_clockdown")) st->clockdown = std::strtol(value, &end, 10);
    else if (!strcasecmp(name, "_vio2sf_sync_type")) st->sync_type = std::strtol(value, &end, 10);
    else if (!strcasecmp(name, "_vio2sf_arm9_clockdown_level")) st->arm9_clockdown_level = std::strtol(value, &end, 10);
    else if (!strcasecmp(name, "_vio2sf_arm7_clockdown_level")) st->arm7_clockdown_level = std::strtol(value, &end, 10);
    return 0;
}

struct twosf_handle {
    NDS_state* emu = nullptr;
    loader_state ld;
};

/* Point a fresh emulator at the parsed ROM + save state and start it from the top. */
bool setup_emu(twosf_handle* h) {
    h->emu = new (std::nothrow) NDS_state();
    if (!h->emu)
        return false;
    if (state_init(h->emu)) {
        delete h->emu;
        h->emu = nullptr;
        return false;
    }
    h->emu->dwInterpolation = TWOSF_RESAMPLE_QUALITY;
    h->emu->dwChannelMute = 0;
    h->emu->initial_frames = h->ld.initial_frames;
    h->emu->sync_type = h->ld.sync_type;
    h->emu->arm7_clockdown_level = h->ld.arm7_clockdown_level;
    h->emu->arm9_clockdown_level = h->ld.arm9_clockdown_level;
    if (h->ld.rom)
        state_setrom(h->emu, h->ld.rom, static_cast<u32>(h->ld.rom_size), 0);
    state_loadstate(h->emu, h->ld.state, static_cast<u32>(h->ld.state_size));
    return true;
}

} // namespace

extern "C" {

/* Reject anything not a PSF-family file of version 0x24 (2SF). Returns 1 for 2SF, else 0. */
int twosf_shim_is_2sf(const char* path) {
    unsigned char hd[4];
    FILE* f = std::fopen(path, "rb");
    if (!f)
        return 0;
    size_t got = std::fread(hd, 1, 4, f);
    std::fclose(f);
    return got == 4 && hd[0] == 'P' && hd[1] == 'S' && hd[2] == 'F' && hd[3] == 0x24;
}

void* twosf_shim_open(const char* path) {
    if (!twosf_shim_is_2sf(path))
        return nullptr;
    twosf_handle* h = new (std::nothrow) twosf_handle();
    if (!h)
        return nullptr;
    if (psf_load(path, &TWOSF_IO, 0x24, twosf_loader, &h->ld, twosf_info, &h->ld, 1, nullptr, nullptr) < 0) {
        std::free(h->ld.rom);
        std::free(h->ld.state);
        delete h;
        return nullptr;
    }
    if (!h->ld.arm7_clockdown_level) h->ld.arm7_clockdown_level = h->ld.clockdown;
    if (!h->ld.arm9_clockdown_level) h->ld.arm9_clockdown_level = h->ld.clockdown;
    if (!setup_emu(h)) {
        std::free(h->ld.rom);
        std::free(h->ld.state);
        delete h;
        return nullptr;
    }
    return h;
}

/* Render `frames` interleaved stereo s16 frames at 44.1 kHz. Returns frames produced. */
int twosf_shim_render(void* handle, int16_t* buf, int frames) {
    twosf_handle* h = static_cast<twosf_handle*>(handle);
    if (!h || !h->emu || frames <= 0)
        return 0;
    state_render(h->emu, buf, static_cast<unsigned>(frames));
    return frames;
}

/* Seek-by-reset: tear down and rebuild the emulator from the retained ROM + save state. */
void twosf_shim_reset(void* handle) {
    twosf_handle* h = static_cast<twosf_handle*>(handle);
    if (!h)
        return;
    if (h->emu) {
        state_deinit(h->emu);
        delete h->emu;
        h->emu = nullptr;
    }
    setup_emu(h); /* reuses h->ld.rom / h->ld.state (not freed by state_deinit) */
}

void twosf_shim_close(void* handle) {
    twosf_handle* h = static_cast<twosf_handle*>(handle);
    if (!h)
        return;
    if (h->emu) {
        state_deinit(h->emu);
        delete h->emu;
    }
    std::free(h->ld.rom);
    std::free(h->ld.state);
    delete h;
}

} // extern "C"
