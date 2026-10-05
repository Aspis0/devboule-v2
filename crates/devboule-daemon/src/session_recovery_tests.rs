//! Tests for the recovery road (`session_recovery.rs`): the text a replacement
//! session carries, and the road that decides to take it.
//!
//! The claims are split the way the module is. The *renderer* is pure, so it is
//! judged on its own — which events are conversation, what a budget does to
//! them, and what the header says about each case. The *road* is judged through
//! `SessionRegistry::resume` against the real stub provider, because the one
//! assertion that matters is the prompt the agent actually received: a log line
//! saying the conversation was recovered would pass while the prompt was empty.

use super::session_recovery::{
    preamble_with_recovered, recovered_context, RECOVERED_CUT_MARKER, RECOVERED_HEADER,
};
use super::session_resume_fixture::{acp_row, take_bystander_slot, AcpEnv, ResumeFixture};
use super::*;
use devboule_protocol::{UserMessageAuthor, UserMessageKind};

/// A fresh store directory per test. Removal is best effort: the store
/// hardens the session folders it creates.
struct ScratchDir(std::path::PathBuf);

impl ScratchDir {
    fn new(tag: &str) -> Self {
        Self(crate::test_dirs::test_temp_dir(&format!(
            "devboule-recovery-{tag}"
        )))
    }

    fn store(&self) -> crate::attachment_store::AttachmentStore {
        crate::attachment_store::AttachmentStore::new(&self.0)
    }
}

impl Drop for ScratchDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn user(text: &str) -> SessionEvent {
    SessionEvent::AgentUserMessage {
        message_id: None,
        text: text.to_string(),
        author: UserMessageAuthor::Human,
        message_kind: UserMessageKind::Composer,
        at_ms: None,
        images: Vec::new(),
    }
}

fn agent(text: &str) -> SessionEvent {
    SessionEvent::AgentMessage {
        message_id: None,
        text: text.to_string(),
        parent_tool_use_id: None,
        spawn_depth: None,

        images: Vec::new(),
    }
}

/// A chunk of one named message, the way a provider streams it: the id is what
/// makes two frames one turn.
fn agent_chunk(id: &str, text: &str) -> SessionEvent {
    SessionEvent::AgentMessage {
        message_id: Some(id.to_string()),
        text: text.to_string(),
        parent_tool_use_id: None,
        spawn_depth: None,

        images: Vec::new(),
    }
}

/// The two sides, and only them: a thought, an output frame and a daemon notice
/// are not something either side *said*, and a recovery that quoted the daemon
/// to itself would be a worse reconstruction than a shorter one. Chunks that
/// name the same message are one turn; frames that name nothing are not
/// guessed together.
/// Mutants: `AgentMessage` dropped (the agent's half vanishes); the kind filter
/// dropped (a notice is handed back as if the human had written it); the id
/// rule dropped (`id == id` only when both are `Some` — two unnamed frames
/// would merge into one turn nobody could budget for).
#[test]
fn the_context_carries_what_the_two_sides_said_and_merges_one_message() {
    let events = vec![
        SessionEvent::AgentThought {
            message_id: None,
            text: "thinking out loud".to_string(),
            parent_tool_use_id: None,
            spawn_depth: None,
        },
        user("did you check the tests?"),
        agent_chunk("m1", "yes"),
        agent_chunk("m1", " — and the gate too"),
        agent("and this is a second message"),
        SessionEvent::Output {
            seq: 1,
            data: "raw frame".to_string(),
        },
        SessionEvent::AgentUserMessage {
            message_id: None,
            text: "Agent 'kid' created".to_string(),
            author: UserMessageAuthor::Creation,
            message_kind: UserMessageKind::Creation,
            at_ms: None,
            images: Vec::new(),
        },
    ];
    let scratch = ScratchDir::new("context");
    let store = scratch.store();
    let context =
        recovered_context("s.old", "it is gone", &events, 4096, &store).expect("a conversation");
    assert!(
        context.text.starts_with(RECOVERED_HEADER),
        "{}",
        context.text
    );
    assert!(context.text.contains("user: did you check the tests?"));
    assert!(
        context
            .text
            .contains("agent: yes — and the gate too\n\nagent: and this is a second message"),
        "one named message is one turn, and the next message is another: {}",
        context.text
    );
    assert!(
        !context.text.contains("thinking out loud"),
        "{}",
        context.text
    );
    assert!(!context.text.contains("raw frame"), "{}", context.text);
    assert!(
        !context.text.contains("created"),
        "a creation stamp is not the human speaking: {}",
        context.text
    );
    assert_eq!((context.kept, context.dropped), (3, 0));
    assert!(
        context.text.contains("(3 of 3 turns)"),
        "the header counts the turns it carried: {}",
        context.text
    );
    assert!(!context.declares_a_cut(), "{}", context.text);
}

