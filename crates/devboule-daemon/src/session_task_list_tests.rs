//! One daemon-level proof: a child agent appears as a running task and turns
//! finished — through the request and through the live event, on the real
//! publish path — plus the guards around it: an attachment without the cap
//! sees nothing, and a dead parent cancels what still ran.

use devboule_protocol::{AgentTaskState, OwnerId, SessionEvent, SessionTaskKind, SessionTaskState};

use super::super::session_idle_close_tests::{linked_child, linked_creator};
use super::super::session_queue_fixtures::attached_without_queue_capability;
use super::super::tests::{test_owner, tmp_delete_registry};
use super::super::*;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

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

fn background_call(id: &str) -> SessionEvent {
    SessionEvent::AgentToolCall {
        tool_call_id: id.to_string(),
        title: format!("run {id}"),
        status: "pending".to_string(),
        kind: Some("execute".to_string()),
        locations: None,
        subagent_type: None,
        parent_tool_use_id: None,
        spawn_depth: None,
        command: Some(format!("run {id}")),
        exit_code: None,
        background: Some(true),
    }
}

fn notification(task_id: &str, tool_use_id: Option<&str>) -> SessionEvent {
    SessionEvent::AgentTaskNotification {
        task_id: task_id.to_string(),
        tool_use_id: tool_use_id.map(str::to_string),
        status: devboule_protocol::SubagentTaskStatus::Completed,
        summary: None,
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
    runtime.test_reset_tasks_throttle();

    let (tasks, omitted) = registry.session_tasks(parent, &owner).expect("tasks");
    assert_eq!(omitted, 0);
    assert_eq!(tasks.len(), 1, "the child is the parent's one task");
    assert_eq!(tasks[0].kind, SessionTaskKind::Agent);
    assert_eq!(tasks[0].state, SessionTaskState::Running);
    assert_eq!(tasks[0].child_session_id.as_deref(), Some("s.tasks.child"));

    let events = snapshots_of(&registry, parent, &owner, 41);
    let lists = task_lists(&events);
    assert_eq!(lists.len(), 1, "the attach hands one snapshot");
    assert_eq!(lists[0], &tasks, "the same list the request answers");
    let (epoch, revision) = events
        .iter()
        .find_map(|event| match event {
            SessionEvent::TasksSnapshot {
                epoch, revision, ..
            } => Some((epoch.clone(), *revision)),
            _ => None,
        })
        .unwrap_or_else(|| panic!("no tasks_snapshot in {events:?}"));
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

    let (tasks, _) = registry.session_tasks(parent, &owner).expect("tasks");
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
        SessionEvent::TasksSnapshot {
            epoch: next_epoch,
            revision: next,
            ..
        } => Some((next_epoch.clone(), *next)),
        _ => None,
    });
    assert!(
        later.is_some_and(|(next_epoch, next)| next_epoch == epoch && next > revision),
        "the later snapshot carries the same epoch and a newer revision"
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
    registry.refresh_session_tasks(parent, &[]);
    let mut later = Vec::new();
    loop {
        let batch = conn.pull_events();
        if batch.is_empty() {
            break;
        }
        for event in &batch {
            conn.event_sent(event);
        }
        later.extend(batch.into_iter().map(|pending| pending.envelope.event));
    }
    assert!(
        task_lists(&events).is_empty() && task_lists(&later).is_empty(),
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

    let (tasks, _) = registry.session_tasks(parent, &owner).expect("tasks");
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0].state, SessionTaskState::Cancelled);
    assert!(
        tasks[0].ended_at_ms.is_some(),
        "the parent's end times the cancellation"
    );
}

#[test]
fn an_exit_publishes_immediately_past_the_debounce() {
    let (_dir, registry, _journal) = tmp_delete_registry();
    let owner = test_owner("tasks-urgent-user", "tasks-urgent-client");
    let parent = "s.tasks.urgent.parent";
    linked_creator(&registry, parent, &owner);
    linked_child(&registry, "s.tasks.urgent.child", &owner, parent);
    let runtime = registry
        .live_runtime(parent, &owner)
        .expect("parent runtime");
    registry.configure_runtime_attention(&runtime, &owner);

    // The observer attaches before the death: the exit's publish must reach
    // the already-attached channel without waiting out the window below.
    let conn = attached_without_queue_capability(&registry, parent, 44, &owner);
    let drain_conn = |conn: &Arc<ConnHandle>| {
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
    };
    let _ = drain_conn(&conn);

    // One derive to close the window: the death that follows must still
    // publish without waiting out the 500 ms. The exit refresh runs on its
    // own thread, so the test polls instead of sleeping.
    registry.refresh_session_tasks(parent, &[]);
    runtime.mark_exited(Some(1));

    super::super::session_queue_fixtures::eventually("exit snapshot", || {
        task_lists(&drain_conn(&conn))
            .into_iter()
            .flat_map(|tasks| tasks.iter())
            .any(|task| task.state == SessionTaskState::Cancelled)
    });
}

