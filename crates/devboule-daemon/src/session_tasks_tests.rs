//! Tests for one topic: the task list derivation — what the daemon already
//! holds, turned into rows — and the state transitions that move those rows.

use devboule_protocol::{
    AgentBackgroundTask, AgentTaskState, SessionEvent, SessionState, SessionTaskKind,
    SessionTaskState, SubagentTaskStatus, TranscriptIntegrity,
};
use serde_json::json;

use super::{
    background_launch_failed, cap_published, command_title, derive_tasks, summarize_child,
    truncate_title, HeldChild, TaskRow, TASKS_MAX_ROWS, TASK_TITLE_MAX_CHARS,
};

fn held(id: &str, state: SessionState) -> HeldChild {
    HeldChild {
        id: id.to_string(),
        title: format!("{id} title"),
        state,
        started_at_ms: 10,
        ended_at_ms: None,
        model: Some("model-x".to_string()),
        tool_call_count: Some(4),
    }
}

fn live(generation: u64) -> SessionState {
    SessionState::Live { generation }
}

fn ended(code: Option<u32>) -> SessionState {
    SessionState::Ended {
        generation: 1,
        code,
        integrity: TranscriptIntegrity::Complete,
    }
}

fn row(event: SessionEvent, ts: u64) -> TaskRow {
    TaskRow {
        event,
        ts_ms: Some(ts),
        pos: None,
    }
}

