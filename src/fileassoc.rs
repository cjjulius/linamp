//! Register LinAmp as a candidate handler for the audio file types it plays
//! (#166), so it shows up in "Open with" and the system Default Apps, and a file
//! launched with it opens and plays. Everything is written per-user, with no
//! admin rights, to match the installer's per-user model.
//!
//! The extension list is `crate::playlist::SUPPORTED_EXTS`, so associations track
//! exactly what the player can decode. Registering is a candidate registration:
//! it never silently seizes the user's default (since Windows 8 the default is
//! hash-protected and can only be set by the user), it only makes LinAmp
//! available to choose.
//!
//! Windows writes under `HKCU\Software\Classes` (a ProgID, per-extension
//! OpenWithProgids, an Applications entry, and a RegisteredApplications
//! capability block). Linux installs a `~/.local/share/applications` desktop
//! entry listing the audio MIME types and refreshes the desktop database, and
//! teaches the per-user MIME database the tracker types that standard
//! shared-mime-info omits. Other targets do nothing.

/// Register LinAmp as a handler for its audio types. Idempotent and best-effort:
/// any failure is ignored so a registry/permissions hiccup never blocks startup.
/// Called once per launch, which also keeps the registered path current if the
/// executable moves.
pub fn register(icon_dark_mode: bool) {
    #[cfg(windows)]
    {
        // The Windows taskbar icon is set live via `WM_SETICON`, not from an
        // installed file, so the colour does not affect the registry writes.
        let _ = icon_dark_mode;
        windows_impl::register();
    }
    #[cfg(target_os = "linux")]
    linux_impl::register(icon_dark_mode);
    #[cfg(not(any(windows, target_os = "linux")))]
    let _ = icon_dark_mode;
}

/// Remove the registrations `register` created. Not called in normal operation
/// (kept for an uninstall step or a future Preferences toggle).
#[allow(dead_code)]
pub fn unregister() {
    #[cfg(windows)]
    windows_impl::unregister();
    #[cfg(target_os = "linux")]
    linux_impl::unregister();
}

