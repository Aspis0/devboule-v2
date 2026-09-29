//! Tests for the checklist seed: the journal fold and its byte gates.

use std::sync::Arc;

use devboule_protocol::SessionEvent;

use super::{envelope_is_lifecycle, envelope_may_carry_tasks, seed_claude_task_state};
use crate::session::SessionRuntime;

#[test]
fn the_task_marker_prefilter_passes_task_envelopes_only() {
    for name in ["TodoWrite", "TaskCreate", "TaskUpdate", "TaskList"] {
        let payload = format!(
            r#"{{"type":"assistant","message":{{"content":[{{"type":"tool_use","id":"1","name":"{name}","input":{{}}}}]}}}}"#
        );
        assert!(
            envelope_may_carry_tasks(payload.as_bytes()),
            "{name} must pass the prefilter"
        );
    }
    // Plain text, ordinary tool calls and bare results carry no task tool —
    // even when prose mentions one without the wire's quoted name.
    for payload in [
        r#"{"type":"assistant","message":{"content":[{"type":"text","text":"use TodoWrite for planning"}]}}"#,
        r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"1","name":"Bash","input":{"command":"ls"}}]}}"#,
        r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"1","content":"ok"}]}}"#,
    ] {
        assert!(
            !envelope_may_carry_tasks(payload.as_bytes()),
            "task-less payload must skip parsing: {payload}"
        );
    }
    // Undecodable bytes parse rather than vanish: the replay skips them.
    assert!(envelope_may_carry_tasks(&[0xff, 0xfe]));
}

#[test]
fn the_lifecycle_gate_admits_init_and_result_only() {
    // `system/init` (rebind reset) and turn-end `result` (pending-call
    // clearing) parse in the seed however innocent their prose. Anything
    // else without a task name or a pending pair stays out.
    for payload in [
        r#"{"type":"system","subtype":"init","session_id":"cli-1","permissionMode":"default","model":"m"}"#,
        r#"{"type":"result","subtype":"success","result":"All done, filed the summary."}"#,
    ] {
        assert!(
            envelope_is_lifecycle(payload.as_bytes()),
            "lifecycle frames parse: {payload}"
        );
        assert!(
            !envelope_may_carry_tasks(payload.as_bytes()),
            "lifecycle frames name no task tool: {payload}"
        );
    }
    for payload in [
        r#"{"type":"system","subtype":"status","model":"m"}"#,
        r#"{"type":"assistant","message":{"content":[{"type":"text","text":"hi"}]}}"#,
        r#"{"type":"control_response","response":{"request_id":"r1"}}"#,
    ] {
        assert!(
            !envelope_is_lifecycle(payload.as_bytes()),
            "nothing else parses as lifecycle: {payload}"
        );
    }
    assert!(envelope_is_lifecycle(&[0xff, 0xfe]));
}

