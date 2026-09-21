//! The refusal road of the recovery (`SessionRegistry::resume`): the provider
//! answers that it does not have the handle, and the conversation the daemon
//! still holds goes to a session that can start.
//!
//! It is the case the recovery was built for and could not reach — the rows
//! that predate the `cwd` column have no directory to prove gone, so the
//! pre-flight refuses nothing and the refusal arrives from the far side. The
//! road is judged through the real stub provider, and the one assertion that
//! matters is the prompt the agent actually received: a log line saying the
//! conversation was recovered would pass while the prompt was empty.
//!
//! The negative control lives here too, because it is the half that decides
//! whether the road is honest: a provider that does not *answer* must not
//! produce a replacement, or a real fault would be hidden behind a new
//! session.

use super::session_recovery::RECOVERED_HEADER;
use super::session_resume_fixture::{
    acp_row, take_bystander_slot, until_row, AcpEnv, ResumeFixture,
};
use super::*;
use devboule_protocol::{UserMessageAuthor, UserMessageKind};

fn user(text: &str) -> SessionEvent {
    SessionEvent::AgentUserMessage {
        message_id: None,
        text: text.to_string(),
        author: UserMessageAuthor::Human,
        message_kind: UserMessageKind::Composer,
    }
}

fn agent(text: &str) -> SessionEvent {
    SessionEvent::AgentMessage {
        message_id: None,
        text: text.to_string(),
        parent_tool_use_id: None,
        spawn_depth: None,
    }
}