/// The MIME type for a supported audio extension, or None for one we do not map.
/// Several extensions share a type (all the MPEG audio layers are audio/mpeg), so
/// the Linux desktop entry lists each type once. Kept platform-agnostic (and built
/// under test) so it is unit-tested everywhere, though only the Linux backend uses
/// it at runtime.
#[cfg(any(target_os = "linux", test))]
fn mime_for_ext(ext: &str) -> Option<&'static str> {
    Some(match ext {
        "mp3" | "mp1" | "mp2" | "mpa" => "audio/mpeg",
        "flac" => "audio/flac",
        "ogg" | "oga" => "audio/ogg",
        "opus" => "audio/opus",
        "wav" => "audio/x-wav",
        "m4a" | "m4b" | "mp4" => "audio/mp4",
        "aac" => "audio/aac",
        "aiff" | "aif" => "audio/x-aiff",
        "ape" => "audio/x-ape",
        "wv" => "audio/x-wavpack",
        "tta" => "audio/x-tta",
        "mpc" | "mp+" | "mpp" => "audio/x-musepack",
        "dsf" => "audio/x-dsf",
        "dff" => "audio/x-dff",
        "spx" => "audio/speex",
        "amr" => "audio/amr",
        "awb" => "audio/amr-wb",
        "ts" | "m2ts" | "mts" => "video/mp2t",
        "avi" => "video/x-msvideo",
        "flv" => "video/x-flv",
        "mpg" | "mpeg" | "m2p" => "video/mpeg",
        "vob" => "video/dvd",
        "ac3" => "audio/ac3",
        "eac3" | "ec3" => "audio/eac3",
        // Windows Media Audio, only in the `--features ffmpeg` build (so this arm
        // matches exactly when "wma" is in SUPPORTED_EXTS).
        #[cfg(feature = "ffmpeg")]
        "wma" => "audio/x-ms-wma",
        #[cfg(feature = "ffmpeg")]
        "dts" => "audio/vnd.dts",
        // Dolby TrueHD / MLP (raw streams), also ffmpeg-only.
        #[cfg(feature = "ffmpeg")]
        "thd" | "mlp" => "audio/vnd.dolby.mlp",
        // RealAudio / RealMedia, also ffmpeg-only. The freedesktop canonical types
        // (FFmpeg's own audio/x-pn-realaudio is a legacy alias of audio/vnd.rn-realaudio);
        // mime_package_xml declares these same strings so typing and handler agree.
        #[cfg(feature = "ffmpeg")]
        "ra" => "audio/vnd.rn-realaudio",
        #[cfg(feature = "ffmpeg")]
        "rm" => "application/vnd.rn-realmedia",
        // TAK (Tom's lossless Audio Kompressor), also ffmpeg-only.
        #[cfg(feature = "ffmpeg")]
        "tak" => "audio/x-tak",
        // Sony ATRAC, also ffmpeg-only: OpenMG containers, raw ATRAC3 in RIFF, MiniDisc.
        #[cfg(feature = "ffmpeg")]
        "oma" | "omg" | "aa3" => "audio/x-oma",
        #[cfg(feature = "ffmpeg")]
        "at3" => "audio/x-atrac3",
        #[cfg(feature = "ffmpeg")]
        "aea" => "audio/x-aea",
        // Older/retro formats FFmpeg decodes natively, also ffmpeg-only.
        #[cfg(feature = "ffmpeg")]
        "shn" => "audio/x-shorten",
        #[cfg(feature = "ffmpeg")]
        "8svx" => "audio/x-8svx",
        #[cfg(feature = "ffmpeg")]
        "adx" => "audio/x-adx",
        #[cfg(feature = "ffmpeg")]
        "voc" => "audio/x-voc",
        #[cfg(feature = "ffmpeg")]
        "au" => "audio/basic",
        #[cfg(feature = "ffmpeg")]
        "w64" => "audio/x-w64",
        // Chiptune / console music via libgme (the `gme` feature).
        #[cfg(feature = "gme")]
        "nsf" | "nsfe" => "audio/x-nsf",
        #[cfg(feature = "gme")]
        "gbs" => "audio/x-gbs",
        #[cfg(feature = "gme")]
        "vgm" | "vgz" => "audio/x-vgm",
        #[cfg(feature = "gme")]
        "spc" => "audio/x-spc",
        #[cfg(feature = "gme")]
        "ay" => "audio/x-ay",
        #[cfg(feature = "gme")]
        "sap" => "audio/x-sap",
        #[cfg(feature = "gme")]
        "kss" => "audio/x-kss",
        #[cfg(feature = "gme")]
        "hes" => "audio/x-hes",
        #[cfg(feature = "gme")]
        "gym" => "audio/x-gym",
        // The tracker "zoo" via libopenmpt (the `openmpt` feature): one shared module type for
        // all ~30 formats, since one decoder handles them. Keep in step with OPENMPT_EXTS.
        #[cfg(feature = "openmpt")]
        "mptm" | "669" | "amf" | "ams" | "c67" | "dbm" | "digi" | "dmf" | "dsm" | "dtm"
        | "far" | "gdm" | "imf" | "j2b" | "mdl" | "med" | "mt2" | "mtm" | "okt" | "plm"
        | "psm" | "ptm" | "stm" | "stp" | "stx" | "symmod" | "ult" | "umx" => {
            "audio/x-openmpt-module"
        }
        // AdLib / OPL formats via AdPlug (the `adplug` feature): one shared type for all ~40.
        #[cfg(feature = "adplug")]
        "a2m" | "adl" | "agd" | "amd" | "bam" | "bmf" | "cff" | "cmf" | "d00" | "dfm"
        | "dmo" | "dro" | "got" | "ha2" | "hsc" | "hsp" | "hsq" | "ims" | "jbm"
        | "laa" | "lds" | "mad" | "mdi" | "mkj" | "msc" | "mus" | "rad" | "rix"
        | "sa2" | "sat" | "sdb" | "sng" | "sop" | "sqx" | "wlf" | "xad" | "xsm" => {
            "audio/x-adlib"
        }
        // Commodore 64 SID music via libsidplayfp (the `sid` feature); `audio/prs.sid` is the
        // IANA-registered type.
        #[cfg(feature = "sid")]
        "sid" | "psid" | "rsid" => "audio/prs.sid",
        // Streamed game-engine audio via libvgmstream (the `vgmstream` feature). These game formats
        // have no registered MIME types; use a shared vendor type taught to the DB by MIME_PACKAGE_XML.
        #[cfg(feature = "vgmstream")]
        "genh" | "dsp" | "idsp" | "vag" | "svag" | "hca" | "brstm" | "bcstm" | "bfstm" | "bfwav"
        | "bwav" | "at9" | "xvag" | "fsb" | "xwb" | "hps" | "ktss" | "wem" | "xma" => "audio/x-vgmstream",
        // PlayStation PSF1/PSF2 music via libupse (the `psf` feature). No registered MIME type;
        // use a vendor type taught to the DB by MIME_PACKAGE_XML.
        #[cfg(feature = "psf")]
        "psf" | "minipsf" => "audio/x-psf",
        // Sega Saturn SSF / Dreamcast DSF music via Highly_Theoretical (the `ssf` feature). `.dsf`
        // keeps its DSD mapping above; SSF claims it only by content, so it is not remapped here.
        #[cfg(feature = "ssf")]
        "ssf" | "minissf" | "minidsf" | "dcsf" | "minidcsf" => "audio/x-ssf",
        // Capcom QSound QSF arcade music via Highly_Quixotic (the `qsf` feature).
        #[cfg(feature = "qsf")]
        "qsf" | "miniqsf" => "audio/x-qsf",
        // Nintendo DS NCSF music via SSEQPlayer (the `ncsf` feature).
        #[cfg(feature = "ncsf")]
        "ncsf" | "minincsf" => "audio/x-ncsf",
        // Nintendo 64 USF music via lazyusf2 (the `usf` feature).
        #[cfg(feature = "usf")]
        "usf" | "miniusf" => "audio/x-usf",
        // Nintendo DS 2SF music via vio2sf (the `twosf` feature).
        #[cfg(feature = "twosf")]
        "2sf" | "mini2sf" => "audio/x-2sf",
        // Game Boy Advance GSF music via libgsf (the `gsf` feature).
        #[cfg(feature = "gsf")]
        "gsf" | "minigsf" => "audio/x-gsf",
        // QOA (Quite OK Audio) via qoaudio (the `qoa` feature).
        #[cfg(feature = "qoa")]
        "qoa" => "audio/x-qoa",
        // Video containers opened for their audio track (#180), plus CAF.
        "mov" => "video/quicktime",
        "m4v" => "video/x-m4v",
        "3gp" => "video/3gpp",
        "mkv" => "video/x-matroska",
        "webm" => "video/webm",
        "ogv" => "video/ogg",
        "caf" => "audio/x-caf",
        "mid" | "midi" => "audio/midi",
        // Tracker modules, taught to the MIME database by MIME_PACKAGE_XML.
        "mod" => "audio/x-mod",
        "xm" => "audio/x-xm",
        "s3m" => "audio/x-s3m",
        "it" => "audio/x-it",
        _ => return None,
    })
}

