//! Provider discovery for the command-line agents available to Devboule.
//!
//! The six-agent alias table is adapted from herdr, licensed under the Apache
//! License, Version 2.0, commit `3150bd9`, specifically
//! `herdr-src/src/detect/mod.rs:149-218`.
//! The executable-file check is likewise adapted from herdr under that
//! license and commit, from `herdr-src/src/integration/registry.rs:161-179`.
//! The launch resolver is Devboule code: it follows PATHEXT but retains only
//! extensions that `std::process::Command` can launch directly. The provider
//! catalog shape, ACP launch metadata, and explicit unknown authentication
//! state are also Devboule code.

use std::path::{Path, PathBuf};
#[cfg(test)]
use std::process::{Command, Output};

#[cfg(windows)]
const DEFAULT_PATHEXT: &str = ".COM;.EXE;.BAT;.CMD";

/// A known CLI name and its aliases, with the ACP invocation when supported.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KnownAgent {
    pub id: &'static str,
    pub aliases: &'static [&'static str],
    pub acp_args: Option<&'static [&'static str]>,
}

/// Agents currently relevant to the first provider catalog slice.
///
/// Keep one row per agent so adding a known CLI does not require changing the
/// discovery algorithm.
pub const KNOWN_AGENTS: &[KnownAgent] = &[
    KnownAgent {
        id: "claude",
        aliases: &["claude", "claude-code"],
        acp_args: None,
    },
    KnownAgent {
        id: "codex",
        aliases: &["codex"],
        acp_args: None,
    },
    KnownAgent {
        id: "grok",
        aliases: &["grok", "grok-build"],
        acp_args: Some(&["agent", "stdio"]),
    },
    KnownAgent {
        id: "pi",
        aliases: &["pi"],
        acp_args: None,
    },
    KnownAgent {
        id: "qwen",
        aliases: &["qwen", "qwen-code", "qwen code"],
        acp_args: Some(&["--experimental-acp"]),
    },
    KnownAgent {
        id: "gemini",
        aliases: &["gemini"],
        acp_args: Some(&["--acp"]),
    },
];

/// Explicit product policy for selecting the default ACP provider.
///
/// This is deliberately separate from `KNOWN_AGENTS` layout: on 2026-09-04,
/// grok was the only ACP agent verified to complete a working session on this
/// machine; qwen completed the handshake but declares its ACP flag deprecated.
const ACP_PREFERENCE: &[&str] = &["grok", "qwen", "gemini"];

/// Authentication is intentionally not probed by the catalog.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AuthenticationStatus {
    Unknown,
}

/// One known provider found on this machine.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InstalledAgent {
    pub id: &'static str,
    pub aliases: &'static [&'static str],
    pub executable: PathBuf,
    /// The command as displayed to a user, using the canonical agent name.
    pub acp_command: Option<Vec<String>>,
    pub authentication: AuthenticationStatus,
}

/// Return the known agents whose executable can be resolved from PATH.
pub fn discover() -> Vec<InstalledAgent> {
    KNOWN_AGENTS
        .iter()
        .filter_map(|spec| {
            let executable = spec
                .aliases
                .iter()
                .find_map(|alias| resolve_command_path(alias))?;
            let acp_command = spec.acp_args.map(|args| {
                let mut command = Vec::with_capacity(args.len() + 1);
                command.push(spec.id.to_string());
                command.extend(args.iter().map(|arg| (*arg).to_string()));
                command
            });
            Some(InstalledAgent {
                id: spec.id,
                aliases: spec.aliases,
                executable,
                acp_command,
                authentication: AuthenticationStatus::Unknown,
            })
        })
        .collect()
}

/// Return the first discovered provider that offers ACP.
pub fn first_acp_available() -> Option<InstalledAgent> {
    let discovered = discover();
    ACP_PREFERENCE.iter().find_map(|preferred_id| {
        discovered
            .iter()
            .find(|agent| agent.id == *preferred_id && agent.acp_command.is_some())
            .cloned()
    })
}

pub(crate) fn executable_file_exists(path: &Path) -> bool {
    let Ok(metadata) = path.metadata() else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o111 != 0
    }

    #[cfg(not(unix))]
    {
        true
    }
}

