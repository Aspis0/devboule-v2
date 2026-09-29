use super::{fixture_frames, CodexView, PlanCompletion};
use devboule_protocol::{AgentTaskStatus, SessionEvent};
use serde_json::json;

const LIVE_PLAN: &str = include_str!("../fixtures/wire/codex/codex-plan-items-live.jsonl");
// SYNTHETIC: a `turn/plan/updated` frame shaped like the protocol's
// plan update — invented steps, no measured run behind it (no fixture
// carries one today).
const PLAN_UPDATE_SYNTHETIC: &str =
    include_str!("../fixtures/wire/codex/codex-plan-update-synthetic.jsonl");

fn plan_update_envelope() -> serde_json::Value {
    fixture_frames(PLAN_UPDATE_SYNTHETIC)
        .into_iter()
        .find(|frame| frame["method"] == "turn/plan/updated")
        .expect("synthetic plan update fixture")
}

fn agent_tasks(events: Vec<SessionEvent>) -> Vec<devboule_protocol::AgentTaskItem> {
    match events.as_slice() {
        [SessionEvent::AgentTasks { items }] => items.clone(),
        other => panic!("expected exactly one AgentTasks event, got {other:?}"),
    }
}

#[test]
fn live_plan_item_is_held_for_a_plan_card_only_after_clean_completion() {
    let frames = fixture_frames(LIVE_PLAN);
    let mut view = CodexView::new(None);
    view.set_plan_mode(true);
    view.set_capture_plan(true);
    let mut plan = None;
    let mut completed = None;
    for frame in &frames {
        match frame["method"].as_str().expect("method") {
            "item/started" | "item/completed" => {
                let events = view.ingest(frame);
                assert!(events.is_empty(), "plan items stay behind the card");
                if frame["method"] == "item/completed" {
                    plan = frame
                        .pointer("/params/item/text")
                        .and_then(|text| text.as_str())
                        .map(str::to_string);
                }
            }
            "turn/completed" => completed = Some(frame),
            _ => unreachable!(),
        }
    }
    let completion = completed.expect("captured completion frame");
    // The capture's own shape: the plan item is the turn's `-plan` item, and
    // the completion carries the turn's real status and timing fields — the
    // invented fixture this replaced had placeholder ids and none of them.
    let item = frames
        .iter()
        .find(|frame| frame["method"] == "item/completed")
        .expect("captured item frame");
    let turn_id = item["params"]["turnId"].as_str().expect("turn id");
    assert_eq!(
        item["params"]["item"]["id"].as_str().expect("item id"),
        format!("{turn_id}-plan"),
        "the captured plan item is the turn's -plan item"
    );
    assert!(
        item["params"]["completedAtMs"].is_u64(),
        "the capture carries the item's completion time"
    );
    assert_eq!(
        view.take_completed_plan(&completion["params"]),
        plan.clone().map(PlanCompletion::Card)
    );
    // A turn that does not complete cleanly still folds the text into the
    // transcript as the plain plan row, instead of losing it.
    let mut failed = CodexView::new(None);
    failed.set_plan_mode(true);
    failed.set_capture_plan(true);
    failed.ingest(item);
    assert_eq!(
        failed.take_completed_plan(&json!({"turn":{"status":"interrupted"}})),
        Some(PlanCompletion::Row(plan.expect("captured plan text")))
    );
}

#[test]
fn plan_update_steps_are_latest_until_a_plan_item_replaces_them() {
    let mut view = CodexView::new(None);
    view.set_plan_mode(true);
    view.set_capture_plan(true);
    view.ingest(&json!({
        "method": "turn/plan/updated",
        "params": {"plan": [{"step": "Inspect the project", "status": "pending"}]}
    }));
    let item = json!({
        "method": "item/completed",
        "params": {"item": {"id": "plan-1", "type": "plan", "text": "## Final plan"}}
    });
    view.ingest(&item);
    assert_eq!(
        view.take_completed_plan(&json!({"turn":{"status":"completed"}})),
        Some(PlanCompletion::Card("## Final plan".to_string()))
    );
}

#[test]
fn plan_update_alone_supplies_the_completed_plan() {
    let mut view = CodexView::new(None);
    view.set_plan_mode(true);
    view.set_capture_plan(true);
    view.ingest(&json!({
        "method": "turn/plan/updated",
        "params": {"plan": [
            {"step": "Inspect the project", "status": "completed"},
            {"step": "Create the file", "status": "pending"}
        ]}
    }));
    assert_eq!(
        view.take_completed_plan(&json!({"turn":{"status":"completed"}})),
        Some(PlanCompletion::Card(
            ["- Inspect the project", "- Create the file"].join("\n")
        ))
    );
}

