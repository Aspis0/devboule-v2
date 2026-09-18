//! The `devboule_list_devices` tool body: the paired-device discovery half of
//! the broker, answered from this daemon's own `peers` rows.
//!
//! Answers locally — never a dial. The list is scoped to the calling session's
//! own user (the `paired_by_user` each row records at pairing time), and it
//! carries only what an agent needs to *name* a device: id, display name,
//! role, reachable now. The key, the fingerprint, the address, the binding
//! and the pairing user are withheld by not being in the document at all.

use devboule_protocol::{OwnerId, PeerRole};
use serde_json::{json, Value};

use crate::server::ServerState;

/// The paired-device document for the calling owner: `{"devices": [...]}` in
/// catalog order, or the error sentence the broker answers `-32603` with.
pub(crate) fn list_devices_document(
    state: &ServerState,
    caller: &OwnerId,
) -> Result<Value, String> {
    let records = state.peers()?;
    let devices = records
        .iter()
        // Scope: the rows the calling session's own user paired. A row with
        // no pairing user belongs to nobody here — absent is not "everyone" —
        // and a revoked row is a pairing that no longer exists.
        .filter(|record| {
            record.revoked_at.is_none() && record.paired_by_user.as_deref() == Some(&caller.user)
        })
        .map(|record| {
            json!({
                "deviceId": record.device_id,
                "displayName": record.display_name,
                "role": PeerRole::parse(&record.role)
                    .unwrap_or(PeerRole::Daemon)
                    .as_str(),
                "online": state.is_peer_online(&record.device_id),
            })
        })
        .collect::<Vec<_>>();
    Ok(json!({ "devices": devices }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(
        device_id: &str,
        paired_by: Option<String>,
        revoked_at: Option<i64>,
    ) -> crate::journal::PeerRecord {
        crate::journal::PeerRecord {
            device_id: device_id.to_string(),
            display_name: format!("Device {device_id}"),
            role: "daemon".to_string(),
            public_key: vec![7u8; 32],
            paired_by_user: paired_by,
            binding_kind: "tailnet".to_string(),
            binding_stable_id: Some("nstable".to_string()),
            binding_node_name: None,
            binding_login_name: None,
            address: "100.64.0.2:47831".to_string(),
            paired_at: 1,
            revoked_at,
            caps: vec!["view".to_string()],
        }
    }

    fn state_with(rows: &[crate::journal::PeerRecord]) -> std::sync::Arc<ServerState> {
        let state = ServerState::new("mcp-device-roster".to_string());
        for record in rows {
            state.peer_upsert(record.clone()).expect("peer row");
        }
        state
    }

    /// The list carries exactly the four fields an agent needs, for exactly
    /// the rows the calling session's own user paired: another user's rows,
    /// rows with no pairing user, and revoked rows are all absent. Absent is
    /// not "everyone".
    #[test]
    fn the_device_list_is_scoped_to_the_caller_and_names_only_the_four_fields() {
        let caller = OwnerId::new("S-1-5-21-devices", "claude").expect("owner");
        let state = state_with(&[
            row("dev-mine", Some(caller.user.clone()), None),
            row("dev-theirs", Some("S-1-5-21-other".to_string()), None),
            row("dev-anon", None, None),
            row("dev-revoked", Some(caller.user.clone()), Some(99)),
        ]);

        let document = list_devices_document(&state, &caller).expect("document");
        let devices = document["devices"].as_array().expect("devices array");
        assert_eq!(
            devices.len(),
            1,
            "only the caller's own live row: {document}"
        );
        assert_eq!(devices[0]["deviceId"], "dev-mine");
        assert_eq!(devices[0]["displayName"], "Device dev-mine");
        assert_eq!(devices[0]["role"], "daemon");
        assert_eq!(devices[0]["online"], false);

        // Naming, not auditing: none of the withheld facts may appear under
        // any spelling.
        let rendered = serde_json::to_string(&document).expect("render");
        for withheld in [
            "publicKey",
            "keyFingerprint",
            "address",
            "binding",
            "pairedByUser",
            "paired_by_user",
            "caps",
        ] {
            assert!(
                !rendered.contains(withheld),
                "{withheld} must not be in {rendered}"
            );
        }
    }
}