/// The negative control the road's own decision rests on: a session where
/// nothing was ever said has no conversation to hand on, so there is no
/// context at all — and the daemon's own refusal stands instead of being
/// replaced by an empty new session.
/// Mutant: the emptiness check dropped — the header alone would be handed to an
/// agent as if it were a conversation.
#[test]
fn a_session_where_nothing_was_said_yields_no_context() {
    let events = vec![
        SessionEvent::Output {
            seq: 1,
            data: "raw frame".to_string(),
        },
        SessionEvent::AgentThought {
            message_id: None,
            text: "hmm".to_string(),
            parent_tool_use_id: None,
            spawn_depth: None,
        },
        SessionEvent::AgentUserMessage {
            message_id: None,
            text: "Recovered session.".to_string(),
            author: UserMessageAuthor::Agent,
            message_kind: UserMessageKind::SystemNotice,
            at_ms: None,
            images: Vec::new(),
        },
    ];
    let scratch = ScratchDir::new("context");
    let store = scratch.store();
    assert!(
        recovered_context("s.old", "it is gone", &events, 4096, &store).is_none(),
        "no conversation, no context"
    );
    assert!(recovered_context("s.old", "it is gone", &[], 4096, &store).is_none());
}

/// The cut, from the oldest end, and **declared**: the header says how many of
/// how many turns survived and why, and the newest turn is the one that is
/// always there — a context that dropped the present to keep the past would be
/// useless to an agent asked to continue the work.
/// Mutants: the cut silent (the marker dropped — the agent would be told this
/// is the whole conversation); the walk forwards instead of backwards (the
/// oldest turns survive and the newest is dropped).
#[test]
fn a_conversation_over_the_budget_declares_the_oldest_turns_omitted() {
    let mut events = vec![user("the first thing that was ever said")];
    for index in 0..12 {
        events.push(agent(&format!("turn {index} {}", "x".repeat(200))));
    }
    events.push(user("the last thing that was said"));
    let budget = 1024;
    let scratch = ScratchDir::new("context");
    let store = scratch.store();
    let context =
        recovered_context("s.old", "it is gone", &events, budget, &store).expect("a conversation");
    assert!(
        context.text.contains(RECOVERED_CUT_MARKER),
        "the header declares the cut: {}",
        context.text
    );
    assert!(
        context.text.contains("the last thing that was said"),
        "the newest turn survives: {}",
        context.text
    );
    assert!(
        !context.text.contains("the first thing that was ever said"),
        "the cut comes off the oldest end: {}",
        context.text
    );
    assert!(
        context.dropped > 0,
        "turns were dropped: {:?}",
        context.dropped
    );
    assert!(
        context
            .text
            .contains(&format!("({} of 14 turns", context.kept)),
        "the header counts what it kept out of what there was: {}",
        context.text
    );
}

