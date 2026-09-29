//! The goal suite's shared send harness: the live-agent birth row, the event
//! filters, and the native-road double. One phrase for the file — the goal
//! tests' setup — beside the four topic files that use it.

use std::sync::{Arc, Mutex};

use devboule_protocol::{
    AttachmentReference, NoticeSeverity, PromptAttachment, SessionEvent, SessionKind,
};

use super::super::tests::{
    attach_live_agent_for_test, insert_live_agent_with_kind_and_writer, RecordingWriter,
};
use super::super::{ConnHandle, OutOfBandCommands, OwnerId, SessionRegistry, SessionRuntime};

pub(super) fn runtime_without_journal() -> Arc<SessionRuntime> {
    Arc::new(SessionRuntime::with_journal(
        "s.goal.unit".to_string(),
        None,
    ))
}

pub(super) fn goal_text_of(events: &[SessionEvent]) -> Vec<Option<String>> {
    events
        .iter()
        .filter_map(|event| match event {
            SessionEvent::GoalChanged { goal } => Some(goal.clone()),
            _ => None,
        })
        .collect()
}

pub(super) fn notices_of(events: &[SessionEvent]) -> Vec<(String, NoticeSeverity)> {
    events
        .iter()
        .filter_map(|event| match event {
            SessionEvent::SessionNotice { text, severity } => Some((text.clone(), *severity)),
            _ => None,
        })
        .collect()
}

pub(super) fn pulled_events(conn: &Arc<ConnHandle>) -> Vec<SessionEvent> {
    conn.pull_events()
        .into_iter()
        .map(|event| event.envelope.event)
        .collect()
}

pub(super) fn user_messages(events: &[SessionEvent]) -> Vec<String> {
    events
        .iter()
        .filter_map(|event| match event {
            SessionEvent::AgentUserMessage { text, .. } => Some(text.clone()),
            _ => None,
        })
        .collect()
}

/// A live agent row the way production births it: the journal row first, then
/// the runtime beside it, so the goal road records beside the live update.
pub(super) fn live_agent(
    registry: &SessionRegistry,
    id: &str,
    owner: &OwnerId,
    kind: SessionKind,
    received: &Arc<Mutex<Vec<u8>>>,
) -> (Arc<SessionRuntime>, Arc<ConnHandle>) {
    registry
        .journal
        .as_ref()
        .expect("the test registry has a journal")
        .create_session(crate::journal::new_session_record(
            id,
            owner.user.clone(),
            None,
            kind.clone(),
            "Agent",
        ))
        .expect("birth row");
    let runtime = insert_live_agent_with_kind_and_writer(
        registry,
        id,
        owner.clone(),
        kind,
        Box::new(RecordingWriter(Arc::clone(received))),
    );
    let conn = attach_live_agent_for_test(&runtime, id, 41);
    (runtime, conn)
}

/// A native goal road keeps the text whole: the intercept declines it, the
/// door below runs the provider's own command, and the stored goal is
/// untouched — the `thread/goal/*` answer moves it, never this road.
pub(super) struct ClaimingOutOfBand;

impl OutOfBandCommands for ClaimingOutOfBand {
    fn handles_out_of_band(&self, text: &str) -> bool {
        text.starts_with("/goal")
    }

    fn run_out_of_band(&self, _text: &str, _runtime: &Arc<SessionRuntime>) {}
}

pub(super) fn png_attachment() -> PromptAttachment {
    PromptAttachment {
        name: "a.png".to_string(),
        mime_type: "image/png".to_string(),
        data: "AAAA".to_string(),
    }
}

/// A well-formed stored-attachment reference: same session, 64 hex chars, a
/// small size. Validation is shape-only, so no deposit is needed to reach the
/// refusal below it.
pub(super) fn stored_reference(session_id: &str) -> AttachmentReference {
    AttachmentReference {
        session_id: session_id.to_string(),
        digest: "a".repeat(64),
        stored_bytes: 1024,
    }
}
