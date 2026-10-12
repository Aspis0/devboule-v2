//! A live file change is confined before it is queued: an edit outside the
//! workspace root leaves the event, and one inside it arrives root-relative.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use devboule_protocol::{
    AgentEditKind, AgentEditText, AgentToolEdit, SessionEvent, SessionKind, WorkspaceIsolation,
};

use super::{PendingItem, SessionRuntime};
use crate::journal::{
    new_session_record, now_ms, tmp_journal, Journal, ProjectRecord, WorkspaceRecord,
};
use crate::test_support::fixture_path;

const SESSION: &str = "s.live.edits";

/// A runtime journaled under a workspace rooted at the fixture root, with its
/// provider's directory set to `cwd`.
fn journaled_runtime(cwd: &Path) -> (PathBuf, SessionRuntime) {
    let (dir, path) = tmp_journal();
    let journal = Journal::open(&path).expect("open");
    journal
        .project_add(ProjectRecord {
            id: "p.live".to_string(),
            name: "Live".to_string(),
            path: crate::test_support::FIXTURE_ROOT.to_string(),
            git_state: "not_repository".to_string(),
            created_at_ms: now_ms(),
            updated_at_ms: now_ms(),
        })
        .expect("project row");
    journal
        .workspace_create(WorkspaceRecord {
            id: "ws.live".to_string(),
            project_id: "p.live".to_string(),
            title: "Live".to_string(),
            isolation: WorkspaceIsolation::Local,
            path: crate::test_support::FIXTURE_ROOT.to_string(),
            branch: None,
            created_at_ms: now_ms(),
            updated_at_ms: now_ms(),
        })
        .expect("workspace row");
    let mut record = new_session_record(
        SESSION,
        "S-1-5-21-1",
        Some("ws.live".to_string()),
        SessionKind::Claude,
        "Ada",
    );
    record.cwd = Some(cwd.to_string_lossy().into_owned());
    journal.upsert_blocking(record).expect("session row");
    let runtime = SessionRuntime::with_journal(SESSION.to_string(), Some(Arc::new(journal)));
    (dir, runtime)
}

fn replace(path: &str) -> AgentToolEdit {
    AgentToolEdit {
        path: path.to_string(),
        kind: AgentEditKind::Edit,
        text: Some(AgentEditText::Replace {
            old: "one\n".to_string(),
            new: "two\n".to_string(),
        }),
        truncated: false,
        added: 1,
        removed: 1,
    }
}

fn tool_call(edits: Vec<AgentToolEdit>) -> SessionEvent {
    SessionEvent::AgentToolCall {
        tool_call_id: "toolu_live".to_string(),
        title: "Edit".to_string(),
        status: "pending".to_string(),
        kind: Some("edit".to_string()),
        locations: None,
        edits: Some(edits),
        subagent_type: None,
        parent_tool_use_id: None,
        spawn_depth: None,
        command: None,
        exit_code: None,
        background: None,
    }
}

/// The edits of every tool call the runtime queued for its subscribers, in order.
fn queued_edits(runtime: &SessionRuntime) -> Vec<Option<Vec<AgentToolEdit>>> {
    let stream = runtime.stream.lock().expect("stream");
    stream
        .agent_backlog
        .iter()
        .filter_map(|item| match item {
            PendingItem::Agent {
                event: SessionEvent::AgentToolCall { edits, .. },
                ..
            } => Some(edits.clone()),
            _ => None,
        })
        .collect()
}

#[test]
fn a_live_edit_outside_the_workspace_leaves_the_queued_event() {
    let (dir, runtime) = journaled_runtime(Path::new(crate::test_support::FIXTURE_ROOT));
    let outside = Path::new(crate::test_support::FIXTURE_ROOT)
        .parent()
        .expect("the fixture root has a parent")
        .join("other.txt");
    runtime.publish_agent_event(
        tool_call(vec![
            replace(&fixture_path("src/lib.rs").to_string_lossy()),
            replace("../secret"),
            replace(&outside.to_string_lossy()),
        ]),
        None,
    );
    let queued = queued_edits(&runtime);
    drop(runtime);
    let _ = std::fs::remove_dir_all(&dir);

    let [Some(edits)] = queued.as_slice() else {
        panic!("expected one queued tool call with edits, got {queued:?}");
    };
    let paths: Vec<&str> = edits.iter().map(|edit| edit.path.as_str()).collect();
    assert_eq!(paths, ["src/lib.rs"]);
}

#[test]
fn a_live_edit_with_only_outside_paths_queues_no_edits_at_all() {
    let (dir, runtime) = journaled_runtime(Path::new(crate::test_support::FIXTURE_ROOT));
    runtime.publish_agent_event(tool_call(vec![replace("../secret")]), None);
    let queued = queued_edits(&runtime);
    drop(runtime);
    let _ = std::fs::remove_dir_all(&dir);

    assert_eq!(queued, vec![None]);
}