fn wait_for_prompt(path: &std::path::Path) -> String {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        if let Ok(text) = std::fs::read_to_string(path) {
            if !text.trim().is_empty() {
                return text;
            }
        }
        assert!(Instant::now() < deadline, "the provider was never prompted");
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// The whole point of the road, judged on the **prompt the provider received**:
/// the provider refused to reopen the handle, so the daemon starts a new
/// session of the same family and the conversation goes to the agent as the
/// preamble of that session's first prompt — the human's own words last, where
/// a person's ask belongs. The refused row keeps its mark and its handle (that
/// provider will not reopen it) beside the recovered session.
///
/// Mutants: the recovery arm dropped from the failed-spawn arm (the call comes
/// back a refusal and this test's `expect` dies); the recovered text dropped
/// from the prompt (the agent sees only "continue from here"); the context
/// handed to the old row instead of the new one (a fresh session gets nothing,
/// and the wait times out); the mark cleared by the recovery (the mark
/// assertion dies); the idle-shutdown slot never taken (the live-session count
/// is one, not two).
#[test]
fn a_provider_that_refuses_the_handle_recovers_the_conversation_into_a_new_session() {
    let fixture = ResumeFixture::new("refusal-recover");
    let id = fixture.id("refusal-recover");
    let prompts = fixture.dir.join("stub prompts.txt");
    let _env = AcpEnv::stub(&[
        ("DEVBOULE_STUB_REFUSE_LOAD", "1".to_string()),
        (
            "DEVBOULE_ACP_STUB_PROMPT_FILE",
            prompts.to_string_lossy().into_owned(),
        ),
    ]);
    let _broker = fixture.state.mcp.start(&fixture.state).expect("MCP server");
    // The measured old row: no workspace and no recorded directory (every row
    // that predates the column), which is why the pre-flight refuses nothing.
    fixture.write_row(acp_row(&id, &fixture.owner, "handle-refused"));
    fixture.record_turn(&id, 1, &user("did you check the tests?"));
    fixture.record_turn(&id, 2, &agent("yes — and the gate too"));
    take_bystander_slot(&fixture.state);

    let session = fixture
        .resume(&id, &fixture.conn())
        .expect("a refused handle is recovered from the journal");
    assert_ne!(
        session.id, id,
        "the old row is not reopened: the provider does not have it"
    );
    assert!(
        matches!(session.state, SessionState::Live { .. }),
        "the replacement is a live session: {:?}",
        session.state
    );
    assert_eq!(
        session.display_name.as_deref(),
        Some("Resumed agent (recovered)"),
        "the name says what happened to it"
    );
    let refused = until_row(
        &fixture,
        &id,
        "the refused row was marked and ended",
        |row| row.status != PersistStatus::Live && row.disowned_peer_session_id.is_some(),
    );
    assert_eq!(
        fixture.state.live_session_count(),
        2,
        "the bystander and the recovered session: the daemon counts the child it is holding"
    );
    assert_eq!(
        refused.disowned_peer_session_id.as_deref(),
        Some("handle-refused"),
        "the old row records the refusal against the handle that was tried"
    );
    assert_eq!(
        refused.peer_session_id.as_deref(),
        Some("handle-refused"),
        "the handle is never destroyed: the mark is recorded beside it"
    );

    // The human's next line, on the ordinary send path: this is the prompt the
    // provider receives, and the whole claim is about its text.
    let conn = ConnHandle::with_peer(11, None);
    fixture
        .state
        .sessions
        .send_with_subscription_timeout(&SendRequest {
            session_id: &session.id,
            subscription_id: 1,
            text: "continue from here",
            attachments: &[],
            attachment_references: &[],
            owner: &fixture.owner,
            conn: &conn,
            mcp_timeout: crate::mcp_broker::ready_timeout(),
            active_turn_behavior: None,
            require_attachment: false,
            interrupt_on_steer_refusal: true,
            message_slot: None,
            preset_preamble: None,
            author: UserMessageAuthor::Human,
            message_kind: UserMessageKind::Composer,
        })
        .expect("the first prompt reaches the recovered session");

    let sent = wait_for_prompt(&prompts);
    assert!(
        sent.starts_with(RECOVERED_HEADER),
        "the agent is told the context is recovered before it reads it: {sent}"
    );
    assert!(
        sent.contains("user: did you check the tests?")
            && sent.contains("agent: yes — and the gate too"),
        "the conversation both sides had is in the prompt: {sent}"
    );
    assert!(
        sent.contains("the provider no longer has this session"),
        "the agent is told which session could not be reopened and why: {sent}"
    );
    assert!(
        sent.trim_end().ends_with("continue from here"),
        "the human's own words come last: {sent}"
    );
    let _ = fixture
        .state
        .sessions
        .close(&session.id, &fixture.owner, &None);
    fixture.finish();
}

/// The other half of the truth: the human clicked *this* session and got
/// another one, so the transcript they are looking at has to say why. The
/// reason is the provider's — its own sentence, quoted — and never the folder
/// sentence, which names a fact about a directory that this row does not even
/// record.
/// Mutants: the notice dropped (the new session's transcript opens with nothing
/// saying what happened); the reason passed as the folder's own
/// (`session_folder_gone` in place of `provider_refused_session`) — the last
/// assertion dies while the notice's presence does not.
#[test]
fn a_recovered_refusal_journals_the_providers_own_reason() {
    let fixture = ResumeFixture::new("refusal-notice");
    let id = fixture.id("refusal-notice");
    let _env = AcpEnv::stub(&[("DEVBOULE_STUB_REFUSE_LOAD", "1".to_string())]);
    let _broker = fixture.state.mcp.start(&fixture.state).expect("MCP server");
    fixture.write_row(acp_row(&id, &fixture.owner, "handle-notice"));
    fixture.record_turn(&id, 1, &user("did you check the tests?"));
    take_bystander_slot(&fixture.state);

    let session = fixture
        .resume(&id, &fixture.conn())
        .expect("the conversation is recovered");
    fixture.journal().flush().expect("flush");
    let events = fixture
        .journal()
        .replay(&session.id)
        .expect("the new session's transcript")
        .events;
    let notice = events
        .iter()
        .find_map(|event| match event {
            SessionEvent::SessionNotice { text, .. } => Some(text.clone()),
            _ => None,
        })
        .expect("the replacement says why it exists");
    assert!(
        notice.contains("the provider no longer has this session"),
        "the notice states the fact in the daemon's words: {notice}"
    );
    assert!(
        notice.contains("ACP request failed (-32002): Resource not found: handle-notice"),
        "and quotes the provider's own sentence after it: {notice}"
    );
    assert!(
        !notice.contains("the folder this session worked in no longer exists"),
        "the folder sentence is a fact about a directory this row never recorded: {notice}"
    );
    assert!(
        notice.contains(&id),
        "the notice names the session that could not be reopened: {notice}"
    );
    let _ = fixture
        .state
        .sessions
        .close(&session.id, &fixture.owner, &None);
    fixture.finish();
}

/// The negative control the whole road rests on: a provider that never answers
/// says nothing about the far session, so nothing is recovered and the refusal
/// of before is what comes back. A replacement born here would hide a real
/// fault — a handshake that timed out — behind a session that looks like
/// progress.
/// Mutants: the fallback triggered on any error instead of on the refusal (the
/// call comes back `Ok` with a replacement and the `expect_err` dies); the
/// classification widened to `Io` (same).
#[test]
fn a_provider_that_never_answers_is_not_recovered_into_a_new_session() {
    let fixture = ResumeFixture::new("refusal-silent");
    let id = fixture.id("refusal-silent");
    let _env = AcpEnv::stub(&[
        // The load is answered only after five seconds; the daemon's own bound
        // on one awaited response is a quarter of that, so this resume fails on
        // the deadline and not on anything the provider said.
        ("DEVBOULE_STUB_DELAY_LOAD_MS", "5000".to_string()),
        ("DEVBOULE_ACP_RESPONSE_TIMEOUT_MS", "250".to_string()),
    ]);
    fixture.write_row(acp_row(&id, &fixture.owner, "handle-silent"));
    fixture.record_turn(&id, 1, &user("did you check the tests?"));
    take_bystander_slot(&fixture.state);

    let error = fixture
        .resume(&id, &fixture.conn())
        .expect_err("the handshake never gets an answer");
    assert_eq!(
        error.code,
        ErrorCode::Io,
        "a timeout is a transport failure, not a refusal: {error:?}"
    );
    assert!(
        error.message.contains("did not answer within"),
        "the deadline's own sentence: {error:?}"
    );
    assert_eq!(
        fixture.row_ids(),
        vec![id.clone()],
        "a transient failure must not spawn a replacement"
    );
    let silent = fixture.row(&id);
    assert!(
        silent.disowned_peer_session_id.is_none(),
        "a provider that did not answer is not a provider that refused: {silent:?}"
    );
    assert_eq!(
        silent.peer_session_id.as_deref(),
        Some("handle-silent"),
        "the handle stays: nothing was learned about the far session"
    );
    fixture.finish();
}
