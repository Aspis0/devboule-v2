//! The browser gate: one card for the session, and what it does and does not
//! stand in front of.
//!
//! Every test here drives the real road — a loopback call and a fake host — so
//! the card is raised, answered and remembered the way a person's answer
//! reaches it.

use std::sync::Arc;
use std::time::Duration;

use serde_json::json;

use super::browser_tools_harness::{panel, tool_text, FakeHost};
use crate::provider_catalog::{
    MCP_BROWSER_ACT_TOOL, MCP_BROWSER_CLICK_TOOL, MCP_BROWSER_FILL_TOOL, MCP_BROWSER_NEW_TAB_TOOL,
    MCP_BROWSER_SNAPSHOT_TOOL,
};
use devboule_protocol::{PermissionOutcome, SessionEvent, SessionKind};

/// A page no local-port rule has an opinion about.
const PAGE: &str = "https://example.test/start";

/// A `browser_new_tab` call as an agent writes it.
fn open_tab() -> serde_json::Value {
    json!({ "url": PAGE })
}

/// The mode the gate reads at call time, recorded the way a provider
/// handshake records it: a manifest row on the live runtime. The mode table
/// reads the session's kind too, which the test sets on the runtime itself.
fn set_mode(runtime: &Arc<crate::session::SessionRuntime>, mode: &str) {
    runtime.store_session_manifest(SessionEvent::SessionManifest {
        provider_id: None,
        current_model_id: None,
        models: Vec::new(),
        modes: Some(devboule_protocol::SessionModeStateView {
            current_mode_id: mode.to_string(),
            available_modes: Vec::new(),
        }),
        current_model_provider_id: None,
    });
}

/// The first browser action of a session asks once, and every later action of
/// every kind runs without asking again.
#[test]
fn one_card_opens_the_gate_for_every_later_action() {
    let panel = panel("browser-gate-once");
    let host = FakeHost::register(&panel.state, 4);
    let first = panel.in_background(MCP_BROWSER_NEW_TAB_TOOL, open_tab());
    let (card, request) = panel.wait_for_card();

    // The card names the session asking, and says what allowing buys.
    let SessionEvent::PermissionRequest {
        title,
        description,
        options,
        ..
    } = &request
    else {
        panic!("the browser gate raises a permission request");
    };
    assert!(
        title.ends_with(" wants to use the browser"),
        "the card names who is asking: {title}"
    );
    let description = description.as_deref().unwrap_or_default();
    assert!(
        description.contains("for the rest of this session without asking again"),
        "the card says what an allow buys: {description}"
    );
    assert_eq!(
        options
            .iter()
            .map(|option| (option.option_id.as_str(), option.kind.as_str()))
            .collect::<Vec<_>>(),
        vec![("session", "allow_session"), ("deny", "reject_once")],
        "the card offers one allow and one refusal"
    );
    assert_eq!(
        panel.pending_cards(),
        vec![card.clone()],
        "one card for the session, not one per act"
    );

    panel.answer(&card, PermissionOutcome::AllowOnce, "session");
    let request = host.next();
    host.answer_ok(&panel.state, &request, json!({"browserId": "tab-1"}));
    let opened = first.join().expect("first call");
    assert_eq!(opened.pointer("/result/isError"), Some(&json!(false)));

    // Three other acts of the lane, each a different tool: none of them asks.
    for (id, (tool, arguments)) in [
        (
            2,
            (
                MCP_BROWSER_ACT_TOOL,
                json!({"browserId": "tab-1", "steps": [{"command": "click", "ref": "e3"}]}),
            ),
        ),
        (
            3,
            (
                MCP_BROWSER_CLICK_TOOL,
                json!({"browserId": "tab-1", "ref": "e3"}),
            ),
        ),
        (
            4,
            (
                MCP_BROWSER_FILL_TOOL,
                json!({"browserId": "tab-1", "ref": "e4", "text": "hello"}),
            ),
        ),
    ] {
        let later = panel.in_background_as(id, tool, arguments);
        let request = host.next();
        host.answer_ok(&panel.state, &request, json!({"ok": true}));
        assert_eq!(
            later.join().expect("later call").pointer("/result/isError"),
            Some(&json!(false)),
            "{tool} ran on the grant"
        );
        assert!(
            panel.pending_cards().is_empty(),
            "{tool} asked again on an open gate"
        );
    }
}

