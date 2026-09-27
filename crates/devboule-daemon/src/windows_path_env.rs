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

use crate::verbatim_path::plain_path;
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

/// One PATH entry as the snapshot compares it: the as-written spelling
/// plus the resolved spelling. `resolved` is the canonical long form when
/// a resolver could open the entry, else the plain spelling of the raw
/// entry; `canonicalised` is false exactly then, and those entries are
/// counted in `PathSnapshot::canonicalize_failures` instead of silently
/// behaving like the pre-fix bug.
#[derive(Clone, Debug)]
struct DirEntry {
    raw: PathBuf,
    resolved: PathBuf,
    canonicalised: bool,
}

/// A PATH entry in its comparison form. Resolution runs once per entry per
/// snapshot capture (see `PathSnapshot::capture`); every later comparison
/// reuses the stored forms and performs no I/O, so a dead mapped drive on
/// PATH costs one probe per capture, never one per agent.
fn resolve_entry(raw: PathBuf) -> DirEntry {
    note_resolve_call();
    let canonical = match stage1_canonicalize(&raw) {
        Ok(canonical) => Some(canonical),
        // A denied open says the volume answered: the name may still be
        // queryable without a handle. Any other error — a dead mapped
        // drive included — costs exactly this one probe and falls back.
        Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => long_path_name(&raw),
        Err(_) => None,
    };
    match canonical {
        Some(canonical) => DirEntry {
            resolved: PathBuf::from(plain_path(&canonical.to_string_lossy())),
            raw,
            canonicalised: true,
        },
        None => {
            let resolved = PathBuf::from(plain_path(&raw.to_string_lossy()));
            DirEntry {
                resolved,
                raw,
                canonicalised: false,
            }
        }
    }
}

fn stage1_canonicalize(raw: &Path) -> std::io::Result<PathBuf> {
    #[cfg(test)]
    if let Some(error) = mocked_stage1_failure(raw) {
        return Err(error);
    }
    std::fs::canonicalize(raw)
}

fn wide_path_name(
    path: &Path,
    query: unsafe extern "system" fn(*const u16, *mut u16, u32) -> u32,
) -> Option<PathBuf> {
    use std::os::windows::ffi::{OsStrExt, OsStringExt};
    let wide: Vec<u16> = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    // With an empty buffer the query returns the required size including NUL.
    let required = unsafe { query(wide.as_ptr(), std::ptr::null_mut(), 0) };
    if required == 0 {
        return None;
    }
    let mut buffer = vec![0u16; required as usize];
    let written = unsafe { query(wide.as_ptr(), buffer.as_mut_ptr(), required) };
    if written == 0 || written >= required {
        return None;
    }
    buffer.truncate(written as usize);
    Some(PathBuf::from(std::ffi::OsString::from_wide(&buffer)))
}

/// The long form of a path via GetLongPathNameW: no handle is opened, so
/// it can succeed where canonicalize is denied. Second resolution stage,
/// and the independent expectation for tests (a different syscall than the
/// product's canonicalize).
pub(crate) fn long_path_name(path: &Path) -> Option<PathBuf> {
    use windows_sys::Win32::Storage::FileSystem::GetLongPathNameW;
    wide_path_name(path, GetLongPathNameW)
}

/// The 8.3 short form, for tests that feed a short spelling in and expect
/// the long form out. `None` when the volume holds no alias: short and
/// long are then the same path and the test still passes, with less signal.
#[cfg(test)]
pub(crate) fn short_path_name(path: &Path) -> Option<PathBuf> {
    use windows_sys::Win32::Storage::FileSystem::GetShortPathNameW;
    wide_path_name(path, GetShortPathNameW)
}

#[cfg(test)]
thread_local! {
    static RESOLVE_CALLS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static STAGE1_MOCK: std::cell::RefCell<Option<(Option<String>, std::io::ErrorKind)>> =
        const { std::cell::RefCell::new(None) };
}

#[cfg(test)]
fn note_resolve_call() {
    RESOLVE_CALLS.with(|calls| calls.set(calls.get() + 1));
}

#[cfg(not(test))]
fn note_resolve_call() {}

#[cfg(test)]
fn mocked_stage1_failure(raw: &Path) -> Option<std::io::Error> {
    STAGE1_MOCK.with(|mock| {
        mock.borrow().as_ref().and_then(|(marker, kind)| {
            marker
                .as_ref()
                .is_none_or(|marker| raw.to_string_lossy().contains(marker.as_str()))
                .then(|| std::io::Error::new(*kind, "mocked canonicalize failure"))
        })
    })
}

/// Resolve calls on this thread since the last reset: the P1-1 bound
/// test's counter. Counts entries presented for resolution, not I/O hits.
#[cfg(test)]
pub(crate) fn resolve_call_count() -> usize {
    RESOLVE_CALLS.with(|calls| calls.get())
}