/// The distinct MIME types across all supported extensions, in first-seen order,
/// so the list tracks exactly what the player decodes.
#[cfg(any(target_os = "linux", test))]
fn supported_mime_types() -> Vec<&'static str> {
    let mut out: Vec<&'static str> = Vec::new();
    for ext in crate::playlist::SUPPORTED_EXTS {
        if let Some(m) = mime_for_ext(ext) {
            if !out.contains(&m) {
                out.push(m);
            }
        }
    }
    out
}

/// A small shared-mime-info package (installed under `~/.local/share/mime`) that
/// teaches the file manager the audio types standard shared-mime-info commonly
/// omits or types differently, so a `.mod`/`.dsf`/`.spx`/`.thd`/... file gets the
/// exact type the `.desktop` entry lists instead of a generic one, and LinAmp is
/// offered for it. Declared with LinAmp's own MIME strings so the typing and the
/// handler claim always agree. Common, well-known types (MP3, FLAC, Ogg, WAV, WMA,
/// the video containers) are deliberately left to shared-mime-info so this never
/// overrides the user's existing mappings for everyday formats. The FFmpeg-only
/// types are gated, so a pure-Rust build does not teach a type it cannot play.
#[cfg(any(target_os = "linux", test))]
fn mime_package_xml() -> String {
    let mut types: Vec<(&str, &str, &[&str])> = vec![
        ("audio/x-mod", "Amiga SoundTracker module", &["*.mod"]),
        ("audio/x-xm", "FastTracker 2 module", &["*.xm"]),
        ("audio/x-s3m", "ScreamTracker 3 module", &["*.s3m"]),
        ("audio/x-it", "Impulse Tracker module", &["*.it"]),
        ("audio/x-dsf", "DSD Stream File", &["*.dsf"]),
        ("audio/x-dff", "DSDIFF audio", &["*.dff"]),
        ("audio/speex", "Speex audio", &["*.spx"]),
        ("audio/amr", "AMR narrowband audio", &["*.amr"]),
        ("audio/amr-wb", "AMR wideband audio", &["*.awb"]),
    ];
    #[cfg(feature = "ffmpeg")]
    {
        types.push(("audio/vnd.dts", "DTS audio", &["*.dts"]));
        types.push((
            "audio/vnd.dolby.mlp",
            "Dolby TrueHD / MLP audio",
            &["*.thd", "*.mlp"],
        ));
        types.push(("audio/vnd.rn-realaudio", "RealAudio", &["*.ra"]));
        types.push((
            "application/vnd.rn-realmedia",
            "RealMedia",
            &["*.rm"],
        ));
        types.push(("audio/x-tak", "TAK (Tom's lossless Audio Kompressor)", &["*.tak"]));
        types.push(("audio/x-oma", "Sony OpenMG / ATRAC audio", &["*.oma", "*.omg", "*.aa3"]));
        types.push(("audio/x-atrac3", "ATRAC3 audio", &["*.at3"]));
        types.push(("audio/x-aea", "ATRAC1 (MiniDisc) audio", &["*.aea"]));
        types.push(("audio/x-shorten", "Shorten lossless audio", &["*.shn"]));
        types.push(("audio/x-8svx", "Amiga 8SVX audio", &["*.8svx"]));
        types.push(("audio/x-adx", "CRI ADX audio", &["*.adx"]));
        types.push(("audio/x-voc", "Creative Voice audio", &["*.voc"]));
        types.push(("audio/basic", "Sun/NeXT audio", &["*.au", "*.snd"]));
        types.push(("audio/x-w64", "Sony Wave64 audio", &["*.w64"]));
    }
    #[cfg(feature = "gme")]
    {
        types.push(("audio/x-nsf", "NES Sound Format", &["*.nsf", "*.nsfe"]));
        types.push(("audio/x-gbs", "Game Boy Sound", &["*.gbs"]));
        types.push(("audio/x-vgm", "Video Game Music", &["*.vgm", "*.vgz"]));
        types.push(("audio/x-spc", "SNES SPC700 audio", &["*.spc"]));
        types.push(("audio/x-ay", "AY (ZX Spectrum / Amstrad) audio", &["*.ay"]));
        types.push(("audio/x-sap", "Atari SAP audio", &["*.sap"]));
        types.push(("audio/x-kss", "MSX KSS audio", &["*.kss"]));
        types.push(("audio/x-hes", "PC Engine HES audio", &["*.hes"]));
        types.push(("audio/x-gym", "Genesis GYM audio", &["*.gym"]));
    }
    #[cfg(feature = "openmpt")]
    {
        types.push((
            "audio/x-openmpt-module",
            "Tracker module (libopenmpt)",
            &[
                "*.mptm", "*.669", "*.amf", "*.ams", "*.c67", "*.dbm", "*.digi", "*.dmf",
                "*.dsm", "*.dtm", "*.far", "*.gdm", "*.imf", "*.j2b", "*.mdl", "*.med",
                "*.mt2", "*.mtm", "*.okt", "*.plm", "*.psm", "*.ptm", "*.stm", "*.stp",
                "*.stx", "*.symmod", "*.ult", "*.umx",
            ],
        ));
    }
    #[cfg(feature = "sid")]
    {
        types.push((
            "audio/prs.sid",
            "Commodore 64 SID music",
            &["*.sid", "*.psid", "*.rsid"],
        ));
    }
    #[cfg(feature = "vgmstream")]
    {
        types.push((
            "audio/x-vgmstream",
            "Game-engine stream audio (vgmstream)",
            &[
                "*.genh", "*.dsp", "*.idsp", "*.vag", "*.svag", "*.hca", "*.brstm", "*.bcstm",
                "*.bfstm", "*.bfwav", "*.bwav", "*.at9", "*.xvag", "*.fsb", "*.xwb", "*.hps",
                "*.ktss", "*.wem", "*.xma",
            ],
        ));
    }
    #[cfg(feature = "psf")]
    {
        types.push((
            "audio/x-psf",
            "PlayStation PSF music",
            &["*.psf", "*.minipsf"],
        ));
    }
    #[cfg(feature = "ssf")]
    {
        types.push((
            "audio/x-ssf",
            "Sega Saturn/Dreamcast SSF music",
            &["*.ssf", "*.minissf", "*.minidsf", "*.dcsf", "*.minidcsf"],
        ));
    }
    #[cfg(feature = "qsf")]
    {
        types.push((
            "audio/x-qsf",
            "Capcom QSound QSF music",
            &["*.qsf", "*.miniqsf"],
        ));
    }
    #[cfg(feature = "ncsf")]
    {
        types.push((
            "audio/x-ncsf",
            "Nintendo DS NCSF music",
            &["*.ncsf", "*.minincsf"],
        ));
    }
    #[cfg(feature = "usf")]
    {
        types.push((
            "audio/x-usf",
            "Nintendo 64 USF music",
            &["*.usf", "*.miniusf"],
        ));
    }
    #[cfg(feature = "twosf")]
    {
        types.push((
            "audio/x-2sf",
            "Nintendo DS 2SF music",
            &["*.2sf", "*.mini2sf"],
        ));
    }
    #[cfg(feature = "gsf")]
    {
        types.push((
            "audio/x-gsf",
            "Game Boy Advance GSF music",
            &["*.gsf", "*.minigsf"],
        ));
    }
    #[cfg(feature = "qoa")]
    {
        types.push((
            "audio/x-qoa",
            "QOA (Quite OK Audio)",
            &["*.qoa"],
        ));
    }
    #[cfg(feature = "adplug")]
    {
        types.push((
            "audio/x-adlib",
            "AdLib / OPL FM music (AdPlug)",
            &[
                "*.a2m", "*.adl", "*.agd", "*.amd", "*.bam", "*.bmf", "*.cff", "*.cmf", "*.d00",
                "*.dfm", "*.dmo", "*.dro", "*.got", "*.ha2", "*.hsc", "*.hsp", "*.hsq", "*.ims",
                "*.jbm", "*.laa", "*.lds", "*.mad", "*.mdi", "*.mkj", "*.msc", "*.mus",
                "*.rad", "*.rix", "*.sa2", "*.sat", "*.sdb", "*.sng", "*.sop",
                "*.sqx", "*.wlf", "*.xad", "*.xsm",
            ],
        ));
    }
    let mut body = String::new();
    for (ty, comment, globs) in &types {
        body.push_str(&format!("  <mime-type type=\"{ty}\">\n"));
        body.push_str(&format!("    <comment>{comment}</comment>\n"));
        for g in *globs {
            body.push_str(&format!("    <glob pattern=\"{g}\"/>\n"));
        }
        body.push_str("  </mime-type>\n");
    }
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <mime-info xmlns=\"http://www.freedesktop.org/standards/shared-mime-info\">\n\
         {body}</mime-info>\n"
    )
}

