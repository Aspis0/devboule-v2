//! The Windows PATH provider discovery and spawn agree on.
//!
//! A daemon launched by the desktop app inherits the launcher's environment —
//! Explorer's login-time environment when the app starts from the Start menu.
//! A folder added to the machine or user PATH after that login is invisible to
//! [`std::env::var_os`] for the daemon's lifetime, so a provider installed
//! into such a folder never reaches the picker even though the registry
//! records it. The registry values are the durable record of what the PATH
//! should be; this module merges them behind the process PATH for discovery,
//! and hands a spawn carried by that discovery the same merged PATH.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use windows_sys::Win32::System::Environment::ExpandEnvironmentStringsW;
use windows_sys::Win32::System::Registry::{
    RegCloseKey, RegOpenKeyExW, RegQueryValueExW, HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE,
    KEY_READ, REG_EXPAND_SZ, REG_SZ,
};

/// Where [`merged_path_directories`] reads the three PATH values from. The
/// seam that keeps tests off the registry: production uses
/// [`RegistryPathSource`]; a test supplies all three values and touches
/// neither `HKLM` nor `HKCU`.
pub(crate) trait WindowsPathSource {
    fn process_path(&self) -> Option<OsString>;
    fn machine_path(&self) -> Option<String>;
    fn user_path(&self) -> Option<String>;
}

/// The production source: the process PATH plus the machine and user `Path`
/// registry values.
pub(crate) struct RegistryPathSource;

const MACHINE_ENVIRONMENT_SUBKEY: &str =
    r"SYSTEM\CurrentControlSet\Control\Session Manager\Environment";
const USER_ENVIRONMENT_SUBKEY: &str = "Environment";
const PATH_VALUE_NAME: &str = "Path";

impl WindowsPathSource for RegistryPathSource {
    fn process_path(&self) -> Option<OsString> {
        std::env::var_os("PATH")
    }

    fn machine_path(&self) -> Option<String> {
        registry_path(HKEY_LOCAL_MACHINE, MACHINE_ENVIRONMENT_SUBKEY)
    }

    fn user_path(&self) -> Option<String> {
        registry_path(HKEY_CURRENT_USER, USER_ENVIRONMENT_SUBKEY)
    }
}

/// The process PATH first, then every machine PATH entry and then every user
/// PATH entry it does not already name, in registry order. Registry values are
/// read at every call, so a folder installed after the daemon started is found
/// by the next discovery without a restart.
pub(crate) fn merged_path_directories(source: &dyn WindowsPathSource) -> Vec<PathBuf> {
    let mut directories: Vec<PathBuf> = source
        .process_path()
        .map(|paths| std::env::split_paths(&paths).collect())
        .unwrap_or_default();
    for value in [source.machine_path(), source.user_path()]
        .into_iter()
        .flatten()
    {
        for entry in std::env::split_paths(&value) {
            if entry.as_os_str().is_empty()
                || directories
                    .iter()
                    .any(|existing| same_directory(existing, &entry))
            {
                continue;
            }
            directories.push(entry);
        }
    }
    directories
}

/// The PATH env pair a spawn of a program in `program_directory` must carry:
/// `None` when the inherited PATH already names that folder (the healthy
/// case, no environment override), `None` when no PATH names it (an npx
/// wrapper row, or a user-declared command whose environment is the user's
/// own), and otherwise the inherited PATH plus exactly the registry entries
/// discovery used — so a provider found through a registry folder still sees
/// its own tools (node, git).
pub(crate) fn spawn_path_for(
    program_directory: &Path,
    source: &dyn WindowsPathSource,
) -> Option<(String, String)> {
    let process = source.process_path();
    let process_directories: Vec<PathBuf> = process
        .as_ref()
        .map(|paths| std::env::split_paths(paths).collect())
        .unwrap_or_default();
    if process_directories
        .iter()
        .any(|existing| same_directory(existing, program_directory))
    {
        return None;
    }
    let merged = merged_path_directories(source);
    if !merged
        .iter()
        .any(|existing| same_directory(existing, program_directory))
    {
        return None;
    }
    let extensions: Vec<String> = merged
        .into_iter()
        .skip(process_directories.len())
        .map(|directory| directory.to_string_lossy().into_owned())
        .collect();
    if extensions.is_empty() {
        return None;
    }
    let value = match process {
        Some(process) => format!("{};{}", process.to_string_lossy(), extensions.join(";")),
        None => extensions.join(";"),
    };
    Some(("PATH".to_string(), value))
}

/// Stamp every row whose executable folder only the registry PATH names with
/// the PATH pair its spawn must carry. Rows the inherited PATH covers and npx
/// wrapper rows (an executable with no folder) stay untouched, so the healthy
/// case carries no override at all.
pub(crate) fn attach_spawn_path_env(
    agents: &mut [crate::provider_catalog::InstalledAgent],
    source: &dyn WindowsPathSource,
) {
    for agent in agents {
        let Some(parent) = agent.executable.parent() else {
            continue;
        };
        if parent.as_os_str().is_empty() {
            continue;
        }
        if let Some(pair) = spawn_path_for(parent, source) {
            agent.spawn_path_env = Some(pair);
        }
    }
}

