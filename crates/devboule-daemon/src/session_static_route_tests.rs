//! The static-route tests, moved whole out of `session_tests.rs` lines
//! 2261-2514 (at `54be819`), the section marker at their head included: a static
//! route sending its own frame and leaving the plain-text writer alone, the
//! route's plan text carrying the reference lines too, and a route that declines
//! keeping the legacy write byte for byte, with the route and plan doubles and
//! their constructor in front of them. Every line below is byte-identical to its
//! text there apart from this header;
//! `insert_live_agent_with_kind_writer_and_sink` is promoted to `pub(super)` for
//! this move (`stored_path` was already promoted by the stored-reference slice),
//! and `clean_png` is imported here rather than through the provider.

use super::tests::{
    attach_live_agent_for_test, attachment, insert_live_agent_with_kind_writer_and_sink,
    stored_path, test_owner, tmp_delete_registry, RecordingWriter,
};
use super::*;
use crate::codex_commands::CodexCommands;
use crate::raster_metadata::clean_png;

// --- the static route (Claude, Codex, Pi) -----------------------------
//
// These pin the send path's half of the three static providers: a session
// that carries a `static_image_sink` takes the plan's text and never the
// legacy walk, and a session whose route declines (or which carries no
// route at all) writes exactly the bytes it always wrote. A double stands
// in for the provider's own frame owner so neither test needs a child.

/// A route double: records that it was consulted and that its plan was the
/// one sent, and answers with a plan carrying the text the caller must
/// journal — or declines, which is what a provider not authorised for
/// inline bytes answers.
struct RecordingStaticSink {
    calls: Arc<AtomicU64>,
    sent: Arc<AtomicU64>,
    answer: Option<&'static str>,
}

impl StaticImageSink for RecordingStaticSink {
    fn plan_prompt(
        &self,
        _store: &AttachmentStore,
        _session_id: &str,
        _text: &str,
        _raw_text: &str,
        _attachments: &[PromptAttachment],
    ) -> Result<Option<Box<dyn PlannedStaticPrompt>>, WireError> {
        self.calls.fetch_add(1, Ordering::AcqRel);
        Ok(self.answer.map(|text| {
            Box::new(RecordingStaticPlan {
                text: text.to_string(),
                sent: Arc::clone(&self.sent),
            }) as Box<dyn PlannedStaticPrompt>
        }))
    }
}

/// The plan half of the double: the text it carries, and the record that it
/// was the one sent. Modelled rather than framed, so neither test below
/// needs a child.
struct RecordingStaticPlan {
    text: String,
    sent: Arc<AtomicU64>,
}

impl PlannedStaticPrompt for RecordingStaticPlan {
    fn text(&self) -> &str {
        &self.text
    }

    /// The same append the three real plans make, so a references test on
    /// this route sees the text a provider would build rather than a
    /// separate composition the double invented.
    fn append_reference_path_lines(&mut self, reference_paths: &[PathBuf]) {
        push_reference_path_lines(&mut self.text, reference_paths);
    }

    fn send(&self) -> Result<(), WireError> {
        self.sent.fetch_add(1, Ordering::AcqRel);
        Ok(())
    }
}

fn test_static_sink(
    answer: Option<&'static str>,
) -> (Arc<RecordingStaticSink>, Arc<AtomicU64>, Arc<AtomicU64>) {
    let calls = Arc::new(AtomicU64::new(0));
    let sent = Arc::new(AtomicU64::new(0));
    let sink = Arc::new(RecordingStaticSink {
        calls: Arc::clone(&calls),
        sent: Arc::clone(&sent),
        answer,
    });
    (sink, calls, sent)
}

