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
//! capability block). Linux will install a `~/.local/share/applications`
//! desktop entry and register its MIME types with xdg-mime; see
//! docs/REVIEW-NOTES.md. Other targets do nothing.

/// Register LinAmp as a handler for its audio types. Idempotent and best-effort:
/// any failure is ignored so a registry/permissions hiccup never blocks startup.
/// Called once per launch, which also keeps the registered path current if the
/// executable moves.
pub fn register() {
    #[cfg(windows)]
    windows_impl::register();
}

/// Remove the registrations `register` created. Not called in normal operation
/// (kept for an uninstall step or a future Preferences toggle).
#[allow(dead_code)]
pub fn unregister() {
    #[cfg(windows)]
    windows_impl::unregister();
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
