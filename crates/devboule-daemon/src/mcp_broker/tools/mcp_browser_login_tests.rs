//! `browser_fill_login`: the two host commands it rides on, the consent in
//! front of it, and what the audit row keeps of it.
//!
//! The host is a fake standing where the app stands, so the daemon is driven
//! through the frames it really sends. The sentinel stands for a password in
//! whatever the host answers, and every frame, result and row below is checked
//! for it.

use serde_json::{json, Value};

use devboule_protocol::{OwnerId, PermissionOutcome, SessionEvent};

use super::super::super::browser_tools_harness::{
    audit_rows, host_document, host_refusal, panel, tool_text, FakeHost, Panel,
};
use crate::provider_catalog::MCP_BROWSER_FILL_LOGIN_TOOL;

/// A value that is obviously a test's, standing in for a password in whatever
/// the host answers.
const SECRET: &str = "SENTINEL-PW-7f3a";
pub(super) const SITE: &str = "https://shop.example.test";
pub(super) const ENTRY: &str = "aaaa0000bbbb1111cccc2222dddd3333";
const OTHER: &str = "eeee4444ffff5555aaaa6666bbbb7777";

/// The owner the harness registered the session under, which is who a card
/// raised on it is looked up by.
pub(super) fn owner(tag: &str) -> OwnerId {
    OwnerId::new(format!("browser-user-{tag}"), "browser-client").expect("owner")
}

/// What the app answers when the daemon asks what a site may offer.
pub(super) fn preview(origin: &str, entries: Value) -> Value {
    json!({ "origin": origin, "entries": entries })
}

pub(super) fn one_entry() -> Value {
    json!([{ "id": ENTRY, "label": "Shop account" }])
}

pub(super) fn asking(refs: Value) -> Value {
    let mut args = refs;
    args["browserId"] = json!("tab-1");
    args
}

