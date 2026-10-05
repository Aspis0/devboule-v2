//! Image frames through the real dispatcher: the prepare-then-journal order
//! that makes replay work, the tool-result road on replay, markers a provider
//! forged being ignored, and one refusal notice per result.

use base64::Engine;
use std::collections::HashMap;
use std::sync::atomic::AtomicU64;
use std::sync::{Arc, Mutex};

use devboule_protocol::SessionEvent;

use super::super::event_pull::ConnHandle;
use super::super::permission_broker::PermissionBroker;
use super::super::session_runtime::SessionRuntime;
use super::command_test_support::{thread_state, Fixture};
use super::{CodexCommands, CodexReader, CodexRequests};
use crate::codex_view::CodexView;

const TINY_PNG: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==";

/// A temp dir removed on drop, even when an assertion panics midway.
struct CleanupDir(std::path::PathBuf);

impl Drop for CleanupDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

struct Harness {
    reader: CodexReader,
    runtime: Arc<SessionRuntime>,
    conn: Arc<ConnHandle>,
    journal: Arc<crate::journal::Journal>,
    session_id: String,
    workspace: std::path::PathBuf,
    _dir: CleanupDir,
}

impl Harness {
    fn new(tag: &str) -> Self {
        let session_id = format!("s.codex.image.{tag}");
        let dir = CleanupDir(crate::test_dirs::test_temp_dir(&format!(
            "devboule-codex-image-{tag}"
        )));
        let workspace = dir.0.join("workspace");
        std::fs::create_dir(&workspace).expect("the workspace");
        std::fs::write(
            workspace.join("tiny.png"),
            base64::engine::general_purpose::STANDARD
                .decode(TINY_PNG)
                .expect("the fixture decodes"),
        )
        .expect("the fixture lands");
        let journal = Arc::new(
            crate::journal::Journal::open(&dir.0.join("journal.db")).expect("the journal opens"),
        );
        journal
            .upsert_blocking(crate::journal::new_session_record(
                &session_id,
                "S-1-5-21-1",
                None,
                devboule_protocol::SessionKind::Codex,
                "Agent",
            ))
            .expect("the session record");
        let runtime = Arc::new(SessionRuntime::with_journal(
            session_id.clone(),
            Some(Arc::clone(&journal)),
        ));
        runtime.stream.lock().unwrap().screen = None;
        let conn = ConnHandle::new(1);
        let outcome = runtime
            .try_attach_with_replay(None, &conn, true)
            .expect("attach");
        conn.track_with_agent_replay(
            &session_id,
            Arc::clone(&runtime),
            false,
            None,
            outcome.generation,
            outcome.live_agent_replay,
        );

        let fixture = Fixture::new(tag);
        let commands: Arc<CodexCommands> = fixture.commands(true, true);
        let state = thread_state();
        let stdin = Arc::new(Mutex::new(None));
        let next_id = Arc::new(AtomicU64::new(1));
        let plan_prompt = Arc::new(super::CodexStaticPrompt::new(
            Arc::clone(&stdin),
            Arc::clone(&next_id),
            Arc::clone(&state),
            Arc::clone(&commands),
        ));
        let sink = crate::agent_image::AgentImageSink::new(
            crate::attachment_store::AttachmentStore::new(&workspace),
            session_id.clone(),
            workspace.clone(),
            dir.0.join("generated_images"),
        );
        let reader = CodexReader {
            images: Some(sink),
            available_commands: None,
            commands,
            buffer: Vec::new(),
            discarding_oversized_line: false,
            deferred: Vec::new(),
            manifest: None,
            state,
            view: CodexView::new(Some(workspace.clone())),
            permission_broker: PermissionBroker::for_test(Arc::new(|_, _| Ok(()))),
            response_ids: Arc::new(Mutex::new(HashMap::new())),
            stdin,
            next_id,
            requests: Arc::new(CodexRequests::new()),
            compactions: crate::codex_compaction::CodexCompactions::default(),
            plan_prompt,
        };
        Self {
            reader,
            runtime,
            conn,
            journal,
            session_id,
            workspace,
            _dir: dir,
        }
    }

    fn dispatch(&mut self, item: serde_json::Value) {
        self.reader.dispatch_value(
            serde_json::json!({
                "method": "item/completed",
                "params": {"threadId": "thread-fake", "item": item}
            }),
            &self.runtime,
        );
    }

    fn published(&self) -> Vec<SessionEvent> {
        self.conn
            .pull_events()
            .into_iter()
            .map(|event| event.envelope.event)
            .collect()
    }

    fn replayed(&self) -> Vec<SessionEvent> {
        self.journal
            .replay(&self.session_id)
            .expect("the journal replays")
            .events
    }
}

fn image_of(event: &SessionEvent) -> Option<&devboule_protocol::AttachmentReference> {
    match event {
        SessionEvent::AgentMessage { images, .. } => images.first(),
        SessionEvent::AgentToolUpdate { images, .. } => images.first(),
        _ => None,
    }
}