#[cfg(windows)]
mod windows_impl {
    use crate::playlist::SUPPORTED_EXTS;

    // Predefined HKEY_CURRENT_USER: the LONG 0x80000001 sign-extended to pointer
    // width, matching the Win32 ((HKEY)(ULONG_PTR)(LONG)0x80000001) definition.
    const HKEY_CURRENT_USER: isize = 0x8000_0001u32 as i32 as isize;
    const KEY_WRITE: u32 = 0x2_0006;
    const REG_SZ: u32 = 1;

    // SHChangeNotify: tell the shell file associations changed so Explorer and the
    // Open-with list refresh without a sign-out.
    const SHCNE_ASSOCCHANGED: i32 = 0x0800_0000;
    const SHCNF_IDLIST: u32 = 0x0000;

    // The ProgID LinAmp registers its audio class under, and the Applications
    // subkey (by exe name) that backs the "Open with > LinAmp" entry.
    const PROGID: &str = "LinAmp.Audio";
    const APP_EXE: &str = "linamp.exe";
    // Where the RegisteredApplications entry points for the Default Apps block.
    const CAPABILITIES_KEY: &str = "Software\\LinAmp\\Capabilities";

    // Bump when the set of registry keys written by `register` changes, so an
    // existing install re-registers under the new layout.
    const SCHEMA_VERSION: u32 = 1;

