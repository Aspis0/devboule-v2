//! Test support for the Codex input-request cards: the capturing sender, the
//! reader harness and the question/elicitation fixtures the topic test files
//! share. Frames are built from Paseo's zod shapes plus the one elicitation
//! frame RECON-A2b §3 quotes — the journal holds no `requestUserInput` row.

use std::collections::HashMap;
use std::sync::atomic::AtomicU64;
use std::sync::{Arc, Mutex};

use devboule_protocol::SessionEvent;

use super::super::codex_input_requests::{codex_permission_frame, CodexPendingResponse};
use super::super::event_pull::ConnHandle;
use super::super::permission_broker::{PermissionBroker, PermissionSender};
use super::super::session_runtime::SessionRuntime;
use super::{catalog_from_response, empty_commands, CodexReader, CodexRequests};
use crate::codex_view::CodexView;

// --- input requests: questions and elicitations --------------------------
//
// Codex asks two ways: `requestUserInput` (a real question) and
// `mcpServer/elicitation/request` (an MCP tool approval in disguise). Both
// used to be declined unseen; both now reach the person as cards. Frames
// below are built from Paseo's zod shapes plus the one elicitation frame
// quoted in RECON-A2b §3 — the journal holds no `requestUserInput` row.

/// One parked Codex answer shaped for the wire through the same entry point
/// the live sender uses, captured instead of written.
fn capturing_codex_sender(
    captured: &Arc<Mutex<Vec<serde_json::Value>>>,
    response_ids: &Arc<Mutex<HashMap<u64, CodexPendingResponse>>>,
) -> Arc<PermissionSender> {
    let captured = Arc::clone(captured);
    let response_ids = Arc::clone(response_ids);
    Arc::new(move |broker_id, result| {
        let pending = response_ids
            .lock()
            .map_err(|_| std::io::Error::other("Codex permission map lock poisoned"))?
            .remove(&broker_id)
            .ok_or_else(|| {
                std::io::Error::other("Codex permission response had no matching request")
            })?;
        captured
            .lock()
            .map_err(|_| std::io::Error::other("captured lock poisoned"))?
            .push(codex_permission_frame(&pending, &result));
        Ok(())
    })
}

fn question_reader(
    broker: Arc<PermissionBroker>,
    response_ids: Arc<Mutex<HashMap<u64, CodexPendingResponse>>>,
) -> CodexReader {
    CodexReader {
        commands: empty_commands(),
        available_commands: None,
        buffer: Vec::new(),
        discarding_oversized_line: false,
        deferred: Vec::new(),
        manifest: None,
        state: Arc::new(crate::codex_view::CodexState::new(
            "thread".to_string(),
            catalog_from_response(&serde_json::json!({
                "data": [{ "id": "model", "isDefault": true }]
            }))
            .expect("catalog"),
            "auto",
        )),
        view: CodexView::new(None),
        permission_broker: Arc::clone(&broker),
        response_ids,
        stdin: Arc::new(Mutex::new(None)),
        next_id: Arc::new(AtomicU64::new(1)),
        requests: Arc::new(CodexRequests::new()),
        compactions: crate::codex_compaction::CodexCompactions::default(),
    }
}

pub(super) type QuestionHarness = (
    Arc<PermissionBroker>,
    Arc<Mutex<Vec<serde_json::Value>>>,
    Arc<SessionRuntime>,
    Arc<ConnHandle>,
    CodexReader,
);

pub(super) fn question_harness() -> QuestionHarness {
    let captured = Arc::new(Mutex::new(Vec::new()));
    let response_ids = Arc::new(Mutex::new(HashMap::new()));
    let broker = PermissionBroker::for_test(capturing_codex_sender(&captured, &response_ids));
    let runtime =
        SessionRuntime::for_acp("s.codex.questions".to_string(), None, Arc::clone(&broker));
    let conn = ConnHandle::new(1);
    let outcome = runtime
        .try_attach_with_replay(None, &conn, true)
        .expect("attach");
    conn.track_with_agent_replay(
        "s.codex.questions",
        Arc::clone(&runtime),
        false,
        None,
        outcome.generation,
        outcome.live_agent_replay,
    );
    let reader = question_reader(Arc::clone(&broker), Arc::clone(&response_ids));
    (broker, captured, runtime, conn, reader)
}

pub(super) fn user_input_line(method: &str, params: serde_json::Value) -> serde_json::Value {
    serde_json::json!({
        "jsonrpc": "2.0",
        "id": "server-1",
        "method": method,
        "params": params,
    })
}

pub(super) fn fence_question_params() -> serde_json::Value {
    serde_json::json!({
        "itemId": "item-1",
        "threadId": "thread-1",
        "turnId": "turn-1",
        "questions": [{
            "id": "q1",
            "header": "Fence colour",
            "question": "Which colour should I paint the fence?",
            "options": [
                {"label": "Forest green (Recommended)", "description": "Blends in."},
                {"label": "Barn red", "description": "Classic."}
            ]
        }, {
            "id": "q2",
            "header": "Toppings",
            "question": "Which toppings?",
            "multiSelect": true,
            "options": [
                {"label": "Cheese"},
                {"label": "Pepperoni"}
            ]
        }]
    })
}

pub(super) fn single_question_params() -> serde_json::Value {
    // One question, one pick, one answer: the shape a lone option pick and
    // a text answer travel. (A bare option id on a multi-question card is
    // not an answer to anything — the card always submits the whole map.)
    serde_json::json!({
        "itemId": "item-1",
        "threadId": "thread-1",
        "turnId": "turn-1",
        "questions": [{
            "id": "q1",
            "header": "Fence colour",
            "question": "Which colour should I paint the fence?",
            "options": [
                {"label": "Forest green (Recommended)", "description": "Blends in."},
                {"label": "Barn red", "description": "Classic."}
            ]
        }]
    })
}

pub(super) fn asked_question(events: &[crate::session::PendingEvent]) -> SessionEvent {
    events
        .iter()
        .find_map(|event| match &event.envelope.event {
            SessionEvent::PermissionRequest { tool_call_id, .. } if tool_call_id == "item-1" => {
                Some(event.envelope.event.clone())
            }
            _ => None,
        })
        .expect("question card")
}

pub(super) fn elicitation_line(message: &str) -> serde_json::Value {
    user_input_line(
        "mcpServer/elicitation/request",
        serde_json::json!({
            "threadId": "01a0d638-b8ec-71e0-8fbe-dcd38ecb4657",
            "turnId": "01a0d638-f6a7-7030-bbb5-0bd8df1299cf",
            "serverName": "devboule",
            "mode": "form",
            "message": message,
            "requestedSchema": {"type": "object", "properties": {}}
        }),
    )
}

pub(super) fn asked_elicitation(
    events: &[crate::session::PendingEvent],
    tool_call_id: &str,
) -> SessionEvent {
    events
        .iter()
        .find_map(|event| match &event.envelope.event {
            SessionEvent::PermissionRequest {
                tool_call_id: id, ..
            } if id == tool_call_id => Some(event.envelope.event.clone()),
            _ => None,
        })
        .expect("elicitation card")
}