/// Windows path comparison is case-insensitive, and a trailing separator
/// carries no directory. The comparison is on the literal text, never
/// canonicalisation: two link spellings of one folder count as two, and a
/// repeated search of a folder is harmless.
fn same_directory(left: &Path, right: &Path) -> bool {
    fn key(path: &Path) -> String {
        path.to_string_lossy()
            .trim_end_matches(['\\', '/'])
            .to_lowercase()
    }
    key(left) == key(right)
}

/// The machine or user `Path` value, expanded when it is `REG_EXPAND_SZ`.
///
/// Read with `RegQueryValueExW` rather than `RegGetValueW`: the one-call
/// helper's auto-expansion is flag-subtle and refused the machine value here
/// (measured 2026-09-23: `RRF_RT_REG_SZ` answered `ERROR_FILE_NOT_FOUND` for
/// `HKLM\...\Session Manager\Environment\Path` from an unsigned process), so
/// the explicit read keeps the type check and the expansion visible. A key or
/// value that cannot be read answers `None`: discovery loses those entries
/// and never fails.
fn registry_path(root: HKEY, subkey: &str) -> Option<String> {
    let subkey_wide = wide(subkey);
    let value_wide = wide(PATH_VALUE_NAME);
    unsafe {
        let mut key: HKEY = std::ptr::null_mut();
        if RegOpenKeyExW(root, subkey_wide.as_ptr(), 0, KEY_READ, &mut key) != 0 {
            return None;
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
        if status != 0 || bytes == 0 || (kind != REG_SZ && kind != REG_EXPAND_SZ) {
            RegCloseKey(key);
            return None;
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
            return None;
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
            expand_environment_strings(&value)
        } else {
            Some(value)
        }
    }
}

/// `ExpandEnvironmentStringsW` against the process environment. A variable
/// the process does not have stays as `%NAME%`; the entry then matches no
/// folder, which is a lost search entry and never an error.
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
    use std::path::Path;

    struct StubSource {
        process: Option<OsString>,
        machine: Option<String>,
        user: Option<String>,
    }

    impl WindowsPathSource for StubSource {
        fn process_path(&self) -> Option<OsString> {
            self.process.clone()
        }

        fn machine_path(&self) -> Option<String> {
            self.machine.clone()
        }

        fn user_path(&self) -> Option<String> {
            self.user.clone()
        }
    }

    #[test]
    fn merged_paths_append_machine_then_user_entries_after_the_process_path() {
        let source = StubSource {
            process: Some(OsString::from(r"C:\Tools")),
            machine: Some(r"D:\FromMachine".to_string()),
            user: Some(r"C:\FromUser".to_string()),
        };

        assert_eq!(
            merged_path_directories(&source),
            vec![
                PathBuf::from(r"C:\Tools"),
                PathBuf::from(r"D:\FromMachine"),
                PathBuf::from(r"C:\FromUser"),
            ]
        );
    }

    #[test]
    fn merged_paths_skip_registry_entries_the_process_path_already_has() {
        let source = StubSource {
            process: Some(OsString::from(r"C:\Tools;C:\Other")),
            machine: Some(r"c:\tools\;D:\OnlyMachine".to_string()),
            user: Some(r"C:\OTHER;C:\OnlyUser".to_string()),
        };

        assert_eq!(
            merged_path_directories(&source),
            vec![
                PathBuf::from(r"C:\Tools"),
                PathBuf::from(r"C:\Other"),
                PathBuf::from(r"D:\OnlyMachine"),
                PathBuf::from(r"C:\OnlyUser"),
            ]
        );
    }

    #[test]
    fn merged_paths_survive_a_missing_process_path_and_empty_entries() {
        let source = StubSource {
            process: None,
            machine: Some(";D:\\FromMachine".to_string()),
            user: None,
        };

        assert_eq!(
            merged_path_directories(&source),
            vec![PathBuf::from(r"D:\FromMachine")]
        );
    }

    #[test]
    fn spawn_path_for_a_registry_folder_carries_the_registry_entries() {
        let source = StubSource {
            process: Some(OsString::from(r"C:\Tools;C:\Other")),
            machine: Some(r"D:\FromMachine".to_string()),
            user: Some(r"c:\other".to_string()),
        };

        assert_eq!(
            spawn_path_for(Path::new(r"D:\FromMachine"), &source),
            Some((
                "PATH".to_string(),
                r"C:\Tools;C:\Other;D:\FromMachine".to_string()
            ))
        );
    }

    #[test]
    fn spawn_path_for_is_none_when_the_inherited_path_covers_the_folder() {
        let source = StubSource {
            process: Some(OsString::from(r"C:\Tools")),
            machine: Some(r"c:\tools".to_string()),
            user: None,
        };

        assert_eq!(spawn_path_for(Path::new(r"C:\Tools"), &source), None);
    }

    #[test]
    fn spawn_path_for_is_none_when_no_path_names_the_folder() {
        let source = StubSource {
            process: Some(OsString::from(r"C:\Tools")),
            machine: Some(r"D:\FromMachine".to_string()),
            user: None,
        };

        assert_eq!(spawn_path_for(Path::new(r"E:\Elsewhere"), &source), None);
    }
}
