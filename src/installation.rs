//! Keep Windows Installed Apps metadata current after an in-place app update.
//! The installer owns registration; portable launches never create an entry.

use std::path::Path;
use windows::Win32::System::Registry::*;
use windows::core::{PCWSTR, w};

const UNINSTALL_KEY: PCWSTR = w!(
    "Software\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\{A30D44CA-5373-4B32-8BFA-B7B3C2560B70}_is1"
);

struct Key(HKEY);
impl Drop for Key {
    fn drop(&mut self) {
        unsafe {
            let _ = RegCloseKey(self.0);
        }
    }
}

pub fn refresh_display_version() {
    if let Ok(executable) = std::env::current_exe() {
        refresh_at_key(UNINSTALL_KEY, &executable);
    }
}

fn refresh_at_key(subkey: PCWSTR, executable: &Path) {
    unsafe {
        let mut handle = HKEY::default();
        // Opening an existing key prevents portable copies from registering
        // themselves as installed applications.
        if RegOpenKeyExW(
            HKEY_CURRENT_USER,
            subkey,
            None,
            KEY_QUERY_VALUE | KEY_SET_VALUE | KEY_WOW64_64KEY,
            &mut handle,
        )
        .is_err()
        {
            return;
        }
        let key = Key(handle);
        let Some(location) = string_value(key.0, w!("InstallLocation")) else {
            return;
        };
        let Some(parent) = executable.parent() else {
            return;
        };
        let Ok(parent) = parent.canonicalize() else {
            return;
        };
        if Path::new(&location).canonicalize().ok().as_ref() != Some(&parent) {
            return;
        }
        let version = env!("CARGO_PKG_VERSION");
        if string_value(key.0, w!("DisplayVersion")).as_deref() == Some(version) {
            return;
        }
        let value: Vec<u8> = version
            .encode_utf16()
            .chain(Some(0))
            .flat_map(u16::to_le_bytes)
            .collect();
        let _ = RegSetValueExW(key.0, w!("DisplayVersion"), None, REG_SZ, Some(&value));
    }
}

unsafe fn string_value(key: HKEY, name: PCWSTR) -> Option<String> {
    unsafe {
        let mut value = vec![0u16; 32768];
        let mut bytes = (value.len() * 2) as u32;
        RegGetValueW(
            key,
            None,
            name,
            RRF_RT_REG_SZ,
            None,
            Some(value.as_mut_ptr().cast()),
            Some(&mut bytes),
        )
        .ok()
        .ok()?;
        if bytes < 2 || bytes % 2 != 0 || bytes as usize > value.len() * 2 {
            return None;
        }
        value.truncate(bytes as usize / 2);
        let end = value.iter().position(|character| *character == 0)?;
        String::from_utf16(&value[..end]).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refresh_only_changes_existing_registration_for_this_install_folder() {
        let name = format!(
            "Software\\TokWatch-Verification-{}-{}",
            std::process::id(),
            crate::model::unix_now()
        );
        let name: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
        let subkey = PCWSTR(name.as_ptr());
        let executable = std::env::current_exe().unwrap();
        unsafe {
            refresh_at_key(subkey, &executable);
            let mut handle = HKEY::default();
            assert!(RegOpenKeyExW(HKEY_CURRENT_USER, subkey, None, KEY_READ, &mut handle).is_err());
            RegCreateKeyExW(
                HKEY_CURRENT_USER,
                subkey,
                None,
                None,
                REG_OPTION_NON_VOLATILE,
                KEY_ALL_ACCESS | KEY_WOW64_64KEY,
                None,
                &mut handle,
                None,
            )
            .ok()
            .unwrap();
            let key = Key(handle);
            let set = |name: PCWSTR, text: &str| {
                let bytes: Vec<u8> = text
                    .encode_utf16()
                    .chain(Some(0))
                    .flat_map(u16::to_le_bytes)
                    .collect();
                RegSetValueExW(key.0, name, None, REG_SZ, Some(&bytes))
                    .ok()
                    .unwrap();
            };
            set(w!("DisplayVersion"), "0.1.0");
            refresh_at_key(subkey, &executable);
            assert_eq!(
                string_value(key.0, w!("DisplayVersion")).as_deref(),
                Some("0.1.0")
            );
            set(w!("InstallLocation"), "C:\\TokWatch-Not-This-Install");
            refresh_at_key(subkey, &executable);
            assert_eq!(
                string_value(key.0, w!("DisplayVersion")).as_deref(),
                Some("0.1.0")
            );
            set(
                w!("InstallLocation"),
                executable.parent().unwrap().to_str().unwrap(),
            );
            refresh_at_key(subkey, &executable);
            assert_eq!(
                string_value(key.0, w!("DisplayVersion")).as_deref(),
                Some(env!("CARGO_PKG_VERSION"))
            );
            drop(key);
            RegDeleteKeyExW(HKEY_CURRENT_USER, subkey, KEY_WOW64_64KEY.0, None)
                .ok()
                .unwrap();
        }
    }
}
