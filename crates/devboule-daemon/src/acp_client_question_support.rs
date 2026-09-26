//! Test support for the grok question cards: the capturing sender, the
//! reader harness, and the enveloped/unenveloped fixtures the topic test
//! files share.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use devboule_protocol::SessionEvent;

use super::super::acp_questions::{grok_question_result, GrokPending};
use super::super::event_pull::ConnHandle;
use super::super::permission_broker::{PermissionBroker, PermissionSender};
use super::super::session_runtime::SessionRuntime;
use super::AcpReader;

pub(super) const SESSION: &str = "s.acp.grok";
pub(super) const FENCE: &str = "Which colour should I paint the fence?";
pub(super) const TOPPINGS: &str = "Which toppings?";

pub(super) type Captured = Arc<Mutex<Vec<serde_json::Value>>>;

/// The production sender's translation, captured instead of written: a
/// parked grok id answers in grok's shape, every other id passes through.
fn capturing_sender(captured: &Captured, pending: &GrokPending) -> Arc<PermissionSender> {
    let captured = Arc::clone(captured);
    let pending = Arc::clone(pending);
    Arc::new(move |broker_id, result| {
        let params = pending
            .lock()
            .map(|map| map.get(&broker_id).cloned())
            .unwrap_or(None);
        let is_grok = params.is_some();
        let result = match &params {
            Some(params) => grok_question_result(params, &result),
            None => result,
        };
        captured
            .lock()
            .expect("captured lock")
            .push(serde_json::json!({
                "jsonrpc": "2.0",
                "id": broker_id,
                "result": result,
            }));
        if is_grok {
            let _ = pending.lock().map(|mut map| map.remove(&broker_id));
        }
        Ok(())
    })
}

pub(super) struct Harness {
    pub(super) broker: Arc<PermissionBroker>,
    pub(super) captured: Captured,
    pub(super) runtime: Arc<SessionRuntime>,
    pub(super) conn: Arc<ConnHandle>,
    pub(super) reader: AcpReader,
}

impl Harness {
    pub(super) fn new() -> Self {
        let pending: GrokPending = Arc::new(Mutex::new(HashMap::new()));
        let captured: Captured = Arc::new(Mutex::new(Vec::new()));
        let broker = PermissionBroker::for_test(capturing_sender(&captured, &pending));
        let runtime = SessionRuntime::for_acp(SESSION.to_string(), None, Arc::clone(&broker));
        let conn = ConnHandle::new(1);
        let outcome = runtime
            .try_attach_with_replay(None, &conn, true)
            .expect("attach");
        conn.track_with_agent_replay(
            SESSION,
            Arc::clone(&runtime),
            false,
            None,
            outcome.generation,
            outcome.live_agent_replay,
        );
        let reader = AcpReader::for_test_with_questions(
            Arc::new(Mutex::new(HashSet::new())),
            SESSION.to_string(),
            Arc::clone(&broker),
            pending,
        );
        Harness {
            broker,
            captured,
            runtime,
            conn,
            reader,
        }
    }

    pub(super) fn dispatch(&self, frame: &serde_json::Value) {
        self.reader.dispatch_value(frame, &self.runtime);
    }
}

pub(super) fn live_turn(reader: &AcpReader) {
    reader.turn.start_prompt(7);
}

pub(super) fn fence_params() -> serde_json::Value {
    serde_json::json!({
        "sessionId": SESSION,
        "toolCallId": "call-fence-0",
        "questions": [{
            "question": FENCE,
            "options": [
                {"label": "Forest green (Recommended)", "description": "Blends in."},
                {"label": "Barn red", "description": "Classic red."},
                {"label": "Weathered grey", "description": "Aged look."}
            ],
            "multiSelect": null
        }],
        "mode": "default"
    })
}

/// The live frame shape from our own journal, as grok 1.0.25/26 sent it.
pub(super) fn enveloped(id: u64) -> serde_json::Value {
    serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": "_x.ai/ask_user_question",
        "params": fence_params(),
    })
}

/// The 1.0.40 shape: the same fields directly under the method.
pub(super) fn unenveloped(id: u64) -> serde_json::Value {
    let mut frame = serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": "_x.ai/ask_user_question",
    });
    for (key, value) in fence_params().as_object().expect("params object").iter() {
        if key.as_str() != "toolCallId" {
            frame[key.as_str()] = value.clone();
        }
    }
    frame["toolCallId"] = serde_json::json!("call-bare-5");
    frame
}

pub(super) fn asked(events: &[crate::session::PendingEvent], tool_call_id: &str) -> SessionEvent {
    events
        .iter()
        .find_map(|event| match &event.envelope.event {
            SessionEvent::PermissionRequest {
                tool_call_id: id, ..
            } if id == tool_call_id => Some(event.envelope.event.clone()),
            _ => None,
        })
        .expect("question card")
}

pub(super) fn has_notice(events: &[crate::session::PendingEvent]) -> bool {
    events.iter().any(|event| {
        matches!(
            event.envelope.event,
            SessionEvent::SessionNotice { .. } | SessionEvent::AgentError { .. }
        )
    })
}
