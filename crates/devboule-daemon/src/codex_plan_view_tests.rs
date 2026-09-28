use super::{fixture_frames, CodexView, PlanCompletion};
use devboule_protocol::SessionEvent;
use serde_json::json;

const LIVE_PLAN: &str = include_str!("../fixtures/wire/codex/codex-plan-items-live.jsonl");

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
