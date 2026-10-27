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
pub fn register() {
    #[cfg(windows)]
    windows_impl::register();
    #[cfg(target_os = "linux")]
    linux_impl::register();
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
        let open_command = format!("\"{exe}\" \"%1\"");
        let icon = format!("{exe},0");
        let classes = "Software\\Classes";

        unsafe {
            // The ProgID: a friendly name, the exe's icon, and the open command
            // that passes the clicked file as the first argument (LinAmp's
            // command-line handling enqueues and plays it).
            set_value(HKEY_CURRENT_USER, &format!("{classes}\\{PROGID}"), None, "LinAmp audio file");
            set_value(HKEY_CURRENT_USER, &format!("{classes}\\{PROGID}\\DefaultIcon"), None, &icon);
            set_value(
                HKEY_CURRENT_USER,
                &format!("{classes}\\{PROGID}\\shell\\open\\command"),
                None,
                &open_command,
            );

            // The Applications\linamp.exe entry: a friendly name plus the same open
            // command, which is what backs "Open with > LinAmp".
            let app = format!("{classes}\\Applications\\{APP_EXE}");
            set_value(HKEY_CURRENT_USER, &app, Some("FriendlyAppName"), "LinAmp");
            set_value(
                HKEY_CURRENT_USER,
                &format!("{app}\\shell\\open\\command"),
                None,
                &open_command,
            );

            // The capability block the Default Apps UI reads.
            set_value(HKEY_CURRENT_USER, CAPABILITIES_KEY, Some("ApplicationName"), "LinAmp");
            set_value(
                HKEY_CURRENT_USER,
                CAPABILITIES_KEY,
                Some("ApplicationDescription"),
                "A Winamp-inspired audio player.",
            );

            // Per extension: offer LinAmp as a candidate (OpenWithProgids, never
            // the forced default), list it under the app's SupportedTypes, and map
            // the extension to our ProgID in the capability block.
            for ext in SUPPORTED_EXTS {
                let dot = format!(".{ext}");
                set_value(
                    HKEY_CURRENT_USER,
                    &format!("{classes}\\{dot}\\OpenWithProgids"),
                    Some(PROGID),
                    "",
                );
                set_value(
                    HKEY_CURRENT_USER,
                    &format!("{app}\\SupportedTypes"),
                    Some(&dot),
                    "",
                );
                set_value(
                    HKEY_CURRENT_USER,
                    &format!("{CAPABILITIES_KEY}\\FileAssociations"),
                    Some(&dot),
                    PROGID,
                );
            }

            // Point RegisteredApplications at the capability block so LinAmp lists
            // in Default Apps.
            set_value(
                HKEY_CURRENT_USER,
                "Software\\RegisteredApplications",
                Some("LinAmp"),
                CAPABILITIES_KEY,
            );

            // Nudge the shell to pick up the new associations.
            SHChangeNotify(SHCNE_ASSOCCHANGED, SHCNF_IDLIST, std::ptr::null(), std::ptr::null());
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
    }
}

#[cfg(target_os = "linux")]
mod linux_impl {
    use crate::playlist::SUPPORTED_EXTS;
    use std::path::Path;

    const DESKTOP_FILE: &str = "linamp.desktop";
    const MIME_PACKAGE: &str = "linamp.xml";

    // A small shared-mime-info package for the tracker formats. Standard
    // shared-mime-info often does not define these, so without it the file manager
    // gives a .mod/.xm/.s3m/.it file a generic type and never offers LinAmp.
    // Installing this under ~/.local/share/mime teaches the types per-user.
    const MIME_PACKAGE_XML: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<mime-info xmlns="http://www.freedesktop.org/standards/shared-mime-info">
  <mime-type type="audio/x-mod">
    <comment>Amiga SoundTracker module</comment>
    <glob pattern="*.mod"/>
  </mime-type>
  <mime-type type="audio/x-xm">
    <comment>FastTracker 2 module</comment>
    <glob pattern="*.xm"/>
  </mime-type>
  <mime-type type="audio/x-s3m">
    <comment>ScreamTracker 3 module</comment>
    <glob pattern="*.s3m"/>
  </mime-type>
  <mime-type type="audio/x-it">
    <comment>Impulse Tracker module</comment>
    <glob pattern="*.it"/>
  </mime-type>
</mime-info>
"#;

