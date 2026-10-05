//! `browser_fill_login` when more than one thing is happening to a call: many
//! calls asking one question at once, and a call withdrawn after the person has
//! already answered.

use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use devboule_protocol::{BrowserExecuteRequest, PermissionOutcome};

use super::super::super::browser_tools_harness::{audit_rows, panel, tool_text, FakeHost, Panel};
use super::super::super::tests::{http_request, response_json};
use super::super::first_use::Claim;
use super::tests::{answer, asking, one_entry, preview, wait_for_card, ENTRY, SITE};
use crate::provider_catalog::MCP_BROWSER_FILL_LOGIN_TOOL;

/// One `tools/call` of the tool under its own request id, as an agent that has
/// several in flight writes them.
fn call_as(panel: &Panel, id: u64, arguments: Value) -> JoinHandle<Value> {
    let url = panel.state.mcp.url().to_string();
    let bearer = format!("Bearer {}", panel.token());
    let body = json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": "tools/call",
        "params": {"name": MCP_BROWSER_FILL_LOGIN_TOOL, "arguments": arguments},
    })
    .to_string();
    std::thread::spawn(move || response_json(&http_request(&url, Some(&bearer), &body)))
}

/// The agent's `notifications/cancelled` for one of its requests.
fn cancel(panel: &Panel, id: u64) {
    let url = panel.state.mcp.url().to_string();
    let bearer = format!("Bearer {}", panel.token());
    let body = json!({
        "jsonrpc": "2.0",
        "method": "notifications/cancelled",
        "params": {"requestId": id},
    })
    .to_string();
    http_request(&url, Some(&bearer), &body);
}

/// Every command pushed to the host until `count` have arrived.
fn arrivals(host: &FakeHost, count: usize) -> Vec<BrowserExecuteRequest> {
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut seen = Vec::new();
    while seen.len() < count {
        seen.extend(host.pending());
        assert!(
            Instant::now() < deadline,
            "only {} of {count} arrived",
            seen.len()
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    seen
}

/// Calls that find the same site and the same entry at the same moment are one
/// question, and the person is asked it once.
#[test]
fn calls_racing_one_question_raise_one_card() {
    let tag = "fill-login-race";
    let panel = panel(tag);
    let host = FakeHost::register(&panel.state, 21);
    let replies: Vec<_> = (1..=6)
        .map(|id| call_as(&panel, id, asking(json!({ "passwordRef": "e14" }))))
        .collect();
    for request in arrivals(&host, 6) {
        host.answer_ok(&panel.state, &request, preview(SITE, one_entry()));
    }

    let (card_id, _) = wait_for_card(&panel, tag);
    std::thread::sleep(Duration::from_millis(300));
    let raised = panel
        .state
        .sessions
        .live_runtime("session", &super::tests::owner(tag))
        .and_then(|runtime| runtime.permission_broker())
        .map_or(0, |broker| broker.test_pending_ids().len());
    assert_eq!(raised, 1, "more than one card is waiting on the person");

    answer(
        &panel,
        tag,
        &card_id,
        PermissionOutcome::AllowOnce,
        &format!("once:{ENTRY}"),
    );
    host.answer_ok(
        &panel.state,
        &arrivals(&host, 1)[0],
        json!({ "filled": ["passwordRef"] }),
    );

    let bodies: Vec<Value> = replies
        .into_iter()
        .map(|reply| reply.join().expect("the call"))
        .collect();
    let typed = bodies
        .iter()
        .filter(|body| body.pointer("/result/isError") == Some(&json!(false)))
        .count();
    let waiting = bodies
        .iter()
        .filter(|body| tool_text(body).contains("permission pending"))
        .count();
    assert_eq!((typed, waiting), (1, 5), "{bodies:?}");
}

/// A grant that lets a call straight through is still a call the agent can
/// withdraw while the host is looking at the page: once it has, nothing is
/// typed, and the row says so.
#[test]
fn a_call_withdrawn_after_the_answer_and_before_the_fill_types_nothing() {
    let tag = "fill-login-withdrawn";
    let panel = panel(tag);
    let host = FakeHost::register(&panel.state, 22);
    let first = panel.in_background(
        MCP_BROWSER_FILL_LOGIN_TOOL,
        asking(json!({ "passwordRef": "e14" })),
    );
    host.answer_ok(&panel.state, &host.next(), preview(SITE, one_entry()));
    let (card_id, _) = wait_for_card(&panel, tag);
    answer(
        &panel,
        tag,
        &card_id,
        PermissionOutcome::AllowOnce,
        &format!("session:{ENTRY}"),
    );
    host.answer_ok(
        &panel.state,
        &host.next(),
        json!({ "filled": ["passwordRef"] }),
    );
    first.join().expect("the granting call");

    // The broker finds the session's permission broker through this binding,
    // which the harness's registration does not make.
    let runtime = panel
        .state
        .sessions
        .live_runtime("session", &super::tests::owner(tag))
        .expect("the live session");
    panel.state.mcp.bind_runtime("session", &runtime);
    let second = call_as(&panel, 2, asking(json!({ "passwordRef": "e14" })));
    let looked = host.next();
    cancel(&panel, 2);
    host.answer_ok(&panel.state, &looked, preview(SITE, one_entry()));

    let body = second.join().expect("the withdrawn call");
    assert_eq!(body.pointer("/error/code"), Some(&json!(-32800)), "{body}");
    assert!(host.pending().is_empty(), "the fill was sent to the host");
    assert_eq!(
        audit_rows(&panel.state).last(),
        Some(&("browser_fill_login".to_owned(), "cancelled".to_owned()))
    );
}

/// The claim itself, with every thread released at once: whichever way the
/// threads interleave, one is told to raise the card and the rest to wait.
#[test]
fn eight_threads_claiming_one_group_leave_one_to_raise_the_card() {
    let panel = panel("fill-login-claim");
    let groups = vec![format!("saved_login:{ENTRY}@{SITE}")];
    let gate = std::sync::Barrier::new(8);

    let raised = std::thread::scope(|scope| {
        let claims: Vec<_> = (0..8)
            .map(|_| {
                scope.spawn(|| {
                    gate.wait();
                    panel.state.mcp.claim_gate_marks("session", &groups)
                })
            })
            .collect();
        claims
            .into_iter()
            .map(|claim| claim.join().expect("a claiming thread"))
            .filter(|claim| matches!(claim, Claim::Raised))
            .count()
    });

    assert_eq!(raised, 1);
}
