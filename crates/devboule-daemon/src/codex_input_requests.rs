//! Codex input requests: the two provider-initiated asks — `requestUserInput`
//! questions and `mcpServer/elicitation/request` MCP approvals — parsed into
//! broker cards and shaped back into Codex replies.
//!
//! The client owns the transport and the approval cards; this module owns
//! everything about the two input-request carriers: their params, the pending
//! record that shapes each reply, and the dispatch that cards them.

use std::collections::HashMap;
use std::io;
use std::process::ChildStdin;
use std::sync::atomic::AtomicU64;
use std::sync::{Arc, Mutex};

use serde_json::Value;

use super::codex_client::send_frame;
use super::codex_elicitations::codex_elicitation_result;
use super::codex_questions::codex_question_result;
use super::permission_broker::{PermissionBroker, PermissionSender};
use super::write_child_stdin;

/// The broker-side handle for one parked Codex input request: what it needs
/// to card the request and, later, to shape the answer.
pub(super) struct CodexInputDeps {
    pub(super) stdin: Arc<Mutex<Option<ChildStdin>>>,
    pub(super) response_ids: Arc<Mutex<HashMap<u64, CodexPendingResponse>>>,
    pub(super) next_id: Arc<AtomicU64>,
    pub(super) spawn_nonce: String,
    pub(super) permission_broker: Arc<PermissionBroker>,
}

pub(super) fn codex_permission_sender(
    stdin: Arc<Mutex<Option<ChildStdin>>>,
    response_ids: Arc<Mutex<HashMap<u64, CodexPendingResponse>>>,
) -> Arc<PermissionSender> {
    Arc::new(move |broker_id, result| {
        let pending = response_ids
            .lock()
            .map_err(|_| io::Error::other("Codex permission map lock poisoned"))?
            .get(&broker_id)
            .cloned()
            .ok_or_else(|| io::Error::other("Codex permission response had no matching request"))?;
        // The recorded kind shapes the reply, never the result's shape: an
        // approval answers with a decision, a question with its answers map,
        // an elicitation with an action.
        let frame = codex_permission_frame(&pending, &result);
        let result =
            send_frame(&stdin, &frame, "Codex").map_err(|error| io::Error::other(error.message));
        if result.is_ok() {
            let _ = response_ids.lock().map(|mut ids| ids.remove(&broker_id));
        }
        result
    })
}

/// What a pending Codex request is waiting for: the reply shape differs per
/// carrier, so the recorded kind — never the params' shape — decides it.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum CodexPendingKind {
    Approval,
    Question,
    Elicitation,
}

/// One Codex request parked in the broker: the JSON-RPC id to answer, the
/// kind that shapes the reply, and the params the answer maps against.
#[derive(Clone)]
pub(super) struct CodexPendingResponse {
    pub(super) id: Value,
    pub(super) kind: CodexPendingKind,
    pub(super) params: Value,
}

/// One parked answer shaped for the wire: the single entry point both the
/// live sender and the tests shape through, so the two cannot drift.
pub(super) fn codex_permission_frame(pending: &CodexPendingResponse, result: &Value) -> Value {
    match pending.kind {
        CodexPendingKind::Approval => {
            permission_decision_frame(&pending.id, permission_decision(result))
        }
        CodexPendingKind::Question => {
            response_frame(&pending.id, codex_question_result(&pending.params, result))
        }
        CodexPendingKind::Elicitation => {
            response_frame(&pending.id, codex_elicitation_result(result))
        }
    }
}

pub(super) fn permission_decision(result: &Value) -> &'static str {
    if result.pointer("/outcome/outcome").and_then(Value::as_str) != Some("selected") {
        return "cancel";
    }
    match result.pointer("/outcome/optionId").and_then(Value::as_str) {
        Some("allow") => "accept",
        Some("deny") => "decline",
        _ => "cancel",
    }
}

pub(super) fn send_result(
    stdin: &Mutex<Option<ChildStdin>>,
    id: &Value,
    result: Value,
) -> io::Result<()> {
    let frame = response_frame(id, result);
    let mut bytes = serde_json::to_vec(&frame)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    bytes.push(b'\n');
    write_child_stdin(stdin, &bytes, "Codex")
}

pub(super) fn response_frame(id: &Value, result: Value) -> Value {
    serde_json::json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

pub(super) fn permission_decision_frame(id: &Value, decision: &str) -> Value {
    response_frame(id, serde_json::json!({ "decision": decision }))
}