#[test]
fn plan_card_requires_enabled_mode_clean_completion_and_text() {
    let completed = json!({"turn":{"status":"completed"}});

    let mut disabled = CodexView::new(None);
    disabled.set_capture_plan(true);
    disabled.ingest(&json!({
        "method":"item/completed",
        "params":{"item":{"type":"plan","text":"Plan text"}}
    }));
    assert_eq!(disabled.take_completed_plan(&completed), None);

    let mut empty = CodexView::new(None);
    empty.set_plan_mode(true);
    empty.set_capture_plan(true);
    assert_eq!(empty.take_completed_plan(&completed), None);

    let mut interrupted = CodexView::new(None);
    interrupted.set_plan_mode(true);
    interrupted.set_capture_plan(true);
    interrupted.ingest(&json!({
        "method":"item/completed",
        "params":{"item":{"type":"plan","text":"Plan text"}}
    }));
    assert_eq!(
        interrupted.take_completed_plan(&json!({"turn":{"status":"interrupted"}})),
        Some(PlanCompletion::Row("Plan text".to_string()))
    );
}

#[test]
fn plan_mode_off_keeps_the_plan_item_as_a_todo_row() {
    let frames = fixture_frames(LIVE_PLAN);
    let mut view = CodexView::new(None);
    view.set_plan_mode(false);
    view.set_capture_plan(true);
    let start = &frames[0];
    let call = view.ingest(start);
    assert!(
        matches!(call.as_slice(), [SessionEvent::AgentToolCall { kind: Some(kind), .. }] if kind == "plan")
    );
    let update = view.ingest(&frames[1]);
    assert!(
        matches!(update.as_slice(), [SessionEvent::AgentToolUpdate { kind: Some(kind), .. }] if kind == "plan")
    );
    assert_eq!(
        view.take_completed_plan(&json!({"turn":{"status":"completed"}})),
        None
    );
}

#[test]
fn plan_mode_off_emits_agent_tasks_from_the_plan_update() {
    let envelope = plan_update_envelope();
    let mut view = CodexView::new(None);
    view.set_plan_mode(false);
    view.set_capture_plan(true);
    let events = view.ingest(&envelope);
    let items = agent_tasks(events);
    // The plan-to-checklist mapping (`agent_tasks_from_plan`): empty steps
    // would be dropped (none here), the id is the
    // step's index, and the status is kept — `inProgress` stays its own
    // state.
    assert_eq!(items.len(), 3);
    assert_eq!(items[0].id.as_deref(), Some("0"));
    assert_eq!(items[0].text, "Inspect the project layout");
    assert_eq!(items[0].status, AgentTaskStatus::Pending);
    assert_eq!(
        items[0].active_form, None,
        "Codex steps carry no running form"
    );
    assert_eq!(items[1].id.as_deref(), Some("1"));
    assert_eq!(items[1].status, AgentTaskStatus::InProgress);
    assert_eq!(items[2].id.as_deref(), Some("2"));
    assert_eq!(items[2].status, AgentTaskStatus::Completed);
}

#[test]
fn plan_mode_off_drops_empty_steps_and_reads_unknown_status_as_pending() {
    let frame = json!({"method": "turn/plan/updated", "params": {"plan": [
        {"step": "  ", "status": "pending"},
        {"step": "Kept", "status": "some_day"},
        {"step": "Also kept", "status": "in_progress"},
    ]}});
    let mut view = CodexView::new(None);
    view.set_plan_mode(false);
    view.set_capture_plan(true);
    let items = agent_tasks(view.ingest(&frame));
    assert_eq!(items.len(), 2, "the whitespace-only step is dropped");
    assert_eq!(
        items[0].id.as_deref(),
        Some("1"),
        "ids are the original indices"
    );
    assert_eq!(items[0].status, AgentTaskStatus::Pending);
    assert_eq!(items[0].active_form, None);
    assert_eq!(items[1].id.as_deref(), Some("2"));
    assert_eq!(items[1].status, AgentTaskStatus::InProgress);
}