fn tasks_of(held: &[HeldChild], journal: &[TaskRow]) -> Vec<devboule_protocol::SessionTask> {
    derive_tasks("s.parent", held, journal, None)
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

fn update(id: &str, status: &str) -> SessionEvent {
    SessionEvent::AgentToolUpdate {
        tool_call_id: id.to_string(),
        status: Some(status.to_string()),
        text: None,
        title: None,
        kind: None,
        locations: None,
        parent_tool_use_id: None,
        spawn_depth: None,
        command: None,
        exit_code: None,
        replace: false,
        images: Vec::new(),
    }
}

fn child_finished(id: &str, state: AgentTaskState) -> SessionEvent {
    SessionEvent::ChildFinished {
        message_id: None,
        child_session_id: id.to_string(),
        display_name: "child".to_string(),
        state,
        note: None,
        artifacts: Vec::new(),
    }
}

mod derive {
    use super::*;

    #[test]
    fn registry_states_map_to_task_states() {
        let tasks = tasks_of(
            &[
                held("s.live", live(1)),
                held("s.silent", SessionState::Silent { generation: 1 }),
                held("s.ok", ended(Some(0))),
                held("s.bad", ended(Some(1))),
                held("s.nocode", ended(None)),
                held(
                    "s.gone",
                    SessionState::Recovered {
                        generation: 1,
                        integrity: TranscriptIntegrity::Unverifiable {
                            dropped_frames: 0,
                            dropped_bytes: 0,
                            trimmed_bytes: 0,
                        },
                    },
                ),
            ],
            &[],
        );
        let state = |id: &str| {
            tasks
                .iter()
                .find(|t| t.id == id)
                .unwrap_or_else(|| panic!("no task for {id}"))
                .state
        };
        assert_eq!(state("s.live"), SessionTaskState::Running);
        assert_eq!(state("s.silent"), SessionTaskState::Running);
        assert_eq!(state("s.ok"), SessionTaskState::Finished);
        assert_eq!(state("s.bad"), SessionTaskState::Failed);
        assert_eq!(state("s.nocode"), SessionTaskState::Failed);
        assert_eq!(state("s.gone"), SessionTaskState::Cancelled);
    }

    #[test]
    fn agent_rows_carry_parent_link_model_and_count() {
        let tasks = tasks_of(&[held("s.child", live(1))], &[]);
        assert_eq!(tasks.len(), 1);
        let task = &tasks[0];
        assert_eq!(task.kind, SessionTaskKind::Agent);
        assert_eq!(task.session_id, "s.parent");
        assert_eq!(task.child_session_id.as_deref(), Some("s.child"));
        assert_eq!(task.started_at_ms, 10);
        assert_eq!(task.ended_at_ms, None);
        assert_eq!(task.model.as_deref(), Some("model-x"));
        assert_eq!(task.tool_call_count, Some(4));
    }

    #[test]
    fn a_terminal_held_child_keeps_the_registry_end_time() {
        let mut child = held("s.child", ended(Some(0)));
        child.ended_at_ms = Some(77);
        let tasks = tasks_of(&[child], &[]);
        assert_eq!(tasks[0].state, SessionTaskState::Finished);
        assert_eq!(tasks[0].ended_at_ms, Some(77));
    }

    #[test]
    fn agents_come_first_ordered_by_start_then_id() {
        let mut second = held("s.b", live(1));
        second.started_at_ms = 5;
        let mut first = held("s.a", live(1));
        first.started_at_ms = 5;
        let tasks = tasks_of(&[second, first], &[row(background_call("toolu_1"), 1)]);
        let ids: Vec<&str> = tasks.iter().map(|t| t.id.as_str()).collect();
        assert_eq!(ids, vec!["s.a", "s.b", "toolu_1"]);
    }

    #[test]
    fn long_titles_are_cut_at_a_char_boundary() {
        let long = "x".repeat(TASK_TITLE_MAX_CHARS + 40);
        let cut = truncate_title(&long);
        assert_eq!(cut.chars().count(), TASK_TITLE_MAX_CHARS + 1);
        assert!(cut.ends_with('…'));
        assert_eq!(truncate_title("short"), "short");
        let exact = "y".repeat(TASK_TITLE_MAX_CHARS);
        assert_eq!(truncate_title(&exact), exact);
    }

    #[test]
    fn command_titles_mask_credentials_before_the_cut() {
        let title = command_title(
            "curl -H 'Authorization: Bearer abcdef1234567890abcdef123456' https://h.test/x",
        );
        assert!(
            !title.contains("abcdef1234567890abcdef123456"),
            "the token must not survive redaction: {title}"
        );
        assert!(title.contains("curl"), "{title}");
        assert!(title.contains("[redacted]"), "{title}");
    }

    #[test]
    fn a_short_bearer_token_is_masked_by_its_header_context() {
        // Five characters fall under every length check: only the
        // `Header: Bearer` shape names it a credential. The header masks
        // its value word, so the scheme word itself goes with it.
        let title = command_title("curl -H 'Authorization: Bearer SHORT' https://h.test/x");
        assert!(!title.contains("SHORT"), "{title}");
        assert!(title.contains("[redacted]"), "{title}");
        assert!(title.contains("https://h.test/x"), "{title}");
    }

    #[test]
    fn a_bare_scheme_word_without_a_header_is_left_alone() {
        // No `Header:` context, no length shape: the pre-pass must not eat
        // ordinary words. (The argv pass may still mask after `token` — its
        // own one-sided heuristic, pinned in its own file.)
        let masked = crate::process_argv_redact::mask_scheme_credentials("echo bearer bad news");
        assert_eq!(masked, "echo bearer bad news", "{masked}");
    }

    #[test]
    fn backgrounded_provider_tasks_read_as_agent_rows() {
        let started = SessionEvent::AgentTaskStarted {
            task_id: "task-1".to_string(),
            title: Some("Find files".to_string()),
            subagent_type: Some("explorer".to_string()),
            tool_use_id: None,
            is_backgrounded: Some(true),
            spawn_depth: Some(1),
        };
        let tasks = tasks_of(&[], &[row(started, 3)]);
        assert_eq!(tasks.len(), 1);
        let task = &tasks[0];
        assert_eq!(task.kind, SessionTaskKind::Agent);
        assert_eq!(task.title, "Find files");
        assert_eq!(task.state, SessionTaskState::Running);
        assert_eq!(task.child_session_id, None);
        assert_eq!(task.model, None);
    }

    #[test]
    fn an_explicit_foreground_task_is_not_listed() {
        let started = SessionEvent::AgentTaskStarted {
            task_id: "task-1".to_string(),
            title: Some("front work".to_string()),
            subagent_type: None,
            tool_use_id: None,
            is_backgrounded: Some(false),
            spawn_depth: None,
        };
        let tasks = tasks_of(&[], &[row(started, 1)]);
        assert!(tasks.is_empty(), "foreground work is not a background task");
    }

    #[test]
    fn a_legacy_task_without_a_flag_is_still_listed() {
        let started = SessionEvent::AgentTaskStarted {
            task_id: "task-1".to_string(),
            title: Some("old work".to_string()),
            subagent_type: None,
            tool_use_id: None,
            is_backgrounded: None,
            spawn_depth: None,
        };
        let tasks = tasks_of(&[], &[row(started, 1)]);
        assert_eq!(tasks.len(), 1);
    }

    #[test]
    fn background_set_entries_take_their_kind_from_the_type() {
        let changed = SessionEvent::AgentBackgroundTasksChanged {
            tasks: vec![
                AgentBackgroundTask {
                    task_id: "task-1".to_string(),
                    task_type: "local_agent".to_string(),
                    title: "agent work".to_string(),
                },
                AgentBackgroundTask {
                    task_id: "task-2".to_string(),
                    task_type: "shell".to_string(),
                    title: "sleep 60".to_string(),
                },
            ],
        };
        let tasks = tasks_of(&[], &[row(changed, 3)]);
        assert_eq!(tasks.len(), 2);
        assert_eq!(tasks[0].kind, SessionTaskKind::Agent);
        assert_eq!(tasks[1].kind, SessionTaskKind::Command);
    }

    #[test]
    fn rows_merge_by_journal_position_not_arrival() {
        // The trigger sorts after positioned rows even when it arrives first:
        // the fold must answer the journal's order either way.
        let notified = SessionEvent::AgentTaskNotification {
            task_id: "task-1".to_string(),
            tool_use_id: None,
            status: SubagentTaskStatus::Completed,
            summary: None,
        };
        let started = SessionEvent::AgentTaskStarted {
            task_id: "task-1".to_string(),
            title: Some("work".to_string()),
            subagent_type: None,
            tool_use_id: None,
            is_backgrounded: Some(true),
            spawn_depth: None,
        };
        let trigger_first = vec![
            TaskRow {
                event: notified.clone(),
                ts_ms: Some(9),
                pos: None,
            },
            TaskRow {
                event: started.clone(),
                ts_ms: Some(1),
                pos: Some((1, 4)),
            },
            TaskRow {
                event: notified.clone(),
                ts_ms: Some(9),
                pos: Some((1, 9)),
            },
        ];
        let journal_first = vec![
            TaskRow {
                event: started,
                ts_ms: Some(1),
                pos: Some((1, 4)),
            },
            TaskRow {
                event: notified,
                ts_ms: Some(9),
                pos: Some((1, 9)),
            },
        ];
        let left = derive_tasks("s.parent", &[], &trigger_first, None);
        let right = derive_tasks("s.parent", &[], &journal_first, None);
        assert_eq!(left, right);
        assert_eq!(left.len(), 1);
        assert_eq!(left[0].state, SessionTaskState::Finished);
    }

    #[test]
    fn the_publish_cap_keeps_running_newest_first_and_counts_the_rest() {
        let mut rows = vec![row(background_call("toolu_run"), 1)];
        for index in 0..TASKS_MAX_ROWS {
            let id = format!("toolu_old_{index:03}");
            rows.push(row(background_call(&id), 2));
            rows.push(row(update(&id, "failed"), 3));
        }
        let derived = tasks_of(&[], &rows);
        assert_eq!(
            derived.len(),
            TASKS_MAX_ROWS + 1,
            "the derive keeps everything; the cap is a publish concern"
        );
        let (kept, omitted) = cap_published(derived);
        assert_eq!(kept.len(), TASKS_MAX_ROWS);
        assert_eq!(omitted, 1);
        assert!(
            kept.iter().any(|t| t.id == "toolu_run"),
            "running rows sort first and are never the omitted one"
        );
        assert!(
            kept.iter().all(|t| t.id != "toolu_old_000"),
            "the oldest finished row leaves first"
        );
        assert!(
            kept.iter().any(|t| t.id == "toolu_old_199"),
            "the newest finished row stays"
        );
        let (kept, omitted) = cap_published(vec![]);
        assert!(kept.is_empty());
        assert_eq!(omitted, 0);
    }

    #[test]
    fn a_child_summary_counts_calls_only_for_a_complete_replay() {
        let events = vec![
            SessionEvent::SessionManifest {
                provider_id: None,
                current_model_id: Some("m".to_string()),
                models: Vec::new(),
                modes: None,
            },
            background_call("toolu_1"),
        ];
        let (model, complete) = summarize_child(&events, true);
        assert_eq!(model.as_deref(), Some("m"));
        assert_eq!(complete, Some(1));
        let (_, partial) = summarize_child(&events, false);
        assert_eq!(partial, None, "a partial replay is not a count");
    }

    #[test]
    fn only_a_failed_launch_ends_a_background_row() {
        assert!(background_launch_failed(Some("failed")));
        assert!(!background_launch_failed(Some("completed")));
        assert!(!background_launch_failed(None));
        assert!(!background_launch_failed(Some("in_progress")));
    }
}

mod transitions {
    use super::*;

    #[test]
    fn a_launch_acknowledgement_leaves_the_command_running() {
        let tasks = tasks_of(
            &[],
            &[
                row(background_call("toolu_1"), 1),
                row(update("toolu_1", "completed"), 2),
            ],
        );
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].state, SessionTaskState::Running);
        assert_eq!(tasks[0].ended_at_ms, None);
    }

    #[test]
    fn a_failed_launch_ends_the_command_failed() {
        let tasks = tasks_of(
            &[],
            &[
                row(background_call("toolu_1"), 1),
                row(update("toolu_1", "failed"), 2),
            ],
        );
        assert_eq!(tasks[0].state, SessionTaskState::Failed);
        assert_eq!(tasks[0].ended_at_ms, Some(2));
    }

    #[test]
    fn a_notification_for_the_launching_tool_ends_the_command() {
        // Claude's completion names the Bash tool call that launched it.
        let notified = SessionEvent::AgentTaskNotification {
            task_id: "task-77".to_string(),
            tool_use_id: Some("toolu_1".to_string()),
            status: SubagentTaskStatus::Completed,
            summary: Some("done".to_string()),
        };
        let tasks = tasks_of(
            &[],
            &[
                row(background_call("toolu_1"), 1),
                row(update("toolu_1", "completed"), 2),
                row(notified, 9),
            ],
        );
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].state, SessionTaskState::Finished);
        assert_eq!(tasks[0].ended_at_ms, Some(9));
    }

    #[test]
    fn a_notification_for_an_unknown_tool_changes_nothing() {
        let notified = SessionEvent::AgentTaskNotification {
            task_id: "task-77".to_string(),
            tool_use_id: Some("toolu_9".to_string()),
            status: SubagentTaskStatus::Completed,
            summary: None,
        };
        let tasks = tasks_of(&[], &[row(notified, 9)]);
        assert!(tasks.is_empty());
    }

    #[test]
    fn a_foreground_call_is_not_a_task() {
        let mut call = background_call("toolu_1");
        let SessionEvent::AgentToolCall { background, .. } = &mut call else {
            panic!("a tool call");
        };
        *background = None;
        let tasks = tasks_of(&[], &[row(call, 1)]);
        assert!(tasks.is_empty());
    }

    #[test]
    fn an_update_for_an_unknown_call_changes_nothing() {
        let tasks = tasks_of(&[], &[row(update("toolu_9", "completed"), 2)]);
        assert!(tasks.is_empty());
    }

    #[test]
    fn a_child_finish_overrules_the_exit_code_read() {
        // The human stopped it: cancelled, not failed, and the row keeps the
        // registry's start, not the finish row's time.
        let tasks = tasks_of(
            &[held("s.child", ended(Some(3)))],
            &[row(child_finished("s.child", AgentTaskState::Canceled), 99)],
        );
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].state, SessionTaskState::Cancelled);
        assert_eq!(tasks[0].started_at_ms, 10);
        assert_eq!(tasks[0].ended_at_ms, Some(99));
    }

    #[test]
    fn a_finish_row_names_a_closed_child_back_into_the_list() {
        let tasks = tasks_of(
            &[],
            &[row(
                child_finished("s.closed", AgentTaskState::Completed),
                50,
            )],
        );
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].title, "child");
        assert_eq!(tasks[0].state, SessionTaskState::Finished);
        assert_eq!(tasks[0].child_session_id.as_deref(), Some("s.closed"));
    }

    #[test]
    fn a_provider_notification_ends_its_task() {
        let notified = SessionEvent::AgentTaskNotification {
            task_id: "task-1".to_string(),
            tool_use_id: None,
            status: SubagentTaskStatus::Stopped,
            summary: None,
        };
        let started = SessionEvent::AgentTaskStarted {
            task_id: "task-1".to_string(),
            title: None,
            subagent_type: Some("explorer".to_string()),
            tool_use_id: None,
            is_backgrounded: Some(true),
            spawn_depth: None,
        };
        let tasks = tasks_of(&[], &[row(started, 1), row(notified, 9)]);
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].title, "explorer");
        assert_eq!(tasks[0].state, SessionTaskState::Cancelled);
        assert_eq!(tasks[0].ended_at_ms, Some(9));
    }

    #[test]
    fn replaying_the_triggering_row_answers_the_same_list() {
        // The refresh hook folds the just-published row explicitly, because
        // the journal write may not be visible yet; when the row is there too,
        // the keyed fold must not double it.
        let finished = child_finished("s.child", AgentTaskState::Completed);
        let once = tasks_of(&[held("s.child", live(1))], &[row(finished.clone(), 7)]);
        let twice = tasks_of(
            &[held("s.child", live(1))],
            &[row(finished.clone(), 7), row(finished, 7)],
        );
        assert_eq!(once, twice);
        assert_eq!(twice.len(), 1);
        assert_eq!(twice[0].state, SessionTaskState::Finished);
    }

    #[test]
    fn a_vouched_entry_missing_from_the_next_set_leaves() {
        let first = SessionEvent::AgentBackgroundTasksChanged {
            tasks: vec![AgentBackgroundTask {
                task_id: "task-1".to_string(),
                task_type: "local_agent".to_string(),
                title: "work".to_string(),
            }],
        };
        let second = SessionEvent::AgentBackgroundTasksChanged { tasks: vec![] };
        let tasks = tasks_of(&[], &[row(first, 1), row(second, 2)]);
        assert!(tasks.is_empty());
    }

    #[test]
    fn a_started_entry_survives_a_set_that_never_named_it() {
        // A lifecycle of its own outlives the set: the replacement rule only
        // drops what the set itself vouched for.
        let started = SessionEvent::AgentTaskStarted {
            task_id: "task-1".to_string(),
            title: Some("front work".to_string()),
            subagent_type: None,
            tool_use_id: None,
            is_backgrounded: Some(true),
            spawn_depth: None,
        };
        let changed = SessionEvent::AgentBackgroundTasksChanged { tasks: vec![] };
        let tasks = tasks_of(&[], &[row(started, 1), row(changed, 2)]);
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].state, SessionTaskState::Running);
    }

    #[test]
    fn a_parent_end_cancels_what_still_runs() {
        let started = SessionEvent::AgentTaskStarted {
            task_id: "task-1".to_string(),
            title: Some("work".to_string()),
            subagent_type: None,
            tool_use_id: None,
            is_backgrounded: Some(true),
            spawn_depth: None,
        };
        let tasks = derive_tasks(
            "s.parent",
            &[held("s.child", live(1))],
            &[row(background_call("toolu_1"), 1), row(started, 2)],
            Some(60),
        );
        assert_eq!(tasks.len(), 3);
        for task in &tasks {
            assert_eq!(task.state, SessionTaskState::Cancelled);
            assert_eq!(task.ended_at_ms, Some(60));
        }
    }

    #[test]
    fn a_replay_tail_marker_does_not_cancel_a_live_parent() {
        // Every journal replay ends with a synthetic Exit/Recovered tail
        // row: it is the replay talking, not the parent, so the fold must
        // ignore it and only the explicit parent end cancels.
        let exit = SessionEvent::Exit { code: Some(1) };
        let recovered = SessionEvent::Recovered {
            integrity: TranscriptIntegrity::Unverifiable {
                dropped_frames: 0,
                dropped_bytes: 0,
                trimmed_bytes: 0,
            },
        };
        let tasks = derive_tasks(
            "s.parent",
            &[held("s.child", live(1))],
            &[
                row(background_call("toolu_1"), 1),
                row(exit, 55),
                row(recovered, 56),
            ],
            None,
        );
        assert!(tasks.iter().all(|t| t.state == SessionTaskState::Running));
    }

    #[test]
    fn a_late_update_does_not_revive_a_cancelled_row() {
        let tasks = derive_tasks(
            "s.parent",
            &[],
            &[
                row(background_call("toolu_1"), 1),
                row(update("toolu_1", "completed"), 9),
            ],
            Some(5),
        );
        // The ack arrived after the parent's end: the cancellation stands.
        assert_eq!(tasks[0].state, SessionTaskState::Cancelled);
        assert_eq!(tasks[0].ended_at_ms, Some(5));
    }

    #[test]
    fn the_claude_task_frames_match_the_measured_envelopes() {
        // Spot-check the fold against the wire shape `claude_view_tasks`
        // ingests, so a drift in either place breaks one of them.
        let envelope = json!({
            "type": "system",
            "subtype": "task_started",
            "task_id": "task-1",
            "description": "Find the relevant files",
        });
        let started = SessionEvent::AgentTaskStarted {
            task_id: envelope["task_id"].as_str().unwrap().to_string(),
            title: envelope["description"].as_str().map(str::to_string),
            subagent_type: None,
            tool_use_id: None,
            is_backgrounded: None,
            spawn_depth: None,
        };
        let tasks = tasks_of(&[], &[row(started, 1)]);
        assert_eq!(tasks[0].title, "Find the relevant files");
    }
}