    /// Path of the stamp file recording the last registration signature, so a
    /// launch that would write the same thing can skip the registry writes.
    fn stamp_path() -> std::path::PathBuf {
        dirs::data_local_dir()
            .unwrap_or_else(|| std::path::PathBuf::from("."))
            .join("LinAmp")
            .join("fileassoc.stamp")
    }

    #[link(name = "advapi32")]
    extern "system" {
        fn RegCreateKeyExW(
            key: isize,
            sub_key: *const u16,
            reserved: u32,
            class: *const u16,
            options: u32,
            desired: u32,
            security: *const core::ffi::c_void,
            result: *mut isize,
            disposition: *mut u32,
        ) -> i32;
        fn RegSetValueExW(
            key: isize,
            value_name: *const u16,
            reserved: u32,
            ty: u32,
            data: *const u8,
            data_len: u32,
        ) -> i32;
        fn RegCloseKey(key: isize) -> i32;
        // Used only by `unregister` (an uninstall helper), so unused in a normal build.
        #[allow(dead_code)]
        fn RegDeleteTreeW(key: isize, sub_key: *const u16) -> i32;
    }

    #[link(name = "shell32")]
    extern "system" {
        fn SHChangeNotify(event: i32, flags: u32, item1: *const core::ffi::c_void, item2: *const core::ffi::c_void);
    }

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    /// Create `root\subkey` and set one value in it to the string `value`. `name`
    /// is the value name, or `None` for the key's default value. Returns whether
    /// the write succeeded; callers collect failures but press on so a single bad
    /// key can't abort the whole registration.
    unsafe fn set_value(root: isize, subkey: &str, name: Option<&str>, value: &str) -> bool {
        let sub = wide(subkey);
        let mut hkey: isize = 0;
        if RegCreateKeyExW(
            root,
            sub.as_ptr(),
            0,
            std::ptr::null(),
            0,
            KEY_WRITE,
            std::ptr::null(),
            &mut hkey,
            std::ptr::null_mut(),
        ) != 0
        {
            return false;
        }
        let data = wide(value); // REG_SZ data includes the trailing null.
        let name_w = name.map(wide);
        let name_ptr = name_w.as_ref().map_or(std::ptr::null(), |v| v.as_ptr());
        let ok = RegSetValueExW(
            hkey,
            name_ptr,
            0,
            REG_SZ,
            data.as_ptr() as *const u8,
            (data.len() * 2) as u32,
        ) == 0;
        RegCloseKey(hkey);
        ok
    }

