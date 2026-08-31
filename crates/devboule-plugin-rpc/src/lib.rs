//! Host↔plugin-backend conversation over the same protocol frames as the daemon.
//!
//! The plugin backend binds a named pipe, the host connects, they handshake
//! with plugin-scoped capabilities, and the host sends [`devboule_protocol::ClientMessage::Invoke`].
//! Process membership is a Windows Job Object with `KILL_ON_JOB_CLOSE` so an
//! orphan cannot outlive the host. Tested by killing the backend mid-request.

mod error;
mod pipe;
mod server;
mod session;
mod spawn;

pub use error::PluginError;
pub use server::{unix_millis, PluginBackend};
pub use session::{method_is_granted, workspace_root_from_value, PluginSession, SpawnSpec};
pub use spawn::{pipe_name_from_env_or_argv, unique_pipe_name, PIPE_ENV, PLUGIN_ID_ENV};

use devboule_protocol::{caps, Capability, OwnerId};
use std::collections::BTreeMap;

/// Capabilities the host is willing to grant this plugin, given what the
/// manifest requested and the confined workspace root (if any).
pub fn granted_capabilities(
    requested: &[String],
    workspace_root: Option<&str>,
) -> (Vec<Capability>, BTreeMap<String, String>) {
    let mut capabilities = vec![Capability::new(caps::PING)];
    let mut grants = BTreeMap::new();
    let requested_root = requested.iter().any(|name| name == caps::WORKSPACE_ROOT);
    if requested_root {
        if let Some(root) = workspace_root {
            capabilities.push(Capability::new(caps::WORKSPACE_ROOT));
            grants.insert(caps::WORKSPACE_ROOT.to_string(), root.to_string());
        }
    }
    (capabilities, grants)
}

pub fn host_owner() -> Result<OwnerId, PluginError> {
    #[cfg(windows)]
    {
        let user = devboule_daemon::current_user_sid()?;
        OwnerId::new(user, "devboule-app").map_err(PluginError::Protocol)
    }
    #[cfg(not(windows))]
    {
        OwnerId::new("unix", "devboule-app").map_err(PluginError::Protocol)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workspace_root_is_granted_only_when_requested_and_known() {
        let (caps, grants) = granted_capabilities(&["workspace.root".into()], Some(r"C:\repo"));
        assert!(caps.iter().any(|cap| cap.as_str() == "workspace.root"));
        assert_eq!(grants.get("workspace.root").map(String::as_str), Some(r"C:\repo"));

        let (caps, grants) = granted_capabilities(&["workspace.root".into()], None);
        assert!(!caps.iter().any(|cap| cap.as_str() == "workspace.root"));
        assert!(grants.is_empty());

        let (caps, grants) = granted_capabilities(&["oracle.search".into()], Some(r"C:\repo"));
        assert!(!caps.iter().any(|cap| cap.as_str() == "workspace.root"));
        assert!(grants.is_empty());
    }
}
