// C shim over libsidplayfp (GPL-2.0+), which is C++-only, so src/sid.rs can drive it through a
// plain FFI, the same shape as the AdPlug shim. It wires up the reSIDfp engine, loads a SID tune,
// and renders interleaved f32 stereo. SID is a mono chip, so we render MONO and duplicate to both
// channels (the AdLib lesson). ROM-less by design: no Commodore KERNAL/BASIC/CHARGEN ROMs are set,
// which plays most PSID and the majority of RSID tunes and keeps us shipping zero copyrighted ROM
// bytes. Compiled by build.rs via the `cc` crate only when the `sid` feature is on.

#include <sidplayfp/sidplayfp.h>
#include <sidplayfp/SidTune.h>
#include <sidplayfp/SidTuneInfo.h>
#include <sidplayfp/SidConfig.h>
#include <sidplayfp/SidInfo.h>
#include <sidplayfp/builders/residfp.h>

#include <new>

struct SidHandle {
    sidplayfp *engine;
    SidTune *tune;
    ReSIDfpBuilder *builder;
};

extern "C" {

// Open a SID file and prepare its default subsong with the reSIDfp engine, ROM-less. Returns null
// if the file is not a valid SID tune or the engine cannot be configured (so the Rust side treats
// it as skip-unplayable). `rate` is the output sample rate (Hz), rendered MONO.
SidHandle *sid_open(const char *path, int rate) {
    SidTune *tune = new (std::nothrow) SidTune(path);
    if (!tune || !tune->getStatus()) {
        delete tune;
        return nullptr;
    }
    tune->selectSong(0); // 0 selects the tune's own default start song

    sidplayfp *engine = new (std::nothrow) sidplayfp();
    if (!engine) {
        delete tune;
        return nullptr;
    }
    ReSIDfpBuilder *builder = new (std::nothrow) ReSIDfpBuilder("residfp");
    if (!builder) {
        delete engine;
        delete tune;
        return nullptr;
    }
    builder->create(engine->info().maxsids());
    if (!builder->getStatus()) {
        delete builder;
        delete engine;
        delete tune;
        return nullptr;
    }

    SidConfig cfg = engine->config();
    cfg.frequency = (unsigned int)rate;
    cfg.playback = SidConfig::MONO;
    cfg.sidEmulation = builder;
    if (!engine->config(cfg) || !engine->load(tune)) {
        delete builder;
        delete engine;
        delete tune;
        return nullptr;
    }

    SidHandle *h = new (std::nothrow) SidHandle();
    if (!h) {
        delete builder;
        delete engine;
        delete tune;
        return nullptr;
    }
    h->engine = engine;
    h->tune = tune;
    h->builder = builder;
    return h;
}

int sid_subsongs(SidHandle *h) {
    return (h && h->tune) ? (int)h->tune->getInfo()->songs() : 0;
}

// Render up to `frames` interleaved-stereo f32 frames into `out` (which must hold frames*2 floats).
// libsidplayfp renders MONO; each sample is written to both channels so it plays centered. SID
// tunes loop endlessly, so this normally fills the whole request; the Rust side caps the total
// length and fades it out. Returns the frames written (fewer only if the engine stops/errors).
int sid_render(SidHandle *h, float *out, int frames) {
    if (!h) {
        return 0;
    }
    const int CHUNK = 4096;
    short tmp[CHUNK];
    int written = 0;
    while (written < frames) {
        int n = frames - written;
        if (n > CHUNK) {
            n = CHUNK;
        }
        unsigned int got = h->engine->play(tmp, (unsigned int)n); // MONO: n shorts
        if (got == 0) {
            break; // engine stopped or errored
        }
        float *dst = out + (size_t)written * 2;
        for (unsigned int i = 0; i < got; i++) {
            float s = (float)tmp[i] / 32768.0f;
            dst[i * 2] = s;
            dst[i * 2 + 1] = s;
        }
        written += (int)got;
        if (got < (unsigned int)n) {
            break;
        }
    }
    return written;
}

void sid_close(SidHandle *h) {
    if (!h) {
        return;
    }
    delete h->engine; // engine uses the builder + tune, so free it first
    delete h->builder;
    delete h->tune;
    delete h;
}

} // extern "C"
