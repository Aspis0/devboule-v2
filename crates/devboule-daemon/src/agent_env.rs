//! Environment injected into every session child so an agent (or our stub)
//! can find the daemon pipe and name the session it is running in.
//!
//! Shape taken from herdr `pane.rs` `apply_pane_launch_env` and
//! `integration/env.rs` `apply_pane_base_env` (Apache-2.0, commit 3150bd9).
//! Names use the `DEVBOULE_` prefix; values are ours.
//!
//! Outer-identity stripping mirrors herdr `pane.rs` `apply_pane_launch_env`
//! and `apply_pane_terminal_env` (Apache-2.0, commit 8ac95427).

use crate::paths::RuntimePaths;
use crate::session::PtyCommand;

/// Marker: this process is running inside a Devboule session.
pub const ENV_MARKER: &str = "DEVBOULE_ENV";
pub const ENV_MARKER_VALUE: &str = "1";
/// Named-pipe path the child reopens to talk to the daemon.
pub const SOCKET_PATH: &str = "DEVBOULE_SOCKET_PATH";
/// Devboule session id the child must claim when announcing.
pub const SESSION_ID: &str = "DEVBOULE_SESSION_ID";
/// Path of this daemon binary, when it can be resolved.
pub const BIN_PATH: &str = "DEVBOULE_BIN_PATH";
/// Workspace id, when the session has one.
pub const WORKSPACE_ID: &str = "DEVBOULE_WORKSPACE_ID";

/// Agent-session markers a new session must not inherit from whoever launched
/// the daemon: with any of these set, the child agent behaves as a nested
/// session of the outer agent instead of a fresh one.
const OUTER_AGENT_IDENTITY_VARS: &[&str] = &[
    "CODEX_THREAD_ID",
    "OMPCODE",
    "CLAUDECODE",
    "CLAUDE_CODE_CHILD_SESSION",
    "CLAUDE_CODE_SESSION_ID",
    "CLAUDE_CODE_MESSAGING_TOKEN",
];

/// Outer-terminal handles a new session must not inherit: they name the
/// window, pane or multiplexer holding the daemon, never the child's own.
/// TERM_PROGRAM* are stripped, not replaced: we hand over a ConPTY, we are
/// not the terminal.
const OUTER_TERMINAL_IDENTITY_VARS: &[&str] = &[
    "ITERM_SESSION_ID",
    "LC_TERMINAL",
    "LC_TERMINAL_VERSION",
    "TERM_PROGRAM",
    "TERM_PROGRAM_VERSION",
    "WEZTERM_PANE",
    "KITTY_WINDOW_ID",
    "WT_SESSION",
    "TMUX",
    "TMUX_PANE",
    "STY",
    "ZELLIJ",
    "ZELLIJ_SESSION_NAME",
    "ZELLIJ_PANE_ID",
];

/// Every inherited identity marker a session child must not receive, in one
/// place so the two overlays and the tests cannot drift apart.
fn outer_identity_vars() -> impl Iterator<Item = &'static str> {
    OUTER_AGENT_IDENTITY_VARS
        .iter()
        .chain(OUTER_TERMINAL_IDENTITY_VARS)
        .copied()
}

/// Pure view over the same lists the overlays remove: true for an inherited
/// marker, false for anything the child may keep.
#[cfg(test)]
fn is_outer_identity_var(name: &str) -> bool {
    OUTER_AGENT_IDENTITY_VARS.contains(&name) || OUTER_TERMINAL_IDENTITY_VARS.contains(&name)
}

/// Overlay explicit session env over a scrubbed base for a piped (non-PTY)
/// agent child. Removal runs first, so an explicit entry can still opt back
/// into an intentional nested identity.
pub(crate) fn overlay_piped_child_env(
    command: &mut std::process::Command,
    env: &[(String, String)],
) {
    for key in outer_identity_vars() {
        command.env_remove(key);
    }
    for (key, value) in env {
        command.env(key, value);
    }
}

/// Overlay explicit session env over a scrubbed base for a PTY child, with
/// the same removal-first rule as [`overlay_piped_child_env`].
pub(crate) fn overlay_pty_child_env(
    builder: &mut portable_pty::CommandBuilder,
    env: &[(String, String)],
) {
    for key in outer_identity_vars() {
        builder.env_remove(key);
    }
    for (key, value) in env {
        builder.env(key, value);
    }
}

