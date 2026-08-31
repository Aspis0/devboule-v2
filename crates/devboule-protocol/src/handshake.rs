//! Bidirectional handshake and the version-overlap rule.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::capability::{intersect_capabilities, Capability};
use crate::error::{ErrorCode, ErrorDetails, WireError};
use crate::ids::OwnerId;
use crate::{PROTOCOL_MIN_VERSION, PROTOCOL_VERSION};

/// First client frame. States what the client speaks, not what it wishes the
/// daemon were.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ClientHello {
    pub protocol_version: u32,
    pub min_protocol_version: u32,
    pub client_name: String,
    pub client_version: String,
    pub capabilities: Vec<Capability>,
    pub owner: OwnerId,
    /// Capability values the host grants this peer. Empty on the app↔daemon
    /// conversation. A plugin backend reads `workspace.root` here: the
    /// confined project root, chosen by the host, never by the plugin.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub grants: BTreeMap<String, String>,
}

impl ClientHello {
    pub fn m3a(owner: OwnerId, client_name: impl Into<String>) -> Self {
        Self {
            protocol_version: PROTOCOL_VERSION,
            min_protocol_version: PROTOCOL_MIN_VERSION,
            client_name: client_name.into(),
            client_version: env!("CARGO_PKG_VERSION").to_string(),
            capabilities: crate::m3a_client_capabilities(),
            owner,
            grants: BTreeMap::new(),
        }
    }

    /// Host→plugin-backend hello. Same wire type as [`Self::m3a`]; a
    /// different capability set and the grants map.
    pub fn plugin_host(
        owner: OwnerId,
        client_name: impl Into<String>,
        capabilities: Vec<Capability>,
        grants: BTreeMap<String, String>,
    ) -> Self {
        Self {
            protocol_version: PROTOCOL_VERSION,
            min_protocol_version: PROTOCOL_MIN_VERSION,
            client_name: client_name.into(),
            client_version: env!("CARGO_PKG_VERSION").to_string(),
            capabilities,
            owner,
            grants,
        }
    }
}

/// First daemon frame after a successful hello. States what this process
/// supports. `instance_id` changes every daemon process so a reconnecting
/// client can tell a replacement from a resume of the same instance.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DaemonHello {
    pub protocol_version: u32,
    pub min_protocol_version: u32,
    pub daemon_version: String,
    pub instance_id: String,
    pub pid: u32,
    pub capabilities: Vec<Capability>,
}

impl DaemonHello {
    /// Plugin-backend first reply after a successful hello. Same type as
    /// the daemon's hello so a pipe client can reuse framing.
    pub fn plugin_backend(instance_id: impl Into<String>, pid: u32) -> Self {
        Self {
            protocol_version: PROTOCOL_VERSION,
            min_protocol_version: PROTOCOL_MIN_VERSION,
            daemon_version: env!("CARGO_PKG_VERSION").to_string(),
            instance_id: instance_id.into(),
            pid,
            capabilities: crate::plugin_backend_capabilities(),
        }
    }
}

/// Result of [`negotiate`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Negotiation {
    pub protocol_version: u32,
    pub capabilities: Vec<Capability>,
}

/// Apply the overlap rule. Pure: no I/O.
pub fn negotiate(client: &ClientHello, daemon: &DaemonHello) -> Result<Negotiation, WireError> {
    if client.min_protocol_version > client.protocol_version {
        return Err(WireError::new(
            ErrorCode::InvalidRequest,
            "client min_protocol_version is greater than protocol_version",
        ));
    }
    let agreed = client.protocol_version.min(daemon.protocol_version);
    if agreed < client.min_protocol_version || agreed < daemon.min_protocol_version {
        return Err(version_mismatch(client, daemon));
    }
    Ok(Negotiation {
        protocol_version: agreed,
        capabilities: intersect_capabilities(&client.capabilities, &daemon.capabilities),
    })
}

