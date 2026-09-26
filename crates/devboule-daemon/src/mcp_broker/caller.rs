//! Who a bearer call is: caller identity, the peer door, and the tool audit.

use std::sync::Arc;

use crate::journal::AuditRecord;
use crate::server::ServerState;
use devboule_protocol::{PeerRole, SessionOriginKind};
use serde_json::Value;

use super::dispatch::rpc_error;

/// Who is calling through this bearer, resolved once per `tools/call` from the
/// registry row for `registration.session_id` — never from the loopback
/// connection, which is this machine's own by construction and lies about a
/// peer's child by design (see `session.rs::caller_origin`).
///
/// `Local` is the person at this machine's own agent: the door allows without
/// consulting the policy, so local behaviour and sentences are byte-identical.
/// `Peer` carries the device, the role it was paired as, and that device's
/// current capability set (fail-closed: a missing, unreadable or revoked row
/// holds nothing). `Unknown` is a stored origin the daemon cannot establish —
/// an `Unknown` row, or a peer-shaped row without a device or a role — and the
/// door refuses it hard: unlike absence it never resolves. `Absent` is no
/// readable row at all, and the door refuses it with the pre-existing retryable
/// absence sentence: an agent's first call can land before its own commit, and
/// a reaped session's in-flight calls outlive its row, and in both cases the
/// ecosystem already retries exactly that sentence. Absent is still a refusal —
/// on a consent surface the unknown never renders as the benign one — but it
/// is a transient refusal, not a verdict.
#[derive(Debug)]
pub(super) enum McpCaller {
    Local,
    Peer {
        device_id: String,
        role: PeerRole,
        caps: Vec<String>,
    },
    Unknown,
    Absent,
}

pub(super) fn resolve_mcp_caller(state: &ServerState, caller_session_id: &str) -> McpCaller {
    let Some(origin) = state.sessions.caller_origin(caller_session_id) else {
        return McpCaller::Absent;
    };
    match origin.kind {
        SessionOriginKind::Local => McpCaller::Local,
        SessionOriginKind::Peer => match (origin.device_id, origin.role) {
            (Some(device_id), Some(role)) => {
                let caps = state.peer_caps(&device_id);
                McpCaller::Peer {
                    device_id,
                    role,
                    caps,
                }
            }
            _ => McpCaller::Unknown,
        },
        SessionOriginKind::Unknown => McpCaller::Unknown,
    }
}

/// The tool door: judge what this call performs with the same `peer_allows`
/// function the wire dispatcher uses, on the wire equivalents the closed table
/// (`peer_policy::mcp_tool_wire`) names. The first `Deny` wins and nothing is
/// touched; the refusal carries the policy's own sentence, rendered as the wire
/// renders it.
///
/// Returns the reply to send when the call is refused before touching anything.
/// `None` means allowed (local callers always; peers whose device holds every
/// capability the tool's equivalents name; the explicitly unjudged list tool;
/// unknown tool names, which fall through to the broker's own `Unknown tool`
/// arm that touches nothing). Every `Some` is a refusal, never the benign
/// reading: the unknown-origin case hard, the absent-row case with the
/// retryable absence sentence.
pub(super) fn mcp_peer_door(
    caller: &McpCaller,
    tool_name: Option<&str>,
    id: &Value,
) -> Option<Value> {
    let tool = tool_name?;
    match caller {
        McpCaller::Local => None,
        McpCaller::Peer { role, caps, .. } => {
            crate::peer_policy::mcp_tool_denial(*role, caps, tool).map(|reason| {
                rpc_error(
                    id.clone(),
                    -32601,
                    &crate::peer_policy::capability_refusal_message(reason),
                )
            })
        }
        McpCaller::Unknown => Some(rpc_error(
            id.clone(),
            -32601,
            "the calling session's origin is unknown; the call is refused",
        )),
        // No readable row: refuse, retryably, with the sentence every caller
        // already retries — the arms below used to answer absence themselves
        // (create with "No session with that id.", the roster with whatever it
        // could list), and the stub's retry loop recognises exactly this one.
        McpCaller::Absent => Some(rpc_error(id.clone(), -32601, "No session with that id.")),
    }
}