#[test]
fn plan_mode_on_keeps_the_plan_card_path_unchanged() {
    // Regression pin: with plan mode on, `turn/plan/updated` captures the
    // text for the approval card and emits no checklist event.
    let envelope = plan_update_envelope();
    let mut view = CodexView::new(None);
    view.set_plan_mode(true);
    view.set_capture_plan(true);
    let events = view.ingest(&envelope);
    assert!(
        events.is_empty(),
        "plan mode on emits no AgentTasks: {events:?}"
    );
    assert_eq!(
        view.take_completed_plan(&json!({"turn": {"status": "completed"}})),
        Some(PlanCompletion::Card(
            [
                "- Inspect the project layout",
                "- Draft the greeting file",
                "- Verify the file on disk"
            ]
            .join("\n")
        ))
    );
}

#[test]
fn replay_equals_live_for_the_plan_update() {
    // The replay path drives the same view through `drive_replay`; the live
    // reader applies the same settings per frame. A plan frame on a foreign
    // thread is dropped live and on replay; a root one emits on both.
    let thread = "thread-root";
    let foreign = json!({"method": "turn/plan/updated", "params": {
        "threadId": "thread-other",
        "plan": [{"step": "Foreign", "status": "pending"}],
    }});
    let root = json!({"method": "turn/plan/updated", "params": {
        "plan": [{"step": "Root", "status": "pending"}],
    }});
    // Live: per-frame capture from the owned thread, mode off.
    let mut live = CodexView::new(None);
    let mut live_events = Vec::new();
    for frame in [&foreign, &root] {
        live.set_capture_plan(crate::codex_compaction::is_root_thread(
            &frame["params"],
            thread,
        ));
        live.set_plan_mode(false);
        live_events.extend(live.ingest(frame));
    }
    // Replay: the shared driver over the same frames.
    let mut replay = CodexView::new(None);
    let plans = std::collections::HashSet::new();
    let mut replay_events = Vec::new();
    for frame in [&foreign, &root] {
        replay_events.extend(super::drive_replay(
            &mut replay,
            Some(thread),
            &plans,
            frame,
        ));
    }
    assert_eq!(live_events, replay_events);
    assert_eq!(replay_events.len(), 1);
    match &replay_events[0] {
        SessionEvent::AgentTasks { items } => {
            assert_eq!(items.len(), 1);
            assert_eq!(items[0].text, "Root");
        }
        other => panic!("expected AgentTasks, got {other:?}"),
    }
}

#[test]
fn replay_suppresses_agent_tasks_for_a_turn_with_a_plan_card() {
    // A plan-mode-ON turn raises an approval card under `{turn}-plan`; the
    // replay recovers the mode from that mark, so the same
    // `turn/plan/updated` emits nothing on replay, as live.
    let turn = "turn-on-1";
    let started = json!({"method": "turn/started", "params": {
        "threadId": "t",
        "turn": {"id": turn},
    }});
    let update = json!({"method": "turn/plan/updated", "params": {
        "threadId": "t",
        "plan": [{"step": "Planned", "status": "pending"}],
    }});
    // Live, plan mode on: the update captures but emits nothing.
    let mut live = CodexView::new(None);
    live.set_capture_plan(true);
    assert!(live.ingest(&started).is_empty());
    live.set_plan_mode(true);
    assert!(live.ingest(&update).is_empty());
    // Replay over the same frames with the turn's card mark: same silence.
    let mut replay = CodexView::new(None);
    let plans = std::collections::HashSet::from([turn.to_string()]);
    assert!(super::drive_replay(&mut replay, Some("t"), &plans, &started).is_empty());
    assert!(
        super::drive_replay(&mut replay, Some("t"), &plans, &update).is_empty(),
        "a card-marked turn replays suppressed, as live"
    );
    // Without the mark the same frames emit, as a plan-mode-OFF turn does.
    let mut replay_off = CodexView::new(None);
    let empty = std::collections::HashSet::new();
    assert!(super::drive_replay(&mut replay_off, Some("t"), &empty, &started).is_empty());
    let items = agent_tasks(super::drive_replay(
        &mut replay_off,
        Some("t"),
        &empty,
        &update,
    ));
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].text, "Planned");
}

