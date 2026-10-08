//! One daemon-level proof: a child agent appears as a running task and turns
//! finished — through the request and through the live event, on the real
//! publish path — plus the guards around it: an attachment without the cap
//! sees nothing, and a dead parent cancels what still ran.

use devboule_protocol::{AgentTaskState, OwnerId, SessionEvent, SessionTaskKind, SessionTaskState};

use super::super::session_idle_close_tests::{linked_child, linked_creator};
use super::super::session_queue_fixtures::attached_without_queue_capability;
use super::super::tests::{test_owner, tmp_delete_registry};
use super::super::*;
use super::{refresh_due, TASKS_REFRESH_DEBOUNCE};
use std::time::{Duration, Instant};

fn snapshots_of(
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

fn task_lists(events: &[SessionEvent]) -> Vec<&Vec<devboule_protocol::SessionTask>> {
    events
        .iter()
        .filter_map(|event| match event {
            SessionEvent::TasksSnapshot { tasks, .. } => Some(tasks),
            _ => None,
        })
        .collect()
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
    runtime.test_reset_tasks_throttle();

    let tasks = registry.session_tasks(parent, &owner).expect("tasks");
    assert_eq!(tasks.len(), 1, "the child is the parent's one task");
    assert_eq!(tasks[0].kind, SessionTaskKind::Agent);
    assert_eq!(tasks[0].state, SessionTaskState::Running);
    assert_eq!(tasks[0].child_session_id.as_deref(), Some("s.tasks.child"));

    let events = snapshots_of(&registry, parent, &owner, 41);
    let lists = task_lists(&events);
    assert_eq!(lists.len(), 1, "the attach hands one snapshot");
    assert_eq!(lists[0], &tasks, "the same list the request answers");
    let revision = match events.iter().find_map(|event| match event {
        SessionEvent::TasksSnapshot { revision, .. } => Some(*revision),
        _ => None,
    }) {
        Some(revision) => revision,
        None => panic!("no tasks_snapshot in {events:?}"),
    };
    assert_eq!(revision, 1, "revisions count from 1");

    runtime.test_reset_tasks_throttle();
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

    runtime.test_reset_tasks_throttle();
    let events = snapshots_of(&registry, parent, &owner, 42);
    let finished: Vec<_> = task_lists(&events)
        .into_iter()
        .flat_map(|tasks| tasks.iter())
        .collect();
    assert!(
        finished
            .iter()
            .any(|task| task.state == SessionTaskState::Finished),
        "the finish publishes a snapshot that says so"
    );
    let later = events.iter().find_map(|event| match event {
        SessionEvent::TasksSnapshot { revision, .. } => Some(*revision),
        _ => None,
    });
    assert!(
        later.is_some_and(|next| next > revision),
        "the later snapshot carries a newer revision"
    );
}

#[test]
fn an_attachment_without_the_capability_sees_no_snapshot() {
    let (_dir, registry, _journal) = tmp_delete_registry();
    let owner = test_owner("tasks-cap-user", "tasks-cap-client");
    let parent = "s.tasks.cap.parent";
    linked_creator(&registry, parent, &owner);
    linked_child(&registry, "s.tasks.cap.child", &owner, parent);

    let conn = {
        let conn = ConnHandle::new(43);
        conn.set_session_queue_negotiated(false);
        conn.set_session_tasks_negotiated(false);
        registry
            .attach_with_subscription(parent, 43, None, &conn, &owner, false)
            .expect("attach");
        conn
    };
    registry.refresh_session_tasks(parent, &[]);
    let mut events = Vec::new();
    loop {
        let batch = conn.pull_events();
        if batch.is_empty() {
            break;
        }
        for event in &batch {
            conn.event_sent(event);
        }
        events.extend(batch.into_iter().map(|pending| pending.envelope.event));
    }
    assert!(
        task_lists(&events).is_empty(),
        "a v27-style attachment is never sent the event, not even an empty one"
    );
}

#[test]
fn a_dead_parent_cancels_what_still_ran() {
    let (_dir, registry, _journal) = tmp_delete_registry();
    let owner = test_owner("tasks-exit-user", "tasks-exit-client");
    let parent = "s.tasks.exit.parent";
    linked_creator(&registry, parent, &owner);
    linked_child(&registry, "s.tasks.exit.child", &owner, parent);
    let runtime = registry
        .live_runtime(parent, &owner)
        .expect("parent runtime");
    registry.configure_runtime_attention(&runtime, &owner);

    runtime.mark_exited(Some(1));

    let tasks = registry.session_tasks(parent, &owner).expect("tasks");
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0].state, SessionTaskState::Cancelled);
    assert!(
        tasks[0].ended_at_ms.is_some(),
        "the parent's end times the cancellation"
    );
}

#[test]
fn the_debounce_opens_after_its_window() {
    let now = Instant::now();
    assert!(refresh_due(None, now), "the first refresh always derives");
    assert!(
        !refresh_due(Some(now), now),
        "a trigger inside the window defers"
    );
    assert!(
        !refresh_due(
            Some(now),
            now + TASKS_REFRESH_DEBOUNCE - Duration::from_millis(1)
        ),
        "one millisecond early still defers"
    );
    assert!(
        refresh_due(Some(now), now + TASKS_REFRESH_DEBOUNCE),
        "the window's end derives again"
    );
}