/// The consent card this tool raised on the session, waited for.
pub(super) fn wait_for_card(panel: &Panel, tag: &str) -> (String, SessionEvent) {
    for _ in 0..900 {
        if let Some(runtime) = panel.state.sessions.live_runtime("session", &owner(tag)) {
            if let Some(broker) = runtime.permission_broker() {
                if let Some(card_id) = broker.test_pending_ids().pop() {
                    let card = broker
                        .test_pending_request(&card_id)
                        .expect("the pending card");
                    return (card_id, card);
                }
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    panic!("the saved-login card was never raised");
}

pub(super) fn answer(
    panel: &Panel,
    tag: &str,
    card_id: &str,
    outcome: PermissionOutcome,
    option: &str,
) {
    panel
        .state
        .sessions
        .live_runtime("session", &owner(tag))
        .expect("live session")
        .permission_broker()
        .expect("test broker")
        .test_answer(card_id, outcome, option)
        .expect("answer the saved-login card");
}

/// The card's title, which names the login and the site a person is approving.
fn title_of(card: &SessionEvent) -> String {
    let SessionEvent::PermissionRequest { title, .. } = card else {
        panic!("a saved-login card is a permission request");
    };
    title.clone()
}

/// The card's option ids, in the order the person sees them.
fn options_of(card: &SessionEvent) -> Vec<String> {
    let SessionEvent::PermissionRequest { options, .. } = card else {
        panic!("a saved-login card is a permission request");
    };
    options
        .iter()
        .map(|option| option.option_id.clone())
        .collect()
}

#[test]
fn the_card_names_the_login_and_the_site_and_the_fill_carries_only_the_entry_id() {
    let tag = "fill-login";
    let panel = panel(tag);
    let host = FakeHost::register(&panel.state, 9);
    let reply = panel.in_background(
        MCP_BROWSER_FILL_LOGIN_TOOL,
        asking(json!({ "usernameRef": "e13", "passwordRef": "e14" })),
    );

    let previewed = host.next();
    assert_eq!(previewed.command, "fill_login_preview");
    assert_eq!(
        previewed.args,
        json!({ "browserId": "tab-1", "usernameRef": "e13", "passwordRef": "e14" }),
        "the preview carries what the agent asked for and nothing else"
    );
    host.answer_ok(&panel.state, &previewed, preview(SITE, one_entry()));

    let (card_id, card) = wait_for_card(&panel, tag);
    assert_eq!(
        title_of(&card),
        format!("Use saved login 'Shop account' on {SITE}?"),
        "{card:?}"
    );
    assert_eq!(
        options_of(&card),
        vec![format!("once:{ENTRY}"), "deny".to_owned()],
        "this one call, and the refusal: no grant for a session exists"
    );
    let said = format!("{card:?}");
    assert!(!said.contains(SECRET), "the card holds no password: {said}");

    answer(
        &panel,
        tag,
        &card_id,
        PermissionOutcome::AllowOnce,
        &format!("once:{ENTRY}"),
    );
    let filled = host.next();
    assert_eq!(filled.command, "fill_login");
    assert_eq!(
        filled.args,
        json!({
            "browserId": "tab-1",
            "usernameRef": "e13",
            "passwordRef": "e14",
            "entryId": ENTRY,
        }),
        "the entry the person chose, and nothing else added"
    );
    host.answer_ok(
        &panel.state,
        &filled,
        json!({ "filled": ["usernameRef", "passwordRef"] }),
    );

    let body = reply.join().expect("the tool call");
    assert_eq!(
        tool_text(&body),
        r#"{"filled":["usernameRef","passwordRef"]}"#
    );
}

/// The answer the agent reads is constant: what was filled, and nothing the
/// page said.
#[test]
fn the_agent_reads_only_the_arguments_that_were_filled() {
    let tag = "fill-login-answer";
    let panel = panel(tag);
    let host = FakeHost::register(&panel.state, 10);
    let reply = panel.in_background(
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
        &format!("once:{ENTRY}"),
    );
    host.answer_ok(
        &panel.state,
        &host.next(),
        json!({"filled": ["passwordRef"]}),
    );

    let body = reply.join().expect("the tool call");
    assert_eq!(host_document(&body), json!({ "filled": ["passwordRef"] }));
    assert_eq!(body.pointer("/result/isError"), Some(&json!(false)));
}

/// A site this machine has no login for is refused before a card is raised,
/// and the refusal names the site and no entry.
#[test]
fn a_site_with_no_saved_login_answers_before_any_card() {
    let panel = panel("fill-login-none");
    let host = FakeHost::register(&panel.state, 11);
    let reply = panel.in_background(
        MCP_BROWSER_FILL_LOGIN_TOOL,
        asking(json!({ "passwordRef": "e14" })),
    );
    host.answer_ok(&panel.state, &host.next(), preview(SITE, json!([])));

    let body = reply.join().expect("the tool call");

    assert_eq!(body.pointer("/result/isError"), Some(&json!(true)));
    let said = tool_text(&body);
    assert!(said.starts_with("no_saved_login:"), "{said}");
    assert!(said.contains(SITE), "{said}");
    assert!(host.pending().is_empty(), "the typing is never asked for");
}

/// A person who says no types nothing, and no mark is left behind: the next
/// call asks again rather than finding a grant nobody gave.
#[test]
fn a_refused_card_fills_nothing_and_asks_again_next_time() {
    let tag = "fill-login-deny";
    let panel = panel(tag);
    let host = FakeHost::register(&panel.state, 12);
    let first = panel.in_background(
        MCP_BROWSER_FILL_LOGIN_TOOL,
        asking(json!({ "passwordRef": "e14" })),
    );
    host.answer_ok(&panel.state, &host.next(), preview(SITE, one_entry()));
    let (card_id, _) = wait_for_card(&panel, tag);
    answer(&panel, tag, &card_id, PermissionOutcome::Deny, "deny");
    let refused = first.join().expect("the first call");
    assert_eq!(refused.pointer("/result/isError"), Some(&json!(true)));
    assert!(host.pending().is_empty(), "the typing was never asked for");

    let second = panel.in_background(
        MCP_BROWSER_FILL_LOGIN_TOOL,
        asking(json!({ "passwordRef": "e14" })),
    );
    host.answer_ok(&panel.state, &host.next(), preview(SITE, one_entry()));
    let (again, _) = wait_for_card(&panel, tag);
    answer(&panel, tag, &again, PermissionOutcome::Deny, "deny");
    assert_eq!(
        second
            .join()
            .expect("the second call")
            .pointer("/result/isError"),
        Some(&json!(true)),
        "a refusal opens nothing, so the next call asks again"
    );
}

/// The person is asked on every use, in an automatic mode too: the second call
/// on the same site, for the same login, in the same session raises its own
/// card — nothing a first answer said carries over.
#[test]
fn saved_login_cards_every_use_in_auto_mode() {
    let tag = "fill-login-auto";
    let panel = panel(tag);
    set_automatic_mode(&panel, tag);
    let host = FakeHost::register(&panel.state, 13);
    let run = |panel: &Panel| {
        let reply = panel.in_background(
            MCP_BROWSER_FILL_LOGIN_TOOL,
            asking(json!({ "passwordRef": "e14" })),
        );
        host.answer_ok(&panel.state, &host.next(), preview(SITE, one_entry()));
        let (card_id, card) = wait_for_card(panel, tag);
        let SessionEvent::PermissionRequest { options, .. } = &card else {
            panic!("a saved-login card is a permission request");
        };
        assert!(
            options.iter().all(|option| option.kind != "allow_session"),
            "no answer to this card is remembered: {options:?}"
        );
        answer(
            panel,
            tag,
            &card_id,
            PermissionOutcome::AllowOnce,
            &format!("once:{ENTRY}"),
        );
        host.answer_ok(
            &panel.state,
            &host.next(),
            json!({ "filled": ["passwordRef"] }),
        );
        reply.join().expect("the call")
    };

    for use_number in 1..=3 {
        assert_eq!(
            run(&panel).pointer("/result/isError"),
            Some(&json!(false)),
            "use {use_number} raised its own card and was typed after the answer"
        );
    }
}

/// An automatic mode: what a provider calls bypass, here as a recorded
/// handshake mode on the live session, which is what the gate reads.
fn set_automatic_mode(panel: &Panel, tag: &str) {
    let runtime = panel
        .state
        .sessions
        .live_runtime("session", &owner(tag))
        .expect("the live session");
    runtime.set_agent_kind(devboule_protocol::SessionKind::Claude);
    runtime.store_session_manifest(devboule_protocol::SessionEvent::SessionManifest {
        provider_id: Some("claude".to_string()),
        current_model_id: None,
        models: Vec::new(),
        modes: Some(devboule_protocol::SessionModeStateView {
            current_mode_id: "bypassPermissions".to_string(),
            available_modes: Vec::new(),
        }),
        current_model_provider_id: None,
    });
}

/// Two logins for one site: the card offers both, and the answer decides which
/// one is typed.
#[test]
fn two_logins_for_one_site_are_chosen_on_the_card() {
    let tag = "fill-login-two";
    let panel = panel(tag);
    let host = FakeHost::register(&panel.state, 14);
    let reply = panel.in_background(
        MCP_BROWSER_FILL_LOGIN_TOOL,
        asking(json!({ "passwordRef": "e14" })),
    );
    host.answer_ok(
        &panel.state,
        &host.next(),
        preview(
            SITE,
            json!([
                { "id": ENTRY, "label": "Shop account" },
                { "id": OTHER, "label": "Shop test account" },
            ]),
        ),
    );
    let (card_id, card) = wait_for_card(&panel, tag);
    assert_eq!(title_of(&card), format!("Use a saved login on {SITE}?"));
    assert_eq!(
        options_of(&card),
        vec![
            format!("once:{ENTRY}"),
            format!("once:{OTHER}"),
            "deny".to_owned(),
        ],
        "both logins, then the refusal"
    );
    answer(
        &panel,
        tag,
        &card_id,
        PermissionOutcome::AllowOnce,
        &format!("once:{OTHER}"),
    );
    let filled = host.next();
    assert_eq!(
        filled.args["entryId"],
        json!(OTHER),
        "the one that was chosen"
    );
    host.answer_ok(&panel.state, &filled, json!({ "filled": ["passwordRef"] }));
    assert_eq!(
        reply.join().expect("the call").pointer("/result/isError"),
        Some(&json!(false))
    );
}

/// The audit row says which login went to which site and how it ended. It says
/// nothing else: no username, no password, and none of what a page said.
#[test]
fn the_audit_row_names_the_entry_the_origin_and_the_outcome_and_nothing_else() {
    let tag = "fill-login-audit";
    let panel = panel(tag);
    let host = FakeHost::register(&panel.state, 15);
    let reply = panel.in_background(
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
        &format!("once:{ENTRY}"),
    );
    host.answer_ok(
        &panel.state,
        &host.next(),
        json!({ "filled": ["passwordRef"] }),
    );
    reply.join().expect("the call");

    let rows = audit_rows(&panel.state);
    let outcome = rows
        .iter()
        .find(|(action, _)| action == MCP_BROWSER_FILL_LOGIN_TOOL)
        .map(|(_, outcome)| outcome.clone())
        .expect("a row for the fill");
    assert!(outcome.contains(ENTRY), "the entry: {outcome}");
    assert!(outcome.contains(SITE), "the site: {outcome}");
    assert!(
        !outcome.contains(SECRET),
        "no password in the row: {outcome}"
    );
    assert!(
        !outcome.contains("person@"),
        "no username in the row: {outcome}"
    );
}

/// A host that answers a refusal with a password in it reaches the agent as
/// the host wrote it, and stops there: the audit row keeps the code.
#[test]
fn a_refusal_the_page_wrote_never_reaches_the_audit_row() {
    let tag = "fill-login-refusal";
    let panel = panel(tag);
    let host = FakeHost::register(&panel.state, 16);
    let reply = panel.in_background(
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
        &format!("once:{ENTRY}"),
    );
    host.answer_error(
        &panel.state,
        &host.next(),
        host_refusal(&format!("the page said {SECRET}")),
    );

    let body = reply.join().expect("the tool call");
    assert!(
        tool_text(&body).contains(SECRET),
        "the host's own sentence reaches the agent: {}",
        tool_text(&body)
    );
    for (action, outcome) in audit_rows(&panel.state) {
        if action == MCP_BROWSER_FILL_LOGIN_TOOL {
            assert!(!outcome.contains(SECRET), "the row kept it: {outcome}");
        }
    }
}

/// No frame this tool sends the host can carry a password: the agent chooses
/// fields, the daemon chooses the entry the person picked, and neither of
/// them ever holds one.
#[test]
fn the_frames_this_tool_sends_name_no_password() {
    let tag = "fill-login-frames";
    let panel = panel(tag);
    let host = FakeHost::register(&panel.state, 17);
    let reply = panel.in_background(
        MCP_BROWSER_FILL_LOGIN_TOOL,
        asking(json!({ "passwordRef": "e14" })),
    );
    let previewed = host.next();
    let (card_id, _) = {
        host.answer_ok(
            &panel.state,
            &previewed,
            preview(SITE, json!([{ "id": ENTRY, "label": SECRET }])),
        );
        wait_for_card(&panel, tag)
    };
    answer(
        &panel,
        tag,
        &card_id,
        PermissionOutcome::AllowOnce,
        &format!("once:{ENTRY}"),
    );
    let filled = host.next();
    host.answer_ok(&panel.state, &filled, json!({ "filled": ["passwordRef"] }));
    reply.join().expect("the call");

    // Even a label that IS the value travels nowhere but the card the person
    // read: the frames and the tool result are free of it.
    assert!(!previewed.args.to_string().contains(SECRET));
    assert!(!filled.args.to_string().contains(SECRET));
}