    pub(super) fn register() {
        let Ok(exe) = std::env::current_exe() else {
            return;
        };
        let exe = exe.to_string_lossy().into_owned();
        // Skip the registry writes when nothing relevant changed since the last
        // launch. The signature covers the schema version, the exe path and the
        // extension list, so a moved executable or a new supported type still
        // re-registers.
        let signature = format!("{SCHEMA_VERSION}|{exe}|{}", SUPPORTED_EXTS.join(","));
        if std::fs::read_to_string(stamp_path()).ok().as_deref() == Some(signature.as_str()) {
            return;
        }
        let open_command = format!("\"{exe}\" \"%1\"");
        let icon = format!("{exe},0");
        let classes = "Software\\Classes";

        // Track whether every write succeeded, so a failed or partial registration
        // (a locked hive, a permissions hiccup) is not stamped as done, which would
        // defeat the self-heal on the next launch.
        let mut ok = true;
        unsafe {
            // The ProgID: a friendly name, the exe's icon, and the open command
            // that passes the clicked file as the first argument (LinAmp's
            // command-line handling enqueues and plays it).
            ok &= set_value(HKEY_CURRENT_USER, &format!("{classes}\\{PROGID}"), None, "LinAmp audio file");
            ok &= set_value(HKEY_CURRENT_USER, &format!("{classes}\\{PROGID}\\DefaultIcon"), None, &icon);
            ok &= set_value(
                HKEY_CURRENT_USER,
                &format!("{classes}\\{PROGID}\\shell\\open\\command"),
                None,
                &open_command,
            );

            // The Applications\linamp.exe entry: a friendly name plus the same open
            // command, which is what backs "Open with > LinAmp".
            let app = format!("{classes}\\Applications\\{APP_EXE}");
            ok &= set_value(HKEY_CURRENT_USER, &app, Some("FriendlyAppName"), "LinAmp");
            ok &= set_value(
                HKEY_CURRENT_USER,
                &format!("{app}\\shell\\open\\command"),
                None,
                &open_command,
            );

            // The capability block the Default Apps UI reads.
            ok &= set_value(HKEY_CURRENT_USER, CAPABILITIES_KEY, Some("ApplicationName"), "LinAmp");
            ok &= set_value(
                HKEY_CURRENT_USER,
                CAPABILITIES_KEY,
                Some("ApplicationDescription"),
                "A classic-style audio player.",
            );

            // Per extension: offer LinAmp as a candidate (OpenWithProgids, never
            // the forced default), list it under the app's SupportedTypes, and map
            // the extension to our ProgID in the capability block.
            for ext in SUPPORTED_EXTS {
                let dot = format!(".{ext}");
                ok &= set_value(
                    HKEY_CURRENT_USER,
                    &format!("{classes}\\{dot}\\OpenWithProgids"),
                    Some(PROGID),
                    "",
                );
                ok &= set_value(
                    HKEY_CURRENT_USER,
                    &format!("{app}\\SupportedTypes"),
                    Some(&dot),
                    "",
                );
                ok &= set_value(
                    HKEY_CURRENT_USER,
                    &format!("{CAPABILITIES_KEY}\\FileAssociations"),
                    Some(&dot),
                    PROGID,
                );
            }

            // Point RegisteredApplications at the capability block so LinAmp lists
            // in Default Apps.
            ok &= set_value(
                HKEY_CURRENT_USER,
                "Software\\RegisteredApplications",
                Some("LinAmp"),
                CAPABILITIES_KEY,
            );

            // Nudge the shell to pick up the new associations.
            SHChangeNotify(SHCNE_ASSOCCHANGED, SHCNF_IDLIST, std::ptr::null(), std::ptr::null());
        }

        // Record what we just registered so the next launch can skip the writes,
        // but only when every write succeeded. A partial registration is left
        // unstamped so the next launch retries it.
        if ok {
            let stamp = stamp_path();
            if let Some(parent) = stamp.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let _ = std::fs::write(stamp, signature);
        }
    }

