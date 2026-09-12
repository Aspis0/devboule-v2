//! Protocol error codes. Stable names; new codes are additive.

use serde::{Deserialize, Serialize};

use crate::messages::PeerRole;

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

    /// The same error, stripped of everything a paired device has no business
    /// reading. `None` (a local pipe connection) is the person's own screen,
    /// so it is returned unchanged.
    ///
    /// Three things in a daemon message are local facts: absolute paths into
    /// this machine's filesystem, this device's own id, and bare 64-hex
    /// digests (key fingerprints and content hashes). A remote reader gets
    /// `<path>`, `<device>` and `<digest>` instead. The codes and the prose
    /// stay: the point is that a peer can still tell *what* failed, never
    /// *where* this machine keeps it (`DESIGN-remote-agents.md` §8 R7).
    pub fn redacted_for(self, role: Option<&PeerRole>) -> Self {
        if role.is_none() {
            return self;
        }
        Self {
            message: redact_text(&self.message),
            ..self
        }
    }
}

/// What a redacted message says in place of the local fact it removed.
const REDACTED_PATH: &str = "<path>";
const REDACTED_DEVICE: &str = "<device>";
const REDACTED_DIGEST: &str = "<digest>";

/// Replace every local fact in `text`. Over-redaction is the safe direction: a
/// path run swallows the text that follows it until sentence punctuation, so a
/// path containing spaces cannot leak its tail.
fn redact_text(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut redacted = String::with_capacity(text.len());
    let mut index = 0usize;
    while index < bytes.len() {
        if let Some((placeholder, length)) = redaction_at(text, index) {
            redacted.push_str(placeholder);
            index += length;
            continue;
        }
        let Some(character) = text[index..].chars().next() else {
            break;
        };
        redacted.push(character);
        index += character.len_utf8();
    }
    redacted
}

/// The replacement that starts at `index`, with how many bytes it covers.
fn redaction_at(text: &str, index: usize) -> Option<(&'static str, usize)> {
    let rest = &text[index..];
    let bytes = rest.as_bytes();
    let previous = if index == 0 {
        None
    } else {
        text.as_bytes().get(index - 1).copied()
    };
    if previous.is_some_and(|byte| !starts_a_token(byte)) {
        return None;
    }
    // `C:\…` and `C:/…`: a path that names a drive on this machine.
    if bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && matches!(bytes[2], b'\\' | b'/')
    {
        return Some((REDACTED_PATH, path_run_len(bytes)));
    }
    // `\\host\share` (UNC) and `\\?\` device paths.
    if bytes.len() >= 2 && bytes[0] == b'\\' && matches!(bytes[1], b'\\' | b'/') {
        return Some((REDACTED_PATH, path_run_len(bytes)));
    }
    // A POSIX absolute path.
    if bytes[0] == b'/' && bytes.len() > 1 && !bytes[1].is_ascii_whitespace() {
        return Some((REDACTED_PATH, path_run_len(bytes)));
    }
    // A UUID first: it contains hex runs, so the digest rule must not claim it.
    if let Some(length) = uuid_len(bytes) {
        return Some((REDACTED_DEVICE, length));
    }
    if let Some(length) = hex_run_len(bytes, 64) {
        return Some((REDACTED_DIGEST, length));
    }
    None
}

/// Whether a byte can precede a local fact. A path or a digest that starts in
/// the middle of a word is not a path or a digest.
fn starts_a_token(byte: u8) -> bool {
    byte.is_ascii_whitespace()
        || matches!(
            byte,
            b'(' | b'[' | b'{' | b'"' | b'\'' | b'=' | b',' | b';' | b':' | b'|' | b'<'
        )
}

/// Bytes up to the punctuation that ends a path run, or the end of the text.
fn path_run_len(bytes: &[u8]) -> usize {
    bytes
        .iter()
        .position(|byte| {
            matches!(
                byte,
                b'(' | b')'
                    | b'['
                    | b']'
                    | b'{'
                    | b'}'
                    | b';'
                    | b'"'
                    | b'\''
                    | b'|'
                    | b'<'
                    | b'>'
                    | b','
                    | b'\n'
                    | b'\r'
                    | b'\t'
            )
        })
        .unwrap_or(bytes.len())
}

/// `8-4-4-4-12` lowercase or uppercase hex, and nothing hex-or-dash after it.
fn uuid_len(bytes: &[u8]) -> Option<usize> {
    const GROUPS: [usize; 5] = [8, 4, 4, 4, 12];
    if bytes.len() < 36 {
        return None;
    }
    let mut offset = 0usize;
    for (group, length) in GROUPS.iter().enumerate() {
        if group > 0 {
            if bytes[offset] != b'-' {
                return None;
            }
            offset += 1;
        }
        for _ in 0..*length {
            if !bytes[offset].is_ascii_hexdigit() {
                return None;
            }
            offset += 1;
        }
    }
    if bytes
        .get(offset)
        .is_some_and(|byte| byte.is_ascii_hexdigit() || *byte == b'-')
    {
        return None;
    }
    Some(36)
}