#[test]
fn task_seed_folds_journalled_task_envelopes_through_ingest() {
    // The seed drives the shared replay road folding page by page: pairs
    // land across the noise, and the lifecycle frames parse without moving
    // the list — the init carries `session_id`, `permissionMode` and
    // `model` like the real one, and the turn-end result carries innocent
    // prose that names no task tool.
    let (dir, path) = crate::journal::tmp_journal();
    let journal = Arc::new(crate::journal::Journal::open(&path).expect("open"));
    let session_id = "s.claude.seed";
    journal
        .upsert_blocking(crate::journal::new_session_record(
            session_id,
            "owner",
            None,
            devboule_protocol::SessionKind::Claude,
            "Seed",
        ))
        .expect("upsert");
    let init = serde_json::json!({"type": "system", "subtype": "init",
        "session_id": "cli-session-1", "permissionMode": "default",
        "model": "claude-sonnet-4-6"});
    let todo = serde_json::json!({"type": "assistant", "message": {"id": "m1",
        "role": "assistant", "content": [{"type": "tool_use", "id": "todo-1",
        "name": "TodoWrite", "input": {"todos": [{"content": "Legacy"}]}}]}});
    let noise = serde_json::json!({"type": "assistant", "message": {"id": "m2",
        "role": "assistant", "content": [{"type": "text", "text": "just thinking"}]}});
    let create = serde_json::json!({"type": "assistant", "message": {"id": "m3",
        "role": "assistant", "content": [{"type": "tool_use", "id": "create-1",
        "name": "TaskCreate", "input": {"subject": "Alpha"}}]}});
    let created = serde_json::json!({"type": "user", "message": {"role": "user",
        "content": [{"type": "tool_result", "tool_use_id": "create-1",
        "content": "ok"}]}, "tool_use_result": {"task": {"id": "1", "subject": "Alpha"}}});
    let done = serde_json::json!({"type": "result", "subtype": "success",
        "result": "All done, filed the summary."});
    for (seq, envelope) in [init, todo, noise, create, created, done]
        .into_iter()
        .enumerate()
    {
        journal
            .append_blocking(
                crate::journal::acp_envelope_record(session_id, 1, seq as u64 + 1, &envelope)
                    .expect("record"),
            )
            .expect("append");
    }
    let runtime = SessionRuntime::with_journal(session_id.to_string(), Some(Arc::clone(&journal)));
    let mut view = crate::claude_view::ClaudeView::new(None);
    assert!(seed_claude_task_state(&runtime, &mut view));
    view.ingest(
        &serde_json::json!({"type": "assistant", "message": {"id": "m4",
        "role": "assistant", "content": [{"type": "tool_use", "id": "update-1",
        "name": "TaskUpdate", "input": {"taskId": "1", "status": "in_progress"}}]}}),
    );
    let events = view.ingest(&serde_json::json!({"type": "user",
        "message": {"role": "user", "content": [{"type": "tool_result",
        "tool_use_id": "update-1", "content": "ok"}]}}));
    assert!(
        events.iter().any(|event| matches!(
            event,
            SessionEvent::AgentTasks { items } if items.len() == 2
                && items[1].status == devboule_protocol::AgentTaskStatus::InProgress
        )),
        "the seeded list accepts the live update: {events:?}"
    );
    assert_eq!(runtime.task_seed_failure_count(), 0);
    journal.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn task_seed_rebind_resets_across_cli_sessions() {
    // Two CLI session ids in one journal: the second init resets the list
    // during the seed, so the first session's task is gone and only the
    // second session's work lands.
    let (dir, path) = crate::journal::tmp_journal();
    let journal = Arc::new(crate::journal::Journal::open(&path).expect("open"));
    let session_id = "s.claude.seed.rebind";
    journal
        .upsert_blocking(crate::journal::new_session_record(
            session_id,
            "owner",
            None,
            devboule_protocol::SessionKind::Claude,
            "Seed",
        ))
        .expect("upsert");
    let init_a = serde_json::json!({"type": "system", "subtype": "init",
        "session_id": "cli-a", "permissionMode": "default", "model": "m"});
    let todo = serde_json::json!({"type": "assistant", "message": {"id": "m1",
        "role": "assistant", "content": [{"type": "tool_use", "id": "todo-1",
        "name": "TodoWrite", "input": {"todos": [{"content": "Old"}]}}]}});
    let init_b = serde_json::json!({"type": "system", "subtype": "init",
        "session_id": "cli-b", "permissionMode": "default", "model": "m"});
    for (seq, envelope) in [init_a, todo, init_b].into_iter().enumerate() {
        journal
            .append_blocking(
                crate::journal::acp_envelope_record(session_id, 1, seq as u64 + 1, &envelope)
                    .expect("record"),
            )
            .expect("append");
    }
    let runtime = SessionRuntime::with_journal(session_id.to_string(), Some(Arc::clone(&journal)));
    let mut view = crate::claude_view::ClaudeView::new(None);
    // The seeded state, not the return contract: the fold test pins that.
    let _ = seed_claude_task_state(&runtime, &mut view);
    // The first session's task is gone: its update applies nothing.
    view.ingest(
        &serde_json::json!({"type": "assistant", "message": {"id": "m2",
        "role": "assistant", "content": [{"type": "tool_use", "id": "u1",
        "name": "TaskUpdate", "input": {"taskId": "legacy:0", "status": "completed"}}]}}),
    );
    let events = view.ingest(&serde_json::json!({"type": "user",
        "message": {"role": "user", "content": [{"type": "tool_result",
        "tool_use_id": "u1", "content": "ok"}]}}));
    assert!(
        events
            .iter()
            .all(|event| !matches!(event, SessionEvent::AgentTasks { .. })),
        "the rebound seed holds no task to update: {events:?}"
    );
    assert_eq!(runtime.task_seed_failure_count(), 0);
    journal.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn task_seed_turn_end_clears_pending_calls() {
    // The turn-end `result` names no task tool — its closing prose is
    // innocent — yet the seed must parse it: `end_turn` clears the
    // TaskCreate call, so a late result for it applies nothing afterwards.
    let (dir, path) = crate::journal::tmp_journal();
    let journal = Arc::new(crate::journal::Journal::open(&path).expect("open"));
    let session_id = "s.claude.seed.endturn";
    journal
        .upsert_blocking(crate::journal::new_session_record(
            session_id,
            "owner",
            None,
            devboule_protocol::SessionKind::Claude,
            "Seed",
        ))
        .expect("upsert");
    let create = serde_json::json!({"type": "assistant", "message": {"id": "m1",
        "role": "assistant", "content": [{"type": "tool_use", "id": "create-1",
        "name": "TaskCreate", "input": {"subject": "Alpha"}}]}});
    let done = serde_json::json!({"type": "result", "subtype": "success",
        "result": "All done, filed the summary."});
    for (seq, envelope) in [create, done].into_iter().enumerate() {
        journal
            .append_blocking(
                crate::journal::acp_envelope_record(session_id, 1, seq as u64 + 1, &envelope)
                    .expect("record"),
            )
            .expect("append");
    }
    let runtime = SessionRuntime::with_journal(session_id.to_string(), Some(Arc::clone(&journal)));
    let mut view = crate::claude_view::ClaudeView::new(None);
    // Unchecked like the rebind test above: state, not contract.
    let _ = seed_claude_task_state(&runtime, &mut view);
    // The interrupted call never resolves: its late result applies nothing.
    let events = view.ingest(&serde_json::json!({"type": "user",
        "message": {"role": "user", "content": [{"type": "tool_result",
        "tool_use_id": "create-1", "content": "ok"}]},
        "tool_use_result": {"task": {"id": "9", "subject": "Ghost"}}}));
    assert!(
        events
            .iter()
            .all(|event| !matches!(event, SessionEvent::AgentTasks { .. })),
        "the cleared call applies nothing late: {events:?}"
    );
    assert_eq!(runtime.task_seed_failure_count(), 0);
    journal.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn task_seed_on_an_empty_journal_succeeds_with_nothing() {
    let (dir, path) = crate::journal::tmp_journal();
    let journal = Arc::new(crate::journal::Journal::open(&path).expect("open"));
    let session_id = "s.claude.seed.empty";
    journal
        .upsert_blocking(crate::journal::new_session_record(
            session_id,
            "owner",
            None,
            devboule_protocol::SessionKind::Claude,
            "Seed",
        ))
        .expect("upsert");
    let runtime = SessionRuntime::with_journal(session_id.to_string(), Some(Arc::clone(&journal)));
    let mut view = crate::claude_view::ClaudeView::new(None);
    assert!(seed_claude_task_state(&runtime, &mut view));
    assert_eq!(runtime.task_seed_failure_count(), 0);
    journal.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn task_seed_failure_counts_and_does_not_flag() {
    // A journal that cannot answer leaves the view untouched, counts the
    // failure for diagnostics, and reports false so the reader retries on
    // the next frame instead of freezing on an empty list.
    let (dir, path) = crate::journal::tmp_journal();
    let journal = Arc::new(crate::journal::Journal::open(&path).expect("open"));
    let session_id = "s.claude.seed.dead";
    journal.shutdown();
    let runtime = SessionRuntime::with_journal(session_id.to_string(), Some(Arc::clone(&journal)));
    let mut view = crate::claude_view::ClaudeView::new(None);
    assert!(!seed_claude_task_state(&runtime, &mut view));
    assert_eq!(runtime.task_seed_failure_count(), 1);
    let _ = std::fs::remove_dir_all(&dir);
}
