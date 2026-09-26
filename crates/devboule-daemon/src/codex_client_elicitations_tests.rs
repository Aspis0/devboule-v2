//! Tests for Codex `mcpServer/elicitation/request` approvals: the ordinary
//! card, its three answers, the immediate declines, the child-creation
//! approval, and the cross-spawn card identity.

use std::sync::Arc;

use devboule_protocol::SessionEvent;

use super::super::codex_elicitations::codex_elicitation_result;
use super::input_test_support::{asked_elicitation, elicitation_line, question_harness};

#[test]
fn elicitation_accept_decline_and_cancel() {
    let (broker, captured, runtime, conn, mut reader) = question_harness();
    for (index, tool_call) in [
        "mcp-elicitation-1",
        "mcp-elicitation-2",
        "mcp-elicitation-3",
    ]
    .iter()
    .enumerate()
    {
        let mut line = elicitation_line("Allow the devboule MCP server to run a tool?");
        line["id"] = serde_json::json!(format!("server-{index}"));
        line["params"]["elicitationId"] = serde_json::json!(tool_call);
        reader.dispatch_value(line, &runtime);
    }
    let events = conn.pull_events();
    let cards = events
        .iter()
        .filter(|event| matches!(event.envelope.event, SessionEvent::PermissionRequest { .. }))
        .count();
    assert_eq!(cards, 3);
    assert_eq!(broker.pending_len(), 3);
    // The card is an ordinary Allow/Deny pair on the agent's own message.
    match asked_elicitation(&events, "mcp-elicitation-1") {
        SessionEvent::PermissionRequest {
            title, description, ..
        } => {
            assert_eq!(title, "MCP approval: devboule");
            assert_eq!(
                description.as_deref(),
                Some("Allow the devboule MCP server to run a tool?")
            );
        }
        _ => panic!("expected a permission request"),
    }
    broker
        .respond_with_option(
            "mcp-elicitation-1",
            devboule_protocol::PermissionOutcome::AllowOnce,
            Some("allow".to_string()),
            None,
        )
        .expect("accept");
    broker
        .respond_with_option(
            "mcp-elicitation-2",
            devboule_protocol::PermissionOutcome::Deny,
            Some("deny".to_string()),
            None,
        )
        .expect("decline");
    broker.close();
    let frames = captured.lock().expect("captured");
    assert_eq!(frames.len(), 3);
    assert_eq!(
        frames[0]["result"],
        serde_json::json!({ "action": "accept", "content": {}, "_meta": null })
    );
    assert_eq!(
        frames[1]["result"],
        serde_json::json!({ "action": "decline", "content": null, "_meta": null })
    );
    assert_eq!(
        frames[2]["result"],
        serde_json::json!({ "action": "cancel", "content": null, "_meta": null })
    );
}

#[test]
fn elicitation_url_and_required_decline_at_once() {
    let (broker, captured, runtime, conn, mut reader) = question_harness();
    let mut url = elicitation_line("Open this page?");
    url["params"]["mode"] = serde_json::json!("url");
    reader.dispatch_value(url, &runtime);
    let mut required = elicitation_line("Fill this in?");
    required["params"]["requestedSchema"] =
        serde_json::json!({"type": "object", "required": ["name"]});
    reader.dispatch_value(required, &runtime);
    assert_eq!(broker.pending_len(), 0);
    assert!(!conn
        .pull_events()
        .iter()
        .any(|event| matches!(event.envelope.event, SessionEvent::PermissionRequest { .. })));
    assert!(captured.lock().expect("captured").is_empty());
    // The declined bytes are pinned, not the stdin write (which has no
    // child in tests): decline carries no content, cancel neither.
    assert_eq!(
        codex_elicitation_result(
            &serde_json::json!({ "outcome": { "outcome": "selected", "optionId": "deny" } })
        ),
        serde_json::json!({ "action": "decline", "content": null, "_meta": null })
    );
}

