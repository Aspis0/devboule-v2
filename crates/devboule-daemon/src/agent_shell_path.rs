//! The PATH a Codex child is started with, so the shell its Windows sandbox
//! launches is a real PowerShell rather than a Microsoft Store execution alias.

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};

/// The PATH for a Codex child with a real PowerShell first, or `None` when no
/// real PowerShell is installed and the inherited PATH should stand.
pub(crate) fn path_with_real_shell() -> Option<OsString> {
    let path = std::env::var_os("PATH")?;
    path_with_shells_first(&path, &real_shell_dirs())
}

/// The folders holding a PowerShell executable, PowerShell 7 before Windows
/// PowerShell. Only these are offered as the shell, never a Store alias.
fn real_shell_dirs() -> Vec<PathBuf> {
    let program_files = std::env::var_os("ProgramFiles")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\Program Files"));
    let system_root = std::env::var_os("SystemRoot")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\Windows"));
    let candidates = [
        (program_files.join("PowerShell").join("7"), "pwsh.exe"),
        (
            system_root
                .join("System32")
                .join("WindowsPowerShell")
                .join("v1.0"),
            "powershell.exe",
        ),
    ];
    candidates
        .into_iter()
        .filter(|(dir, exe)| dir.join(exe).is_file())
        .map(|(dir, _)| dir)
        .collect()
}

/// The shell folders first, then the PATH without its Store alias folder and
/// without a repeat of a shell folder. The shell is found by name on the PATH,
/// and a restricted token cannot start the alias (`CreateProcessAsUserW` fails
/// with access denied), so the alias must not be the first match.
fn path_with_shells_first(path: &OsStr, shell_dirs: &[PathBuf]) -> Option<OsString> {
    let shells: Vec<PathBuf> = shell_dirs
        .iter()
        .filter(|dir| !is_store_alias_dir(dir))
        .cloned()
        .collect();
    if shells.is_empty() {
        return None;
    }
    let rest = std::env::split_paths(path)
        .filter(|dir| !is_store_alias_dir(dir) && !shells.iter().any(|shell| same_dir(shell, dir)));
    std::env::join_paths(shells.iter().cloned().chain(rest)).ok()
}

fn is_store_alias_dir(dir: &Path) -> bool {
    dir.file_name()
        .is_some_and(|name| name.eq_ignore_ascii_case("WindowsApps"))
}

fn same_dir(a: &Path, b: &Path) -> bool {
    let trimmed = |dir: &Path| {
        dir.to_string_lossy()
            .trim_end_matches(['\\', '/'])
            .to_ascii_lowercase()
    };
    trimmed(a) == trimmed(b)
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;
    use std::path::PathBuf;

    use super::path_with_shells_first;

    const STORE: &str = r"C:\Users\gualt\AppData\Local\Microsoft\WindowsApps";
    const SYSTEM32: &str = r"C:\Windows\System32";
    const WINDOWS_POWERSHELL: &str = r"C:\Windows\System32\WindowsPowerShell\v1.0";
    const POWERSHELL_SEVEN: &str = r"C:\Program Files\PowerShell\7";

    fn path_of(dirs: &[&str]) -> OsString {
        std::env::join_paths(dirs.iter().map(PathBuf::from)).expect("a PATH")
    }

    fn entries(path: &OsString) -> Vec<String> {
        std::env::split_paths(path)
            .map(|dir| dir.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn a_real_shell_goes_ahead_and_the_store_aliases_leave_the_path() {
        let path = path_of(&[STORE, SYSTEM32, r"C:\Tools"]);
        let shells = [PathBuf::from(WINDOWS_POWERSHELL)];

        let result = path_with_shells_first(&path, &shells).expect("a shell is offered");

        assert_eq!(
            entries(&result),
            [WINDOWS_POWERSHELL, SYSTEM32, r"C:\Tools"]
        );
    }

    #[test]
    fn a_shell_already_on_the_path_moves_to_the_front_once() {
        let path = path_of(&[
            r"C:\Tools",
            r"C:\Windows\System32\WindowsPowerShell\v1.0\",
            SYSTEM32,
        ]);
        let shells = [PathBuf::from(WINDOWS_POWERSHELL)];

        let result = path_with_shells_first(&path, &shells).expect("a shell is offered");

        assert_eq!(
            entries(&result),
            [WINDOWS_POWERSHELL, r"C:\Tools", SYSTEM32]
        );
    }

    #[test]
    fn powershell_seven_is_offered_before_windows_powershell() {
        let path = path_of(&[STORE, SYSTEM32]);
        let shells = [
            PathBuf::from(POWERSHELL_SEVEN),
            PathBuf::from(WINDOWS_POWERSHELL),
        ];

        let result = path_with_shells_first(&path, &shells).expect("a shell is offered");

        assert_eq!(
            entries(&result),
            [POWERSHELL_SEVEN, WINDOWS_POWERSHELL, SYSTEM32]
        );
    }

    #[test]
    fn without_a_real_shell_the_path_is_left_alone() {
        assert_eq!(path_with_shells_first(&path_of(&[STORE]), &[]), None);
    }

    #[test]
    fn a_store_alias_is_never_offered_as_the_shell() {
        let shells = [PathBuf::from(STORE)];

        assert_eq!(path_with_shells_first(&path_of(&[STORE]), &shells), None);
    }
}
