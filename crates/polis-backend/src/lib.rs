//! Polis plugin backend. The city graph and Augur will live here; today it
//! serves the host-granted workspace city over the plugin pipe.

use std::collections::BTreeMap;

use devboule_plugin_rpc::unix_millis;
use devboule_protocol::{
    caps, invoke_method_capability, Capability, ClientMessage, DaemonMessage, ErrorCode, WireError,
    WorkspaceRootBody,
};

/// Leave room for the serialized InvokeResult envelope and small protocol
/// changes. The backend refuses before the pipe writer attempts a 1 MiB frame.
pub const CITY_FRAME_ENVELOPE_MARGIN_BYTES: usize = 4096;

pub(crate) fn city_response_within_frame(value: &serde_json::Value) -> bool {
    let Ok(serialized) = serde_json::to_vec(value) else {
        return false;
    };
    serialized.len()
        <= devboule_protocol::MAX_FRAME_BYTES.saturating_sub(CITY_FRAME_ENVELOPE_MARGIN_BYTES)
}

fn city_response_too_large_error(id: u64) -> DaemonMessage {
    DaemonMessage::Error(
        WireError::new(
            ErrorCode::InvalidRequest,
            format!(
                "plugin response is too large (maximum 1 MiB; {} bytes reserved for the frame envelope)",
                CITY_FRAME_ENVELOPE_MARGIN_BYTES
            ),
        )
        .with_id(id),
    )
}
mod city;
mod findings;
pub use city::{build_city, CityBuildError, MAX_CITY_FILES, MAX_CITY_FILE_BYTES};
pub use findings::{get_findings, inspect_finding, FindingsError, InspectError};

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
        ClientMessage::Invoke { id, method, payload } => {
            dispatch_invoke(grants, granted, id, &method, payload)
        }
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
    payload: Option<serde_json::Value>,
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
    maybe_hang_for_crash_test();
    if method == caps::WORKSPACE_ROOT {
        let root = grants
            .get(caps::WORKSPACE_ROOT)
            .cloned()
            .unwrap_or_default();
        let body = WorkspaceRootBody::ok(root);
        match serde_json::to_value(body) {
            Ok(value) => DaemonMessage::InvokeResult { id, value },
            Err(error) => DaemonMessage::Error(
                WireError::new(ErrorCode::Internal, error.to_string()).with_id(id),
            ),
        }
    } else if method == caps::CITY_GET {
        let Some(root) = grants.get(caps::WORKSPACE_ROOT) else {
            return DaemonMessage::Error(
                WireError::new(
                    ErrorCode::WorkspaceUnavailable,
                    "city.get requires the host-granted workspace.root",
                )
                .with_id(id),
            );
        };
        match build_city(std::path::Path::new(root)) {
            Ok(value) if !city_response_within_frame(&value) => city_response_too_large_error(id),
            Ok(value) => DaemonMessage::InvokeResult { id, value },
            Err(error) => {
                DaemonMessage::Error(WireError::new(ErrorCode::Io, error.to_string()).with_id(id))
            }
        }
    } else if method == caps::FINDINGS_GET {
        let Some(root) = grants.get(caps::WORKSPACE_ROOT) else {
            return DaemonMessage::Error(
                WireError::new(
                    ErrorCode::WorkspaceUnavailable,
                    "findings.get requires the host-granted workspace.root",
                )
                .with_id(id),
            );
        };
        match get_findings(std::path::Path::new(root)) {
            Ok(value) if !city_response_within_frame(&value) => city_response_too_large_error(id),
            Ok(value) => DaemonMessage::InvokeResult { id, value },
            Err(error) => {
                DaemonMessage::Error(WireError::new(ErrorCode::Io, error.to_string()).with_id(id))
            }
        }
    } else if method == caps::FINDING_INSPECT {
        let Some(root) = grants.get(caps::WORKSPACE_ROOT) else {
            return DaemonMessage::Error(
                WireError::new(
                    ErrorCode::WorkspaceUnavailable,
                    "finding.inspect requires the host-granted workspace.root",
                )
                .with_id(id),
            );
        };
        match inspect_finding(std::path::Path::new(root), payload.as_ref()) {
            Ok(value) if !city_response_within_frame(&value) => city_response_too_large_error(id),
            Ok(value) => DaemonMessage::InvokeResult { id, value },
            Err(InspectError::InvalidId) => DaemonMessage::Error(
                WireError::new(ErrorCode::InvalidRequest, InspectError::InvalidId.to_string())
                    .with_id(id),
            ),
            Err(InspectError::NotFound) => DaemonMessage::Error(
                WireError::new(ErrorCode::InvalidRequest, InspectError::NotFound.to_string())
                    .with_id(id),
            ),
            Err(error) => {
                DaemonMessage::Error(WireError::new(ErrorCode::Io, error.to_string()).with_id(id))
            }
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

    #[test]
    fn findings_get_returns_mapped_findings_when_workspace_is_granted() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join("src")).unwrap();
        std::fs::write(root.path().join("src/lib.rs"), "pub fn x() {}\n").unwrap();
        let mut grants = BTreeMap::new();
        grants.insert(
            caps::WORKSPACE_ROOT.to_string(),
            root.path().to_string_lossy().into_owned(),
        );
        let granted = vec![
            Capability::new(caps::PING),
            Capability::new(caps::WORKSPACE_ROOT),
            Capability::new(caps::FINDINGS_GET),
        ];
        let reply = dispatch(
            &grants,
            &granted,
            ClientMessage::Invoke {
                id: 8,
                method: caps::FINDINGS_GET.to_string(),
                payload: None,
            },
        );
        match reply {
            DaemonMessage::InvokeResult { id, value } => {
                assert_eq!(id, 8);
                assert_eq!(value["scanned"], true);
                assert!(value["completed"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|item| item == "secrets"));
                assert!(value["completed"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|item| item == "untested"));
                assert!(!value["completed"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|item| item == "clippy"));
                assert!(value["findings"].as_array().is_some());
            }
            other => panic!("expected findings result, got {other:?}"),
        }
    }

    #[test]
    fn finding_inspect_requires_workspace_root() {
        let grants = BTreeMap::new();
        let granted = vec![
            Capability::new(caps::PING),
            Capability::new(caps::FINDING_INSPECT),
        ];
        let reply = dispatch(
            &grants,
            &granted,
            ClientMessage::Invoke {
                id: 9,
                method: caps::FINDING_INSPECT.to_string(),
                payload: Some(serde_json::json!({"id": "a".repeat(64)})),
            },
        );
        match reply {
            DaemonMessage::Error(error) => {
                assert_eq!(error.id, Some(9));
                assert_eq!(error.code, ErrorCode::WorkspaceUnavailable);
            }
            other => panic!("expected workspace error, got {other:?}"),
        }
    }

    #[test]
    fn finding_inspect_refuses_a_malformed_id() {
        let root = tempfile::tempdir().unwrap();
        let mut grants = BTreeMap::new();
        grants.insert(
            caps::WORKSPACE_ROOT.to_string(),
            root.path().to_string_lossy().into_owned(),
        );
        let granted = vec![
            Capability::new(caps::PING),
            Capability::new(caps::WORKSPACE_ROOT),
            Capability::new(caps::FINDING_INSPECT),
        ];
        let reply = dispatch(
            &grants,
            &granted,
            ClientMessage::Invoke {
                id: 10,
                method: caps::FINDING_INSPECT.to_string(),
                payload: Some(serde_json::json!({"id": "nope"})),
            },
        );
        match reply {
            DaemonMessage::Error(error) => {
                assert_eq!(error.code, ErrorCode::InvalidRequest);
                assert_eq!(error.message, "finding.inspect requires a 64-hex id");
            }
            other => panic!("expected invalid id, got {other:?}"),
        }
    }

    #[test]
    fn city_get_returns_host_city_when_workspace_is_granted() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("README.md"), "hello\n").unwrap();
        let mut grants = BTreeMap::new();
        grants.insert(
            caps::WORKSPACE_ROOT.to_string(),
            root.path().to_string_lossy().into_owned(),
        );
        let granted = vec![
            Capability::new(caps::PING),
            Capability::new(caps::WORKSPACE_ROOT),
            Capability::new(caps::CITY_GET),
        ];
        let reply = dispatch(
            &grants,
            &granted,
            ClientMessage::Invoke {
                id: 5,
                method: caps::CITY_GET.to_string(),
                payload: None,
            },
        );
        match reply {
            DaemonMessage::InvokeResult { id, value } => {
                assert_eq!(id, 5);
                assert_eq!(value["dataSource"], "host");
                assert_eq!(value["agents"], serde_json::json!([]));
                assert_eq!(value["findings"], serde_json::json!([]));
            }
            other => panic!("expected city result, got {other:?}"),
        }
    }

    #[test]
    fn city_response_guard_rejects_a_value_just_over_the_frame_budget() {
        let value = serde_json::Value::String(
            "x".repeat(devboule_protocol::MAX_FRAME_BYTES - CITY_FRAME_ENVELOPE_MARGIN_BYTES + 1),
        );
        assert!(!city_response_within_frame(&value));
        match city_response_too_large_error(7) {
            DaemonMessage::Error(error) => {
                assert_eq!(error.id, Some(7));
                assert_eq!(error.code, ErrorCode::InvalidRequest);
                assert!(error.message.contains("plugin response is too large"));
            }
            other => panic!("expected oversize refusal, got {other:?}"),
        }
    }
}
