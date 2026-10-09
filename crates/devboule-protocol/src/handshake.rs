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
    /// The effective plugin payload budget. Present only on host→plugin
    /// hellos. It travels through this existing handshake so the backend can
    /// configure its Framed after the small bootstrap frame; it is not a
    /// capability grant and therefore does not belong in `grants`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plugin_payload_bytes: Option<u64>,
    /// Whether this daemon holds at least one workspace, on a peer-to-peer
    /// hello. Present in v32 and absent in v30 — the only legal absence,
    /// because a v30 peer has no way to state it.
    ///
    /// The far daemon binds the bit to the device id its Noise handshake
    /// verified and uses it to choose that connection's session scope. It is
    /// service presence, not a role and not a grant: the receiving daemon
    /// never accepts it from a caller-supplied owner label, and a device with
    /// no workspace still pairs, still connects and still receives the sends
    /// its grant allows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_host: Option<bool>,
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
            plugin_payload_bytes: None,
            workspace_host: None,
        }
    }

    /// A daemon's hello to another daemon over a peer link. Same wire type as
    /// [`Self::m3a`], with the one field a peer conversation needs that an
    /// app's does not: this daemon's workspace-host presence, computed from
    /// its own workspace database.
    pub fn peer(owner: OwnerId, client_name: impl Into<String>, workspace_host: bool) -> Self {
        let mut hello = Self {
            workspace_host: Some(workspace_host),
            ..Self::m3a(owner, client_name)
        };
        // The presence bit and the advertised service are one fact: a peer
        // that does not host workspaces does not offer the service, and the
        // receiver refuses a hello where the two disagree. Keeping the two in
        // step here makes every hello this build sends consistent by
        // construction.
        if !workspace_host {
            hello
                .capabilities
                .retain(|capability| capability.as_str() != crate::caps::HOSTED_WORKSPACES);
        }
        hello
    }

    /// Host→plugin-backend hello. Same wire type as [`Self::m3a`]; a
    /// different capability set and the grants map.
    pub fn plugin_host(
        owner: OwnerId,
        client_name: impl Into<String>,
        capabilities: Vec<Capability>,
        grants: BTreeMap<String, String>,
        plugin_payload_bytes: usize,
    ) -> Self {
        Self {
            protocol_version: PROTOCOL_VERSION,
            min_protocol_version: PROTOCOL_MIN_VERSION,
            client_name: client_name.into(),
            client_version: env!("CARGO_PKG_VERSION").to_string(),
            capabilities,
            owner,
            grants,
            plugin_payload_bytes: Some(plugin_payload_bytes as u64),
            workspace_host: None,
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
    /// This daemon's workspace-host presence, on a peer-to-peer hello. Absent
    /// in a v30 hello and on the plugin-backend hello, which serves no
    /// workspace database of its own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_host: Option<bool>,
}

impl DaemonHello {
    /// State this daemon's workspace-host presence for a peer to bind to its
    /// verified device id.
    pub fn with_workspace_host(mut self, workspace_host: bool) -> Self {
        self.workspace_host = Some(workspace_host);
        // Same rule as [`ClientHello::peer`]: the bit and the service name are
        // one fact, and a daemon with no workspace does not advertise it.
        if !workspace_host {
            self.capabilities
                .retain(|capability| capability.as_str() != crate::caps::HOSTED_WORKSPACES);
        }
        self
    }

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
            workspace_host: None,
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
    use crate::capability::Capability;
    use crate::ids::OwnerId;
    use crate::{PROTOCOL_MIN_VERSION, PROTOCOL_VERSION};
    use std::collections::BTreeMap;

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
            plugin_payload_bytes: None,
            workspace_host: None,
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
            workspace_host: None,
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

    /// The workspace-host presence bit rides both v32 hellos and is absent
    /// from a v30 one: absence is the v30 dialect's only legal state, and the
    /// daemon's peer path is what reads it — and, from the first dialect that
    /// sends the bit, requires it.
    #[test]
    fn the_workspace_host_bit_is_present_in_a_v32_hello_and_absent_in_a_v30_one() {
        let hosted = ClientHello::peer(owner(), "devboule-daemon", true);
        let value = serde_json::to_value(&hosted).expect("json");
        assert_eq!(value["workspaceHost"], true);
        assert_eq!(value["protocolVersion"], PROTOCOL_VERSION);
        let plain = ClientHello::m3a(owner(), "devboule-app");
        let plain_value = serde_json::to_value(&plain).expect("json");
        assert!(
            plain_value.get("workspaceHost").is_none(),
            "an app hello carries no host presence: {plain_value}"
        );
        // An older hello decodes with the bit absent, which is the one state a
        // negotiated v30 peer can be in. The version is spelled, not derived:
        // `PROTOCOL_VERSION - 1` is the dialect a queued branch owns, not v30.
        let v30_json = serde_json::json!({
            "protocolVersion": 30,
            "minProtocolVersion": PROTOCOL_MIN_VERSION,
            "clientName": "devboule-daemon",
            "clientVersion": "0.1.0",
            "capabilities": [],
            "owner": {"user": "S-1-5-21-1-2-3-1001", "client": "peer_1"},
        });
        let decoded: ClientHello = serde_json::from_value(v30_json).expect("a v30 hello decodes");
        assert_eq!(decoded.workspace_host, None);
        let hosted_daemon =
            DaemonHello::with_workspace_host(daemon(PROTOCOL_VERSION, PROTOCOL_MIN_VERSION), false);
        assert_eq!(hosted_daemon.workspace_host, Some(false));
    }

    /// The deposit name is the whole of what tells a client this daemon accepts
    /// a `SessionDeposit`, so it has to survive the intersection the handshake
    /// takes and not merely sit in one list.
    #[test]
    fn the_agreed_capabilities_carry_the_attachment_deposit_name() {
        let agreed = negotiate(
            &client(PROTOCOL_VERSION, PROTOCOL_MIN_VERSION),
            &daemon(PROTOCOL_VERSION, PROTOCOL_MIN_VERSION),
        )
        .expect("overlap");
        assert!(agreed
            .capabilities
            .iter()
            .any(|capability| capability.as_str() == crate::caps::ATTACHMENTS_DEPOSIT));
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
        assert!(err.message.contains("reinstall the app"));
    }

    #[test]
    fn daemon_newer_no_overlap_tells_client_to_update_app() {
        let err = negotiate(&client(1, 1), &daemon(2, 2)).unwrap_err();
        assert_eq!(err.code, ErrorCode::ProtocolVersionMismatch);
        assert!(err.message.contains("daemon is newer"));
        assert!(err.message.contains("Update the app"));
    }

    /// A peer older than the floor must fail at the handshake in both directions,
    /// before the queue can mistake a missing reading for an idle session. Reverting
    /// the floor's guard makes both `unwrap_err()`s panic.
    #[test]
    fn current_crate_refuses_pre_floor_peer() {
        let older = PROTOCOL_MIN_VERSION - 1;
        let err = negotiate(
            &client(PROTOCOL_VERSION, PROTOCOL_MIN_VERSION),
            &daemon(older, older),
        )
        .unwrap_err();
        assert_eq!(err.code, ErrorCode::ProtocolVersionMismatch);
        assert!(err.message.contains("daemon is older"));
        assert!(err.message.contains("Update the daemon"));
        assert!(err.message.contains("reinstall the app"));

        let err = negotiate(
            &client(older, older),
            &daemon(PROTOCOL_VERSION, PROTOCOL_MIN_VERSION),
        )
        .unwrap_err();
        assert_eq!(err.code, ErrorCode::ProtocolVersionMismatch);
        assert!(err.message.contains("daemon is newer"));
        assert!(err.message.contains("Update the app"));
    }

    /// 17 added only optional output-only fields, so the current crate and a
    /// floor peer still speak: the older side keeps its version.
    #[test]
    fn current_crate_still_speaks_with_a_floor_peer() {
        let agreed = negotiate(
            &client(PROTOCOL_VERSION, PROTOCOL_MIN_VERSION),
            &daemon(PROTOCOL_MIN_VERSION, PROTOCOL_MIN_VERSION),
        )
        .expect("the floor peer still overlaps");
        assert_eq!(agreed.protocol_version, PROTOCOL_MIN_VERSION);

        let agreed = negotiate(
            &client(PROTOCOL_MIN_VERSION, PROTOCOL_MIN_VERSION),
            &daemon(PROTOCOL_VERSION, PROTOCOL_MIN_VERSION),
        )
        .expect("the floor daemon still overlaps");
        assert_eq!(agreed.protocol_version, PROTOCOL_MIN_VERSION);
    }

    #[test]
    fn hello_uses_camel_case() {
        let hello = ClientHello::m3a(owner(), "devboule-app");
        let value = serde_json::to_value(&hello).expect("json");
        assert_eq!(value["protocolVersion"], PROTOCOL_VERSION);
        assert_eq!(value["minProtocolVersion"], PROTOCOL_MIN_VERSION);
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
            crate::DEFAULT_PLUGIN_PAYLOAD_BYTES,
        );
        let value = serde_json::to_value(&hello).expect("json");
        assert_eq!(value["grants"]["workspace.root"], r"C:\repo");
        assert_eq!(
            value["pluginPayloadBytes"],
            crate::DEFAULT_PLUGIN_PAYLOAD_BYTES
        );

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
            crate::DEFAULT_PLUGIN_PAYLOAD_BYTES,
        );
        let backend = DaemonHello::plugin_backend("plugin-1", 9);
        let agreed = negotiate(&hello, &backend).expect("overlap");
        assert_eq!(
            agreed.capabilities,
            vec![Capability::new(crate::caps::WORKSPACE_ROOT)]
        );
    }
}
