//! The PATH a Codex child is started with, so the shell its Windows sandbox
//! launches is a real PowerShell rather than a Microsoft Store execution alias.

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};

/// The PATH for a Codex child with a real PowerShell first, or `None` when no
/// real PowerShell is installed and the inherited PATH should stand.
pub(crate) fn path_with_real_shell() -> Option<OsString> {
    let path = std::env::var_os("PATH")?;
    let alias_dir = user_alias_dir();
    path_with_shells_first(&path, &real_shell_dirs(), alias_dir.as_deref())
}

/// The folder holding the user's App Execution Aliases. The `pwsh.exe` alias
/// in it cannot start under the sandbox's restricted token.
fn user_alias_dir() -> Option<PathBuf> {
    std::env::var_os("LOCALAPPDATA")
        .map(|local| PathBuf::from(local).join("Microsoft").join("WindowsApps"))
}

/// The folders holding a PowerShell executable, PowerShell 7 before Windows
/// PowerShell.
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

/// The shell folders first, then the PATH without the user's alias folder and
/// without a repeat of a shell folder. Only that folder leaves: other folders
/// named `WindowsApps` are tooling the child may need.
fn path_with_shells_first(
    path: &OsStr,
    shell_dirs: &[PathBuf],
    alias_dir: Option<&Path>,
) -> Option<OsString> {
    let is_alias = |dir: &Path| alias_dir.is_some_and(|alias| same_dir(alias, dir));
    let shells: Vec<PathBuf> = shell_dirs
        .iter()
        .filter(|dir| !is_alias(dir))
        .cloned()
        .collect();
    if shells.is_empty() {
        return None;
    }
    let rest = std::env::split_paths(path)
        .filter(|dir| !is_alias(dir) && !shells.iter().any(|shell| same_dir(shell, dir)));
    std::env::join_paths(shells.iter().cloned().chain(rest)).ok()
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
    use std::path::{Path, PathBuf};

    use super::path_with_shells_first;

    const ALIAS: &str = r"C:\Users\gualt\AppData\Local\Microsoft\WindowsApps";
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
    fn the_user_alias_folder_leaves_the_path_and_the_shell_goes_ahead() {
        let path = path_of(&[ALIAS, SYSTEM32, r"C:\Tools"]);
        let shells = [PathBuf::from(WINDOWS_POWERSHELL)];

        let result =
            path_with_shells_first(&path, &shells, Some(Path::new(ALIAS))).expect("a shell");

        assert_eq!(
            entries(&result),
            [WINDOWS_POWERSHELL, SYSTEM32, r"C:\Tools"]
        );
    }

    #[test]
    fn when_only_an_alias_pwsh_exists_the_path_falls_back_to_windows_powershell() {
        // Only the MSIX-backed alias is installed: no pwsh remains on the PATH,
        // and the shell that does resolve is the inbox one.
        let path = path_of(&[ALIAS, SYSTEM32]);
        let shells = [PathBuf::from(WINDOWS_POWERSHELL)];

        let result =
            path_with_shells_first(&path, &shells, Some(Path::new(ALIAS))).expect("a shell");

        assert_eq!(entries(&result), [WINDOWS_POWERSHELL, SYSTEM32]);
    }

    #[test]
    fn other_windows_apps_folders_and_look_alikes_stay_on_the_path() {
        let path = path_of(&[
            r"D:\Company\WindowsApps",
            r"C:\Users\gualt\AppData\Local\Microsoft\WindowsApps2",
            r"C:\Users\gualt\AppData\Local\Microsoft\WindowsApps\",
            SYSTEM32,
        ]);
        let shells = [PathBuf::from(WINDOWS_POWERSHELL)];

        let result =
            path_with_shells_first(&path, &shells, Some(Path::new(ALIAS))).expect("a shell");

        assert_eq!(
            entries(&result),
            [
                WINDOWS_POWERSHELL,
                r"D:\Company\WindowsApps",
                r"C:\Users\gualt\AppData\Local\Microsoft\WindowsApps2",
                SYSTEM32,
            ]
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

        let result = path_with_shells_first(&path, &shells, None).expect("a shell");

        assert_eq!(
            entries(&result),
            [WINDOWS_POWERSHELL, r"C:\Tools", SYSTEM32]
        );
    }

    #[test]
    fn powershell_seven_is_offered_before_windows_powershell() {
        let path = path_of(&[ALIAS, SYSTEM32]);
        let shells = [
            PathBuf::from(POWERSHELL_SEVEN),
            PathBuf::from(WINDOWS_POWERSHELL),
        ];

        let result =
            path_with_shells_first(&path, &shells, Some(Path::new(ALIAS))).expect("a shell");

        assert_eq!(
            entries(&result),
            [POWERSHELL_SEVEN, WINDOWS_POWERSHELL, SYSTEM32]
        );
    }

    #[test]
    fn without_a_real_shell_the_path_is_left_alone() {
        assert_eq!(
            path_with_shells_first(&path_of(&[ALIAS]), &[], Some(Path::new(ALIAS))),
            None
        );
    }

    #[test]
    fn a_shell_inside_the_user_alias_folder_is_never_offered() {
        let shells = [PathBuf::from(ALIAS)];

        assert_eq!(
            path_with_shells_first(&path_of(&[ALIAS]), &shells, Some(Path::new(ALIAS))),
            None
        );
    }
}
