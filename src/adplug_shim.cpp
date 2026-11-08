// C shim over AdPlug (LGPL-2.1+), which is C++-only and has no C API. It exposes a handful of
// `extern "C"` functions so src/adplug.rs can drive it through a plain FFI, the same shape as the
// libgme/libopenmpt side-decoders. AdPlug's replayer does not emit PCM itself: a CPlayer writes
// OPL register events into a Copl emulator at the song's own refresh rate, and the host pulls PCM
// from the emulator. This shim runs that update()/getrefresh()/emulate loop and returns interleaved
// f32 stereo, so the Rust side just appends samples into a buffer and wraps it as a Source.
//
// Compiled by build.rs via the `cc` crate only when the `adplug` feature is on.

#include <adplug/adplug.h>
#include <adplug/emuopl.h>
#include <adplug/player.h>

#include <string>
#include <new>

namespace {
constexpr int TMP_FRAMES = 2048; // scratch frames per emulator render call
}

struct AdplugHandle {
    CEmuopl *opl;
    CPlayer *player;
    double toadd;   // fractional frames owed before the next replay tick
    bool playing;
    int rate;
};

extern "C" {

// Open a file and prepare its default subsong. Returns null if AdPlug recognizes no format in it
// (so the Rust side can treat it as "not an AdPlug file" / skip-unplayable). Stereo, 16-bit.
AdplugHandle *adplug_open(const char *path, int rate) {
    CEmuopl *opl = new (std::nothrow) CEmuopl(rate, true, true);
    if (!opl) {
        return nullptr;
    }
    opl->init();
    CPlayer *player = CAdPlug::factory(std::string(path), opl);
    if (!player) {
        delete opl;
        return nullptr;
    }
    AdplugHandle *h = new (std::nothrow) AdplugHandle();
    if (!h) {
        delete player;
        delete opl;
        return nullptr;
    }
    h->opl = opl;
    h->player = player;
    h->toadd = 0.0;
    h->playing = true;
    h->rate = rate;
    h->player->rewind(-1); // default subsong
    return h;
}

// Length of the default subsong in milliseconds (AdPlug simulates the song to measure it).
long adplug_length_ms(AdplugHandle *h) {
    return h ? (long)h->player->songlength(-1) : 0;
}

int adplug_subsongs(AdplugHandle *h) {
    return h ? (int)h->player->getsubsongs() : 0;
}

// Render up to `frames` interleaved-stereo f32 frames into `out` (which must hold frames*2 floats).
// Returns the number of frames written; fewer than requested means the song ended (one pass, no
// loop), matching the play-once behaviour of the gme/openmpt decoders.
int adplug_render(AdplugHandle *h, float *out, int frames) {
    if (!h) {
        return 0;
    }
    short tmp[TMP_FRAMES * 2];
    int written = 0;
    while (written < frames && h->playing) {
        if (h->toadd < 1.0) {
            h->playing = h->player->update(); // one replay tick; false at end of song
            float refresh = h->player->getrefresh();
            if (refresh <= 0.0f) {
                refresh = 50.0f; // guard against a bad rate
            }
            h->toadd += (double)h->rate / (double)refresh;
            if (!h->playing) {
                break;
            }
        }
        int n = frames - written;
        if (n > (int)h->toadd) {
            n = (int)h->toadd;
        }
        if (n > TMP_FRAMES) {
            n = TMP_FRAMES;
        }
        if (n < 1) {
            n = 1;
        }
        h->opl->update(tmp, n); // n frames, stereo -> 2n shorts
        float *dst = out + (size_t)written * 2;
        for (int i = 0; i < n * 2; i++) {
            dst[i] = (float)tmp[i] / 32768.0f;
        }
        written += n;
        h->toadd -= n;
    }
    return written;
}

void adplug_close(AdplugHandle *h) {
    if (!h) {
        return;
    }
    delete h->player;
    delete h->opl;
    delete h;
}

} // extern "C"