#[test]
fn a_finished_launch_leaves_no_armed_id_behind() {
    let (_dir, registry, _journal) = tmp_delete_registry();
    let owner = test_owner("tasks-arm-user", "tasks-arm-client");
    let parent = "s.tasks.arm.parent";
    linked_creator(&registry, parent, &owner);
    let runtime = registry
        .live_runtime(parent, &owner)
        .expect("parent runtime");

    runtime.publish_agent_event(background_call("toolu_1"), None);
    assert_eq!(
        runtime.test_armed_background_calls(),
        vec!["toolu_1".to_string()],
        "the background call arms its id"
    );
    runtime.publish_agent_event(notification("task-9", Some("toolu_1")), None);
    assert!(
        runtime.test_armed_background_calls().is_empty(),
        "the notification that ends the row disarms it"
    );
}

#[test]
fn the_armed_set_is_bounded_oldest_first() {
    let (_dir, registry, _journal) = tmp_delete_registry();
    let owner = test_owner("tasks-armcap-user", "tasks-armcap-client");
    let parent = "s.tasks.armcap.parent";
    linked_creator(&registry, parent, &owner);
    let runtime = registry
        .live_runtime(parent, &owner)
        .expect("parent runtime");

    for index in 0..(crate::session_tasks::TASKS_MAX_ROWS + 5) {
        runtime.publish_agent_event(background_call(&format!("toolu_{index:03}")), None);
    }
    let armed = runtime.test_armed_background_calls();
    assert_eq!(armed.len(), crate::session_tasks::TASKS_MAX_ROWS);
    assert!(
        !armed.contains(&"toolu_000".to_string()),
        "the oldest launch leaves first"
    );
    assert!(
        armed.contains(&format!(
            "toolu_{:03}",
            crate::session_tasks::TASKS_MAX_ROWS + 4
        )),
        "the newest launch stays"
    );
}

#[test]
fn a_deferred_trigger_is_stashed_for_the_trailing_run() {
    let (_dir, registry, _journal) = tmp_delete_registry();
    let owner = test_owner("tasks-stash-user", "tasks-stash-client");
    let parent = "s.tasks.stash.parent";
    linked_creator(&registry, parent, &owner);
    let runtime = registry
        .live_runtime(parent, &owner)
        .expect("parent runtime");
    registry.configure_runtime_attention(&runtime, &owner);

    registry.refresh_session_tasks(parent, &[]);
    runtime.publish_agent_event(background_call("toolu_1"), None);
    assert!(
        runtime
            .test_pending_tasks_extra()
            .iter()
            .any(|(event, _)| matches!(
                event,
                SessionEvent::AgentToolCall { tool_call_id, .. } if tool_call_id == "toolu_1"
            )),
        "a trigger inside the window is stashed, not dropped"
    );
}

