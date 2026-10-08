//! The always-asking cards on the real road: the router, a live session in an
//! automatic mode, a card the person must answer, and a paired device that is
//! dialled only after the answer. The saved-login half lives with its tool
//! (`browser_login`'s `saved_login_cards_every_use_in_auto_mode`).

use std::sync::atomic::AtomicBool;
use std::sync::mpsc::Receiver;
use std::sync::Arc;
use std::time::{Duration, Instant};

use devboule_protocol::{
    AgentMessageState, ClientMessage, DaemonMessage, PermissionOutcome, SessionEvent, SessionKind,
};
use serde_json::{json, Value};

use super::dispatch::handle_rpc;
use super::tools::first_use::{ensure_write_approved, Approval, TERMINAL_KEYS_GROUP};
use super::tools::messaging_peer::tests::{owner, pinned_responder, remote_send_capability, USER};
use super::RegisteredSession;
use crate::provider_catalog::ToolOverlay;
use crate::server::ServerState;

pub(super) const SESSION: &str = "s.local.sender";
pub(super) const DEVICE: &str = "dev-far";

pub(super) fn registration() -> RegisteredSession {
    RegisteredSession {
        session_id: SESSION.to_string(),
        owner: owner(),
        provider_id: Some("claude".to_string()),
        depth: 0,
        overlay: ToolOverlay::NONE,
        bearer: "the bearer".to_string(),
        claude_config_path: None,
        runtime: None,
        broker_ready: Arc::new(AtomicBool::new(true)),
    }
}

/// A live Claude session in `mode`, the way a provider handshake records it.
pub(super) fn live_session(
    state: &Arc<ServerState>,
    mode: &str,
) -> Arc<crate::session::SessionRuntime> {
    let runtime = crate::session::insert_test_live_agent_with_kind(
        &state.sessions,
        SESSION,
        owner(),
        SessionKind::Claude,
    );
    runtime.set_agent_kind(SessionKind::Claude);
    runtime.store_session_manifest(SessionEvent::SessionManifest {
        provider_id: Some("claude".to_string()),
        current_model_id: None,
        models: Vec::new(),
        modes: Some(devboule_protocol::SessionModeStateView {
            current_mode_id: mode.to_string(),
            available_modes: Vec::new(),
        }),
        current_model_provider_id: None,
    });
    runtime
}

pub(super) fn send_call(text: &str) -> Value {
    json!({
        "jsonrpc": "2.0", "id": 1, "method": "tools/call",
        "params": {"name": crate::provider_catalog::MCP_SEND_MESSAGE_TOOL, "arguments": {
            "deviceId": DEVICE, "to_agent": "s.far.target", "text": text,
        }},
    })
}

/// One `tools/call` through the router, on its own thread: it parks on the card.
pub(super) fn route(state: &Arc<ServerState>, message: Value) -> std::thread::JoinHandle<Value> {
    let state = Arc::clone(state);
    std::thread::spawn(move || {
        handle_rpc(&state, state.mcp.as_ref(), &registration(), &message)
            .expect("the router does not fail")
            .expect("the router answers")
    })
}

/// The card the person is looking at, waited for.
pub(super) fn the_card(runtime: &Arc<crate::session::SessionRuntime>) -> (String, SessionEvent) {
    let broker = runtime.permission_broker().expect("the test broker");
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(id) = broker.test_pending_ids().pop() {
            let card = broker.test_pending_request(&id).expect("the pending card");
            return (id, card);
        }
        assert!(Instant::now() < deadline, "no card was raised");
        std::thread::sleep(Duration::from_millis(10));
    }
}

pub(super) fn answer(runtime: &Arc<crate::session::SessionRuntime>, id: &str, option: &str) {
    let outcome = if option == "deny" {
        PermissionOutcome::Deny
    } else {
        PermissionOutcome::AllowOnce
    };
    runtime
        .permission_broker()
        .expect("the test broker")
        .test_answer(id, outcome, option)
        .expect("the person answers");
}

pub(super) fn far_end(state: &Arc<ServerState>) -> Receiver<Option<ClientMessage>> {
    pinned_responder(
        state,
        DEVICE,
        Some(USER),
        remote_send_capability(),
        DaemonMessage::AgentMessageReceipt {
            id: 0,
            state: AgentMessageState::Accepted,
        },
        4,
    )
}

/// The next message the far end was asked to *send*, within `wait`: the roster
/// read the card makes to name the session is a dial too, and is not a send.
pub(super) fn next_send(
    requests: &Receiver<Option<ClientMessage>>,
    wait: Duration,
) -> Option<String> {
    let deadline = Instant::now() + wait;
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        match requests.recv_timeout(left) {
            Ok(Some(ClientMessage::AgentMessageSend { text, .. })) => return Some(text),
            Ok(_) => {}
            Err(_) => return None,
        }
    }
}

pub(super) fn card_text(card: &SessionEvent) -> (String, String, Vec<(String, String)>) {
    let SessionEvent::PermissionRequest {
        title,
        description,
        options,
        ..
    } = card
    else {
        panic!("a permission card");
    };
    (
        title.clone(),
        description.clone().unwrap_or_default(),
        options
            .iter()
            .map(|option| (option.option_id.clone(), option.kind.clone()))
            .collect(),
    )
}

