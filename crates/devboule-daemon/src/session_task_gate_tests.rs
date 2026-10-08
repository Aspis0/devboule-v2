//! The revision gate on the task-list stream, seen from the exit road: an exit
//! publish is not dropped behind a refresh that published first, a refresh
//! that captured the parent before the exit cannot land after it, and a
//! refresh that captured after the exit goes through the usual gate.

use devboule_protocol::{OwnerId, SessionEvent, SessionTask, SessionTaskState};

use super::super::session_idle_close_tests::{linked_child, linked_creator};
use super::super::session_queue_fixtures::attached_without_queue_capability;
use super::super::session_runtime::TasksPublish;
use super::super::tests::{test_owner, tmp_delete_registry};
use super::super::*;
use std::sync::Arc;

/// The task-list snapshots the observer has pulled so far, as (revision, list).
fn drained_snapshots(conn: &ConnHandle) -> Vec<(u64, Vec<SessionTask>)> {
    let mut snapshots = Vec::new();
    loop {
        let batch = conn.pull_events();
        if batch.is_empty() {
            return snapshots;
        }
        for pending in &batch {
            conn.event_sent(pending);
        }
        for pending in batch {
            if let SessionEvent::TasksSnapshot {
                revision, tasks, ..
            } = pending.envelope.event
            {
                snapshots.push((revision, tasks));
            }
        }
    }
}

/// A live parent with one running child, observed by a connection that has
/// already been handed its attach snapshot.
fn observed_parent(
    registry: &SessionRegistry,
    parent: &str,
    conn_id: u64,
    owner: &OwnerId,
) -> (Arc<SessionRuntime>, Arc<ConnHandle>) {
    linked_creator(registry, parent, owner);
    linked_child(registry, &format!("{parent}.child"), owner, parent);
    let runtime = registry
        .live_runtime(parent, owner)
        .expect("parent runtime");
    registry.configure_runtime_attention(&runtime, owner);
    let conn = attached_without_queue_capability(registry, parent, conn_id, owner);
    assert_eq!(
        drained_snapshots(&conn).len(),
        1,
        "the attach hands one snapshot"
    );
    (runtime, conn)
}

fn cancels_the_child(tasks: &[SessionTask]) -> bool {
    tasks
        .iter()
        .any(|task| task.state == SessionTaskState::Cancelled)
}

#[test]
fn an_exit_passes_the_gate_behind_a_refresh_that_already_published() {
    let (_dir, registry, _journal) = tmp_delete_registry();
    let owner = test_owner("tasks-gate-user", "tasks-gate-client");
    let parent = "s.tasks.gate.parent";
    let (runtime, conn) = observed_parent(&registry, parent, 51, &owner);

    // The refresh in flight at the death took its revision before the exit
    // road ran, and its pre-death list (child still running) published first.
    let (pre_death, omitted) = registry.session_tasks(parent, &owner).expect("tasks");
    let in_flight = runtime.next_tasks_revision();
    assert!(
        runtime.publish_tasks_snapshot(
            registry.tasks_epoch.clone(),
            pre_death,
            TasksPublish::Refresh {
                revision: in_flight,
                exit_sent_at_capture: false,
            },
            omitted,
        ),
        "the in-flight refresh publishes first"
    );

    registry.publish_session_exit_tasks(&runtime, parent);

    let snapshots = drained_snapshots(&conn);
    let (exit_revision, exit_tasks) = snapshots.last().expect("the exit snapshot");
    assert!(
        *exit_revision > in_flight,
        "the exit carries a newer revision than the refresh"
    );
    assert!(
        cancels_the_child(exit_tasks),
        "the exit snapshot cancels the child"
    );
    assert!(
        runtime
            .lock_stream()
            .is_ok_and(|stream| stream.tasks_exit_published),
        "the exit wait settles on the publish"
    );
}

