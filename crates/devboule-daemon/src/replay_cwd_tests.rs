//! The replay-cwd contract: every replay road derives tool locations the way
//! the live views do — relative under the session cwd, unchanged outside it.

use super::super::*;

use serde_json::{json, Value};

use super::test_support::{attach_live_agent_replay, drain};

fn tool_call_paths(events: &[SessionEvent]) -> Vec<String> {
    events
        .iter()
        .filter_map(|event| match event {
            SessionEvent::AgentToolCall { locations, .. } => locations
                .as_ref()
                .and_then(|locations| locations.first())
                .map(|location| location.path.clone()),
            _ => None,
        })
        .collect()
}

/// `<cwd>/src/a.rs`, spelled with the platform's separators.
fn under_root(root: &str) -> String {
    std::path::Path::new(root)
        .join("src")
        .join("a.rs")
        .to_string_lossy()
        .into_owned()
}

fn relative_tail() -> String {
    std::path::Path::new("src")
        .join("a.rs")
        .to_string_lossy()
        .into_owned()
}

/// An absolute path outside every test cwd, in the platform's own spelling:
/// a POSIX path is not absolute on Windows and would never reach the cwd check.
#[cfg(windows)]
const OUTSIDE: &str = r"C:\Windows\Temp\devboule-outside\a.rs";
#[cfg(not(windows))]
const OUTSIDE: &str = "/etc/devboule-outside/a.rs";

fn session_row(id: &str, kind: SessionKind, cwd: &str) -> crate::journal::SessionRecord {
    let mut record = new_session_record(id, "S-1-5-21-1", None, kind, "Replay cwd");
    record.cwd = Some(cwd.to_string());
    record
}

fn envelope_rows(id: &str, envelopes: &[Value]) -> Vec<crate::journal::EventRecord> {
    envelopes
        .iter()
        .enumerate()
        .map(|(index, envelope)| {
            crate::journal::acp_envelope_record(id, 1, index as u64 + 1, envelope).expect("row")
        })
        .collect()
}

/// The rebuild road: the session row plus the envelopes, replayed whole
/// through `Journal::replay`.
fn rebuild_events(kind: SessionKind, cwd: &str, envelopes: &[Value]) -> Vec<SessionEvent> {
    let dir = crate::test_dirs::test_temp_dir("devboule-replay-cwd-rebuild");
    let journal = Journal::open(&dir.join("journal.db")).expect("open");
    let id = "s.replay.cwd.rebuild";
    journal
        .create_session(session_row(id, kind, cwd))
        .expect("birth");
    for row in envelope_rows(id, envelopes) {
        journal.append_blocking(row).expect("append");
    }
    let replay = journal.replay(id).expect("replay");
    journal.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
    replay.events
}

/// The paged road: the same journal shape, attached live and drained.
fn paged_events(
    kind: SessionKind,
    agent_kind: Option<SessionKind>,
    cwd: &str,
    envelopes: &[Value],
) -> Vec<SessionEvent> {
    let dir = crate::test_dirs::test_temp_dir("devboule-replay-cwd-paged");
    let id = "s.replay.cwd.paged";
    let (_journal, _runtime, conn) = attach_live_agent_replay(
        &dir,
        session_row(id, kind, cwd),
        agent_kind,
        envelope_rows(id, envelopes),
    );
    let events = drain(&conn);
    let _ = std::fs::remove_dir_all(&dir);
    events
}

fn claude_read_envelope(root: &str) -> Value {
    json!({"type": "assistant", "message": {"id": "m1", "role": "assistant",
        "content": [{"type": "tool_use", "id": "t1", "name": "Read",
            "input": {"file_path": under_root(root)}}]}})
}

fn codex_file_change_envelope(root: &str) -> Value {
    json!({"method": "item/started", "params": {"threadId": "t-1",
        "item": {"id": "i1", "type": "fileChange",
            "changes": [{"path": under_root(root)}]}}})
}

fn acp_tool_call_envelope(root: &str) -> Value {
    json!({"method": "session/update", "params": {"sessionId": "grok-sess",
        "update": {"sessionUpdate": "tool_call", "toolCallId": "c1",
            "title": "Read file", "kind": "read",
            "locations": [{"path": under_root(root), "line": 3}]}}})
}