/// Exactly `length` hex characters, not part of a longer hex run.
fn hex_run_len(bytes: &[u8], length: usize) -> Option<usize> {
    if bytes.len() < length {
        return None;
    }
    if !bytes[..length].iter().all(u8::is_ascii_hexdigit) {
        return None;
    }
    if bytes
        .get(length)
        .is_some_and(|byte| byte.is_ascii_hexdigit() || *byte == b'-')
    {
        return None;
    }
    Some(length)
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

    /// A remote reader must learn what failed, never where this machine keeps
    /// it. Real message shapes from the daemon's own error paths.
    #[test]
    fn a_remote_error_redacts_paths_and_local_identities() {
        let device = "6f1c1c2e-9b0a-4f6d-8f3a-2b1e5c7d9a01";
        let digest = "9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08";
        let error = WireError::new(
            ErrorCode::Io,
            format!(
                "Could not store an attached file: replace failed (os error 5); keeping backup at \
                 \"C:\\Users\\gualt\\AppData\\Local\\Temp\\devboule\\a.gguf\"; device {device}; \
                 fingerprint {digest} (os error 5)"
            ),
        );
        let redacted = error.clone().redacted_for(Some(&PeerRole::Client));
        assert!(
            !redacted.message.contains("C:\\Users"),
            "{}",
            redacted.message
        );
        assert!(!redacted.message.contains("gualt"), "{}", redacted.message);
        assert!(!redacted.message.contains(device), "{}", redacted.message);
        assert!(!redacted.message.contains(digest), "{}", redacted.message);
        assert!(redacted.message.contains("<path>"), "{}", redacted.message);
        assert!(
            redacted.message.contains("<device>"),
            "{}",
            redacted.message
        );
        assert!(
            redacted.message.contains("<digest>"),
            "{}",
            redacted.message
        );
        // The prose and the code stay, so the failure is still diagnosable.
        assert_eq!(redacted.code, ErrorCode::Io);
        assert!(
            redacted
                .message
                .contains("Could not store an attached file"),
            "{}",
            redacted.message
        );
        assert!(
            redacted.message.contains("(os error 5)"),
            "{}",
            redacted.message
        );
    }

    /// The person at this machine reads their own paths; `None` is the pipe.
    #[test]
    fn a_local_error_preserves_paths() {
        let error = WireError::new(
            ErrorCode::Io,
            "Could not store an attached file: C:\\Users\\gualt\\tmp\\a.png",
        );
        let kept = error.clone().redacted_for(None);
        assert_eq!(kept, error);
        assert!(kept.message.contains("C:\\Users\\gualt\\tmp\\a.png"));
    }

    /// Both shapes are redacted, and the prose between two local facts survives
    /// when each fact is its own token. An *unquoted* run is deliberately
    /// greedy — it swallows everything after the first path, because a path
    /// with spaces in it would otherwise leak its tail — so this asserts the
    /// quoted form as well as the plain one.
    #[test]
    fn a_posix_path_and_a_unc_path_are_both_redacted() {
        let posix = WireError::new(ErrorCode::Io, "could not write /home/gualt/runtime/a.png")
            .redacted_for(Some(&PeerRole::Daemon));
        assert_eq!(posix.message, "could not write <path>");

        let unc = WireError::new(ErrorCode::Io, "could not write \\\\host\\share\\b.png")
            .redacted_for(Some(&PeerRole::Daemon));
        assert_eq!(unc.message, "could not write <path>");

        let both = WireError::new(
            ErrorCode::Io,
            "could not write \"/home/gualt/runtime/a.png\" and \"\\\\host\\\\share\\\\b.png\"",
        )
        .redacted_for(Some(&PeerRole::Daemon));
        assert_eq!(both.message, "could not write \"<path>\" and \"<path>\"");

        // Greedy is the safe direction: the tail after an unquoted path is
        // over-redacted rather than half-redacted.
        let greedy = WireError::new(
            ErrorCode::Io,
            "could not write /home/gualt/runtime/a.png and 31337 bytes",
        )
        .redacted_for(Some(&PeerRole::Daemon));
        assert_eq!(greedy.message, "could not write <path>");
    }

    /// A digest is only a digest when it is a whole token: part of a longer
    /// hex run or a session id is not one.
    #[test]
    fn a_hex_run_inside_a_longer_token_is_not_a_digest() {
        let error = WireError::new(
            ErrorCode::InvalidRequest,
            "id s.9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08.01 is not known",
        );
        let redacted = error.redacted_for(Some(&PeerRole::Client));
        assert!(
            !redacted.message.contains("<digest>"),
            "{}",
            redacted.message
        );
        assert!(
            redacted.message.contains(".01 is not known"),
            "{}",
            redacted.message
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
