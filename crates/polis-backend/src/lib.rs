//! Polis plugin backend. The city graph and Augur will live here; today it
//! only proves the pipe: handshake, ping, and `workspace.root`.

use std::collections::BTreeMap;

use devboule_plugin_rpc::unix_millis;
use devboule_protocol::{
    caps, invoke_method_capability, Capability, ClientMessage, DaemonMessage, ErrorCode,
    WorkspaceRootBody, WireError,
};

pub fn dispatch(
    grants: &BTreeMap<String, String>,
    granted: &[Capability],
    request: ClientMessage,
) -> DaemonMessage {
    match request {
        ClientMessage::Hello(_) => DaemonMessage::Error(WireError::new(
            ErrorCode::InvalidRequest,
            "hello already completed",
        )),
        ClientMessage::Ping { id } => DaemonMessage::Pong {
            id,
            ts_ms: unix_millis(),
        },
        ClientMessage::Invoke { id, method, .. } => dispatch_invoke(grants, granted, id, &method),
        other => {
            let id = other.request_id();
            let mut error = WireError::new(
                ErrorCode::Unimplemented,
                "polis-backend does not serve that request yet",
            );
            if let Some(id) = id {
                error = error.with_id(id);
            }
            DaemonMessage::Error(error)
        }
    }
}

fn maybe_hang_for_crash_test() {
    if let Ok(ms) = std::env::var("DEVBOULE_PLUGIN_HANG_MS") {
        if let Ok(ms) = ms.parse::<u64>() {
            std::thread::sleep(std::time::Duration::from_millis(ms));
        }
    }
}

fn dispatch_invoke(
    grants: &BTreeMap<String, String>,
    granted: &[Capability],
    id: u64,
    method: &str,
) -> DaemonMessage {
    let capability = invoke_method_capability(method);
    if !granted.iter().any(|item| item.as_str() == capability) {
        return DaemonMessage::Error(
            WireError::new(
                ErrorCode::CapabilityNotSupported,
                format!("capability '{capability}' was not negotiated"),
            )
            .with_id(id),
        );
    }
    if method == caps::WORKSPACE_ROOT {
        maybe_hang_for_crash_test();
        let root = grants.get(caps::WORKSPACE_ROOT).cloned().unwrap_or_default();
        let body = WorkspaceRootBody::ok(root);
        match serde_json::to_value(body) {
            Ok(value) => DaemonMessage::InvokeResult { id, value },
            Err(error) => DaemonMessage::Error(
                WireError::new(ErrorCode::Internal, error.to_string()).with_id(id),
            ),
        }
    } else {
        DaemonMessage::Error(
            WireError::new(
                ErrorCode::Unimplemented,
                format!("polis-backend does not serve '{method}' yet"),
            )
            .with_id(id),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use devboule_protocol::Capability;

    fn granted_root() -> (BTreeMap<String, String>, Vec<Capability>) {
        let mut grants = BTreeMap::new();
        grants.insert(caps::WORKSPACE_ROOT.to_string(), r"C:\repo".to_string());
        (
            grants,
            vec![
                Capability::new(caps::PING),
                Capability::new(caps::WORKSPACE_ROOT),
            ],
        )
    }

    #[test]
    fn workspace_root_echoes_the_grant() {
        let (grants, granted) = granted_root();
        let reply = dispatch(
            &grants,
            &granted,
            ClientMessage::Invoke {
                id: 3,
                method: caps::WORKSPACE_ROOT.to_string(),
                payload: None,
            },
        );
        match reply {
            DaemonMessage::InvokeResult { id, value } => {
                assert_eq!(id, 3);
                assert_eq!(value["root"], r"C:\repo");
                assert_eq!(value["status"], "ok");
            }
            other => panic!("expected invoke_result, got {other:?}"),
        }
    }

    #[test]
    fn ungranted_method_is_refused() {
        let (grants, granted) = granted_root();
        let reply = dispatch(
            &grants,
            &granted,
            ClientMessage::Invoke {
                id: 4,
                method: caps::ORACLE_SEARCH.to_string(),
                payload: None,
            },
        );
        match reply {
            DaemonMessage::Error(error) => {
                assert_eq!(error.id, Some(4));
                assert_eq!(error.code, ErrorCode::CapabilityNotSupported);
            }
            other => panic!("expected capability error, got {other:?}"),
        }
    }
}