fn live_paths(envelope: &Value, cwd: &std::path::Path) -> Vec<String> {
    if envelope.get("method").and_then(Value::as_str) == Some("session/update") {
        return tool_call_paths(&crate::acp_view::view_from_envelope_in(
            envelope,
            "grok-sess",
            Some(cwd),
        ));
    }
    if envelope.get("method").and_then(Value::as_str) == Some("item/started") {
        return tool_call_paths(
            &crate::codex_view::CodexView::new(Some(cwd.to_path_buf())).ingest(envelope),
        );
    }
    tool_call_paths(&crate::claude_view::ClaudeView::new(Some(cwd.to_path_buf())).ingest(envelope))
}

/// One provider's under-cwd location, on the live derivation and both replay
/// roads: the rebuild and the paged attach must match what live shows.
fn assert_relative_on_all_roads(
    kind: SessionKind,
    agent_kind: Option<SessionKind>,
    envelope_for: impl Fn(&str) -> Value,
) {
    let dir = crate::test_dirs::test_temp_dir("devboule-replay-cwd-root");
    let root = dir.to_string_lossy().into_owned();
    assert_eq!(
        live_paths(&envelope_for(&root), &dir),
        vec![relative_tail()],
        "live derivation"
    );
    assert_eq!(
        tool_call_paths(&paged_events(
            kind.clone(),
            agent_kind,
            &root,
            std::slice::from_ref(&envelope_for(&root))
        )),
        vec![relative_tail()],
        "paged attach road"
    );
    assert_eq!(
        tool_call_paths(&rebuild_events(
            kind,
            &root,
            std::slice::from_ref(&envelope_for(&root))
        )),
        vec![relative_tail()],
        "journal rebuild road"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// One provider's out-of-cwd location: every road returns the path exactly as
/// received, so an action outside the workspace stays visible as such.
fn assert_absolute_on_all_roads(
    kind: SessionKind,
    agent_kind: Option<SessionKind>,
    envelope_for: impl Fn(&str) -> Value,
) {
    assert!(
        std::path::Path::new(OUTSIDE).is_absolute(),
        "the fixture must take the cwd branch"
    );
    let dir = crate::test_dirs::test_temp_dir("devboule-replay-cwd-outside");
    let root = dir.to_string_lossy().into_owned();
    let absolute = vec![OUTSIDE.to_string()];
    assert_eq!(
        live_paths(&substitute_path(&envelope_for(&root), OUTSIDE), &dir),
        absolute,
        "live derivation"
    );
    assert_eq!(
        tool_call_paths(&paged_events(
            kind.clone(),
            agent_kind,
            &root,
            std::slice::from_ref(&substitute_path(&envelope_for(&root), OUTSIDE))
        )),
        absolute,
        "paged attach road"
    );
    assert_eq!(
        tool_call_paths(&rebuild_events(
            kind,
            &root,
            std::slice::from_ref(&substitute_path(&envelope_for(&root), OUTSIDE))
        )),
        absolute,
        "journal rebuild road"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// The envelope's path replaced with `path`, keeping the rest of the shape.
fn substitute_path(envelope: &Value, path: &str) -> Value {
    let mut envelope = envelope.clone();
    match envelope.get("method").and_then(Value::as_str) {
        Some("item/started") => {
            envelope["params"]["item"]["changes"][0]["path"] = json!(path);
        }
        Some("session/update") => {
            envelope["params"]["update"]["locations"][0]["path"] = json!(path);
        }
        _ => {
            envelope["message"]["content"][0]["input"]["file_path"] = json!(path);
        }
    }
    envelope
}

#[test]
fn replayed_read_locations_relativize_like_live() {
    assert_relative_on_all_roads(SessionKind::Claude, None, claude_read_envelope);
}

#[test]
fn replayed_codex_file_change_relativizes_like_live() {
    assert_relative_on_all_roads(
        SessionKind::Codex,
        Some(SessionKind::Codex),
        codex_file_change_envelope,
    );
}

#[test]
fn replayed_acp_tool_call_locations_relativize_like_live() {
    assert_relative_on_all_roads(SessionKind::Acp, None, acp_tool_call_envelope);
}

#[test]
fn paths_outside_the_session_cwd_stay_absolute_on_every_road() {
    assert_absolute_on_all_roads(SessionKind::Claude, None, claude_read_envelope);
    assert_absolute_on_all_roads(
        SessionKind::Codex,
        Some(SessionKind::Codex),
        codex_file_change_envelope,
    );
    assert_absolute_on_all_roads(SessionKind::Acp, None, acp_tool_call_envelope);
}
