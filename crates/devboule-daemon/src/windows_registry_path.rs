//! Reading the machine and user `Path` values from the Windows registry.
//!
//! A read answers [`RegistryPathRead`], never `Option`: the difference
//! between "the value is absent or unreadable" and "the value is empty" is
//! what the daemon's diagnostics show when machine or user providers
//! disappear, so the error travels instead of being flattened at the source.
//! An unreadable value is a lost search entry and never a discovery failure.

use std::fmt;

use windows_sys::Win32::System::Environment::ExpandEnvironmentStringsW;
use windows_sys::Win32::System::Registry::{
    RegCloseKey, RegOpenKeyExW, RegQueryValueExW, HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE,
    KEY_READ, REG_EXPAND_SZ, REG_SZ,
};

/// Why a `Path` value could not be read. `Win32` carries the OS error code
/// from the open or the query; the other two are the reader's own refusals.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum RegistryPathError {
    Win32(u32),
    EmptyValue,
    UnexpectedType(u32),
}

impl fmt::Display for RegistryPathError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Win32(code) => write!(formatter, "error {code}"),
            Self::EmptyValue => write!(formatter, "empty value"),
            Self::UnexpectedType(kind) => write!(formatter, "unexpected type {kind}"),
        }
    }
}

/// One `Path` value, expanded when it is `REG_EXPAND_SZ`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum RegistryPathRead {
    Read(String),
    Failed(RegistryPathError),
}

impl RegistryPathRead {
    /// The value, when it was read.
    pub(crate) fn value(&self) -> Option<&str> {
        match self {
            Self::Read(value) => Some(value),
            Self::Failed(_) => None,
        }
    }

    /// The one diagnostics line for this read: entries counted, or the reason
    /// it was not read. The value itself is never spelled out — folder names
    /// are not diagnostics.
    #[cfg_attr(not(feature = "server"), allow(dead_code))]
    pub(crate) fn describe(&self, label: &str) -> String {
        match self {
            Self::Read(value) => {
                let entries = std::env::split_paths(value)
                    .filter(|entry| !entry.as_os_str().is_empty())
                    .count();
                format!("{label}: read, {entries} entries")
            }
            Self::Failed(error) => format!("{label}: not read: {error}"),
        }
    }
}

const MACHINE_ENVIRONMENT_SUBKEY: &str =
    r"SYSTEM\CurrentControlSet\Control\Session Manager\Environment";
const USER_ENVIRONMENT_SUBKEY: &str = "Environment";
const PATH_VALUE_NAME: &str = "Path";

/// The machine PATH.
pub(crate) fn read_machine_path() -> RegistryPathRead {
    read_path_value(HKEY_LOCAL_MACHINE, MACHINE_ENVIRONMENT_SUBKEY)
}

/// The user PATH.
pub(crate) fn read_user_path() -> RegistryPathRead {
    read_path_value(HKEY_CURRENT_USER, USER_ENVIRONMENT_SUBKEY)
}

/// Read with `RegQueryValueExW` rather than `RegGetValueW`: the one-call
/// helper's auto-expansion is flag-subtle and refused the machine value here
/// (measured 2026-09-23: `RRF_RT_REG_SZ` answered `ERROR_FILE_NOT_FOUND` for
/// `HKLM\...\Session Manager\Environment\Path` from this process), so the
/// explicit read keeps the type check and the expansion visible.
fn read_path_value(root: HKEY, subkey: &str) -> RegistryPathRead {
    let subkey_wide = wide(subkey);
    let value_wide = wide(PATH_VALUE_NAME);
    unsafe {
        let mut key: HKEY = std::ptr::null_mut();
        let status = RegOpenKeyExW(root, subkey_wide.as_ptr(), 0, KEY_READ, &mut key);
        if status != 0 {
            return RegistryPathRead::Failed(RegistryPathError::Win32(status));
        }
        let mut kind: u32 = 0;
        let mut bytes: u32 = 0;
        let status = RegQueryValueExW(
            key,
            value_wide.as_ptr(),
            std::ptr::null(),
            &mut kind,
            std::ptr::null_mut(),
            &mut bytes,
        );
        if status != 0 {
            RegCloseKey(key);
            return RegistryPathRead::Failed(RegistryPathError::Win32(status));
        }
        if bytes == 0 {
            RegCloseKey(key);
            return RegistryPathRead::Failed(RegistryPathError::EmptyValue);
        }
        if kind != REG_SZ && kind != REG_EXPAND_SZ {
            RegCloseKey(key);
            return RegistryPathRead::Failed(RegistryPathError::UnexpectedType(kind));
        }
        let mut buffer = vec![0u8; bytes as usize + 2];
        let mut size = bytes;
        let status = RegQueryValueExW(
            key,
            value_wide.as_ptr(),
            std::ptr::null(),
            &mut kind,
            buffer.as_mut_ptr(),
            &mut size,
        );
        RegCloseKey(key);
        if status != 0 {
            return RegistryPathRead::Failed(RegistryPathError::Win32(status));
        }
        let used_bytes = (size as usize).min(buffer.len());
        let units: Vec<u16> = buffer[..used_bytes]
            .as_chunks::<2>()
            .0
            .iter()
            .map(|pair| u16::from_le_bytes(*pair))
            .collect();
        let mut value = String::from_utf16_lossy(&units);
        while value.ends_with('\0') {
            value.pop();
        }
        if kind == REG_EXPAND_SZ {
            match expand_environment_strings(&value) {
                Some(expanded) => RegistryPathRead::Read(expanded),
                None => RegistryPathRead::Failed(RegistryPathError::Win32(0)),
            }
        } else {
            RegistryPathRead::Read(value)
        }
    }
}

/// `ExpandEnvironmentStringsW` against the process environment. A variable
/// the process does not have stays as `%NAME%`; the entry then matches no
/// folder, which is a lost search entry and never an error (recorded review
/// finding #6: a variable added to the registry after this process started
/// resolves only after a restart).
fn expand_environment_strings(value: &str) -> Option<String> {
    let source = wide(value);
    unsafe {
        let needed = ExpandEnvironmentStringsW(source.as_ptr(), std::ptr::null_mut(), 0);
        if needed == 0 {
            return None;
        }
        let mut buffer = vec![0u16; needed as usize];
        let written = ExpandEnvironmentStringsW(source.as_ptr(), buffer.as_mut_ptr(), needed);
        if written == 0 || written as usize > buffer.len() {
            return None;
        }
        Some(String::from_utf16_lossy(
            &buffer[..(written as usize).saturating_sub(1)],
        ))
    }
}

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_read_describes_its_entry_count_without_spelling_the_value() {
        let read = RegistryPathRead::Read(r"C:\One;;C:\Two\".to_string());
        assert_eq!(
            read.describe("machine PATH"),
            "machine PATH: read, 2 entries"
        );
        assert_eq!(read.value(), Some(r"C:\One;;C:\Two\"));
    }

    #[test]
    fn a_failed_read_describes_the_reason_it_was_not_read() {
        assert_eq!(
            RegistryPathRead::Failed(RegistryPathError::Win32(2)).describe("user PATH"),
            "user PATH: not read: error 2"
        );
        assert_eq!(
            RegistryPathRead::Failed(RegistryPathError::EmptyValue).describe("user PATH"),
            "user PATH: not read: empty value"
        );
        assert_eq!(
            RegistryPathRead::Failed(RegistryPathError::UnexpectedType(3)).describe("machine PATH"),
            "machine PATH: not read: unexpected type 3"
        );
        assert_eq!(
            RegistryPathRead::Failed(RegistryPathError::Win32(2)).value(),
            None
        );
    }
}