#[test]
fn replay_drives_items_but_not_the_checklist_for_a_marked_turn() {
    // A card-marked turn on replay: the plan frames stay silent (live never
    // listed them), but the item rows re-derive — the plan's text lives
    // only in its item frames, while card rows journal no text.
    let turn = "turn-on-1";
    let started = json!({"method": "turn/started", "params": {
        "threadId": "t", "turn": {"id": turn}}});
    let update = json!({"method": "turn/plan/updated", "params": {
        "threadId": "t", "plan": [{"step": "Planned", "status": "pending"}],
    }});
    let item = json!({"method": "item/completed", "params": {
        "threadId": "t",
        "item": {"id": "plan-1", "type": "plan", "text": "## Planned"}}});
    let mut replay = CodexView::new(None);
    let plans = std::collections::HashSet::from([turn.to_string()]);
    assert!(super::drive_replay(&mut replay, Some("t"), &plans, &started).is_empty());
    assert!(super::drive_replay(&mut replay, Some("t"), &plans, &update).is_empty());
    match super::drive_replay(&mut replay, Some("t"), &plans, &item).as_slice() {
        [SessionEvent::AgentToolUpdate { text, kind, .. }] => {
            assert_eq!(kind.as_deref(), Some("plan"));
            assert_eq!(text.as_deref(), Some("## Planned"));
        }
        other => panic!("expected the re-derived plan row, got {other:?}"),
    }
}

#[test]
fn plan_card_marks_its_turn() {
    use devboule_protocol::{PermissionOption, PermissionRequestKind, SessionOrigin};
    let card = SessionEvent::PermissionRequest {
        tool_call_id: "turn-on-1-plan".to_string(),
        title: "Plan".to_string(),
        description: None,
        command: None,
        args: None,
        cwd: None,
        env: None,
        options: vec![PermissionOption {
            option_id: "implement".to_string(),
            name: "Implement".to_string(),
            kind: "allow_once".to_string(),
        }],
        is_chooser: Some(false),
        kind: Some(PermissionRequestKind::Plan),
        plan: Some("Do it".to_string()),
        questions: None,
        origin: SessionOrigin::local(),
        create_agent: None,
    };
    assert_eq!(
        crate::codex_plan_marks::plan_card_turn_id(&card),
        Some("turn-on-1")
    );
    // The journalled outcome row marks the same turn: the request itself
    // rides live only, the verdict is what the journal keeps.
    let outcome = SessionEvent::AgentToolUpdate {
        tool_call_id: "turn-on-1-plan".to_string(),
        status: Some("completed".to_string()),
        text: None,
        title: Some("Approved".to_string()),
        kind: Some("plan".to_string()),
        locations: None,
        parent_tool_use_id: None,
        spawn_depth: None,
    };
    assert_eq!(
        crate::codex_plan_marks::plan_card_turn_id(&outcome),
        Some("turn-on-1")
    );
    // Plain plan rows are not cards: an item row under the same id shape
    // marks nothing, call or completed update alike.
    for event in super::plain_plan_row_events("turn-on-1-plan", "Do it") {
        assert_eq!(crate::codex_plan_marks::plan_card_turn_id(&event), None);
    }
    let item_update = SessionEvent::AgentToolUpdate {
        tool_call_id: "turn-on-1-plan".to_string(),
        status: Some("completed".to_string()),
        text: Some("## Plan".to_string()),
        title: Some("Plan".to_string()),
        kind: Some("plan".to_string()),
        locations: None,
        parent_tool_use_id: None,
        spawn_depth: None,
    };
    assert_eq!(
        crate::codex_plan_marks::plan_card_turn_id(&item_update),
        None
    );
}

#[test]
fn plan_mode_off_still_captures_the_text_and_still_discards_it() {
    // The capture is not gated on the emission: the text is captured as
    // before, and `take_completed_plan` still discards it because the
    // checklist event already carried the steps. The capture is proved by
    // flipping the mode back on: the same turn then completes as the card.
    let envelope = plan_update_envelope();
    let mut view = CodexView::new(None);
    view.set_plan_mode(false);
    view.set_capture_plan(true);
    let items = agent_tasks(view.ingest(&envelope));
    assert_eq!(items.len(), 3);
    view.set_plan_mode(true);
    assert_eq!(
        view.take_completed_plan(&json!({"turn": {"status": "completed"}})),
        Some(PlanCompletion::Card(
            [
                "- Inspect the project layout",
                "- Draft the greeting file",
                "- Verify the file on disk"
            ]
            .join("\n")
        ))
    );
}

#[test]
fn starting_a_new_turn_clears_the_previous_plan() {
    let mut view = CodexView::new(None);
    view.set_plan_mode(true);
    view.set_capture_plan(true);
    view.ingest(&json!({
        "method": "turn/plan/updated",
        "params": {"plan": [{"step": "Old plan", "status": "pending"}]}
    }));
    view.ingest(&json!({"method":"turn/started","params":{}}));
    assert_eq!(
        view.take_completed_plan(&json!({"turn":{"status":"completed"}})),
        None
    );
}
