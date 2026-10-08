//! One daemon-level proof: a child agent appears as a running task and turns
//! finished — through the request and through the live event, on the real
//! publish path.

use devboule_protocol::{AgentTaskState, OwnerId, SessionEvent, SessionTaskKind, SessionTaskState};

use super::super::session_idle_close_tests::{linked_child, linked_creator};
use super::super::session_queue_fixtures::attached_without_queue_capability;
use super::super::tests::{test_owner, tmp_delete_registry};
use super::super::SessionRegistry;

fn published_snapshots(
    registry: &SessionRegistry,
    parent: &str,
    owner: &OwnerId,
    conn_id: u64,
) -> Vec<SessionEvent> {
    let conn = attached_without_queue_capability(registry, parent, conn_id, owner);
    let mut events = Vec::new();
    loop {
        let batch = conn.pull_events();
        if batch.is_empty() {
            return events;
        }
        for event in &batch {
            conn.event_sent(event);
        }
        events.extend(batch.into_iter().map(|pending| pending.envelope.event));
    }
}

#[test]
fn a_child_is_a_running_task_then_a_finished_one() {
    let (_dir, registry, _journal) = tmp_delete_registry();
    let owner = test_owner("tasks-user", "tasks-client");
    let parent = "s.tasks.parent";
    linked_creator(&registry, parent, &owner);
    linked_child(&registry, "s.tasks.child", &owner, parent);
    let runtime = registry
        .live_runtime(parent, &owner)
        .expect("parent runtime");
    registry.configure_runtime_attention(&runtime, &owner);

    let tasks = registry.session_tasks(parent, &owner).expect("tasks");
    assert_eq!(tasks.len(), 1, "the child is the parent's one task");
    assert_eq!(tasks[0].kind, SessionTaskKind::Agent);
    assert_eq!(tasks[0].state, SessionTaskState::Running);
    assert_eq!(tasks[0].child_session_id.as_deref(), Some("s.tasks.child"));

    let events = published_snapshots(&registry, parent, &owner, 41);
    let snapshot = events.iter().find_map(|event| match event {
        SessionEvent::TasksSnapshot { tasks } => Some(tasks),
        _ => None,
    });
    assert_eq!(
        snapshot.unwrap_or_else(|| panic!("no tasks_snapshot in {events:?}")),
        &tasks,
        "the attach hands the same list the request answers"
    );

    runtime
        .publish_child_finished(
            None,
            "s.tasks.child",
            "child",
            AgentTaskState::Completed,
            None,
            Vec::new(),
        )
        .expect("finish publishes");

    let tasks = registry.session_tasks(parent, &owner).expect("tasks");
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0].state, SessionTaskState::Finished);
    assert!(
        tasks[0].ended_at_ms.is_some(),
        "the finish row times the end"
    );

    let events = published_snapshots(&registry, parent, &owner, 42);
    let finished = events.iter().filter_map(|event| match event {
        SessionEvent::TasksSnapshot { tasks } => Some(tasks),
        _ => None,
    });
    assert!(
        finished
            .flat_map(|tasks| tasks.iter())
            .any(|task| task.state == SessionTaskState::Finished),
        "the finish publishes a snapshot that says so"
    );
}