#[cfg(test)]
pub(crate) fn reset_resolve_calls() {
    RESOLVE_CALLS.with(|calls| calls.set(0));
}

/// Fail stage-1 resolution for entries whose spelling contains `marker`,
/// without touching the filesystem: an existing-but-unopenable directory.
#[cfg(test)]
pub(crate) fn mock_stage1_failure_for(marker: &str, kind: std::io::ErrorKind) {
    STAGE1_MOCK.with(|mock| {
        *mock.borrow_mut() = Some((Some(marker.to_string()), kind));
    });
}

/// Fail stage-1 resolution for every entry: the marker cannot name an 8.3
/// alias the test has not seen yet.
#[cfg(test)]
pub(crate) fn mock_stage1_failure_all(kind: std::io::ErrorKind) {
    STAGE1_MOCK.with(|mock| *mock.borrow_mut() = Some((None, kind)));
}

#[cfg(test)]
pub(crate) fn clear_stage1_mock() {
    STAGE1_MOCK.with(|mock| *mock.borrow_mut() = None);
}

/// One captured view of the three PATH values. A request captures once and
/// answers discovery, spawn policy and diagnostics from the capture; the
/// registry is never re-read per agent.
pub(crate) struct PathSnapshot {
    process: Option<OsString>,
    machine: RegistryPathRead,
    user: RegistryPathRead,
    process_entries: Vec<DirEntry>,
    merged_entries: Vec<DirEntry>,
}

impl PathSnapshot {
    /// Read the process PATH and the two registry values.
    pub(crate) fn capture(source: &dyn WindowsPathSource) -> Self {
        let process = source.process_path();
        let machine = source.machine_path();
        let user = source.user_path();
        // Each entry is resolved at most once per capture and the stored
        // forms serve every later comparison on this snapshot.
        let process_entries: Vec<DirEntry> = process
            .as_ref()
            .map(|paths| std::env::split_paths(paths).map(resolve_entry).collect())
            .unwrap_or_default();
        let mut merged_entries = process_entries.clone();
        for read in [&machine, &user] {
            let Some(value) = read.value() else {
                continue;
            };
            for entry in std::env::split_paths(value).map(resolve_entry) {
                if entry.raw.as_os_str().is_empty()
                    || merged_entries
                        .iter()
                        .any(|existing| entries_match(existing, &entry))
                {
                    continue;
                }
                merged_entries.push(entry);
            }
        }
        Self {
            process,
            machine,
            user,
            process_entries,
            merged_entries,
        }
    }

    /// The directories discovery searches, in resolved form: the inherited
    /// PATH first, then the machine and user registry entries it does not
    /// already name.
    pub(crate) fn directories(&self) -> Vec<PathBuf> {
        self.merged_entries
            .iter()
            .map(|entry| entry.resolved.clone())
            .collect()
    }

    /// Entries compared as written because neither resolver could open
    /// them: reported in `path_registry_outcome_lines`, never silent.
    pub(crate) fn canonicalize_failures(&self) -> usize {
        self.merged_entries
            .iter()
            .filter(|entry| !entry.canonicalised)
            .count()
    }

