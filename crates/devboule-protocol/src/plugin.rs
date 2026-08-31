//! Plugin-backend tenant bodies. They travel as [`crate::ClientMessage::Invoke`]
//! / [`crate::DaemonMessage::InvokeResult`] payloads, not as a second framing.

use serde::{Deserialize, Serialize};

/// Successful `workspace.root` answer: the confined root the host granted,
/// plus a ping-style liveness flag. The city graph is not this type.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceRootBody {
    pub root: String,
    pub status: String,
}

impl WorkspaceRootBody {
    pub fn ok(root: impl Into<String>) -> Self {
        Self {
            root: root.into(),
            status: "ok".to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workspace_root_body_is_camel_case() {
        let body = WorkspaceRootBody::ok(r"C:\repo");
        let value = serde_json::to_value(&body).expect("json");
        assert_eq!(value["root"], r"C:\repo");
        assert_eq!(value["status"], "ok");
        assert!(value.get("workspaceRoot").is_none());
    }
}