pub fn inject_session_env(
    command: &mut PtyCommand,
    session_id: &str,
    workspace_id: Option<&str>,
    paths: &RuntimePaths,
) {
    upsert_env(&mut command.env, ENV_MARKER, ENV_MARKER_VALUE);
    // The address the child reopens to reach the daemon: the named pipe on
    // Windows, the domain socket on Unix.
    #[cfg(windows)]
    let address = paths.pipe_name.clone();
    #[cfg(unix)]
    let address = paths.socket_path.to_string_lossy().into_owned();
    #[cfg(all(not(windows), not(unix)))]
    let address = paths.pipe_name.clone();
    upsert_env(&mut command.env, SOCKET_PATH, &address);
    upsert_env(&mut command.env, SESSION_ID, session_id);
    if let Some(workspace_id) = workspace_id {
        if !workspace_id.is_empty() {
            upsert_env(&mut command.env, WORKSPACE_ID, workspace_id);
        }
    }
    if let Ok(executable) = std::env::current_exe() {
        upsert_env(&mut command.env, BIN_PATH, &executable.to_string_lossy());
    }
}

fn upsert_env(env: &mut Vec<(String, String)>, key: &str, value: &str) {
    if let Some(existing) = env.iter_mut().find(|(name, _)| name == key) {
        existing.1 = value.to_string();
        return;
    }
    env.push((key.to_string(), value.to_string()));
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::session::PtyCommand;

    fn seed_outer_identity(command: &mut std::process::Command) {
        for key in outer_identity_vars() {
            command.env(key, "outer-session");
        }
    }

    fn outer_keys() -> Vec<&'static str> {
        outer_identity_vars().collect()
    }

    #[test]
    fn outer_identity_predicate_matches_the_removal_lists() {
        for key in outer_keys() {
            assert!(
                is_outer_identity_var(key),
                "{key} must count as outer identity"
            );
        }
        assert!(is_outer_identity_var("TERM_PROGRAM"));
        assert!(is_outer_identity_var("TERM_PROGRAM_VERSION"));
        for key in [
            "KEEP",
            "PATH",
            "ANTHROPIC_API_KEY",
            "DISPLAY",
            "DEVBOULE_SESSION_ID",
        ] {
            assert!(!is_outer_identity_var(key), "{key} must survive the scrub");
        }
    }

    fn piped_has(command: &std::process::Command, key: &str) -> bool {
        // `get_envs` also yields explicit removals as `(key, None)`; only a
        // `Some` value reaches the child.
        command
            .get_envs()
            .any(|(name, value)| name == key && value.is_some())
    }

    #[test]
    fn piped_child_sheds_outer_identity_but_keeps_explicit_env() {
        let mut command = std::process::Command::new("probe.exe");
        seed_outer_identity(&mut command);
        // Seeded outside the list loop so the asserts below stay
        // non-vacuous if the names ever leave the removal list.
        command.env("TERM_PROGRAM", "vscode");
        command.env("TERM_PROGRAM_VERSION", "outer-version");
        overlay_piped_child_env(
            &mut command,
            &[
                ("KEEP".to_string(), "yes".to_string()),
                ("ANTHROPIC_API_KEY".to_string(), "fake-key".to_string()),
                ("DISPLAY".to_string(), ":42".to_string()),
            ],
        );
        for key in outer_keys() {
            assert!(
                !piped_has(&command, key),
                "{key} must not leak into the child"
            );
        }
        assert!(piped_has(&command, "KEEP"));
        assert!(piped_has(&command, "ANTHROPIC_API_KEY"));
        assert!(piped_has(&command, "DISPLAY"));
        assert!(!piped_has(&command, "TERM_PROGRAM"));
        assert!(!piped_has(&command, "TERM_PROGRAM_VERSION"));
    }

    #[test]
    fn piped_child_explicit_env_opts_back_into_outer_identity() {
        let mut command = std::process::Command::new("probe.exe");
        command.env("CLAUDECODE", "1");
        overlay_piped_child_env(
            &mut command,
            &[("CLAUDECODE".to_string(), "intentional-child".to_string())],
        );
        let value = command
            .get_envs()
            .find(|(name, _)| *name == "CLAUDECODE")
            .and_then(|(_, value)| value)
            .map(|value| value.to_string_lossy().into_owned());
        assert_eq!(value.as_deref(), Some("intentional-child"));
    }

    #[test]
    fn pty_child_sheds_outer_identity_but_keeps_explicit_env() {
        let mut builder = portable_pty::CommandBuilder::new("probe.exe");
        for key in outer_keys() {
            builder.env(key, "outer-session");
        }
        // Seeded outside the list loop so the asserts below stay
        // non-vacuous if the names ever leave the removal list.
        builder.env("TERM_PROGRAM", "vscode");
        builder.env("TERM_PROGRAM_VERSION", "outer-version");
        // Seeding through `env` is faithful: `env_remove` cannot tell a base
        // entry from an explicit one, so this exercises the removal path the
        // inherited block takes, without touching the process environment.
        overlay_pty_child_env(
            &mut builder,
            &[
                ("KEEP".to_string(), "yes".to_string()),
                ("ANTHROPIC_API_KEY".to_string(), "fake-key".to_string()),
                ("DISPLAY".to_string(), ":42".to_string()),
            ],
        );
        for key in outer_keys() {
            assert!(
                builder.get_env(key).is_none(),
                "{key} must not leak into the child"
            );
        }
        assert_eq!(
            builder
                .get_env("KEEP")
                .map(|value| value.to_string_lossy().into_owned()),
            Some("yes".to_string())
        );
        assert!(builder.get_env("ANTHROPIC_API_KEY").is_some());
        assert!(builder.get_env("DISPLAY").is_some());
        assert!(builder.get_env("TERM_PROGRAM").is_none());
        assert!(builder.get_env("TERM_PROGRAM_VERSION").is_none());
    }

    #[test]
    fn pty_child_sheds_outer_terminal_program() {
        let mut builder = portable_pty::CommandBuilder::new("probe.exe");
        builder.env("TERM_PROGRAM", "vscode");
        builder.env("TERM_PROGRAM_VERSION", "outer-version");
        overlay_pty_child_env(&mut builder, &[]);
        assert!(builder.get_env("TERM_PROGRAM").is_none());
        assert!(builder.get_env("TERM_PROGRAM_VERSION").is_none());
    }

    #[test]
    fn pty_child_explicit_env_overrides_terminal_program() {
        let mut builder = portable_pty::CommandBuilder::new("probe.exe");
        overlay_pty_child_env(
            &mut builder,
            &[("TERM_PROGRAM".to_string(), "custom".to_string())],
        );
        assert_eq!(
            builder
                .get_env("TERM_PROGRAM")
                .map(|value| value.to_string_lossy().into_owned()),
            Some("custom".to_string())
        );
    }

    #[test]
    fn injects_marker_pipe_and_session_id_with_devboule_prefix() {
        let paths = RuntimePaths::from_dir(r"C:\tmp\devboule-env-test");
        let mut command = PtyCommand::new(
            "stub.exe",
            Vec::<String>::new(),
            PathBuf::from(r"C:\work"),
            vec![("KEEP".to_string(), "yes".to_string())],
        );
        inject_session_env(&mut command, "s.client.1", Some("ws-9"), &paths);
        let env: std::collections::BTreeMap<_, _> = command.env.into_iter().collect();
        assert_eq!(env.get("KEEP").map(String::as_str), Some("yes"));
        assert_eq!(env.get(ENV_MARKER).map(String::as_str), Some("1"));
        #[cfg(windows)]
        let expected_address = paths.pipe_name.clone();
        #[cfg(unix)]
        let expected_address = paths.socket_path.to_string_lossy().into_owned();
        #[cfg(all(not(windows), not(unix)))]
        let expected_address = paths.pipe_name.clone();
        assert_eq!(env.get(SOCKET_PATH), Some(&expected_address));
        assert_eq!(env.get(SESSION_ID).map(String::as_str), Some("s.client.1"));
        assert_eq!(env.get(WORKSPACE_ID).map(String::as_str), Some("ws-9"));
        assert!(env.contains_key(BIN_PATH));
        for key in env.keys() {
            if key != "KEEP" {
                assert!(
                    key.starts_with("DEVBOULE_"),
                    "injected env must use DEVBOULE_ prefix, got {key}"
                );
            }
        }
    }
}