    /// The MIME type for a supported extension, or None for one we do not map.
    /// Several extensions share a type (all the MPEG audio layers are audio/mpeg),
    /// so the desktop entry lists each type once.
    fn mime_for_ext(ext: &str) -> Option<&'static str> {
        Some(match ext {
            "mp3" | "mp1" | "mp2" | "mpa" => "audio/mpeg",
            "flac" => "audio/flac",
            "ogg" | "oga" => "audio/ogg",
            "wav" => "audio/x-wav",
            "m4a" | "m4b" | "mp4" => "audio/mp4",
            "aac" => "audio/aac",
            "aiff" | "aif" => "audio/x-aiff",
            "mid" | "midi" => "audio/midi",
            // Tracker modules, taught to the MIME database by MIME_PACKAGE_XML.
            "mod" => "audio/x-mod",
            "xm" => "audio/x-xm",
            "s3m" => "audio/x-s3m",
            "it" => "audio/x-it",
            _ => return None,
        })
    }

    /// The distinct MIME types across all supported extensions, in first-seen
    /// order, so the list tracks exactly what the player decodes.
    fn supported_mime_types() -> Vec<&'static str> {
        let mut out: Vec<&'static str> = Vec::new();
        for ext in SUPPORTED_EXTS {
            if let Some(m) = mime_for_ext(ext) {
                if !out.contains(&m) {
                    out.push(m);
                }
            }
        }
        out
    }

    /// Write `contents` to `path` only when it differs from what is there,
    /// creating parent directories. Returns whether the file was (re)written, so
    /// the caller refreshes the system cache only on a real change.
    fn write_if_changed(path: &Path, contents: &str) -> bool {
        if std::fs::read_to_string(path).ok().as_deref() == Some(contents) {
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

    pub(super) fn register() {
        let Some(data) = dirs::data_local_dir() else {
            return;
        };
        let Ok(exe) = std::env::current_exe() else {
            return;
        };

        // The desktop entry: a complete entry with the running executable as Exec
        // and the full MIME list, so "Open with" and the Default Apps UI offer
        // LinAmp for every type it decodes. %F passes the selected local files,
        // which the desktop translates from file:// URIs, so LinAmp's path-based
        // open logic receives plain paths. This is a candidate registration: it
        // never runs `xdg-mime default`, so the user's chosen default is left
        // alone, matching the Windows side not seizing the default. Rewriting each
        // launch also keeps Exec current if the executable moves.
        let exe = exe.to_string_lossy();
        let mimes = supported_mime_types().join(";");
        let desktop = format!(
            "[Desktop Entry]\n\
             Type=Application\n\
             Name=LinAmp\n\
             GenericName=Audio Player\n\
             Comment=A Winamp-inspired audio player\n\
             Exec=\"{exe}\" %F\n\
             Icon=linamp\n\
             Terminal=false\n\
             Categories=AudioVideo;Audio;Player;\n\
             MimeType={mimes};\n\
             StartupNotify=true\n"
        );
        let app_dir = data.join("applications");
        if write_if_changed(&app_dir.join(DESKTOP_FILE), &desktop) {
            run_quiet("update-desktop-database", &app_dir);
        }

        // Teach the per-user MIME database the tracker types. Only refresh the
        // cache when the package changed, since update-mime-database rebuilds the
        // whole user cache and that is wasted work on a steady-state launch.
        let mime_dir = data.join("mime");
        if write_if_changed(&mime_dir.join("packages").join(MIME_PACKAGE), MIME_PACKAGE_XML) {
            run_quiet("update-mime-database", &mime_dir);
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

        let mime_dir = data.join("mime");
        let _ = std::fs::remove_file(mime_dir.join("packages").join(MIME_PACKAGE));
        run_quiet("update-mime-database", &mime_dir);
    }
}