    #[allow(dead_code)]
    pub(super) fn unregister() {
        let classes = "Software\\Classes";
        unsafe {
            for key in [
                format!("{classes}\\{PROGID}"),
                format!("{classes}\\Applications\\{APP_EXE}"),
                "Software\\LinAmp".to_string(),
            ] {
                RegDeleteTreeW(HKEY_CURRENT_USER, wide(&key).as_ptr());
            }
            // Remove just our OpenWithProgids value on each extension, leaving the
            // rest of the key (other apps' entries) intact.
            for ext in SUPPORTED_EXTS {
                let sub = format!("{classes}\\.{ext}\\OpenWithProgids\\{PROGID}");
                RegDeleteTreeW(HKEY_CURRENT_USER, wide(&sub).as_ptr());
            }
            // Drop the RegisteredApplications pointer (delete the single value by
            // clearing the subtree path to it is not valid, so this is left for the
            // installer's uninstaller; a stale pointer to a missing capability key
            // is harmless and ignored by the shell).
            SHChangeNotify(SHCNE_ASSOCCHANGED, SHCNF_IDLIST, std::ptr::null(), std::ptr::null());
        }
        // Drop the stamp so a later register() rewrites instead of skipping.
        let _ = std::fs::remove_file(stamp_path());
    }
}

#[cfg(target_os = "linux")]
mod linux_impl {
    use std::path::Path;

    const DESKTOP_FILE: &str = "linamp.desktop";
    const MIME_PACKAGE: &str = "linamp.xml";

    /// Write `contents` to `path` only when it differs from what is there,
    /// creating parent directories. Returns whether the file was (re)written, so
    /// the caller refreshes the system cache only on a real change.
    fn write_if_changed(path: &Path, contents: &str) -> bool {
        write_bytes_if_changed(path, contents.as_bytes())
    }

    /// Write `contents` to `path` only when it differs, creating parent
    /// directories. Returns whether the file was (re)written.
    fn write_bytes_if_changed(path: &Path, contents: &[u8]) -> bool {
        if std::fs::read(path).ok().as_deref() == Some(contents) {
            return false;
        }
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        std::fs::write(path, contents).is_ok()
    }

    /// Best-effort run of an xdg cache-refresh command on a directory, output
    /// discarded. A missing tool or a non-zero exit is ignored, matching the
    /// installer's `|| true`.
    fn run_quiet(program: &str, dir: &Path) {
        use std::process::{Command, Stdio};
        let _ = Command::new(program)
            .arg(dir)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }

    /// Best-effort refresh of the hicolor icon cache so the desktop picks up the
    /// icon without a relogin. Missing tool or non-zero exit is ignored.
    fn refresh_icon_cache(theme_dir: &Path) {
        use std::process::{Command, Stdio};
        let _ = Command::new("gtk-update-icon-cache")
            .args(["-f", "-t"])
            .arg(theme_dir)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }

    pub(super) fn register(icon_dark_mode: bool) {
        let Some(data) = dirs::data_local_dir() else {
            return;
        };
        let Ok(exe) = std::env::current_exe() else {
            return;
        };

        // Install the icon per-user so `Icon=linamp` resolves however LinAmp was
        // launched (run from a build tree, the installer never ran). Without this
        // the "Open with" list shows a generic icon. This is also the only icon a
        // GNOME dock or the app launcher shows for the running app (they render the
        // theme icon by name, not the window's `_NET_WM_ICON`), so it has to track
        // Icon Dark Mode (#223): writing the recoloured logo here is what makes the
        // taskbar/launcher follow the toggle, live (set_icon_dark_mode re-runs this)
        // and across restarts. Refresh the icon cache only when a file changed.
        let icons = data.join("icons").join("hicolor");
        let png = crate::icon::window_png(icon_dark_mode);
        let mut icon_changed = match &png {
            Some(bytes) => write_bytes_if_changed(
                &icons.join("256x256").join("apps").join("linamp.png"),
                bytes,
            ),
            None => false,
        };
        icon_changed |= write_if_changed(
            &icons.join("scalable").join("apps").join("linamp.svg"),
            &crate::icon::logo_svg(icon_dark_mode),
        );

        // The desktop entry: a complete entry with the running executable as Exec
        // and the full MIME list, so "Open with" and the Default Apps UI offer
        // LinAmp for every type it decodes. %F passes the selected local files,
        // which the desktop translates from file:// URIs, so LinAmp's path-based
        // open logic receives plain paths. This is a candidate registration: it
        // never runs `xdg-mime default`, so the user's chosen default is left
        // alone, matching the Windows side not seizing the default. Rewriting each
        // launch also keeps Exec current if the executable moves.
        //
        // StartupNotify is off: LinAmp is single-instance, so an "Open with" on an
        // already-running LinAmp forwards the file and the launched process exits
        // without mapping a window. With startup notification on, the launcher would
        // then show a busy cursor until the notification timed out (5-10s). Off, the
        // forward is instant with no stuck cursor.
        let exe = exe.to_string_lossy();
        let mimes = super::supported_mime_types().join(";");
        let desktop = format!(
            "[Desktop Entry]\n\
             Type=Application\n\
             Name=LinAmp\n\
             GenericName=Audio Player\n\
             Comment=A classic-style audio player\n\
             Exec=\"{exe}\" %F\n\
             Icon=linamp\n\
             Terminal=false\n\
             Categories=AudioVideo;Audio;Player;\n\
             MimeType={mimes};\n\
             StartupNotify=false\n"
        );
        let app_dir = data.join("applications");
        let desktop_changed = write_if_changed(&app_dir.join(DESKTOP_FILE), &desktop);

        // Teach the per-user MIME database the tracker types that standard
        // shared-mime-info omits.
        let mime_dir = data.join("mime");
        let mime_changed = write_if_changed(
            &mime_dir.join("packages").join(MIME_PACKAGE),
            &super::mime_package_xml(),
        );

        // Run the cache refreshes off the main thread. update-mime-database
        // rebuilds the whole per-user MIME cache (1-3s on a first launch), and
        // register() runs before the window is created, so doing these inline would
        // stall startup. They only run when a file actually changed, so steady-state
        // launches spawn nothing. The GUI launch outlives the detached thread, so
        // the refreshes finish.
        if icon_changed || desktop_changed || mime_changed {
            std::thread::spawn(move || {
                if icon_changed {
                    refresh_icon_cache(&icons);
                }
                if desktop_changed {
                    run_quiet("update-desktop-database", &app_dir);
                }
                if mime_changed {
                    run_quiet("update-mime-database", &mime_dir);
                }
            });
        }
    }