/// One turn bigger than the whole budget: the tail of the newest message is
/// still the newest thing that was said, so the context carries that rather
/// than nothing — and declares it, because "1 of 1 turns" with a body that is
/// only the end of that turn would otherwise read as the whole conversation.
/// Mutant: the truncation flag dropped — the header claims the whole turn and
/// the agent is handed a sentence without its beginning.
#[test]
fn a_single_turn_over_the_budget_recovers_its_tail_and_says_so() {
    let events = vec![user(&format!("{} the end", "y".repeat(4096)))];
    let scratch = ScratchDir::new("context");
    let store = scratch.store();
    let context =
        recovered_context("s.old", "it is gone", &events, 256, &store).expect("a conversation");
    assert!(
        context.text.contains(RECOVERED_CUT_MARKER),
        "the header declares the cut: {}",
        context.text
    );
    assert!(
        context.truncated && context.dropped == 0,
        "dropped nothing, kept the tail"
    );
    assert!(
        context.text.contains("the end"),
        "the tail of the newest turn survives: {}",
        context.text
    );
    assert!(context.text.len() < 4096, "the body fits the budget");
}

/// The preamble order, the same rule the standing instructions already follow:
/// the caller's own preset first, then the recovered conversation, and `None`
/// only when the caller has neither.
/// Mutant: the two swapped — the recovered history would sit in front of a
/// preset that explains what the session is.
#[test]
fn a_preset_preamble_comes_before_the_recovered_conversation() {
    assert_eq!(preamble_with_recovered(None, None), None);
    assert_eq!(
        preamble_with_recovered(Some("preset"), None).as_deref(),
        Some("preset")
    );
    assert_eq!(
        preamble_with_recovered(None, Some("recovered")).as_deref(),
        Some("recovered")
    );
    assert_eq!(
        preamble_with_recovered(Some("preset"), Some("recovered")).as_deref(),
        Some("preset\n\nrecovered")
    );
}