/// Denying refuses that call and nothing else: the agent reads a tool error,
/// the host never sees the command, and the next browser intent asks again.
#[test]
fn a_refusal_answers_this_call_only() {
    let panel = panel("browser-gate-deny");
    let host = FakeHost::register(&panel.state, 5);
    let refused = panel.in_background(MCP_BROWSER_NEW_TAB_TOOL, open_tab());
    let (card, _) = panel.wait_for_card();
    panel.answer(&card, PermissionOutcome::Deny, "deny");
    let reply = refused.join().expect("refused call");
    assert_eq!(
        reply.pointer("/result/isError"),
        Some(&json!(true)),
        "a refusal is a tool error the agent reads: {reply}"
    );
    assert!(
        tool_text(&reply).contains("permission refused"),
        "the refusal says why: {reply}"
    );
    assert!(
        host.pending().is_empty(),
        "the refused command never reached the host"
    );

    // The next intent asks again: a refusal is not an answer about the next one.
    let again = panel.in_background(MCP_BROWSER_NEW_TAB_TOOL, open_tab());
    let (card, _) = panel.wait_for_card();
    panel.answer(&card, PermissionOutcome::AllowOnce, "session");
    let request = host.next();
    host.answer_ok(&panel.state, &request, json!({"browserId": "tab-1"}));
    assert_eq!(
        again
            .join()
            .expect("second call")
            .pointer("/result/isError"),
        Some(&json!(false))
    );
}

/// A card with the question still on the table is one question: the second
/// call is told to come back rather than spending the person's attention twice.
#[test]
fn a_second_call_finds_the_same_card_rather_than_raising_one() {
    let panel = panel("browser-gate-pending");
    let host = FakeHost::register(&panel.state, 6);
    let first = panel.in_background(MCP_BROWSER_NEW_TAB_TOOL, open_tab());
    let (card, _) = panel.wait_for_card();

    let second = panel.in_background_as(
        2,
        MCP_BROWSER_CLICK_TOOL,
        json!({"browserId": "tab-1", "ref": "e3"}),
    );
    let second_reply = second.join().expect("second call");
    assert_eq!(
        tool_text(&second_reply),
        "permission pending; retry",
        "the racing call waits for the answer instead of asking again"
    );
    assert_eq!(
        panel.pending_cards(),
        vec![card.clone()],
        "the one card out is the one that was already raised"
    );

    panel.answer(&card, PermissionOutcome::AllowOnce, "session");
    let request = host.next();
    host.answer_ok(&panel.state, &request, json!({"browserId": "tab-1"}));
    assert_eq!(
        first.join().expect("first call").pointer("/result/isError"),
        Some(&json!(false))
    );
}

/// Reading a page is not behind the gate: a session that has never been asked
/// reads one without a card.
#[test]
fn the_read_lane_never_asks() {
    let panel = panel("browser-gate-read");
    let host = FakeHost::register(&panel.state, 7);
    let (reply, _) = panel.call(
        &host,
        MCP_BROWSER_SNAPSHOT_TOOL,
        json!({"browserId": "tab-1"}),
        json!({"refs": []}),
    );
    assert_eq!(reply.pointer("/result/isError"), Some(&json!(false)));
    assert!(
        panel.pending_cards().is_empty(),
        "a read is not the browser gate's business"
    );
}

/// An automatic mode opens the gate without a card, and remembers it, so the
/// rest of the session is the same one lookup.
#[test]
fn an_automatic_mode_asks_nothing_and_still_opens_the_gate() {
    let panel = panel("browser-gate-auto");
    let host = FakeHost::register(&panel.state, 8);
    panel.runtime().set_agent_kind(SessionKind::Acp);
    set_mode(&panel.runtime(), "auto_accept");

    for _ in 0..2 {
        let call = panel.in_background(MCP_BROWSER_NEW_TAB_TOOL, open_tab());
        let request = host.next();
        host.answer_ok(&panel.state, &request, json!({"browserId": "tab-1"}));
        assert_eq!(
            call.join().expect("call").pointer("/result/isError"),
            Some(&json!(false))
        );
        std::thread::sleep(Duration::from_millis(20));
        assert!(
            panel.pending_cards().is_empty(),
            "an automatic mode raises no card"
        );
    }
}