#[test]
fn create_agent_elicitation_accepts() {
    // The MCP approval a Codex child creation rides: the card appears, and
    // accepting answers accept. Child creation itself happens downstream of
    // this reply and is not exercised here.
    let (broker, captured, runtime, conn, mut reader) = question_harness();
    reader.dispatch_value(
        elicitation_line("Allow the devboule MCP server to run tool \"devboule_create_agent\"?"),
        &runtime,
    );
    let events = conn.pull_events();
    let card = events
        .iter()
        .find_map(|event| match &event.envelope.event {
            SessionEvent::PermissionRequest { .. } => Some(event.envelope.event.clone()),
            _ => None,
        })
        .expect("MCP approval card");
    match card {
        SessionEvent::PermissionRequest { title, .. } => {
            assert_eq!(title, "MCP approval: devboule");
        }
        _ => unreachable!(),
    }
    let tool_call_id = events
        .iter()
        .find_map(|event| match &event.envelope.event {
            SessionEvent::PermissionRequest { tool_call_id, .. } => Some(tool_call_id.clone()),
            _ => None,
        })
        .expect("no tool call id");
    broker
        .respond_with_option(
            &tool_call_id,
            devboule_protocol::PermissionOutcome::AllowOnce,
            Some("allow".to_string()),
            None,
        )
        .expect("accept the MCP approval");
    let frames = captured.lock().expect("captured");
    assert_eq!(
        frames[0]["result"],
        serde_json::json!({ "action": "accept", "content": {}, "_meta": null })
    );
}

#[test]
fn resumed_session_raises_a_new_card_for_its_first_elicitation() {
    // A form elicitation carries no elicitationId, so the card id is
    // generated — and it must be unique across spawns of one session. The
    // journal's reused-id ledger is sticky per session while the request
    // counter restarts, so a bare counter answers the resumed session's
    // first approval with `cancel` and no card.
    if let Some(reason) = crate::test_support::external_program_skip_reason("node") {
        eprintln!("{reason}");
        return;
    }
    use super::super::codex_elicitations::dispatch_elicitation;
    use super::input_test_support::echo_harness;
    use crate::journal::Journal;

    let dir = crate::test_dirs::test_temp_dir("devboule-codex-resume");
    let path = dir.join("resume.sqlite");
    let _ = std::fs::remove_file(&path);
    let journal = Arc::new(Journal::open(&path).expect("journal"));
    let line = || elicitation_line("Allow the devboule MCP server to run a tool?");

    // First spawn: card, answer, journal row. The generated id is
    // production-made (process, time, counter): the test never chooses it.
    let mut first = echo_harness("s.codex.resume", Some(Arc::clone(&journal)));
    dispatch_elicitation(&first.deps(), &line(), &first.runtime, None);
    let first_id = first
        .conn
        .pull_events()
        .into_iter()
        .find_map(|event| match event.envelope.event {
            SessionEvent::PermissionRequest { tool_call_id, .. } => Some(tool_call_id),
            _ => None,
        })
        .expect("first card");
    assert!(
        first_id.starts_with("mcp-elicitation"),
        "first spawn raises its card: {first_id}"
    );
    first
        .broker
        .respond_with_option(
            &first_id,
            devboule_protocol::PermissionOutcome::AllowOnce,
            Some("allow".to_string()),
            None,
        )
        .expect("accept");
    assert_eq!(
        first.read_frame()["result"],
        serde_json::json!({ "action": "accept", "content": {}, "_meta": null })
    );
    journal.flush().expect("journal flush");

    // Second spawn, same session and journal: the per-spawn counter
    // restarts, but the generated id does not repeat, so the card is raised
    // instead of refused as a reused id.
    let second = echo_harness("s.codex.resume", Some(Arc::clone(&journal)));
    dispatch_elicitation(&second.deps(), &line(), &second.runtime, None);
    let second_id = second
        .conn
        .pull_events()
        .into_iter()
        .find_map(|event| match event.envelope.event {
            SessionEvent::PermissionRequest { tool_call_id, .. } => Some(tool_call_id),
            _ => None,
        })
        .expect("resumed session raises a new card");
    assert_ne!(second_id, first_id);

    journal.shutdown();
    let _ = std::fs::remove_file(path);
    let _ = std::fs::remove_dir_all(dir);
}