fn resolve_command_path(command: &str) -> Option<PathBuf> {
    let paths = std::env::var_os("PATH")?;
    let directories = std::env::split_paths(&paths).collect::<Vec<_>>();
    resolve_launch_command_in_paths(&directories, command)
}

fn launch_path_candidates(dir: &Path, command: &str) -> Vec<PathBuf> {
    #[cfg(windows)]
    {
        launch_path_candidates_for_pathext(dir, command, std::env::var("PATHEXT").ok().as_deref())
    }

    #[cfg(not(windows))]
    {
        launch_path_candidates_for_pathext(dir, command, None)
    }
}

fn launch_path_candidates_for_pathext(
    dir: &Path,
    command: &str,
    pathext: Option<&str>,
) -> Vec<PathBuf> {
    let base = dir.join(command);

    #[cfg(not(windows))]
    {
        let _ = pathext;
        vec![base]
    }

    #[cfg(windows)]
    {
        if let Some(extension) = Path::new(command).extension().and_then(|ext| ext.to_str()) {
            return is_direct_launch_extension(&format!(".{extension}"))
                .then_some(base)
                .into_iter()
                .collect();
        }

        let pathext = pathext
            .filter(|value| !value.trim().is_empty())
            .unwrap_or(DEFAULT_PATHEXT);
        pathext
            .split(';')
            .map(str::trim)
            .filter(|extension| !extension.is_empty())
            .filter_map(|extension| {
                let extension = if extension.starts_with('.') {
                    extension.to_string()
                } else {
                    format!(".{extension}")
                };
                is_direct_launch_extension(&extension)
                    .then(|| dir.join(format!("{command}{extension}")))
            })
            .collect()
    }
}

#[cfg(windows)]
// PATHEXT also names interpreter scripts such as PS1, VBS, and JS. They are
// intentionally excluded: spawn_process uses Command/CreateProcess directly,
// without an interpreter, so a PS1-only installation is not launchable.
fn is_direct_launch_extension(extension: &str) -> bool {
    [".COM", ".EXE", ".BAT", ".CMD"]
        .iter()
        .any(|known| known.eq_ignore_ascii_case(extension))
}

fn resolve_launch_command_in_paths(paths: &[PathBuf], command: &str) -> Option<PathBuf> {
    paths.iter().find_map(|dir| {
        launch_path_candidates(dir, command)
            .into_iter()
            .find_map(|path| executable_file_exists(&path).then(|| absolute_path(&path)))
            .flatten()
    })
}

fn absolute_path(path: &Path) -> Option<PathBuf> {
    let absolute = std::fs::canonicalize(path).ok().or_else(|| {
        if path.is_absolute() {
            Some(path.to_path_buf())
        } else {
            std::env::current_dir().ok().map(|cwd| cwd.join(path))
        }
    })?;

    #[cfg(windows)]
    {
        Some(normalize_windows_path(absolute))
    }

    #[cfg(not(windows))]
    {
        Some(absolute)
    }
}

#[cfg(windows)]
fn normalize_windows_path(path: PathBuf) -> PathBuf {
    use std::ffi::{OsStr, OsString};
    use std::os::windows::ffi::{OsStrExt, OsStringExt};

    let strip_prefix = |prefix: &str| {
        let path = path.as_os_str().encode_wide().collect::<Vec<_>>();
        let prefix = OsStr::new(prefix).encode_wide().collect::<Vec<_>>();
        path.starts_with(&prefix)
            .then(|| OsString::from_wide(&path[prefix.len()..]))
    };

    if let Some(rest) = strip_prefix(r"\\?\UNC\") {
        let mut normalized = OsString::from_wide(&[b'\\' as u16, b'\\' as u16]);
        normalized.push(rest);
        return PathBuf::from(normalized);
    }

    if let Some(rest) = strip_prefix(r"\\?\") {
        let rest = PathBuf::from(rest);
        if rest.is_absolute() {
            return rest;
        }
    }

    path
}