    #[allow(dead_code)]
    pub(super) fn unregister() {
        let Some(data) = dirs::data_local_dir() else {
            return;
        };
        let app_dir = data.join("applications");
        let _ = std::fs::remove_file(app_dir.join(DESKTOP_FILE));
        run_quiet("update-desktop-database", &app_dir);

        let icons = data.join("icons").join("hicolor");
        let _ = std::fs::remove_file(icons.join("256x256").join("apps").join("linamp.png"));
        let _ = std::fs::remove_file(icons.join("scalable").join("apps").join("linamp.svg"));
        refresh_icon_cache(&icons);

        let mime_dir = data.join("mime");
        let _ = std::fs::remove_file(mime_dir.join("packages").join(MIME_PACKAGE));
        run_quiet("update-mime-database", &mime_dir);
    }
}

#[cfg(test)]
mod tests {
    use super::{mime_for_ext, mime_package_xml, supported_mime_types};

    /// The MIME package must teach the file manager every non-standard audio type
    /// LinAmp added (DSD, Speex, AMR, and under ffmpeg DTS/TrueHD/MLP), each with
    /// LinAmp's own MIME string, or the Linux "Open with" never offers LinAmp for
    /// those files even though the .desktop lists the type.
    #[test]
    fn mime_package_declares_the_non_standard_types() {
        let xml = mime_package_xml();
        for (ty, glob) in [
            ("audio/x-dsf", "*.dsf"),
            ("audio/x-dff", "*.dff"),
            ("audio/speex", "*.spx"),
            ("audio/amr", "*.amr"),
            ("audio/amr-wb", "*.awb"),
            ("audio/x-mod", "*.mod"),
        ] {
            assert!(xml.contains(ty), "MIME package missing type {ty}");
            assert!(xml.contains(glob), "MIME package missing glob {glob}");
        }
        #[cfg(feature = "ffmpeg")]
        for (ty, glob) in [
            ("audio/vnd.dts", "*.dts"),
            ("audio/vnd.dolby.mlp", "*.thd"),
            ("audio/vnd.dolby.mlp", "*.mlp"),
        ] {
            assert!(xml.contains(ty), "MIME package missing type {ty}");
            assert!(xml.contains(glob), "MIME package missing glob {glob}");
        }
        // Every glob the package declares must name an extension LinAmp supports, and
        // its type must match mime_for_ext, so the package can't drift from the list.
        assert!(xml.contains("<mime-info"), "not a MIME package: {xml}");
    }

    #[test]
    fn every_supported_ext_has_a_mime_type() {
        // A newly added SUPPORTED_EXTS entry with no MIME mapping would silently
        // drop out of the Linux "Open with" list; this guards against that.
        for ext in crate::playlist::SUPPORTED_EXTS {
            assert!(mime_for_ext(ext).is_some(), "no MIME type for .{ext}");
        }
    }

    #[test]
    fn mime_types_dedup_in_first_seen_order() {
        let mimes = supported_mime_types();
        // The MPEG audio layers collapse to a single audio/mpeg entry.
        assert_eq!(mimes.iter().filter(|m| **m == "audio/mpeg").count(), 1);
        // mp3 is the first supported extension, so audio/mpeg leads.
        assert_eq!(mimes.first(), Some(&"audio/mpeg"));
        // No duplicates across the whole list.
        let mut unique = mimes.clone();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(unique.len(), mimes.len());
    }
}
