//! Protocol error codes. Stable names; new codes are additive.

use serde::{Deserialize, Serialize};

/// Machine-readable failure. Serialized as a snake_case string.
///
/// Mirrored by the `ErrorCode` union in `src/types/ipc.ts`. Alignment is
/// enforced by `error_code_matches_frontend_union`.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    /// Handshake version ranges do not overlap. Actionable: update app or daemon.
    ProtocolVersionMismatch,
    /// The peer is not the pipe owner. Should not happen if the DACL is set.
    Unauthorized,
    /// The RPC exists in the protocol but this daemon build does not serve it.
    Unimplemented,
    /// The RPC exists but was not in the negotiated capability set.
    CapabilityNotSupported,
    /// Malformed frame, bad id, bad idempotency key, etc.
    InvalidRequest,
    SessionNotFound,
    /// Cursor.generation is not the live instance. Replay would lie.
    SessionGenerationMismatch,
    /// Same idempotency key, different payload.
    IdempotencyConflict,
    /// Daemon is exiting; the client should not retry against this instance.
    ShuttingDown,
    /// Journal is unreadable: corrupt, a future schema, or the disk refused
    /// the write. Live sessions still run; recovered replay cannot.
    Journal,
    /// A workspace-root capability was requested while no project is open.
    WorkspaceUnavailable,
    /// The active workspace root was not safely confined to the project path.
    WorkspaceConfinementRefused,
    Internal,
    Io,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ErrorDetails {
    VersionMismatch {
        client: u32,
        client_min: u32,
        daemon: u32,
        daemon_min: u32,
    },
    GenerationMismatch {
        current: u64,
        requested: u64,
    },
    /// Checkout has local changes. Removal requires `force`. The branch is
    /// not deleted either way.
    WorktreeDirty {
        path: String,
        force_required: bool,
    },
    /// Recorded `git_state` said a worktree was possible; live git disagrees.
    WorktreeGitState {
        recorded: String,
        observed: String,
    },
    /// Git's worktree at this path is not the branch the workspace row means.
    WorktreeMismatch {
        path: String,
        expected_branch: String,
        observed_branch: Option<String>,
    },
    /// The worktree is locked. `--force` does not override a lock.
    WorktreeLocked {
        path: String,
    },
    /// The checkout path is not inside this project's worktree root.
    WorktreeNotConfined {
        path: String,
        root: String,
    },
    /// The project folder is gone, so git cannot run. The row can still be
    /// detached; the checkout is left on disk if it exists.
    WorktreeProjectGone {
        leftover_checkout: Option<String>,
    },
}

/// Error payload used both as a handshake-level first frame (`id` is `None`)
/// and as a request response (`id` is the client's request id).
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct WireError {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<u64>,
    pub code: ErrorCode,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<ErrorDetails>,
}

