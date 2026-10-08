//! The creation card follows the creator's mode on the real road: a gate
//! a person opened does not outrank a later mode switch to plan.

use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use super::caller::McpCaller;
use super::tools::creation::request::AgentCreateRequest;
use super::tools::creation::run::create_agent;
use super::RegisteredSession;
use crate::provider_catalog::ToolOverlay;
use crate::server::ServerState;
use devboule_protocol::{OwnerId, PermissionOutcome, SessionKind};

fn owner() -> OwnerId {
    OwnerId::new("S-1-5-21-create-mode", "create-mode-client").expect("owner")
}

/// A user provider row the daemon can launch, written to the state's own
/// runtime dir: the create road refreshes user rows from there before it
/// resolves the provider, so the row is loaded deterministically on every
/// call. Its command names a file that does not exist, so the spawn the
/// road ends in fails fast instead of starting anything.
fn seed_probe_provider(state: &Arc<ServerState>) {
    let dir = state.sessions.runtime_dir().to_path_buf();
    std::fs::create_dir_all(&dir).expect("runtime dir");
    std::fs::write(
        dir.join("providers.json"),
        r#"{"d1a-probe-agent": {"extends": "acp", "command": ["C:\\nonexistent-d1a-probe\\agent.exe"]}}"#,
    )
    .expect("probe row");
    // The store resolves the profile's provider against the catalog now;
    // the road re-reads the same file on every creation.
    let mut gate = crate::user_providers::lock_rows_state();
    crate::user_providers::refresh_user_rows_with(&mut gate, &dir);
    assert!(
        crate::session::catalog_registry()
            .user_row_for("d1a-probe-agent")
            .is_some(),
        "setup: the probe row is live"
    );
}

fn tick_probe_profile(state: &Arc<ServerState>) {
    let document: devboule_protocol::AgentProfilesDocument = serde_json::from_value(json!({
        "profiles": [{
            "id": "d1a-probe-profile",
            "name": "Probe",
            "note": "when to use this one",
            "provider": "d1a-probe-agent",
            "model": "probe-model",
            "modeId": "ask",
            "features": {},
            "toolOverlay": [],
            "enabledForAgents": true,
        }],
        "standingInstructions": "",
    }))
    .expect("probe document");
    // The store resolves the provider against the global catalog, which a
    // concurrent refresh elsewhere can replace between our seed and this
    // set: re-seed and retry rather than flake on another test's window.
    for _ in 0..30 {
        seed_probe_provider(state);
        match state.agent_profiles.set(document.clone()) {
            Ok(()) => return,
            Err(error)
                if error
                    .to_string()
                    .contains("not a provider the catalog publishes") =>
            {
                std::thread::sleep(Duration::from_millis(20))
            }
            Err(error) => panic!("probe profile ticked: {error}"),
        }
    }
    panic!("probe profile never admitted: the catalog kept losing the row");
}

fn creator(state: &Arc<ServerState>, id: &str) -> Arc<crate::session::SessionRuntime> {
    let runtime = crate::session::insert_test_live_agent_with_kind(
        &state.sessions,
        id,
        owner(),
        SessionKind::Claude,
    );
    // Test inserts carry no kind; the road reads it, so it is set the way
    // the spawn path sets it for every live session.
    runtime.set_agent_kind(SessionKind::Claude);
    runtime
}

fn set_mode(runtime: &Arc<crate::session::SessionRuntime>, mode: &str) {
    runtime.store_session_manifest(devboule_protocol::SessionEvent::SessionManifest {
        provider_id: Some("claude".to_string()),
        current_model_id: None,
        models: Vec::new(),
        modes: Some(devboule_protocol::SessionModeStateView {
            current_mode_id: mode.to_string(),
            available_modes: Vec::new(),
        }),
        current_model_provider_id: None,
    });
}

fn registration(session_id: &str) -> RegisteredSession {
    RegisteredSession {
        session_id: session_id.to_string(),
        owner: owner(),
        provider_id: Some("d1a-probe-agent".to_string()),
        depth: 0,
        overlay: ToolOverlay::default(),
        bearer: String::new(),
        claude_config_path: None,
        runtime: None,
        broker_ready: Arc::new(AtomicBool::new(true)),
    }
}