fn version_mismatch(client: &ClientHello, daemon: &DaemonHello) -> WireError {
    let message = if client.min_protocol_version > daemon.protocol_version {
        format!(
            "protocol mismatch: the daemon is older (speaks {}–{}, pid {}) and this client requires at least {}. Update the daemon (or reinstall the app so the matching daemon binary is next to it).",
            daemon.min_protocol_version,
            daemon.protocol_version,
            daemon.pid,
            client.min_protocol_version
        )
    } else {
        format!(
            "protocol mismatch: the daemon is newer (speaks {}–{}, pid {}) and this client speaks {}–{}. Update the app.",
            daemon.min_protocol_version,
            daemon.protocol_version,
            daemon.pid,
            client.min_protocol_version,
            client.protocol_version
        )
    };
    WireError {
        id: None,
        code: ErrorCode::ProtocolVersionMismatch,
        message,
        details: Some(ErrorDetails::VersionMismatch {
            client: client.protocol_version,
            client_min: client.min_protocol_version,
            daemon: daemon.protocol_version,
            daemon_min: daemon.min_protocol_version,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use crate::capability::Capability;
    use crate::ids::OwnerId;

    fn owner() -> OwnerId {
        OwnerId::new("S-1-5-21-1-2-3-1001", "app-1").expect("owner")
    }

    fn client(version: u32, min: u32) -> ClientHello {
        ClientHello {
            protocol_version: version,
            min_protocol_version: min,
            client_name: "test".to_string(),
            client_version: "0.1.0".to_string(),
            capabilities: crate::m3a_client_capabilities(),
            owner: owner(),
            grants: BTreeMap::new(),
        }
    }

    fn daemon(version: u32, min: u32) -> DaemonHello {
        DaemonHello {
            protocol_version: version,
            min_protocol_version: min,
            daemon_version: "0.1.0".to_string(),
            instance_id: "1-abc".to_string(),
            pid: 42,
            capabilities: crate::m3a_daemon_capabilities(),
        }
    }

    #[test]
    fn equal_v1_succeeds() {
        let agreed = negotiate(&client(1, 1), &daemon(1, 1)).expect("overlap");
        assert_eq!(agreed.protocol_version, 1);
        assert!(agreed
            .capabilities
            .iter()
            .any(|capability| capability.as_str() == crate::caps::PING));
    }

    #[test]
    fn daemon_newer_overlapping_speaks_app_version() {
        let agreed = negotiate(&client(1, 1), &daemon(2, 1)).expect("overlap");
        assert_eq!(agreed.protocol_version, 1);
    }

    #[test]
    fn daemon_older_overlapping_speaks_daemon_version() {
        let agreed = negotiate(&client(2, 1), &daemon(1, 1)).expect("overlap");
        assert_eq!(agreed.protocol_version, 1);
    }

    #[test]
    fn daemon_older_no_overlap_tells_client_to_update_daemon() {
        let err = negotiate(&client(2, 2), &daemon(1, 1)).unwrap_err();
        assert_eq!(err.code, ErrorCode::ProtocolVersionMismatch);
        assert!(err.message.contains("daemon is older"));
        assert!(err.message.contains("Update the daemon"));
    }

    #[test]
    fn daemon_newer_no_overlap_tells_client_to_update_app() {
        let err = negotiate(&client(1, 1), &daemon(2, 2)).unwrap_err();
        assert_eq!(err.code, ErrorCode::ProtocolVersionMismatch);
        assert!(err.message.contains("daemon is newer"));
        assert!(err.message.contains("Update the app"));
    }

    #[test]
    fn hello_uses_camel_case() {
        let hello = ClientHello::m3a(owner(), "devboule-app");
        let value = serde_json::to_value(&hello).expect("json");
        assert_eq!(value["protocolVersion"], 1);
        assert_eq!(value["minProtocolVersion"], 1);
        assert_eq!(value["clientName"], "devboule-app");
        assert!(value["owner"]["user"].is_string());
        assert!(
            value.get("grants").is_none(),
            "empty grants must stay off the daemon wire"
        );
    }

    #[test]
    fn plugin_hello_carries_workspace_root_grant() {
        let mut grants = BTreeMap::new();
        grants.insert(
            crate::caps::WORKSPACE_ROOT.to_string(),
            r"C:\repo".to_string(),
        );
        let hello = ClientHello::plugin_host(
            owner(),
            "devboule-app",
            crate::plugin_backend_capabilities(),
            grants,
        );
        let value = serde_json::to_value(&hello).expect("json");
        assert_eq!(value["grants"]["workspace.root"], r"C:\repo");

        let backend = DaemonHello::plugin_backend("plugin-1", 9);
        let agreed = negotiate(&hello, &backend).expect("overlap");
        assert!(agreed
            .capabilities
            .iter()
            .any(|capability| capability.as_str() == crate::caps::WORKSPACE_ROOT));
        assert!(!agreed
            .capabilities
            .iter()
            .any(|capability| capability.as_str() == crate::caps::SESSIONS));
    }

    #[test]
    fn plugin_handshake_drops_capabilities_the_backend_does_not_serve() {
        let hello = ClientHello::plugin_host(
            owner(),
            "devboule-app",
            vec![
                Capability::new(crate::caps::WORKSPACE_ROOT),
                Capability::new(crate::caps::ORACLE_SEARCH),
            ],
            BTreeMap::new(),
        );
        let backend = DaemonHello::plugin_backend("plugin-1", 9);
        let agreed = negotiate(&hello, &backend).expect("overlap");
        assert_eq!(
            agreed.capabilities,
            vec![Capability::new(crate::caps::WORKSPACE_ROOT)]
        );
    }
}
