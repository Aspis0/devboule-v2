//! Environment injected into every session child so an agent (or our stub)
//! can find the daemon pipe and name the session it is running in.
//!
//! Shape taken from herdr `pane.rs` `apply_pane_launch_env` and
//! `integration/env.rs` `apply_pane_base_env` (Apache-2.0, commit 3150bd9).
//! Names use the `DEVBOULE_` prefix; values are ours.

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

pub fn inject_session_env(
    command: &mut PtyCommand,
    session_id: &str,
    workspace_id: Option<&str>,
    paths: &RuntimePaths,
) {
    upsert_env(&mut command.env, ENV_MARKER, ENV_MARKER_VALUE);
    upsert_env(&mut command.env, SOCKET_PATH, &paths.pipe_name);
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
        assert_eq!(
            env.get(SOCKET_PATH).map(String::as_str),
            Some(paths.pipe_name.as_str())
        );
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