#[test]
fn the_static_route_sends_its_own_frame_and_leaves_the_writer_alone() {
    // The route owns the send: on this branch the plain-text writer is not
    // typed into at all, and the text the journal records is the plan's.
    // That is also what holds a send to one materialization per attachment
    // — `with_attachment_paths`, the legacy walk, is reached only when the
    // route answered nothing.
    let (dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("S-1-5-21-static-route", "process-static-route");
    let session_id = "static-route";
    let received = Arc::new(Mutex::new(Vec::new()));
    let (sink, calls, sent) = test_static_sink(Some("the plan's own text"));
    let runtime = insert_live_agent_with_kind_writer_and_sink(
        &registry,
        session_id,
        owner.clone(),
        SessionKind::Claude,
        Box::new(RecordingWriter(Arc::clone(&received))),
        None,
        Some(sink),
    );
    let conn = attach_live_agent_for_test(&runtime, session_id, 71);
    let image = clean_png(0x21);
    registry
        .send_with_subscription(
            session_id,
            71,
            "describe this",
            &[attachment("photo.png", "image/png", &image)],
            &[],
            &owner,
            &conn,
        )
        .expect("send");
    assert!(
        received.lock().expect("writer").is_empty(),
        "the route's frame went out, not a plain-text write"
    );
    assert_eq!(sent.load(Ordering::Acquire), 1, "the plan was sent once");
    assert_eq!(
        calls.load(Ordering::Acquire),
        1,
        "the route is consulted once per send"
    );
    let recorded = conn
        .pull_events()
        .into_iter()
        .find_map(|event| match event.envelope.event {
            SessionEvent::AgentUserMessage { text, .. } => Some(text),
            _ => None,
        })
        .expect("the plan's text is what the journal records");
    assert_eq!(recorded, "the plan's own text");
    journal.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}

/// The same append on a plan route: the references go into the plan's own
/// text, which is the string the provider's frame carries *and* the string
/// the journal records, so the two cannot disagree about which files the
/// prompt named.
#[test]
fn the_static_routes_plan_text_carries_the_reference_lines_too() {
    let (dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("S-1-5-21-ref-static", "process-ref-static");
    let session_id = "ref-static";
    let received = Arc::new(Mutex::new(Vec::new()));
    let (sink, calls, sent) = test_static_sink(Some("the plan's own text"));
    let runtime = insert_live_agent_with_kind_writer_and_sink(
        &registry,
        session_id,
        owner.clone(),
        SessionKind::Claude,
        Box::new(RecordingWriter(Arc::clone(&received))),
        None,
        Some(sink),
    );
    let conn = attach_live_agent_for_test(&runtime, session_id, 73);
    let reference = registry
        .deposit(
            session_id,
            &owner,
            &conn,
            &attachment("deck.png", "image/png", &clean_png(0x37)),
        )
        .expect("deposit");

    registry
        .send_with_subscription(
            session_id,
            73,
            "read the deck",
            &[],
            std::slice::from_ref(&reference),
            &owner,
            &conn,
        )
        .expect("send");

    assert_eq!(
        calls.load(Ordering::Acquire),
        1,
        "the route is consulted once per send"
    );
    assert_eq!(sent.load(Ordering::Acquire), 1, "the plan was the one sent");
    assert!(
        received.lock().expect("writer").is_empty(),
        "the route's frame went out, not a plain-text write"
    );
    let recorded = conn
        .pull_events()
        .into_iter()
        .find_map(|event| match event.envelope.event {
            SessionEvent::AgentUserMessage { text, .. } => Some(text),
            _ => None,
        })
        .expect("the plan's text is what the journal records");
    assert_eq!(
        recorded,
        format!(
            "the plan's own text\n\n[Image available at: {}]",
            stored_path(&registry, session_id, &reference.digest).display()
        ),
        "the reference line is part of the plan's text, not a block beside it"
    );
    journal.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn the_first_picked_command_expands_from_the_raw_message() {
    // The P2 hand-off: the send path must hand the static plan the user's
    // message as `raw_text`, beside the composed text — the plan expands the
    // former, the journal records the expansion. Passing `text` where
    // `raw_text` goes reverts B1: the whole composed string parses as no
    // command, the plan declines, and the literal slash line is journaled.
    let (dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("S-1-5-21-raw-text", "process-raw-text");
    let session_id = "raw-text";
    let home = crate::test_dirs::test_temp_dir("devboule-raw-text-home");
    std::fs::create_dir_all(home.join("prompts")).expect("prompts dir");
    std::fs::write(
        home.join("prompts").join("commit.md"),
        "---\ndescription: Draft\n---\nDo $1\n",
    )
    .expect("prompt file");
    let seen: Arc<Mutex<Vec<(String, String)>>> = Arc::new(Mutex::new(Vec::new()));
    let sent = Arc::new(AtomicU64::new(0));
    let sink = Arc::new(RawKeepingSink {
        seen: Arc::clone(&seen),
        commands: CodexCommands::new(&home, None, false),
        sent: Arc::clone(&sent),
    });
    let received = Arc::new(Mutex::new(Vec::new()));
    let runtime = insert_live_agent_with_kind_writer_and_sink(
        &registry,
        session_id,
        owner.clone(),
        SessionKind::Claude,
        Box::new(RecordingWriter(Arc::clone(&received))),
        None,
        Some(sink),
    );
    let conn = attach_live_agent_for_test(&runtime, session_id, 74);
    registry
        .send_with_subscription_timeout(&SendRequest {
            session_id,
            subscription_id: 74,
            text: "/prompts:commit stage",
            attachments: &[],
            attachment_references: &[],
            owner: &owner,
            conn: &conn,
            mcp_timeout: crate::mcp_broker::ready_timeout(),
            active_turn_behavior: None,
            require_attachment: true,
            interrupt_on_steer_refusal: true,
            message_slot: None,
            preset_preamble: Some("standing instructions"),
            spawn_prompt: None,
            author: UserMessageAuthor::Human,
            message_kind: UserMessageKind::Composer,
        })
        .expect("send");
    assert_eq!(
        seen.lock().expect("seen").as_slice(),
        [(
            "standing instructions\n\n/prompts:commit stage".to_string(),
            "/prompts:commit stage".to_string(),
        )],
        "the plan sees the composed text and, beside it, the user's message"
    );
    assert_eq!(sent.load(Ordering::Acquire), 1, "the plan was sent");
    assert!(
        received.lock().expect("writer").is_empty(),
        "the plan's frame went out, not a plain-text write"
    );
    let recorded = conn
        .pull_events()
        .into_iter()
        .find_map(|event| match event.envelope.event {
            SessionEvent::AgentUserMessage { text, .. } => Some(text),
            _ => None,
        })
        .expect("the plan's text is what the journal records");
    assert_eq!(
        recorded, "standing instructions\n\nDo stage\n",
        "the first picked command expands on the composed turn"
    );
    journal.shutdown();
    let _ = std::fs::remove_dir_all(dir);
    let _ = std::fs::remove_dir_all(home);
}

/// A sink double that keeps the (composed text, raw message) pair the send
/// path handed it and expands a picked prompt against the message with the
/// real command table: the session-level half of the Codex seam. Prompt
/// origins only — a skill's multi-block shape is pinned at the plan level —
/// so any other answer shape is a test bug, stated loudly.
struct RawKeepingSink {
    seen: Arc<Mutex<Vec<(String, String)>>>,
    commands: CodexCommands,
    sent: Arc<AtomicU64>,
}

impl StaticImageSink for RawKeepingSink {
    fn plan_prompt(
        &self,
        _store: &AttachmentStore,
        _session_id: &str,
        text: &str,
        raw_text: &str,
        _attachments: &[PromptAttachment],
    ) -> Result<Option<Box<dyn PlannedStaticPrompt>>, WireError> {
        self.seen
            .lock()
            .expect("seen")
            .push((text.to_string(), raw_text.to_string()));
        // The Codex rule, same as production: resolve the message, keep the
        // composed prefix ahead of the expanded body.
        let prefix = text
            .strip_suffix(raw_text)
            .unwrap_or("")
            .trim_end_matches('\n');
        let expanded = match self
            .commands
            .prompt_input_checked(raw_text, prefix)
            .expect("the test table expands")
        {
            Some(blocks) => blocks
                .as_array()
                .and_then(|blocks| blocks.first())
                .and_then(|block| block.get("text"))
                .and_then(|text| text.as_str())
                .expect("a prompt origin answers one text block")
                .to_string(),
            None => return Ok(None),
        };
        Ok(Some(Box::new(RecordingStaticPlan {
            text: expanded,
            sent: Arc::clone(&self.sent),
        }) as Box<dyn PlannedStaticPrompt>))
    }
}

#[test]
fn a_static_route_that_declines_keeps_the_legacy_write_byte_for_byte() {
    // `None` is the provider saying nothing travels inline — a Pi model
    // that declared no image, or no attachments at all. The send must then
    // write exactly the text it has always written.
    let (dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("S-1-5-21-static-declined", "process-static-declined");
    let session_id = "static-declined";
    let received = Arc::new(Mutex::new(Vec::new()));
    let (sink, calls, sent) = test_static_sink(None);
    let runtime = insert_live_agent_with_kind_writer_and_sink(
        &registry,
        session_id,
        owner.clone(),
        SessionKind::Pi,
        Box::new(RecordingWriter(Arc::clone(&received))),
        None,
        Some(sink),
    );
    let conn = attach_live_agent_for_test(&runtime, session_id, 72);
    let image = clean_png(0x22);
    registry
        .send_with_subscription(
            session_id,
            72,
            "describe this",
            &[attachment("photo.png", "image/png", &image)],
            &[],
            &owner,
            &conn,
        )
        .expect("send");
    let written = String::from_utf8(received.lock().expect("writer").clone()).expect("utf8");
    let legacy = with_attachment_paths(
        &registry.attachments,
        session_id,
        "describe this",
        &[attachment("photo.png", "image/png", &image)],
    )
    .expect("legacy text");
    assert_eq!(written, legacy, "the declined route changes no byte");
    assert_eq!(calls.load(Ordering::Acquire), 1);
    assert_eq!(
        sent.load(Ordering::Acquire),
        0,
        "a declined route sends nothing"
    );
    journal.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}