fn request() -> AgentCreateRequest {
    AgentCreateRequest::parse(&json!({
        "profile": "Probe",
        "title": "Probe child",
        "initialPrompt": "Do the probe thing.",
    }))
    .expect("request parses")
}

/// One road call on a thread: a parked card is answered the way the
/// person's Allow answers it, and the joined value tells what the road
/// decided. Returns whether a card was raised at all.
fn attempt(state: &Arc<ServerState>, frame: u64) -> (Value, bool) {
    let thread_state = Arc::clone(state);
    let handle = std::thread::spawn(move || {
        create_agent(
            &thread_state,
            &thread_state.mcp,
            &McpCaller::Local,
            &registration("d1a-creator"),
            &json!(frame),
            request(),
        )
    });
    let start = Instant::now();
    loop {
        let mut ids = state
            .sessions
            .live_runtime("d1a-creator", &owner())
            .expect("live session")
            .permission_broker()
            .expect("test broker")
            .test_pending_ids();
        if let Some(card) = ids.pop() {
            answer(state, "d1a-creator", &card);
            let landed = handle.join().expect("creation thread");
            return (landed, true);
        }
        if handle.is_finished() {
            let landed = handle.join().expect("creation thread");
            return (landed, false);
        }
        assert!(
            start.elapsed() < Duration::from_secs(20),
            "the creation neither answered nor carded"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn answer(state: &Arc<ServerState>, session: &str, card: &str) {
    state
        .sessions
        .live_runtime(session, &owner())
        .expect("live session")
        .permission_broker()
        .expect("test broker")
        .test_answer(card, PermissionOutcome::AllowOnce, "allow")
        .expect("answer the card");
}

fn error_text(body: &Value) -> &str {
    assert_eq!(
        body.pointer("/result/isError"),
        Some(&json!(true)),
        "a refused creation is a tool error: {body}"
    );
    body.pointer("/result/content/0/text")
        .and_then(Value::as_str)
        .expect("the refusal sentence")
}

/// A gate a person opened does not outrank a later switch to plan: the mode
/// is read on every call, before any reservation, so the second creation is
/// refused with the plan sentence instead of spawning a child.
#[test]
fn a_plan_switch_after_a_person_opened_the_gate_refuses_the_next_creation() {
    let state = ServerState::new("create-mode-plan".to_string());
    tick_probe_profile(&state);
    let runtime = creator(&state, "d1a-creator");

    // First creation in an asking mode: the person opens the gate, and the
    // spawn behind it fails on the probe's missing command. A "provider not
    // installed" answer means a concurrent refresh replaced the global rows
    // between the road's own read and its resolve — the road re-reads on
    // every call, so the attempt is retried with a fresh frame.
    let first = {
        let mut landed = None;
        for frame in 1..10 {
            // The launchability check reads the global snapshot, which a
            // concurrent refresh elsewhere can replace: re-seed first so
            // the row is live when this attempt reads it.
            seed_probe_provider(&state);
            let (value, carded) = attempt(&state, frame);
            if !carded && error_text(&value) == "provider not installed" {
                continue;
            }
            assert!(carded, "the first creation cards in an asking mode");
            landed = Some(value);
            break;
        }
        landed.expect("the first creation reached its spawn")
    };
    let first_text = error_text(&first);
    assert!(
        !first_text.contains("switch mode"),
        "the first call reaches the spawn behind the person's answer: {first_text}"
    );
    let ticket = state
        .sessions
        .reserve_agent_creation("d1a-creator", 1, true)
        .expect("reservation");
    assert!(
        !ticket.card_owed(),
        "setup: the person's answer opened the gate"
    );
    drop(ticket);

    // The session switches to plan. The next creation is refused naming the
    // mode — on the old road it skipped the mode read with the gate open
    // and ended in the spawn error above instead.
    set_mode(&runtime, "plan");
    let landed = {
        let mut landed = None;
        for frame in 11..40 {
            seed_probe_provider(&state);
            let (value, carded) = attempt(&state, frame);
            assert!(!carded, "plan mode raised a card instead of refusing");
            if error_text(&value) == "provider not installed" {
                continue;
            }
            landed = Some(value);
            break;
        }
        landed.expect("the second creation answered")
    };
    assert_eq!(
        error_text(&landed),
        "This session is in plan mode; switch mode to let the agent proceed.",
        "plan refuses even with an open gate"
    );
}