/// The connection a tool body acts through: the caller's own identity, not
/// this machine's. A door-allowed peer must have the act performed and judged
/// exactly as the same act over the wire would be — the delivery attributes
/// the message to the true origin (S4-05), the steer-refusal branch reads the
/// caller for its interrupt authority (S4-01), and the ordinary registry
/// ownership checks apply the peer's own scope. The MCP send body explicitly
/// supplies a local source namespace, so its owner-scoped target lookup does
/// not reach the wire-only daemon-peer allowance; that allowance is reached
/// only by an inbound remote frame. A `Local` caller keeps the unmarked
/// connection, byte-identical to before. `Unknown`/`Absent` never reach a
/// body — the door refuses them — and keep it too.
///
/// `paired_by_user` and the transport binding come from the peer's row: the
/// ownership check for a `Client`-role device compares against the user that
/// ran the pairing, and the binding is the facts recorded at pairing time.
/// Dispatch reads no field of the binding — the handshake owned it — so a
/// row-sourced copy is the truthful thing to carry, not a fresh measurement.
pub(super) fn caller_conn(
    state: &ServerState,
    caller: &McpCaller,
) -> Arc<crate::session::ConnHandle> {
    match caller {
        McpCaller::Peer {
            device_id,
            role,
            caps,
        } => {
            let record = state.peer_get(device_id).ok().flatten();
            let binding = crate::peer_policy::TransportBinding {
                kind: record
                    .as_ref()
                    .map(|record| record.binding_kind.clone())
                    .unwrap_or_default(),
                stable_id: record
                    .as_ref()
                    .and_then(|record| record.binding_stable_id.clone())
                    .unwrap_or_default(),
                node_name: record
                    .as_ref()
                    .and_then(|record| record.binding_node_name.clone())
                    .unwrap_or_default(),
                login_name: record
                    .as_ref()
                    .and_then(|record| record.binding_login_name.clone())
                    .unwrap_or_default(),
            };
            crate::session::ConnHandle::with_peer_caps(
                0,
                None,
                Some(crate::peer_policy::ConnPeer::Remote {
                    device_id: device_id.clone(),
                    role: *role,
                    paired_by_user: record.and_then(|record| record.paired_by_user),
                    binding,
                }),
                caps.clone(),
                crate::session::QuitIntent::default(),
            )
        }
        McpCaller::Local | McpCaller::Unknown | McpCaller::Absent => {
            crate::session::ConnHandle::with_peer(0, None)
        }
    }
}

/// The audit identity for one tool call. A peer-origin caller names its device
/// and role, never `"local"`; an unestablishable origin names `"unknown"`,
/// never the benign one. Local callers keep exactly what they had: this
/// device's id with `"local"`.
pub(super) fn audit_mcp_tool(
    state: &ServerState,
    caller: &McpCaller,
    action: &str,
    session_id: &str,
    outcome: &str,
) {
    let (device_id, role) = match caller {
        McpCaller::Local => match state.device_identity() {
            Ok(identity) => (identity.device_id.clone(), "local".to_string()),
            Err(_) => return,
        },
        McpCaller::Peer {
            device_id, role, ..
        } => (device_id.clone(), role.as_str().to_string()),
        // Who called cannot be established; the audit says so rather than the
        // benign thing. Both absences share the label: the refusal message the
        // caller saw already distinguishes the transient one.
        McpCaller::Unknown | McpCaller::Absent => match state.device_identity() {
            Ok(identity) => (identity.device_id.clone(), "unknown".to_string()),
            Err(_) => return,
        },
    };
    state.audit(AuditRecord {
        device_id,
        role,
        claimed_origin: None,
        action: action.to_string(),
        session_id: Some(session_id.to_string()),
        outcome: outcome.to_string(),
    });
}