#[cfg(test)]
fn probe_version(executable: &Path) -> std::io::Result<Output> {
    Command::new(executable).arg("--version").output()
}

#[cfg(test)]
mod tests {
    use super::discover;
    #[cfg(windows)]
    use super::{
        executable_file_exists, launch_path_candidates_for_pathext, resolve_launch_command_in_paths,
    };
    #[cfg(windows)]
    use std::fs::{self, File};
    #[cfg(windows)]
    use std::path::Path;
    use std::path::PathBuf;
    #[cfg(windows)]
    use std::time::{SystemTime, UNIX_EPOCH};

    #[cfg(windows)]
    fn temporary_directory(label: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        std::env::temp_dir().join(format!(
            "devboule-provider-catalog-{label}-{}-{nonce}",
            std::process::id()
        ))
    }

    #[test]
    fn command_name_candidate_shape_is_platform_specific() {
        let dir = PathBuf::from("provider-catalog-test");
        let candidates =
            super::launch_path_candidates_for_pathext(&dir, "agent", Some(".EXE;.CMD"));

        #[cfg(windows)]
        assert_eq!(
            candidates,
            vec![dir.join("agent.EXE"), dir.join("agent.CMD")]
        );
        #[cfg(not(windows))]
        assert_eq!(candidates, vec![dir.join("agent")]);
    }

    #[cfg(windows)]
    #[test]
    fn windows_fake_npm_shims_resolve_cmd_and_ps1() {
        let dir = temporary_directory("npm-shims");
        fs::create_dir_all(&dir).expect("temporary directory");
        File::create(dir.join("codex")).expect("fake POSIX shim");
        File::create(dir.join("codex.cmd")).expect("fake cmd shim");
        File::create(dir.join("qwen")).expect("fake POSIX shim");
        File::create(dir.join("qwen.ps1")).expect("fake powershell shim");
        File::create(dir.join("qwen.cmd")).expect("fake cmd shim");

        let paths = vec![dir.clone()];
        assert_eq!(
            resolve_launch_command_in_paths(&paths, "codex"),
            Some(super::normalize_windows_path(
                fs::canonicalize(dir.join("codex.cmd")).expect("canonical cmd shim"),
            ))
        );
        assert_eq!(
            resolve_launch_command_in_paths(&paths, "qwen"),
            Some(super::normalize_windows_path(
                fs::canonicalize(dir.join("qwen.cmd")).expect("canonical cmd shim"),
            ))
        );

        fs::remove_dir_all(dir).expect("temporary directory cleanup");
    }

    #[cfg(windows)]
    #[test]
    fn windows_launch_candidates_follow_pathext_and_skip_the_naked_name() {
        let dir = temporary_directory("pathext");
        fs::create_dir_all(&dir).expect("temporary directory");

        assert_eq!(
            launch_path_candidates_for_pathext(&dir, "agent", Some(".BAT;.CMD;.EXE")),
            vec![
                dir.join("agent.BAT"),
                dir.join("agent.CMD"),
                dir.join("agent.EXE"),
            ]
        );
        assert_eq!(
            launch_path_candidates_for_pathext(&dir, "agent.cmd", Some(".BAT;.CMD;.EXE")),
            vec![dir.join("agent.cmd")]
        );

        fs::remove_dir_all(dir).expect("temporary directory cleanup");
    }

    #[cfg(windows)]
    fn existing_launch_candidates(dir: &Path, command: &str, pathext: &str) -> Vec<PathBuf> {
        launch_path_candidates_for_pathext(dir, command, Some(pathext))
            .into_iter()
            .filter(|path| executable_file_exists(path))
            .collect()
    }

    #[cfg(windows)]
    #[test]
    fn windows_only_powershell_shim_is_not_launchable() {
        let dir = temporary_directory("ps1-only");
        fs::create_dir_all(&dir).expect("temporary directory");
        File::create(dir.join("agent.ps1")).expect("fake powershell shim");

        assert!(existing_launch_candidates(&dir, "agent", ".PS1").is_empty());

        fs::remove_dir_all(dir).expect("temporary directory cleanup");
    }

