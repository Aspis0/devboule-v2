//! The browser gate: one card for the session, and what it does and does not
//! stand in front of.
//!
//! Every test here drives the real road — a loopback call and a fake host — so
//! the card is raised, answered and remembered the way a person's answer
//! reaches it.

use std::sync::Arc;
use std::time::Duration;

use serde_json::json;

use super::browser_tools_harness::{audit_rows, panel, tool_text, FakeHost};
use super::{AgentLineage, ServerState};
use crate::mcp_broker::tools::first_use::{GateMark, BROWSER_GROUP};
use crate::provider_catalog::{
    MCP_BROWSER_ACT_TOOL, MCP_BROWSER_CLICK_TOOL, MCP_BROWSER_FILL_TOOL, MCP_BROWSER_NEW_TAB_TOOL,
    MCP_BROWSER_SNAPSHOT_TOOL,
};
use devboule_protocol::{OwnerId, PermissionOutcome, SessionEvent, SessionKind};

/// The session the panel's own caller registers under.
const SESSION: &str = "session";
/// A page no local-port rule has an opinion about.
const PAGE: &str = "https://example.test/start";

/// A `browser_new_tab` call as an agent writes it.
fn open_tab() -> serde_json::Value {
    json!({ "url": PAGE })
}

/// A click as an agent writes it.
fn click(ref_name: &str) -> serde_json::Value {
    json!({ "browserId": "tab-1", "ref": ref_name })
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

    // The card names the session asking, and says exactly what an allow buys.
    let SessionEvent::PermissionRequest {
        title,
        description,
        options,
        ..
    } = &request
    else {
        panic!("the browser gate raises a permission request");
    };
    assert_eq!(
        title, "Agent wants to use the browser",
        "the card names the session itself, not an agent: {title}"
    );
    let description = description.as_deref().unwrap_or_default();
    let expected = "Allowing lets Agent drive the browser — open tabs, go to pages, click, type — \
         for the rest of this session without asking again to drive it. You will still be \
         asked about saved logins and about ports this session does not own. Denying \
         refuses this call; the next browser action asks again.";
    assert_eq!(
        description, expected,
        "the card prices the grant and names the two questions it keeps"
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
        (3, (MCP_BROWSER_CLICK_TOOL, click("e3"))),
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
    assert!(
        audit_rows(&panel.state).contains(&(
            MCP_BROWSER_NEW_TAB_TOOL.to_string(),
            "ok; approved by person".to_string()
        )),
        "the row names the person who opened the gate"
    );
}

/// Denying answers one call and not the lane: the agent reads a tool error,
/// the host never sees that command, and a different browser act asks again.
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

    // Another tool of the lane, not the one that was refused: the lane asks
    // again, because a refusal answered a call and not the browser.
    let again = panel.in_background(MCP_BROWSER_CLICK_TOOL, click("e3"));
    let (card, _) = panel.wait_for_card();
    panel.answer(&card, PermissionOutcome::AllowOnce, "session");
    host.answer_ok(&panel.state, &host.next(), json!({"clicked": true}));
    assert_eq!(
        again
            .join()
            .expect("second call")
            .pointer("/result/isError"),
        Some(&json!(false)),
        "the other act asked, was allowed, and ran"
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

    let second = panel.in_background_as(2, MCP_BROWSER_CLICK_TOOL, click("e3"));
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
    assert!(
        audit_rows(&panel.state)
            .contains(&(MCP_BROWSER_SNAPSHOT_TOOL.to_string(), "ok".to_string())),
        "a read's row names no approver, because nobody was asked"
    );
}

/// An automatic mode asks nothing and stores nothing: the mode is read at
/// every call, so a session switched back to an asking one is asked again
/// rather than keeping a browser a mode it left lent it.
#[test]
fn an_automatic_mode_asks_nothing_and_stores_nothing() {
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
    assert_eq!(
        panel.state.mcp.first_use_mark(SESSION, BROWSER_GROUP),
        None,
        "an automatic mode stores no grant of its own"
    );
    assert!(
        audit_rows(&panel.state).contains(&(
            MCP_BROWSER_NEW_TAB_TOOL.to_string(),
            "ok; approved by automatic mode".to_string()
        )),
        "the row names the mode that approved the act"
    );

    // The switch is a real road, and the next act asks again.
    set_mode(&panel.runtime(), "auto_accept_off");
    let asking = panel.in_background(MCP_BROWSER_CLICK_TOOL, click("e3"));
    let (card, _) = panel.wait_for_card();
    panel.answer(&card, PermissionOutcome::AllowOnce, "session");
    host.answer_ok(&panel.state, &host.next(), json!({"clicked": true}));
    assert_eq!(
        asking
            .join()
            .expect("the asking call")
            .pointer("/result/isError"),
        Some(&json!(false)),
        "an asking mode asked again after the switch"
    );
}

/// A mode that does not act refuses before any card: the call never reaches a
/// person, and the sentence names the mode the session is in.
#[test]
fn a_plan_mode_refuses_a_browser_act_without_a_card() {
    let panel = panel("browser-gate-plan");
    let host = FakeHost::register(&panel.state, 9);
    panel.runtime().set_agent_kind(SessionKind::Claude);
    set_mode(&panel.runtime(), "plan");

    let call = panel.in_background(MCP_BROWSER_NEW_TAB_TOOL, open_tab());
    let reply = call.join().expect("the refused call");
    assert_eq!(reply.pointer("/result/isError"), Some(&json!(true)));
    assert!(
        tool_text(&reply).contains("plan mode"),
        "the refusal names the mode: {reply}"
    );
    assert!(
        panel.pending_cards().is_empty() && host.pending().is_empty(),
        "a mode that does not act spends no card and never reaches the host"
    );
}

/// The gate belongs to the session that was asked: one session's answer opens
/// nothing for another, and the two keep their cards apart.
#[test]
fn two_sessions_hold_two_gates() {
    let mut panel = panel("browser-gate-scopes");
    let host = FakeHost::register(&panel.state, 10);
    let second = panel.join_caller("other", "w-other");
    let first = panel.caller();

    let allowed = first.in_background(&panel.state, 1, MCP_BROWSER_NEW_TAB_TOOL, open_tab());
    let (card, _) = first.wait_for_card(&panel.state);
    first.answer(&panel.state, &card, PermissionOutcome::AllowOnce, "session");
    host.answer_ok(&panel.state, &host.next(), json!({"browserId": "tab-1"}));
    assert_eq!(
        allowed
            .join()
            .expect("the allowed call")
            .pointer("/result/isError"),
        Some(&json!(false))
    );

    let asked = second.in_background(&panel.state, 1, MCP_BROWSER_NEW_TAB_TOOL, open_tab());
    let (card, _) = second.wait_for_card(&panel.state);
    second.answer(&panel.state, &card, PermissionOutcome::Deny, "deny");
    assert_eq!(
        asked
            .join()
            .expect("the refused call")
            .pointer("/result/isError"),
        Some(&json!(true)),
        "one session's answer granted nothing to the other"
    );
    assert!(
        first.pending_cards(&panel.state).is_empty()
            && second.pending_cards(&panel.state).is_empty(),
        "each session answered its own card and holds none"
    );
}

/// The grant is the session's, so it goes when the session goes: a later
/// session never inherits a browser nobody asked it for.
#[test]
fn ending_a_session_forgets_its_grant() {
    let state = ServerState::new("browser-gate-teardown".to_string());
    let owner = OwnerId::new("browser-user-teardown", "browser-client").expect("owner");
    let guard = state
        .mcp
        .register_with_provider(
            "teardown-session",
            &owner,
            &SessionKind::Acp,
            None,
            AgentLineage::root(),
        )
        .expect("registration")
        .expect("MCP guard");
    state
        .mcp
        .open_first_use_gate_for_test("teardown-session", BROWSER_GROUP);
    assert_eq!(
        state.mcp.first_use_mark("teardown-session", BROWSER_GROUP),
        Some(GateMark::Open),
        "the grant is there while the session lives"
    );

    drop(guard);

    assert_eq!(
        state.mcp.first_use_mark("teardown-session", BROWSER_GROUP),
        None,
        "the session ended, so its browser grant went with it"
    );
}