    /// The PATH env pair a spawn of a program launched from
    /// `launch_directory` must carry: `None` when the inherited PATH already
    /// names that folder (the healthy case, no environment override), `None`
    /// when no PATH names it (a user-declared command whose environment is
    /// the user's own), and otherwise the inherited PATH plus exactly the
    /// registry entries discovery used, in resolved form — so a provider
    /// found through a registry folder still sees its own tools (node, git).
    pub(crate) fn spawn_path_for(&self, launch_directory: &Path) -> Option<(String, String)> {
        if self
            .process_entries
            .iter()
            .any(|entry| entry_matches_launch(entry, launch_directory))
        {
            return None;
        }
        if !self
            .merged_entries
            .iter()
            .any(|entry| entry_matches_launch(entry, launch_directory))
        {
            return None;
        }
        let extensions: Vec<String> = self
            .merged_entries
            .iter()
            .skip(self.process_entries.len())
            .map(|entry| entry.resolved.to_string_lossy().into_owned())
            .collect();
        if extensions.is_empty() {
            return None;
        }
        // The inherited prefix is rebuilt from the same resolved
        // directories discovery searched: the raw PATH text can spell a
        // folder short while the launch resolved it long.
        let value = if self.process.is_some() {
            format!(
                "{};{}",
                self.process_entries
                    .iter()
                    .map(|entry| entry.resolved.to_string_lossy().into_owned())
                    .collect::<Vec<_>>()
                    .join(";"),
                extensions.join(";")
            )
        } else {
            extensions.join(";")
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
    let snapshot = PathSnapshot::capture(source);
    let mut lines: Vec<String> = snapshot
        .source_outcomes()
        .map(|(label, read)| read.describe(label))
        .into_iter()
        .collect();
    let failures = snapshot.canonicalize_failures();
    if failures > 0 {
        lines.push(if failures == 1 {
            "1 PATH entry could not be canonicalised and is compared as written".to_string()
        } else {
            format!(
                "{failures} PATH entries could not be canonicalised and are compared as written"
            )
        });
    }
    lines
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

/// Whether two directory spellings name one folder. The key is literal
/// text — case-insensitive, trailing separators ignored, verbatim prefix
/// folded per `plain_path` — never a filesystem probe: both sides arrive
/// resolved where the snapshot could open them, as written where it could
/// not. Callers try the resolved pair first, then the as-written pair, so
/// an entry that failed resolution still matches its own spelling.
fn same_directory(left: &Path, right: &Path) -> bool {
    fn key(path: &Path) -> String {
        plain_path(&path.to_string_lossy())
            .trim_end_matches(['\\', '/'])
            .to_lowercase()
    }
    key(left) == key(right)
}

/// A snapshot entry names `directory`: the resolved forms match, or — the
/// fallback for an entry no resolver could open — the as-written forms do.
fn entry_matches_launch(entry: &DirEntry, directory: &Path) -> bool {
    same_directory(&entry.resolved, directory) || same_directory(&entry.raw, directory)
}

/// Two snapshot entries name one folder: resolved against resolved, else
/// as-written against as-written.
fn entries_match(left: &DirEntry, right: &DirEntry) -> bool {
    same_directory(&left.resolved, &right.resolved) || same_directory(&left.raw, &right.raw)
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
                // None of these folders exists, so every merged entry is
                // compared as written — and the lines say so.
                "3 PATH entries could not be canonicalised and are compared as written".to_string(),
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
                "1 PATH entry could not be canonicalised and is compared as written".to_string(),
            ]
        );
    }

    /// A drop-guard for the stage-1 mock: a panicking assertion must not
    /// leave the hook armed for the next test on this thread.
    struct Stage1MockGuard;
    impl Stage1MockGuard {
        fn arm(marker: &str, kind: std::io::ErrorKind) -> Self {
            mock_stage1_failure_for(marker, kind);
            Stage1MockGuard
        }
        fn arm_all(kind: std::io::ErrorKind) -> Self {
            mock_stage1_failure_all(kind);
            Stage1MockGuard
        }
    }
    impl Drop for Stage1MockGuard {
        fn drop(&mut self) {
            clear_stage1_mock();
        }
    }

    fn real_temp_dir(label: &str) -> PathBuf {
        crate::test_dirs::test_temp_dir(label)
    }

    fn long_form(dir: &Path) -> PathBuf {
        long_path_name(dir).expect("long form of a real directory")
    }

    fn short_form(dir: &Path) -> PathBuf {
        short_path_name(dir).unwrap_or_else(|| dir.to_path_buf())
    }

    /// P1-1: one capture resolves each presented entry once, and every
    /// later comparison reuses the stored forms — no per-agent repetition.
    #[test]
    fn refresh_resolves_each_path_entry_once_no_matter_how_many_agents() {
        let inherited = real_temp_dir("pass1-count-inherited");
        let extra = real_temp_dir("pass1-count-extra");
        let machine_only = real_temp_dir("pass1-count-machine");
        let user_only = real_temp_dir("pass1-count-user");
        let source = StubSource {
            process: Some(std::env::join_paths([&inherited, &extra]).expect("join process PATH")),
            machine: read(
                &std::env::join_paths([&extra, &machine_only])
                    .expect("join machine PATH")
                    .to_string_lossy(),
            ),
            user: read(&user_only.to_string_lossy()),
        };

        reset_resolve_calls();
        let snapshot = PathSnapshot::capture(&source);
        assert_eq!(
            resolve_call_count(),
            5,
            "each presented entry resolved once at capture"
        );
        assert_eq!(
            snapshot.directories(),
            vec![
                long_form(&inherited),
                long_form(&extra),
                long_form(&machine_only),
                long_form(&user_only),
            ]
        );
        for _ in 0..5 {
            let _ = snapshot.directories();
        }
        for _ in 0..25 {
            for directory in snapshot.directories() {
                let _ = snapshot.spawn_path_for(&directory);
            }
        }
        assert_eq!(
            resolve_call_count(),
            5,
            "comparisons reuse the capture: no per-agent work"
        );

        for dir in [&inherited, &extra, &machine_only, &user_only] {
            std::fs::remove_dir_all(dir).expect("temporary directory cleanup");
        }
    }