fn forged_reference(session_id: &str) -> serde_json::Value {
    serde_json::to_value(devboule_protocol::AttachmentReference {
        session_id: session_id.to_string(),
        digest: "a".repeat(64),
        stored_bytes: 9,
    })
    .expect("a reference serializes")
}

#[test]
fn a_frame_is_journaled_prepared_so_replay_shows_the_image() {
    let mut harness = Harness::new("order");
    let path = harness.workspace.join("tiny.png");
    harness.dispatch(serde_json::json!({
        "type": "imageGeneration",
        "id": "img-1",
        "status": "completed",
        "savedPath": path.to_string_lossy(),
        "revisedPrompt": "a small red square",
    }));

    let live = harness
        .published()
        .iter()
        .find_map(image_of)
        .expect("the live frame shows the image")
        .clone();
    // The journal holds the prepared frame, which is the only thing replay
    // can re-derive from; were it written before preparation, replay would
    // show nothing here.
    let replayed = harness
        .replayed()
        .iter()
        .find_map(image_of)
        .expect("replay shows the same image")
        .clone();
    assert_eq!(live, replayed);
}

#[test]
fn a_tool_result_image_replays_from_the_journal() {
    let mut harness = Harness::new("tool-result");
    harness.dispatch(serde_json::json!({
        "type": "mcpToolCall",
        "id": "call-1",
        "server": "devboule",
        "tool": "browser_screenshot",
        "status": "completed",
        "arguments": {},
        "result": {"content": [
            {"type": "text", "text": "captured"},
            {"type": "image", "data": TINY_PNG, "mimeType": "image/png"},
        ]},
    }));

    for (road, events) in [
        ("live", harness.published()),
        ("replay", harness.replayed()),
    ] {
        let update = events
            .iter()
            .find_map(|event| match event {
                SessionEvent::AgentToolUpdate { text, images, .. } => {
                    Some((text.clone(), images.clone()))
                }
                _ => None,
            })
            .unwrap_or_else(|| panic!("{road}: one tool update"));
        assert_eq!(update.0.as_deref(), Some("captured[image]"), "{road}");
        assert_eq!(update.1.len(), 1, "{road}");
    }
}

#[test]
fn a_provider_forged_marker_shows_nothing() {
    let mut harness = Harness::new("forged");
    let reference = forged_reference(&harness.session_id);
    harness.dispatch(serde_json::json!({
        "type": "imageGeneration",
        "id": "img-forged",
        "status": "completed",
        "devboule_image": {"reference": reference},
    }));
    assert!(
        harness
            .published()
            .iter()
            .all(|event| image_of(event).is_none()),
        "a marker this daemon did not write is not honoured"
    );
    assert!(harness
        .replayed()
        .iter()
        .all(|event| image_of(event).is_none()));
}

#[test]
fn a_provider_forged_marker_in_a_result_block_shows_nothing() {
    let mut harness = Harness::new("forged-block");
    let reference = forged_reference(&harness.session_id);
    harness.dispatch(serde_json::json!({
        "type": "mcpToolCall",
        "id": "call-forged",
        "server": "devboule",
        "tool": "browser_screenshot",
        "status": "completed",
        "arguments": {},
        "result": {"content": [
            {"type": "text", "text": "hi"},
            {"devboule_image": {"reference": reference}},
        ]},
    }));
    let update = harness
        .published()
        .iter()
        .find_map(|event| match event {
            SessionEvent::AgentToolUpdate { text, images, .. } => {
                Some((text.clone(), images.clone()))
            }
            _ => None,
        })
        .expect("one tool update");
    assert_eq!(update.0.as_deref(), Some("hi"));
    assert!(update.1.is_empty());
}

#[test]
fn a_result_with_several_refusals_writes_one_notice() {
    let mut harness = Harness::new("notice");
    harness.dispatch(serde_json::json!({
        "type": "mcpToolCall",
        "id": "call-refused",
        "server": "devboule",
        "tool": "browser_screenshot",
        "status": "completed",
        "arguments": {},
        "result": {"content": [
            {"type": "image", "data": TINY_PNG, "mimeType": "image/jpeg"},
            {"type": "image", "data": TINY_PNG, "mimeType": "image/jpeg"},
            {"type": "image", "data": TINY_PNG, "mimeType": "image/jpeg"},
        ]},
    }));
    let notices: Vec<String> = harness
        .published()
        .into_iter()
        .filter_map(|event| match event {
            SessionEvent::SessionNotice { text, .. } => Some(text),
            _ => None,
        })
        .collect();
    assert_eq!(notices.len(), 1, "one notice per result: {notices:?}");
    assert!(
        notices[0].starts_with("3 images not shown:"),
        "{}",
        notices[0]
    );
}