    #[cfg(windows)]
    #[test]
    fn windows_cmd_wins_over_powershell_even_when_pathext_prefers_ps1() {
        let dir = temporary_directory("ps1-before-cmd");
        fs::create_dir_all(&dir).expect("temporary directory");
        File::create(dir.join("agent.ps1")).expect("fake powershell shim");
        File::create(dir.join("agent.cmd")).expect("fake cmd shim");

        assert_eq!(
            existing_launch_candidates(&dir, "agent", ".PS1;.CMD"),
            vec![dir.join("agent.CMD")]
        );

        fs::remove_dir_all(dir).expect("temporary directory cleanup");
    }

    #[cfg(windows)]
    #[test]
    fn windows_ignores_pathext_entries_without_a_direct_launcher() {
        let dir = temporary_directory("unsupported-pathext");
        fs::create_dir_all(&dir).expect("temporary directory");
        File::create(dir.join("agent.VBS")).expect("fake visual basic script");
        File::create(dir.join("agent.JS")).expect("fake javascript script");

        assert!(existing_launch_candidates(&dir, "agent", ".VBS;.JS").is_empty());

        fs::remove_dir_all(dir).expect("temporary directory cleanup");
    }

    #[cfg(windows)]
    #[test]
    fn windows_pathext_fallback_handles_empty_space_and_malformed_entries() {
        let dir = temporary_directory("pathext-fallback");
        fs::create_dir_all(&dir).expect("temporary directory");

        let default_candidates = vec![
            dir.join("agent.COM"),
            dir.join("agent.EXE"),
            dir.join("agent.BAT"),
            dir.join("agent.CMD"),
        ];
        for pathext in [None, Some(""), Some("   "), Some(".VBS;.JS")] {
            let expected = if pathext == Some(".VBS;.JS") {
                Vec::new()
            } else {
                default_candidates.clone()
            };
            assert_eq!(
                launch_path_candidates_for_pathext(&dir, "agent", pathext),
                expected
            );
        }
        assert_eq!(
            launch_path_candidates_for_pathext(&dir, "agent", Some("eXe;cMd;.PS1")),
            vec![dir.join("agent.eXe"), dir.join("agent.cMd")]
        );

        fs::remove_dir_all(dir).expect("temporary directory cleanup");
    }

    #[cfg(windows)]
    #[test]
    fn windows_resolved_paths_drop_verbatim_prefix_without_breaking_unc() {
        assert_eq!(
            super::normalize_windows_path(PathBuf::from(r"\\?\C:\Users\Name\agent.exe")),
            PathBuf::from(r"C:\Users\Name\agent.exe")
        );
        assert_eq!(
            super::normalize_windows_path(PathBuf::from(r"\\?\UNC\server\share\agent.exe")),
            PathBuf::from(r"\\server\share\agent.exe")
        );
    }

    #[test]
    #[ignore = "measurement, not an assertion; run by hand with --ignored --nocapture"]
    fn reports_installed_cli_agents() {
        let agents = discover();
        println!("provider catalog found {} agent(s):", agents.len());
        for agent in agents {
            println!(
                "{} => {} | ACP={:?} | auth={:?}",
                agent.id,
                agent.executable.display(),
                agent.acp_command,
                agent.authentication
            );
        }
        if let Some(agent) = super::first_acp_available() {
            println!(
                "default ACP: {} => {}",
                agent.id,
                agent
                    .acp_command
                    .expect("selected agent offers ACP")
                    .join(" ")
            );
        } else {
            println!("default ACP: none");
        }
    }

    #[test]
    #[ignore = "measurement, not an assertion; run by hand with --ignored --nocapture"]
    fn reports_cli_launchability() {
        for agent in discover() {
            match super::probe_version(&agent.executable) {
                Ok(output) => println!(
                    "{} => STARTED exit={:?} stdout={:?} stderr={:?}",
                    agent.id,
                    output.status.code(),
                    String::from_utf8_lossy(&output.stdout),
                    String::from_utf8_lossy(&output.stderr)
                ),
                Err(error) => println!(
                    "{} => NOT STARTED path={} error={error}",
                    agent.id,
                    agent.executable.display()
                ),
            }
        }
    }
}