impl WireError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            id: None,
            code,
            message: message.into(),
            details: None,
        }
    }

    pub fn with_id(mut self, id: u64) -> Self {
        self.id = Some(id);
        self
    }

    pub fn with_details(mut self, details: ErrorDetails) -> Self {
        self.details = Some(details);
        self
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::path::PathBuf;

    use super::*;

    #[test]
    fn error_code_is_snake_case() {
        let value = serde_json::to_value(ErrorCode::ProtocolVersionMismatch).expect("json");
        assert_eq!(value, "protocol_version_mismatch");
        let value = serde_json::to_value(ErrorCode::SessionGenerationMismatch).expect("json");
        assert_eq!(value, "session_generation_mismatch");
        let value = serde_json::to_value(ErrorCode::IdempotencyConflict).expect("json");
        assert_eq!(value, "idempotency_conflict");
        let value = serde_json::to_value(ErrorCode::Journal).expect("json");
        assert_eq!(value, "journal");
    }

    #[test]
    fn worktree_removal_outcomes_are_typed_details() {
        let details = ErrorDetails::WorktreeDirty {
            path: r"C:\w\checkout".to_string(),
            force_required: true,
        };
        let value = serde_json::to_value(&details).expect("json");
        assert_eq!(value["type"], "worktree_dirty");
        assert_eq!(value["force_required"], true);
        let state = ErrorDetails::WorktreeGitState {
            recorded: "repository".to_string(),
            observed: "not_repository".to_string(),
        };
        let value = serde_json::to_value(&state).expect("json");
        assert_eq!(value["type"], "worktree_git_state");
        assert_eq!(value["recorded"], "repository");
        assert_eq!(value["observed"], "not_repository");
        let locked = ErrorDetails::WorktreeLocked {
            path: r"C:\w\locked".to_string(),
        };
        assert_eq!(
            serde_json::to_value(&locked).expect("json")["type"],
            "worktree_locked"
        );
        let mismatch = ErrorDetails::WorktreeMismatch {
            path: r"C:\w\feature-a".to_string(),
            expected_branch: "feature/a".to_string(),
            observed_branch: Some("feature.a".to_string()),
        };
        assert_eq!(
            serde_json::to_value(&mismatch).expect("json")["type"],
            "worktree_mismatch"
        );
    }

    #[test]
    fn error_code_matches_frontend_union() {
        let path = frontend_ipc_ts_path();
        if !path.is_file() {
            panic!(
                "TypeScript ErrorCode union not found at {}. \
                 Refusing to skip: this test is the guard that keeps ErrorCode aligned with src/types/ipc.ts.",
                path.display()
            );
        }
        let source = std::fs::read_to_string(&path).unwrap_or_else(|err| {
            panic!("failed to read {}: {err}", path.display());
        });
        let ts_names = error_codes_in_typescript_union(&source);

        let mut rust_names = BTreeSet::new();
        for code in every_error_code() {
            let value = serde_json::to_value(code).expect("json");
            let Some(name) = value.as_str() else {
                panic!("{code:?} serialized to {value}, expected a string");
            };
            rust_names.insert(name.to_owned());
        }

        assert_eq!(
            rust_names, ts_names,
            "ErrorCode serde names and the TypeScript ErrorCode union in src/types/ipc.ts drifted"
        );
    }

    fn frontend_ipc_ts_path() -> PathBuf {
        let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        path.pop();
        path.pop();
        path.push("src");
        path.push("types");
        path.push("ipc.ts");
        path
    }

    fn error_codes_in_typescript_union(source: &str) -> BTreeSet<String> {
        const MARKER: &str = "export type ErrorCode";
        let Some(marker_at) = source.find(MARKER) else {
            panic!(
                "src/types/ipc.ts has no `{MARKER}` alias; cannot check alignment with ErrorCode"
            );
        };
        let after_marker = &source[marker_at + MARKER.len()..];
        let Some(eq_at) = after_marker.find('=') else {
            panic!("`{MARKER}` has no `=`");
        };
        let after_eq = &after_marker[eq_at + 1..];
        let Some(semi_at) = after_eq.find(';') else {
            panic!("`{MARKER}` has no terminating `;`");
        };
        let body = &after_eq[..semi_at];

        let mut names = BTreeSet::new();
        let mut rest = body;
        while let Some(start) = rest.find('"') {
            rest = &rest[start + 1..];
            let Some(end) = rest.find('"') else {
                panic!("unterminated string in `{MARKER}` union");
            };
            names.insert(rest[..end].to_owned());
            rest = &rest[end + 1..];
        }
        if names.is_empty() {
            panic!("`{MARKER}` union contains no string literals");
        }
        names
    }

    /// One list feeds both the exhaustive `match` and the values we serialize.
    /// Adding a variant without updating this list is a compile error.
    fn every_error_code() -> Vec<ErrorCode> {
        macro_rules! variants {
            ($($variant:ident),+ $(,)?) => {{
                let codes = vec![$(ErrorCode::$variant),+];
                for code in &codes {
                    match code {
                        $(ErrorCode::$variant => {})+
                    }
                }
                codes
            }};
        }
        variants!(
            ProtocolVersionMismatch,
            Unauthorized,
            Unimplemented,
            CapabilityNotSupported,
            InvalidRequest,
            SessionNotFound,
            SessionGenerationMismatch,
            IdempotencyConflict,
            ShuttingDown,
            Journal,
            WorkspaceUnavailable,
            WorkspaceConfinementRefused,
            Internal,
            Io,
        )
    }
}