#[test]
fn the_debounce_opens_after_its_window() {
    use super::{refresh_due, TASKS_REFRESH_DEBOUNCE};
    use std::time::{Duration, Instant};
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

#[test]
fn the_pull_holds_exit_for_the_exit_publish_then_releases_it() {
    // The forced order behind the CI failure, without threads and without
    // sleeps: the death is older than EXIT_DRAIN and the exit publish has
    // not run, so the pull must hold Exit back; once the urgent refresh
    // runs, the cancelled list arrives first and Exit follows it. The
    // production hook is replaced by a recording stub: insert already
    // installed the real one, whose exit thread could publish ahead of the
    // forcing (the macOS CI failure) — the stub proves the death still
    // reached the hook road while publishing nothing itself.
    let (_dir, registry, _journal) = tmp_delete_registry();
    let owner = test_owner("tasks-race-user", "tasks-race-client");
    let parent = "s.tasks.race.parent";
    linked_creator(&registry, parent, &owner);
    linked_child(&registry, "s.tasks.race.child", &owner, parent);
    let runtime = registry
        .live_runtime(parent, &owner)
        .expect("parent runtime");

    let conn = attached_without_queue_capability(&registry, parent, 45, &owner);
    let drain = || {
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
    };
    assert_eq!(task_lists(&drain()).len(), 1, "the attach snapshot arrives");

    let fired = Arc::new(AtomicBool::new(false));
    let seen = Arc::clone(&fired);
    runtime.set_tasks_refresh_hook(Arc::new(move |event| {
        assert!(event.is_none(), "only the death road fires here");
        seen.store(true, Ordering::SeqCst);
    }));
    runtime.mark_exited(Some(1));
    assert!(
        fired.load(Ordering::SeqCst),
        "the death reached the hook road"
    );
    if let Ok(mut stream) = runtime.lock_stream() {
        stream.last_publish =
            Some(std::time::Instant::now() - super::super::session_items::EXIT_DRAIN);
    }
    assert!(
        drain().is_empty(),
        "Exit is held for the exit publish that has not run"
    );

    registry.refresh_session_tasks_urgent(parent);
    let events = drain();
    let cancelled = events
        .iter()
        .position(|event| matches!(event, SessionEvent::TasksSnapshot { .. }));
    let exit = events
        .iter()
        .position(|event| matches!(event, SessionEvent::Exit { .. }));
    assert!(
        cancelled.is_some_and(|first| exit.is_some_and(|last| first < last)),
        "the cancelled list arrives first and Exit follows it: {events:?}"
    );
}

#[test]
fn the_pull_reports_a_death_its_publish_never_reached() {
    // The backstop: no urgent refresh runs at all, and the recorded death
    // ages past the 2 s fallback — Exit is synthesized anyway, so a stuck
    // publish thread can never hold a death forever.
    let (_dir, registry, _journal) = tmp_delete_registry();
    let owner = test_owner("tasks-fallback-user", "tasks-fallback-client");
    let parent = "s.tasks.fallback.parent";
    linked_creator(&registry, parent, &owner);
    linked_child(&registry, "s.tasks.fallback.child", &owner, parent);
    let runtime = registry
        .live_runtime(parent, &owner)
        .expect("parent runtime");

    let conn = attached_without_queue_capability(&registry, parent, 46, &owner);
    let drain = || {
        let mut events = Vec::new();
        loop {
            let batch = conn.pull_events();
            if batch.is_empty() {
                return events;
            }
            events.extend(batch.into_iter().map(|pending| pending.envelope.event));
        }
    };
    let _ = drain();

    runtime.set_tasks_refresh_hook(Arc::new(|_| {}));
    runtime.mark_exited(Some(1));
    // The stub replaced the production hook before the death, so no exit
    // thread can ever run: the only road left is the 2 s fallback, and no
    // snapshot may arrive on it. Replacing re-arms the wait, which is
    // exactly the pending state under test.
    if let Ok(mut stream) = runtime.lock_stream() {
        let past = std::time::Instant::now() - std::time::Duration::from_secs(3);
        stream.last_publish = Some(past);
        stream.exit_at = Some(past);
    }
    let events = drain();
    assert!(
        task_lists(&events).is_empty(),
        "no publish ran on the stubbed road"
    );
    assert!(
        events
            .into_iter()
            .any(|event| matches!(event, SessionEvent::Exit { .. })),
        "the 2 s fallback reports the death without any publish"
    );
}

#[test]
fn installing_the_tasks_hook_arms_the_exit_wait() {
    // A runtime straight out of the constructor carries no hook: nothing
    // will ever publish exit tasks for it, so the pull must not wait.
    // (Inserted and spawned sessions all get the hook at dress time; only
    // raw runtimes take this default.)
    let journal_dir = crate::test_dirs::test_temp_dir("devboule-tasks-hook-bit");
    let journal = std::sync::Arc::new(
        crate::journal::Journal::open(&journal_dir.join("journal.db")).expect("journal"),
    );
    let runtime = std::sync::Arc::new(SessionRuntime::with_journal(
        "s.hook.bit".to_string(),
        Some(journal),
    ));
    let bit = || {
        runtime
            .lock_stream()
            .map(|stream| stream.tasks_exit_published)
            .unwrap_or(false)
    };
    assert!(bit(), "no hook installed: nothing to wait for");
    let owner = test_owner("tasks-hook-user", "tasks-hook-client");
    let registry = SessionRegistry::new(
        crate::paths::RuntimePaths::from_dir(&journal_dir),
        None,
        "hook-bit-epoch".to_string(),
    );
    registry.configure_runtime_attention(&runtime, &owner);
    assert!(!bit(), "hook installed: the exit publish is awaited");
    let _ = std::fs::remove_dir_all(&journal_dir);
}

#[test]
fn a_hooked_death_is_reported_without_the_fallback_wait() {
    // The sessions the insert and spawn roads build all carry the hook, so
    // their deaths take the publish road, not the fallback: Exit follows
    // EXIT_DRAIN even though the 2 s clock has barely started. (The exit
    // thread publishes and marks alongside; either road reports it.)
    let (_dir, registry, _journal) = tmp_delete_registry();
    let owner = test_owner("tasks-nohook-user", "tasks-nohook-client");
    let parent = "s.tasks.nohook.parent";
    linked_creator(&registry, parent, &owner);
    linked_child(&registry, "s.tasks.nohook.child", &owner, parent);
    let runtime = registry
        .live_runtime(parent, &owner)
        .expect("parent runtime");
    registry.configure_runtime_attention(&runtime, &owner);

    let conn = attached_without_queue_capability(&registry, parent, 47, &owner);
    let drain = || {
        let mut events = Vec::new();
        loop {
            let batch = conn.pull_events();
            if batch.is_empty() {
                return events;
            }
            events.extend(batch.into_iter().map(|pending| pending.envelope.event));
        }
    };
    let _ = drain();

    runtime.mark_exited(Some(1));
    if let Ok(mut stream) = runtime.lock_stream() {
        let recent = std::time::Instant::now() - super::super::session_items::EXIT_DRAIN;
        stream.last_publish = Some(recent);
    }
    // The exit thread publishes and marks alongside; poll for the report
    // rather than assuming the thread already ran. Well under the 2 s
    // fallback: with the mark deleted this takes the full backstop.
    let started = std::time::Instant::now();
    super::super::session_queue_fixtures::eventually("hooked death reported", || {
        drain()
            .into_iter()
            .any(|event| matches!(event, SessionEvent::Exit { .. }))
    });
    assert!(
        started.elapsed() < std::time::Duration::from_secs(1),
        "the publish road reports in {:?}, not at the fallback",
        started.elapsed()
    );
}

#[test]
fn an_exit_publish_survives_its_own_entry_removal() {
    // The reader-EOF ordering: the registry entry is gone before the exit
    // thread looks it up, but the children are separate entries and still
    // present — the cancelled list must still reach the attached observer.
    let (_dir, registry, _journal) = tmp_delete_registry();
    let owner = test_owner("tasks-gone-user", "tasks-gone-client");
    let parent = "s.tasks.gone.parent";
    linked_creator(&registry, parent, &owner);
    linked_child(&registry, "s.tasks.gone.child", &owner, parent);
    let runtime = registry
        .live_runtime(parent, &owner)
        .expect("parent runtime");
    registry.configure_runtime_attention(&runtime, &owner);

    let conn = attached_without_queue_capability(&registry, parent, 47, &owner);
    let drain = || {
        let mut events = Vec::new();
        loop {
            let batch = conn.pull_events();
            if batch.is_empty() {
                return events;
            }
            events.extend(batch.into_iter().map(|pending| pending.envelope.event));
        }
    };
    assert_eq!(task_lists(&drain()).len(), 1);
    runtime.mark_exited(Some(1));
    registry.inner.lock().expect("registry").remove(parent);
    // The thread road carries the runtime instead of looking it up: the
    // entry is gone, the Arc is not.
    registry.publish_session_exit_tasks(&runtime, parent);

    assert!(
        task_lists(&drain())
            .into_iter()
            .flat_map(|tasks| tasks.iter())
            .any(|task| task.state == SessionTaskState::Cancelled),
        "entry or no entry, the exit publish reaches its observer"
    );
}

#[test]
fn an_exit_publish_survives_closed_output() {
    // The reader hits EOF and closes output before the exit thread derives:
    // a transient snapshot is not output bytes, so the closed flag must not
    // swallow the cancellations.
    let (_dir, registry, _journal) = tmp_delete_registry();
    let owner = test_owner("tasks-closed-user", "tasks-closed-client");
    let parent = "s.tasks.closed.parent";
    linked_creator(&registry, parent, &owner);
    linked_child(&registry, "s.tasks.closed.child", &owner, parent);
    let runtime = registry
        .live_runtime(parent, &owner)
        .expect("parent runtime");
    registry.configure_runtime_attention(&runtime, &owner);

    let conn = attached_without_queue_capability(&registry, parent, 48, &owner);
    let drain = || {
        let mut events = Vec::new();
        loop {
            let batch = conn.pull_events();
            if batch.is_empty() {
                return events;
            }
            events.extend(batch.into_iter().map(|pending| pending.envelope.event));
        }
    };
    assert_eq!(task_lists(&drain()).len(), 1);
    runtime.mark_exited(Some(1));
    runtime.close_output();
    registry.refresh_session_tasks_urgent(parent);

    assert!(
        task_lists(&drain())
            .into_iter()
            .flat_map(|tasks| tasks.iter())
            .any(|task| task.state == SessionTaskState::Cancelled),
        "closed output must not swallow the exit snapshot"
    );
}
