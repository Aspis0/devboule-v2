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

use crate::windows_registry_path::RegistryPathRead;

/// Where the PATH values come from. The seam that keeps tests off the
/// registry: production uses [`RegistryPathSource`]; a test supplies the
/// values and touches neither `HKLM` nor `HKCU`.
pub(crate) trait WindowsPathSource {
    fn process_path(&self) -> Option<OsString>;
    fn machine_path(&self) -> RegistryPathRead;
    fn user_path(&self) -> RegistryPathRead;
}

/// The production source: the process PATH plus the machine and user `Path`
/// registry values, read through [`crate::windows_registry_path`].
pub(crate) struct RegistryPathSource;

impl WindowsPathSource for RegistryPathSource {
    fn process_path(&self) -> Option<OsString> {
        std::env::var_os("PATH")
    }

    fn machine_path(&self) -> RegistryPathRead {
        crate::windows_registry_path::read_machine_path()
    }

    fn user_path(&self) -> RegistryPathRead {
        crate::windows_registry_path::read_user_path()
    }
}

/// One captured view of the three PATH values. A request captures once and
/// answers discovery, spawn policy and diagnostics from the capture; the
/// registry is never re-read per agent.
pub(crate) struct PathSnapshot {
    process: Option<OsString>,
    machine: RegistryPathRead,
    user: RegistryPathRead,
}

impl PathSnapshot {
    /// Read the process PATH and the two registry values.
    pub(crate) fn capture(source: &dyn WindowsPathSource) -> Self {
        Self {
            process: source.process_path(),
            machine: source.machine_path(),
            user: source.user_path(),
        }
    }

