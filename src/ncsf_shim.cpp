/* C++ shim over SSEQPlayer (WTFPL) + psflib (MIT) for the `ncsf` feature: Nintendo DS NCSF music
 * (src/ncsf.rs). SSEQPlayer is a Nitro SDAT/SSEQ soundbank sequencer (not an emulator), so it is
 * ROM-less - no Nintendo DS BIOS or firmware is involved; the SDAT (sequences + instrument banks +
 * samples) lives inside the .ncsf/.minincsf file, parsed by the shared psflib (which resolves `_lib`
 * chains). Ported from the Kodi audiodecoder.ncsf reference (src/NCSFCodec.cpp): psf_load builds the
 * SDAT blob + the sseq index, then SDAT() parses it and Player renders it. Output is 44.1 kHz s16
 * stereo. Exposes a C API, like src/sid_shim.cpp. */
#include <cstdint>
#include <cstdio>
#include <cstring>
#include <memory>
#include <vector>

#include "psflib.h"

#include "SSEQPlayer/Player.h"
#include "SSEQPlayer/SDAT.h"
#include "SSEQPlayer/common.h"
#include "SSEQPlayer/convert.h"

namespace {

uint32_t get_le32(const void* p) {
    const unsigned char* b = static_cast<const unsigned char*>(p);
    return static_cast<uint32_t>(b[0]) | (static_cast<uint32_t>(b[1]) << 8) |
           (static_cast<uint32_t>(b[2]) << 16) | (static_cast<uint32_t>(b[3]) << 24);
}

/* stdio file callbacks for psflib */
void* ncsf_fopen(void*, const char* path) { return std::fopen(path, "rb"); }
size_t ncsf_fread(void* buf, size_t sz, size_t n, void* h) { return std::fread(buf, sz, n, static_cast<FILE*>(h)); }
int ncsf_fseek(void* h, int64_t off, int whence) { return std::fseek(static_cast<FILE*>(h), static_cast<long>(off), whence); }
int ncsf_fclose(void* h) { return std::fclose(static_cast<FILE*>(h)); }
long ncsf_ftell(void* h) { return std::ftell(static_cast<FILE*>(h)); }
const psf_file_callbacks NCSF_IO = {
    "\\/", nullptr, ncsf_fopen, ncsf_fread, ncsf_fseek, ncsf_fclose, ncsf_ftell,
};

/* psf_load callback: the reserved section holds the sseq index; the exe section holds the SDAT blob
 * (its real size at offset 8). (NCSFCodec.cpp::NCSFLoader) */
struct loader_state {
    uint32_t sseq = 0;
    std::vector<uint8_t> sdat;
};

int ncsf_load(void* context, const uint8_t* exe, size_t exe_size,
              const uint8_t* reserved, size_t reserved_size) {
    loader_state* st = static_cast<loader_state*>(context);
    if (reserved_size >= 4)
        st->sseq = get_le32(reserved);
    if (exe_size >= 12) {
        uint32_t sdat_size = get_le32(exe + 8);
        if (sdat_size > exe_size)
            return -1;
        if (st->sdat.empty())
            st->sdat.resize(sdat_size, 0);
        else if (st->sdat.size() < sdat_size)
            st->sdat.resize(sdat_size);
        std::memcpy(st->sdat.data(), exe, sdat_size);
    }
    return 0;
}

struct ncsf_handle {
    std::vector<uint8_t> sdatData;
    std::unique_ptr<SDAT> sdat;
    const SSEQ* sseq = nullptr;
    Player player;
    std::vector<uint8_t> scratch;
};

/* Point the Player at the parsed SSEQ and (re)start it from the top. */
void setup_player(ncsf_handle* h) {
    h->player.sampleRate = 44100;
    h->player.interpolation = INTERPOLATION_SINC;
    h->player.sseqVol = Cnv_Scale(h->sseq->info.vol);
    h->player.Setup(h->sseq);
    h->player.Timer();
}

} // namespace

extern "C" {

/* Reject anything not a PSF-family file of version 0x25 (NCSF). Returns 1 for NCSF, else 0. */
int ncsf_shim_is_ncsf(const char* path) {
    unsigned char hd[4];
    FILE* f = std::fopen(path, "rb");
    if (!f)
        return 0;
    size_t got = std::fread(hd, 1, 4, f);
    std::fclose(f);
    return got == 4 && hd[0] == 'P' && hd[1] == 'S' && hd[2] == 'F' && hd[3] == 0x25;
}

void* ncsf_shim_open(const char* path) {
    if (!ncsf_shim_is_ncsf(path))
        return nullptr;
    loader_state st;
    if (psf_load(path, &NCSF_IO, 0x25, ncsf_load, &st, nullptr, nullptr, 0, nullptr, nullptr) < 0 ||
        st.sdat.empty())
        return nullptr;

    ncsf_handle* h = new (std::nothrow) ncsf_handle();
    if (!h)
        return nullptr;
    h->sdatData = std::move(st.sdat);
    PseudoFile file;
    file.data = &h->sdatData;
    try {
        h->sdat.reset(new SDAT(file, st.sseq));
    } catch (...) {
        delete h;
        return nullptr;
    }
    h->sseq = h->sdat->sseq.get();
    if (!h->sseq) {
        delete h;
        return nullptr;
    }
    setup_player(h);
    return h;
}

void ncsf_shim_reset(void* handle) {
    ncsf_handle* h = static_cast<ncsf_handle*>(handle);
    if (!h)
        return;
    h->player.Stop(true);
    setup_player(h);
}

/* Render up to `frames` interleaved stereo s16 frames into buf. Returns frames produced. */
int ncsf_shim_render(void* handle, int16_t* buf, int frames) {
    ncsf_handle* h = static_cast<ncsf_handle*>(handle);
    if (!h || frames <= 0)
        return 0;
    size_t bytes = static_cast<size_t>(frames) * 4; // 2 channels * s16
    if (h->scratch.size() < bytes)
        h->scratch.resize(bytes);
    h->player.GenerateSamples(h->scratch, 0, static_cast<unsigned>(frames));
    std::memcpy(buf, h->scratch.data(), bytes);
    return frames;
}

void ncsf_shim_close(void* handle) {
    delete static_cast<ncsf_handle*>(handle);
}

} // extern "C"