/// In an automatic mode a message to another machine still waits for the
/// person: the card names the device, the session and the message, offers only
/// this call, and nothing is dialled before the answer.
#[test]
fn paired_send_always_cards_in_auto_mode() {
    let state = ServerState::new("always-card-paired-send".to_string());
    let runtime = live_session(&state, "bypassPermissions");
    let requests = far_end(&state);

    let call = route(&state, send_call("deploy the fix"));
    let (id, card) = the_card(&runtime);

    let (title, description, options) = card_text(&card);
    assert!(title.contains("Device dev-far"), "the paired name: {title}");
    for fact in [
        "| device: Device dev-far",
        "| session: s.far.target",
        "| message (14 characters):",
        "| deploy the fix",
    ] {
        assert!(description.contains(fact), "{fact}: {description}");
    }
    assert_eq!(
        options,
        [
            ("once".to_string(), "allow_once".to_string()),
            ("deny".to_string(), "reject_once".to_string()),
        ],
        "this call or nothing: no grant for a session"
    );
    assert_eq!(
        next_send(&requests, Duration::from_millis(300)),
        None,
        "nothing was sent before the person answered"
    );

    answer(&runtime, &id, "once");
    let reply = call.join().expect("the call");
    assert_eq!(
        reply.pointer("/result/isError"),
        Some(&json!(false)),
        "{reply}"
    );
    assert_eq!(
        next_send(&requests, Duration::from_secs(5)).as_deref(),
        Some("deploy the fix"),
        "the far daemon was sent the message"
    );
}

#[test]
fn a_refused_paired_send_dials_nothing_and_the_next_one_asks_again() {
    let state = ServerState::new("always-card-refused".to_string());
    let runtime = live_session(&state, "bypassPermissions");
    let requests = far_end(&state);

    let first = route(&state, send_call("one"));
    let (id, _) = the_card(&runtime);
    answer(&runtime, &id, "deny");
    let refused = first.join().expect("the refused call");
    assert_eq!(
        refused.pointer("/result/isError"),
        Some(&json!(true)),
        "{refused}"
    );
    assert_eq!(
        next_send(&requests, Duration::from_millis(500)),
        None,
        "a refusal sends nothing"
    );

    let second = route(&state, send_call("two"));
    let (again, _) = the_card(&runtime);
    answer(&runtime, &again, "once");
    assert_eq!(
        second
            .join()
            .expect("the second call")
            .pointer("/result/isError"),
        Some(&json!(false))
    );
}

/// A local send, and a local write in an automatic mode, keep the behaviour
/// the mode gives them: no card, no change.
#[test]
fn other_writes_keep_existing_auto_behavior() {
    let state = ServerState::new("always-card-local".to_string());
    let runtime = live_session(&state, "bypassPermissions");
    let broker = runtime.permission_broker().expect("the test broker");

    // A local write: the gate approves by mode and raises nothing.
    let approval = ensure_write_approved(
        &state,
        state.mcp.as_ref(),
        SESSION,
        &owner(),
        TERMINAL_KEYS_GROUP,
        "type into a terminal",
        &[],
    );
    assert_eq!(approval, Ok(Approval::Mode));
    assert!(
        broker.test_pending_ids().is_empty(),
        "an automatic mode raised a card"
    );

    // A send with no deviceId is the local road and never reaches the card.
    let local = handle_rpc(
        &state,
        state.mcp.as_ref(),
        &registration(),
        &json!({
            "jsonrpc": "2.0", "id": 2, "method": "tools/call",
            "params": {"name": crate::provider_catalog::MCP_SEND_MESSAGE_TOOL, "arguments": {
                "to_agent": "s.nobody", "text": "hi",
            }},
        }),
    )
    .expect("the router does not fail")
    .expect("the router answers");
    assert!(
        broker.test_pending_ids().is_empty(),
        "a local send raised a card: {local}"
    );
}

/// No tool reaches a terminal, a session or a workspace on another machine:
/// their schemas are closed and name no device, so a `deviceId` among their
/// arguments is not a road there and does not ask.
#[test]
fn the_terminal_session_and_workspace_tools_cannot_name_a_device() {
    use crate::provider_catalog::{
        MCP_CANCEL_AGENT_TOOL, MCP_CLOSE_AGENT_TOOL, MCP_CREATE_AGENT_TOOL,
        MCP_CREATE_TERMINAL_TOOL, MCP_CREATE_WORKSPACE_TOOL, MCP_KILL_TERMINAL_TOOL,
        MCP_SEND_TERMINAL_KEYS_TOOL, MCP_STOP_AGENT_TOOL,
    };
    for tool in [
        MCP_SEND_TERMINAL_KEYS_TOOL,
        MCP_CREATE_TERMINAL_TOOL,
        MCP_KILL_TERMINAL_TOOL,
        MCP_CREATE_AGENT_TOOL,
        MCP_STOP_AGENT_TOOL,
        MCP_CLOSE_AGENT_TOOL,
        MCP_CANCEL_AGENT_TOOL,
        MCP_CREATE_WORKSPACE_TOOL,
    ] {
        assert!(
            super::tools::always_card::always_card(tool, &json!({"deviceId": "dev-far"})).is_none(),
            "{tool} names no device, so it is not a command to another machine"
        );
    }
}