    /// Two spellings of one folder — the 8.3 short alias on the machine
    /// PATH, the long form on the user PATH — collapse to one entry, and
    /// the child's PATH carries it once, long.
    #[test]
    fn two_spellings_of_one_folder_collapse_to_one_entry() {
        let other = real_temp_dir("pass1-collapse-other");
        let folder = real_temp_dir("pass1-collapse-long-name");
        let short = short_form(&folder);
        let long = long_form(&folder);
        let source = StubSource {
            process: Some(OsString::from(other.as_os_str())),
            machine: read(&short.to_string_lossy()),
            user: read(&long.to_string_lossy()),
        };

        let snapshot = PathSnapshot::capture(&source);
        assert_eq!(
            snapshot.directories(),
            vec![long_form(&other), long.clone()]
        );
        let pair = snapshot.spawn_path_for(&long);
        assert_eq!(
            pair,
            Some((
                "PATH".to_string(),
                format!(
                    "{};{}",
                    long_form(&other).to_string_lossy(),
                    long.to_string_lossy()
                )
            ))
        );
        assert!(
            !pair.expect("spawn pair").1.contains(r"\\?\"),
            "the child PATH carries no verbatim prefix"
        );

        for dir in [&other, &folder] {
            std::fs::remove_dir_all(dir).expect("temporary directory cleanup");
        }
    }

    /// An entry that exists but cannot be opened is compared as written —
    /// the launch still gets its PATH — and the failure is reported, never
    /// silent. The long spelling is fed in: the mock matches on it, and the
    /// entry is kept as written.
    #[test]
    fn an_unresolvable_entry_is_compared_as_written_and_reported() {
        let visible = real_temp_dir("pass1-fallback-visible");
        let blocked = real_temp_dir("pass1-fallback-blocked");
        let _guard = Stage1MockGuard::arm("pass1-fallback-blocked", std::io::ErrorKind::NotFound);

        let snapshot = PathSnapshot::capture(&StubSource {
            process: Some(OsString::from(visible.as_os_str())),
            machine: failed(),
            user: read(&blocked.to_string_lossy()),
        });
        assert_eq!(snapshot.canonicalize_failures(), 1);
        assert_eq!(
            snapshot.directories(),
            vec![long_form(&visible), blocked.clone()]
        );
        assert_eq!(
            snapshot.spawn_path_for(&blocked),
            Some((
                "PATH".to_string(),
                format!(
                    "{};{}",
                    long_form(&visible).to_string_lossy(),
                    blocked.to_string_lossy()
                )
            )),
            "the as-written spelling still matches"
        );
        let lines = path_registry_outcome_lines(&StubSource {
            process: None,
            machine: failed(),
            user: read(&blocked.to_string_lossy()),
        });
        assert!(
            lines
                .iter()
                .any(|line| line.contains("compared as written")),
            "failures are reported: {lines:?}"
        );

        for dir in [&visible, &blocked] {
            std::fs::remove_dir_all(dir).expect("temporary directory cleanup");
        }
    }

    /// A denied open still yields the long form via the handle-free query:
    /// nothing is recorded, nothing falls back. The short spelling is fed
    /// in — the mock-all fails stage 1 for it — and stage 2 still returns
    /// the long form.
    #[test]
    fn permission_denied_falls_back_to_the_long_name_query() {
        let folder = real_temp_dir("pass1-denied-long-name");
        let short = short_form(&folder);
        let _guard = Stage1MockGuard::arm_all(std::io::ErrorKind::PermissionDenied);

        let snapshot = PathSnapshot::capture(&StubSource {
            process: None,
            machine: failed(),
            user: read(&short.to_string_lossy()),
        });
        assert_eq!(snapshot.canonicalize_failures(), 0);
        assert_eq!(snapshot.directories(), vec![long_form(&folder)]);

        std::fs::remove_dir_all(folder).expect("temporary directory cleanup");
    }

    /// Real directories resolve to the plain long form: an 8.3 alias in,
    /// no verbatim prefix out.
    #[test]
    fn real_directories_resolve_to_plain_long_forms() {
        let folder = real_temp_dir("pass1-verbatim-long-name");
        let short = short_form(&folder);
        let long = long_form(&folder);
        for spelling in [&folder, &short] {
            let entry = resolve_entry(spelling.clone());
            assert!(entry.canonicalised, "{spelling:?} resolves");
            assert_eq!(entry.resolved, long, "{spelling:?} resolves long");
            assert!(
                !entry.resolved.to_string_lossy().contains(r"\\?\"),
                "{spelling:?} carries no verbatim prefix"
            );
        }

        std::fs::remove_dir_all(folder).expect("temporary directory cleanup");
    }
}