#[test]
fn a_refresh_published_after_the_exit_cannot_restore_its_list() {
    let (_dir, registry, _journal) = tmp_delete_registry();
    let owner = test_owner("tasks-stale-user", "tasks-stale-client");
    let parent = "s.tasks.stale.parent";
    let (runtime, conn) = observed_parent(&registry, parent, 52, &owner);

    let in_flight = runtime.next_tasks_revision();
    let (pre_death, omitted) = registry.session_tasks(parent, &owner).expect("tasks");
    registry.publish_session_exit_tasks(&runtime, parent);

    assert!(
        !runtime.publish_tasks_snapshot(
            registry.tasks_epoch.clone(),
            pre_death,
            TasksPublish::Refresh {
                revision: in_flight,
                exit_sent_at_capture: false,
            },
            omitted,
        ),
        "a refresh older than the exit is stale"
    );
    let snapshots = drained_snapshots(&conn);
    assert_eq!(snapshots.len(), 1, "the observer sees only the exit");
    assert!(
        cancels_the_child(&snapshots[0].1),
        "the one list left is the cancelled one"
    );
}

#[test]
fn a_refresh_that_captured_before_the_exit_cannot_land_after_it() {
    let (_dir, registry, _journal) = tmp_delete_registry();
    let owner = test_owner("tasks-capture-user", "tasks-capture-client");
    let parent = "s.tasks.capture.parent";
    let (runtime, conn) = observed_parent(&registry, parent, 54, &owner);

    // The derive stamps the exit, then captures the parent while it is still
    // live ...
    let exit_sent_at_capture = runtime.tasks_exit_sent();
    let (pre_death, omitted) = registry.session_tasks(parent, &owner).expect("tasks");
    registry.publish_session_exit_tasks(&runtime, parent);
    // ... and only then takes its revision, which is newer than the exit's.
    let late = runtime.next_tasks_revision();

    assert!(
        !runtime.publish_tasks_snapshot(
            registry.tasks_epoch.clone(),
            pre_death,
            TasksPublish::Refresh {
                revision: late,
                exit_sent_at_capture,
            },
            omitted,
        ),
        "a derive that captured the live parent must not land after the exit"
    );
    let snapshots = drained_snapshots(&conn);
    assert!(
        snapshots
            .last()
            .is_some_and(|(_, tasks)| cancels_the_child(tasks)),
        "the exit stays the last list the observer is sent"
    );
}

#[test]
fn a_refresh_that_captured_after_the_exit_lands_when_newer() {
    let (_dir, registry, _journal) = tmp_delete_registry();
    let owner = test_owner("tasks-after-user", "tasks-after-client");
    let parent = "s.tasks.after.parent";
    let (runtime, conn) = observed_parent(&registry, parent, 55, &owner);

    registry.publish_session_exit_tasks(&runtime, parent);
    let exit_sent_at_capture = runtime.tasks_exit_sent();
    let (captured, omitted) = registry.session_tasks(parent, &owner).expect("tasks");
    let late = runtime.next_tasks_revision();

    assert!(exit_sent_at_capture, "the stamp sees the sent exit");
    assert!(
        runtime.publish_tasks_snapshot(
            registry.tasks_epoch.clone(),
            captured,
            TasksPublish::Refresh {
                revision: late,
                exit_sent_at_capture,
            },
            omitted,
        ),
        "a derive that captured after the exit goes through the stale gate and lands"
    );
    let snapshots = drained_snapshots(&conn);
    assert_eq!(
        snapshots.last().map(|(revision, _)| *revision),
        Some(late),
        "the newer derive is the last list the observer is sent"
    );
}

#[test]
fn a_dropped_refresh_hands_its_triggering_rows_to_the_next_derive() {
    let (_dir, registry, _journal) = tmp_delete_registry();
    let owner = test_owner("tasks-rows-user", "tasks-rows-client");
    let parent = "s.tasks.rows.parent";
    let (runtime, _conn) = observed_parent(&registry, parent, 56, &owner);

    let exit_sent_at_capture = runtime.tasks_exit_sent();
    registry.publish_session_exit_tasks(&runtime, parent);
    let marker = SessionEvent::TasksSnapshot {
        epoch: "restash-marker".to_string(),
        revision: 0,
        tasks: Vec::new(),
        omitted: 0,
    };
    registry.publish_session_tasks(
        &runtime,
        parent,
        &[],
        false,
        exit_sent_at_capture,
        &[(marker, 1)],
    );

    assert!(
        runtime
            .test_pending_tasks_extra()
            .iter()
            .any(|(event, _)| matches!(
                event,
                SessionEvent::TasksSnapshot { epoch, .. } if epoch == "restash-marker"
            )),
        "the dropped refresh's rows wait for the next derive"
    );
}