    /// The directories discovery searches: the inherited PATH first, then the
    /// machine and user registry entries it does not already name.
    pub(crate) fn directories(&self) -> Vec<PathBuf> {
        let mut directories: Vec<PathBuf> = self
            .process
            .as_ref()
            .map(|paths| std::env::split_paths(paths).collect())
            .unwrap_or_default();
        for read in [&self.machine, &self.user] {
            let Some(value) = read.value() else {
                continue;
            };
            for entry in std::env::split_paths(value) {
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

    /// The PATH env pair a spawn of a program launched from
    /// `launch_directory` must carry: `None` when the inherited PATH already
    /// names that folder (the healthy case, no environment override), `None`
    /// when no PATH names it (a user-declared command whose environment is
    /// the user's own), and otherwise the inherited PATH plus exactly the
    /// registry entries discovery used — so a provider found through a
    /// registry folder still sees its own tools (node, git).
    pub(crate) fn spawn_path_for(&self, launch_directory: &Path) -> Option<(String, String)> {
        let process = self
            .process
            .as_ref()
            .map(|paths| paths.to_string_lossy().into_owned());
        let process_directories: Vec<PathBuf> = self
            .process
            .as_ref()
            .map(|paths| std::env::split_paths(paths).collect())
            .unwrap_or_default();
        if process_directories
            .iter()
            .any(|existing| same_directory(existing, launch_directory))
        {
            return None;
        }
        let merged = self.directories();
        if !merged
            .iter()
            .any(|existing| same_directory(existing, launch_directory))
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
            Some(process) => format!("{};{}", process, extensions.join(";")),
            None => extensions.join(";"),
        };
        Some(("PATH".to_string(), value))
    }

    /// One (`label`, read) pair per registry source, machine before user —
    /// the diagnostics view of this capture.
    #[cfg_attr(not(feature = "server"), allow(dead_code))]
    pub(crate) fn source_outcomes(&self) -> [(&'static str, &RegistryPathRead); 2] {
        [("machine PATH", &self.machine), ("user PATH", &self.user)]
    }
}

/// The diagnostics lines for the two registry PATH sources, in machine-then-
/// user order, including the reason a value was not read — a failed read
/// must never look like an empty registry PATH.
#[cfg_attr(not(feature = "server"), allow(dead_code))]
pub(crate) fn path_registry_outcome_lines(source: &dyn WindowsPathSource) -> Vec<String> {
    PathSnapshot::capture(source)
        .source_outcomes()
        .map(|(label, read)| read.describe(label))
        .into_iter()
        .collect()
}

/// The same lines from the real registry, for the diagnostics report.
#[cfg_attr(not(feature = "server"), allow(dead_code))]
pub(crate) fn path_registry_outcomes() -> Vec<String> {
    path_registry_outcome_lines(&RegistryPathSource)
}

/// Stamp every row whose launch folder only the registry PATH names with the
/// PATH pair its spawn must carry. The launch folder is the directory the row
/// was resolved from — the shim's directory after an npm cmd-shim unwrap, the
/// npx directory for a registry wrapper row — falling back to the executable's
/// own parent. Rows the inherited PATH covers and rows with no folder (a
/// user-declared bare command) stay untouched, so the healthy case carries no
/// override at all.
pub(crate) fn attach_spawn_path_env(
    agents: &mut [crate::provider_catalog::InstalledAgent],
    snapshot: &PathSnapshot,
) {
    for agent in agents {
        let directory = match &agent.launch_directory {
            Some(directory) if !directory.as_os_str().is_empty() => directory.clone(),
            _ => match agent.executable.parent() {
                Some(parent) if !parent.as_os_str().is_empty() => parent.to_path_buf(),
                _ => continue,
            },
        };
        if let Some(pair) = snapshot.spawn_path_for(&directory) {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::windows_registry_path::RegistryPathError;
    use std::path::Path;

    struct StubSource {
        process: Option<OsString>,
        machine: RegistryPathRead,
        user: RegistryPathRead,
    }

    impl WindowsPathSource for StubSource {
        fn process_path(&self) -> Option<OsString> {
            self.process.clone()
        }

        fn machine_path(&self) -> RegistryPathRead {
            self.machine.clone()
        }

        fn user_path(&self) -> RegistryPathRead {
            self.user.clone()
        }
    }

    fn read(value: &str) -> RegistryPathRead {
        RegistryPathRead::Read(value.to_string())
    }

    fn failed() -> RegistryPathRead {
        RegistryPathRead::Failed(RegistryPathError::Win32(2))
    }

    #[test]
    fn merged_paths_append_machine_then_user_entries_after_the_process_path() {
        let snapshot = PathSnapshot::capture(&StubSource {
            process: Some(OsString::from(r"C:\Tools")),
            machine: read(r"D:\FromMachine"),
            user: read(r"C:\FromUser"),
        });

        assert_eq!(
            snapshot.directories(),
            vec![
                PathBuf::from(r"C:\Tools"),
                PathBuf::from(r"D:\FromMachine"),
                PathBuf::from(r"C:\FromUser"),
            ]
        );
    }

    #[test]
    fn merged_paths_skip_registry_entries_the_process_path_already_has() {
        let snapshot = PathSnapshot::capture(&StubSource {
            process: Some(OsString::from(r"C:\Tools;C:\Other")),
            machine: read(r"c:\tools\;D:\OnlyMachine"),
            user: read(r"C:\OTHER;C:\OnlyUser"),
        });

        assert_eq!(
            snapshot.directories(),
            vec![
                PathBuf::from(r"C:\Tools"),
                PathBuf::from(r"C:\Other"),
                PathBuf::from(r"D:\OnlyMachine"),
                PathBuf::from(r"C:\OnlyUser"),
            ]
        );
    }

    #[test]
    fn merged_paths_survive_a_missing_process_path_and_unreadable_registry_values() {
        let snapshot = PathSnapshot::capture(&StubSource {
            process: None,
            machine: failed(),
            user: read(r"D:\FromMachine"),
        });

        assert_eq!(
            snapshot.directories(),
            vec![PathBuf::from(r"D:\FromMachine")]
        );
    }

    #[test]
    fn spawn_path_for_a_registry_folder_carries_the_registry_entries() {
        let snapshot = PathSnapshot::capture(&StubSource {
            process: Some(OsString::from(r"C:\Tools;C:\Other")),
            machine: read(r"D:\FromMachine"),
            user: read(r"c:\other"),
        });

        assert_eq!(
            snapshot.spawn_path_for(Path::new(r"D:\FromMachine")),
            Some((
                "PATH".to_string(),
                r"C:\Tools;C:\Other;D:\FromMachine".to_string()
            ))
        );
    }

    #[test]
    fn spawn_path_for_is_none_when_the_inherited_path_covers_the_folder() {
        let snapshot = PathSnapshot::capture(&StubSource {
            process: Some(OsString::from(r"C:\Tools")),
            machine: read(r"c:\tools"),
            user: failed(),
        });

        assert_eq!(snapshot.spawn_path_for(Path::new(r"C:\Tools")), None);
    }

    #[test]
    fn spawn_path_for_is_none_when_no_path_names_the_folder() {
        let snapshot = PathSnapshot::capture(&StubSource {
            process: Some(OsString::from(r"C:\Tools")),
            machine: read(r"D:\FromMachine"),
            user: failed(),
        });

        assert_eq!(snapshot.spawn_path_for(Path::new(r"E:\Elsewhere")), None);
    }

    #[test]
    fn outcome_lines_name_every_source_that_was_read() {
        let lines = path_registry_outcome_lines(&StubSource {
            process: Some(OsString::from(r"C:\Tools")),
            machine: read(r"C:\Tools;D:\FromMachine"),
            user: read(r"C:\FromUser"),
        });

        assert_eq!(
            lines,
            vec![
                "machine PATH: read, 2 entries".to_string(),
                "user PATH: read, 1 entries".to_string(),
            ]
        );
    }

    #[test]
    fn a_failed_registry_read_is_visible_in_the_outcome_lines() {
        let lines = path_registry_outcome_lines(&StubSource {
            process: Some(OsString::from(r"C:\Tools")),
            machine: read(r"C:\Tools"),
            user: failed(),
        });

        assert_eq!(
            lines,
            vec![
                "machine PATH: read, 1 entries".to_string(),
                "user PATH: not read: error 2".to_string(),
            ]
        );
    }
}
