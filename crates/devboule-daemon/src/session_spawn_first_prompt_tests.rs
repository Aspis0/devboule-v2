//! The first prompt and the MCP wait, moved whole out of `session_tests.rs`
//! lines 2807-2926: Pi and Codex each deliver the first prompt without waiting
//! on the MCP handshake, the resume handle refuses the families it was never
//! designed for before anything is registered, and an MCP timeout never writes
//! the prompt. Every line below is byte-identical to its text there apart from
//! this header; `attach_live_agent_for_test` is promoted to `pub(super)` for
//! this move, and the other fixtures come from the provider's own imports.

use super::tests::{
    attach_live_agent_for_test, insert_live_agent_with_kind_and_writer, test_owner,
    tmp_delete_registry, RecordingWriter,
};
use super::*;

#[test]
fn pi_first_prompt_does_not_wait_for_mcp() {
    let (dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("S-1-5-21-pi", "process-pi");
    let received = Arc::new(Mutex::new(Vec::new()));
    let runtime = insert_live_agent_with_kind_and_writer(
        &registry,
        "pi-no-mcp-wait",
        owner.clone(),
        SessionKind::Pi,
        Box::new(RecordingWriter(Arc::clone(&received))),
    );
    let conn = attach_live_agent_for_test(&runtime, "pi-no-mcp-wait", 31);

    registry
        .send("pi-no-mcp-wait", "first prompt", &owner, &conn)
        .expect("Pi prompt should not have an MCP gate");
    assert_eq!(&*received.lock().expect("received"), b"first prompt");

    journal.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn codex_first_prompt_does_not_wait_for_mcp() {
    // S8 twin of the pi rule above: no road calls `require_mcp` for Codex
    // (the S8 bind split keeps `require` ACP/Claude-only), so the send-path
    // gate every prompt crosses is open by construction, verified or not.
    let (dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("S-1-5-21-codex", "process-codex");
    let received = Arc::new(Mutex::new(Vec::new()));
    let runtime = insert_live_agent_with_kind_and_writer(
        &registry,
        "codex-no-mcp-wait",
        owner.clone(),
        SessionKind::Codex,
        Box::new(RecordingWriter(Arc::clone(&received))),
    );
    let conn = attach_live_agent_for_test(&runtime, "codex-no-mcp-wait", 33);

    registry
        .send("codex-no-mcp-wait", "first prompt", &owner, &conn)
        .expect("Codex prompt should not have an MCP gate");
    assert_eq!(&*received.lock().expect("received"), b"first prompt");

    journal.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}

/// The spawn prompt rides the send road, not any provider writer: a
/// creation-shaped send — the shape `create_session_for_agent` builds, and
/// the only shape that sets the slot — carrying the resolved profile's
/// prompt composes standing → spawn → preamble → prompt, and the same send
/// without one is byte-identical to the shape the road already produced.
/// This pins the field's journey through `SendRequest` to the one
/// composition point; the order itself is `compose_first_prompt`'s, pinned
/// in `session_creation_race_tests`.
#[test]
fn the_spawn_prompt_travels_the_send_road_in_front_of_the_preamble() {
    let (dir, registry, journal) = tmp_delete_registry();
    // Standing instructions come from the profile store, attached the way the
    // server state attaches it: the store is read at the prompt, never cached.
    let store_dir = crate::test_dirs::test_temp_dir("devboule spawn-prompt send");
    let store = crate::agent_profiles::AgentProfilesStore::load(&store_dir);
    let document = devboule_protocol::AgentProfilesDocument {
        standing_instructions: "standing".to_string(),
        ..devboule_protocol::AgentProfilesDocument::default()
    };
    store.set(document).expect("standing instructions");
    registry.attach_agent_profiles(Arc::new(store));

    let owner = test_owner("S-1-5-21-spawn", "process-spawn");
    let received = Arc::new(Mutex::new(Vec::new()));
    let runtime = insert_live_agent_with_kind_and_writer(
        &registry,
        "spawn-order-child",
        owner.clone(),
        SessionKind::Pi,
        Box::new(RecordingWriter(Arc::clone(&received))),
    );
    let conn = attach_live_agent_for_test(&runtime, "spawn-order-child", 37);
    registry
        .send_with_subscription_timeout(&SendRequest {
            session_id: "spawn-order-child",
            subscription_id: 0,
            text: "the task",
            attachments: &[],
            attachment_references: &[],
            owner: &owner,
            conn: &conn,
            mcp_timeout: crate::mcp_broker::ready_timeout(),
            active_turn_behavior: None,
            require_attachment: false,
            interrupt_on_steer_refusal: true,
            message_slot: None,
            preset_preamble: Some(crate::provider_catalog::AGENT_PREAMBLE),
            spawn_prompt: Some("spawn"),
            author: UserMessageAuthor::Creation,
            message_kind: UserMessageKind::Creation,
        })
        .expect("the spawn-shaped creation send writes");
    assert_eq!(
        &*received.lock().expect("received"),
        format!(
            "standing\n\nspawn\n\n{}\n\nthe task",
            crate::provider_catalog::AGENT_PREAMBLE
        )
        .as_bytes(),
        "standing, then the profile's spawn prompt, then the preamble, then the task"
    );

    // The same shape without a spawn prompt is what the road sent before the
    // field existed: the absent slot is no blank line and no separator.
    let without = Arc::new(Mutex::new(Vec::new()));
    let runtime_without = insert_live_agent_with_kind_and_writer(
        &registry,
        "no-spawn-child",
        owner.clone(),
        SessionKind::Pi,
        Box::new(RecordingWriter(Arc::clone(&without))),
    );
    let conn_without = attach_live_agent_for_test(&runtime_without, "no-spawn-child", 38);
    registry
        .send_with_subscription_timeout(&SendRequest {
            session_id: "no-spawn-child",
            subscription_id: 0,
            text: "the task",
            attachments: &[],
            attachment_references: &[],
            owner: &owner,
            conn: &conn_without,
            mcp_timeout: crate::mcp_broker::ready_timeout(),
            active_turn_behavior: None,
            require_attachment: false,
            interrupt_on_steer_refusal: true,
            message_slot: None,
            preset_preamble: Some(crate::provider_catalog::AGENT_PREAMBLE),
            spawn_prompt: None,
            author: UserMessageAuthor::Creation,
            message_kind: UserMessageKind::Creation,
        })
        .expect("the bare creation-shaped send writes");
    assert_eq!(
        &*without.lock().expect("received"),
        format!(
            "standing\n\n{}\n\nthe task",
            crate::provider_catalog::AGENT_PREAMBLE
        )
        .as_bytes(),
    );

    journal.shutdown();
    let _ = std::fs::remove_dir_all(dir);
    let _ = std::fs::remove_dir_all(store_dir);
}

#[test]
fn resume_handle_refuses_the_terminal_before_any_registration() {
    // S9: the terminal's resume stays refused at the gate (it has no
    // conversation to load); stage 3 admitted Pi, so the record-kind
    // registration below sees Acp, Claude, Codex and Pi. A refusal here means
    // no bearer is minted for a refused row, ever.
    let owner = test_owner("S-1-5-21-resume", "process-resume");
    let terminal = new_session_record(
        "s.resume.1",
        &owner.user,
        None,
        SessionKind::Terminal,
        "Old",
    );
    let error = super::resume_handle(&terminal, &owner)
        .expect_err("the terminal's resume is refused before anything is minted");
    assert!(
        error
            .message
            .contains("only ACP, Claude, Codex and Pi sessions support"),
        "the refusal names the boundary: {}",
        error.message
    );
    let mut acp = new_session_record("s.resume.2", &owner.user, None, SessionKind::Acp, "Old");
    acp.provider = Some("grok".to_string());
    acp.peer_session_id = Some("peer-1".to_string());
    assert!(
        super::resume_handle(&acp, &owner).is_ok(),
        "an ACP row with its persisted handles passes the gate"
    );
    let mut claude =
        new_session_record("s.resume.3", &owner.user, None, SessionKind::Claude, "Old");
    claude.provider = Some("claude".to_string());
    claude.peer_session_id = Some("peer-9".to_string());
    assert!(
        super::resume_handle(&claude, &owner).is_ok(),
        "a Claude row with its persisted handles passes the gate"
    );
    let mut codex = new_session_record("s.resume.4", &owner.user, None, SessionKind::Codex, "Old");
    codex.provider = Some("codex".to_string());
    codex.peer_session_id = Some("thread-1".to_string());
    assert!(
        super::resume_handle(&codex, &owner).is_ok(),
        "a Codex row with its persisted thread id passes the gate"
    );
    let mut pi = new_session_record("s.resume.5", &owner.user, None, SessionKind::Pi, "Old");
    pi.provider = Some("pi".to_string());
    pi.peer_session_id = Some("01a0c1a7-0b95-731a-9a2e-06db94ff8043".to_string());
    assert!(
        super::resume_handle(&pi, &owner).is_ok(),
        "a Pi row with its persisted session id passes the gate"
    );
}

#[test]
fn mcp_timeout_does_not_write_the_first_prompt() {
    let (dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("S-1-5-21-mcp-timeout", "process-agent");
    let received = Arc::new(Mutex::new(Vec::new()));
    let runtime = insert_live_agent_with_kind_and_writer(
        &registry,
        "mcp-no-prompt-after-timeout",
        owner.clone(),
        SessionKind::Acp,
        Box::new(RecordingWriter(Arc::clone(&received))),
    );
    runtime.require_mcp();
    let conn = attach_live_agent_for_test(&runtime, "mcp-no-prompt-after-timeout", 32);

    let error = registry
        .send_with_mcp_timeout(
            "mcp-no-prompt-after-timeout",
            "must not be written",
            &owner,
            &conn,
            Duration::from_millis(1),
        )
        .expect_err("an unready MCP session must reject its first prompt");
    assert_eq!(error.code, ErrorCode::Io);
    assert!(received.lock().expect("received").is_empty());

    journal.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}