/// A person's composer line to `session_id`, on the ordinary send path.
fn send_human_line(fixture: &ResumeFixture, session_id: &str, text: &str) {
    let conn = ConnHandle::with_peer(11, None);
    fixture
        .state
        .sessions
        .send_with_subscription_timeout(&SendRequest {
            session_id,
            subscription_id: 1,
            text,
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
            spawn_prompt: None,
            author: UserMessageAuthor::Human,
            message_kind: UserMessageKind::Composer,
            steer_origin: SteerOrigin::Person,
            require_no_turn_running: false,
            require_queue_unfenced: false,
        })
        .expect("the first prompt reaches the recovered session");
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
/// the directory the old session worked in is gone, so the daemon starts a new
/// session of the same family and the conversation goes to the agent as the
/// preamble of that session's first prompt — the human's own words last, where
/// a person's ask belongs.
///
/// Mutants: the recovered text dropped from the prompt (the agent sees only
/// "continue from here" — this test dies); the context handed to the old row
/// instead of the new one (a fresh session gets nothing, and the test's wait
/// times out); the new session created without the old one's provider (the
/// stub's prompt file never appears); the idle-shutdown slot never taken (the
/// live-session count is one, not two).
#[test]
fn a_resume_for_a_gone_directory_recovers_the_conversation_into_a_new_session() {
    let fixture = ResumeFixture::new("recover");
    let id = fixture.id("recover");
    let prompts = fixture.dir.join("stub prompts.txt");
    let _env = AcpEnv::stub(&[(
        "DEVBOULE_ACP_STUB_PROMPT_FILE",
        prompts.to_string_lossy().into_owned(),
    )]);
    // The MCP invariant gates an agent session's first prompt: the broker has
    // to be up for the provider to authenticate against it and call
    // `tools/list`, which is what this session's first prompt waits for.
    let _broker = fixture.state.mcp.start(&fixture.state).expect("MCP server");
    let mut row = acp_row(&id, &fixture.owner, "handle-recover");
    row.cwd = Some(
        fixture
            .dir
            .join("removed-worktree")
            .to_string_lossy()
            .into_owned(),
    );
    fixture.write_row(row);
    fixture.record_turn(&id, 1, &user("did you check the tests?"));
    fixture.record_turn(&id, 2, &agent("yes — and the gate too"));
    take_bystander_slot(&fixture.state);

    let session = fixture
        .resume(&id, &fixture.conn())
        .expect("a conversation is recovered into a new session");
    assert_ne!(
        session.id, id,
        "the old row is not reopened: the provider cannot reopen it"
    );
    assert_eq!(
        fixture.state.live_session_count(),
        2,
        "the bystander and the recovered session: the daemon counts the child it is holding"
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
    assert_eq!(
        fixture.row(&id).generation,
        1,
        "the old row was never opened for a new generation"
    );

    // The human's next line, on the ordinary send path: this is the prompt the
    // provider receives, and the whole claim is about its text.
    send_human_line(&fixture, &session.id, "continue from here");

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
        sent.trim_end().ends_with("continue from here"),
        "the human's own words come last: {sent}"
    );
    let _ = fixture
        .state
        .sessions
        .close(&session.id, &fixture.owner, &None);
    fixture.finish();
}

/// A journaled turn holds only what the person typed, so the standing
/// instructions must come from the replacement's own first-prompt composition,
/// read from the store at that send, in front of the recovered conversation.
#[test]
fn a_recovered_first_prompt_still_carries_the_standing_instructions() {
    let fixture = ResumeFixture::new("recover-standing");
    fixture
        .state
        .agent_profiles
        .set(devboule_protocol::AgentProfilesDocument {
            standing_instructions: "Always run the gate.".to_string(),
            ..devboule_protocol::AgentProfilesDocument::default()
        })
        .expect("standing instructions");
    let id = fixture.id("recover-standing");
    let prompts = fixture.dir.join("stub prompts.txt");
    let _env = AcpEnv::stub(&[(
        "DEVBOULE_ACP_STUB_PROMPT_FILE",
        prompts.to_string_lossy().into_owned(),
    )]);
    let _broker = fixture.state.mcp.start(&fixture.state).expect("MCP server");
    let mut row = acp_row(&id, &fixture.owner, "handle-recover-standing");
    row.cwd = Some(
        fixture
            .dir
            .join("removed-worktree")
            .to_string_lossy()
            .into_owned(),
    );
    fixture.write_row(row);
    fixture.record_turn(&id, 1, &user("did you check the tests?"));
    take_bystander_slot(&fixture.state);

    let session = fixture
        .resume(&id, &fixture.conn())
        .expect("a conversation is recovered into a new session");
    send_human_line(&fixture, &session.id, "continue from here");

    let sent = wait_for_prompt(&prompts);
    assert!(
        sent.starts_with(&format!("Always run the gate.\n\n{RECOVERED_HEADER}")),
        "the standing instructions lead, then the recovered conversation: {sent}"
    );
    assert_eq!(
        sent.matches("Always run the gate.").count(),
        1,
        "carried once: {sent}"
    );
    let _ = fixture
        .state
        .sessions
        .close(&session.id, &fixture.owner, &None);
    fixture.finish();
}

/// The other half of the truth: the human clicked *this* session and got
/// another one, so the transcript they are looking at has to say why. The
/// notice is a journal event, so it is still there after a restart — the one
/// road that outlives the daemon that wrote it.
/// Mutant: the notice dropped — the new session's transcript opens empty, with
/// nothing saying what happened to the session that was clicked.
#[test]
fn a_recovered_session_journals_why_it_replaced_the_session_that_was_clicked() {
    let fixture = ResumeFixture::new("recover-notice");
    let id = fixture.id("recover-notice");
    let _env = AcpEnv::stub(&[]);
    let _broker = fixture.state.mcp.start(&fixture.state).expect("MCP server");
    let gone = fixture.dir.join("removed-worktree");
    let mut row = acp_row(&id, &fixture.owner, "handle-notice");
    row.cwd = Some(gone.to_string_lossy().into_owned());
    fixture.write_row(row);
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
        notice.contains(&format!(
            "the folder this session worked in no longer exists: {}",
            crate::verbatim_path::plain_path(&gone.to_string_lossy())
        )),
        "the sentence is the pre-flight's own, path included: {notice}"
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

/// One image deposited into `store` for session `s.old`: its stored path and
/// the reference a journaled turn carries.
fn deposited_ref(
    store: &crate::attachment_store::AttachmentStore,
    name: &str,
    byte: u8,
) -> (std::path::PathBuf, devboule_protocol::AttachmentReference) {
    use base64::Engine;
    let image = crate::raster_metadata::clean_png(byte);
    let deposited = store
        .deposit(
            "s.old",
            &devboule_protocol::PromptAttachment {
                name: name.to_string(),
                mime_type: "image/png".to_string(),
                data: base64::engine::general_purpose::STANDARD.encode(&image),
            },
        )
        .expect("deposit");
    let reference = devboule_protocol::AttachmentReference {
        session_id: "s.old".to_string(),
        digest: deposited.digest.clone(),
        stored_bytes: deposited.stored_bytes,
    };
    (deposited.path, reference)
}

fn user_with_images(
    text: &str,
    images: Vec<devboule_protocol::AttachmentReference>,
) -> SessionEvent {
    SessionEvent::AgentUserMessage {
        message_id: None,
        text: text.to_string(),
        author: UserMessageAuthor::Human,
        message_kind: UserMessageKind::Composer,
        at_ms: None,
        images,
    }
}

/// A journaled user turn with refs resolves both into the recovered provider
/// context, as the same path lines the live send composes — so a recovered
/// agent still sees the images.
#[test]
fn recovered_context_resolves_both_refs_into_path_lines() {
    let scratch = ScratchDir::new("images");
    let store = scratch.store();
    let (first, first_ref) = deposited_ref(&store, "first.png", 0x0b);
    let (second, second_ref) = deposited_ref(&store, "second.png", 0x0c);
    let events = vec![user_with_images(
        "look at these",
        vec![first_ref, second_ref],
    )];
    let context =
        recovered_context("s.old", "it is gone", &events, 4096, &store).expect("a conversation");
    assert!(
        context.text.contains("user: look at these"),
        "the turn's own words stay first: {}",
        context.text
    );
    for path in [&first, &second] {
        assert!(
            context
                .text
                .contains(&format!("[Image available at: {}]", path.display())),
            "each deposited file is named by its stored path: {}",
            context.text
        );
    }
}

/// A file that is gone (deposited elsewhere stands in for an expired one)
/// still leaves a line, so the replacement agent knows an image was there.
#[test]
fn a_missing_ref_is_declared_beside_the_resolved_one() {
    let scratch = ScratchDir::new("missing");
    let store = scratch.store();
    let elsewhere = ScratchDir::new("missing-elsewhere");
    let (kept, kept_ref) = deposited_ref(&store, "kept.png", 0x0d);
    let (_, gone_ref) = deposited_ref(&elsewhere.store(), "gone.png", 0x0e);
    let events = vec![user_with_images("look at these", vec![kept_ref, gone_ref])];
    let context =
        recovered_context("s.old", "it is gone", &events, 4096, &store).expect("a conversation");
    assert!(
        context.text.contains(&format!(
            "user: look at these\n\n[Image available at: {}]\n[Image no longer available]",
            kept.display()
        )),
        "the resolved line, then one line for the missing image: {}",
        context.text
    );
}

/// An image-only turn whose files are all gone says so, line per image,
/// instead of handing the agent an empty `user: ` turn.
#[test]
fn an_image_only_turn_whose_files_are_all_gone_declares_each_one() {
    let scratch = ScratchDir::new("all-missing");
    let store = scratch.store();
    let elsewhere = ScratchDir::new("all-missing-elsewhere");
    let (_, first_ref) = deposited_ref(&elsewhere.store(), "first.png", 0x0f);
    let (_, second_ref) = deposited_ref(&elsewhere.store(), "second.png", 0x10);
    let events = vec![user_with_images("", vec![first_ref, second_ref])];
    let context =
        recovered_context("s.old", "it is gone", &events, 4096, &store).expect("a conversation");
    assert!(
        context
            .text
            .contains("user: [Image no longer available]\n[Image no longer available]"),
        "one line per missing image, on the turn itself: {}",
        context.text
    );
}
