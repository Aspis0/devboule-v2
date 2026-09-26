//! Parked ACP answers: one kind-tagged record per broker registration, and
//! the sender that shapes each answer from its record.
//!
//! The broker's table is keyed by card id, but the wire reply goes to the
//! JSON-RPC id — and grok's question dialect differs from the ACP one — so
//! every ACP registration parks what its answer needs here first. The sender
//! answers from the parked kind, never from the result's shape: an id with
//! no record is refused unwritten, so a late or duplicate answer cannot go
//! out in the wrong dialect. A record is taken before its write, so a failed
//! write leaves no stale mark behind; the agent's own timeout is the
//! backstop then.

use std::collections::HashMap;
use std::io;
use std::process::ChildStdin;
use std::sync::{Arc, Mutex};

use serde_json::Value;

use super::acp_questions::grok_question_result;
use super::permission_broker::PermissionSender;
use super::write_child_stdin;

/// What one parked ACP answer is waiting for: the reply dialect differs per
/// carrier, so the recorded kind — never the result's shape — decides it.
pub(super) enum AcpPendingKind {
    Permission,
    Question,
}

/// One ACP request parked in the broker: the kind that shapes the reply,
/// and the params the answer maps against (a grok question's texts).
pub(super) struct AcpPendingResponse {
    pub(super) kind: AcpPendingKind,
    pub(super) params: Value,
}

pub(super) type AcpPending = Arc<Mutex<HashMap<u64, AcpPendingResponse>>>;

/// The sender every ACP session installs: a parked question answers in
/// grok's shape, a parked permission passes through in the ACP shape the
/// broker already built, and an id with no record is refused — nothing is
/// written in a guessed dialect. The record is taken before the write, so a
/// failed write leaves no stale mark; by the same token the broker's
/// fallback retry finds no record and fails, so for ACP the first write is
/// the only write and the agent's own timeout is the backstop.
pub(super) fn acp_response_sender(
    stdin: Arc<Mutex<Option<ChildStdin>>>,
    pending: AcpPending,
) -> Arc<PermissionSender> {
    Arc::new(move |broker_id, result| {
        let record = match pending.lock() {
            Ok(mut map) => map.remove(&broker_id),
            Err(_) => {
                return Err(io::Error::other("ACP pending map lock poisoned"));
            }
        };
        let Some(record) = record else {
            return Err(io::Error::other("ACP response had no matching request"));
        };
        let result = match record.kind {
            AcpPendingKind::Question => grok_question_result(&record.params, &result),
            AcpPendingKind::Permission => result,
        };
        let frame = serde_json::json!({ "jsonrpc": "2.0", "id": broker_id, "result": result });
        let mut bytes = serde_json::to_vec(&frame)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        bytes.push(b'\n');
        write_child_stdin(&stdin, &bytes, "ACP")
    })
}
