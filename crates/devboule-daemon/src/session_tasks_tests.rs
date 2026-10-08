//! Tests for one topic: the task list derivation — what the daemon already
//! holds, turned into rows — and the state transitions that move those rows.

use devboule_protocol::{
    AgentBackgroundTask, AgentTaskState, SessionEvent, SessionState, SessionTaskKind,
    SessionTaskState, SubagentTaskStatus,
};
use serde_json::json;

use super::{derive_tasks, truncate_title, HeldChild, TASK_TITLE_MAX_CHARS};

fn held(id: &str, state: SessionState) -> HeldChild {
    HeldChild {
        id: id.to_string(),
        title: format!("{id} title"),
        state,
        started_at_ms: 10,
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
        integrity: devboule_protocol::TranscriptIntegrity::Complete,
    }
}

fn tasks_of(
    held: &[HeldChild],
    journal: &[(SessionEvent, Option<u64>)],
) -> Vec<devboule_protocol::SessionTask> {
    derive_tasks("s.parent", held, journal)
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
                        integrity: devboule_protocol::TranscriptIntegrity::Unverifiable {
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
    fn agents_come_first_ordered_by_start_then_id() {
        let call = SessionEvent::AgentToolCall {
            tool_call_id: "toolu_1".to_string(),
            title: "sleep 60".to_string(),
            status: "pending".to_string(),
            kind: Some("execute".to_string()),
            locations: None,
            subagent_type: None,
            parent_tool_use_id: None,
            spawn_depth: None,
            command: Some("sleep 60".to_string()),
            exit_code: None,
            background: Some(true),
        };
        let mut second = held("s.b", live(1));
        second.started_at_ms = 5;
        let mut first = held("s.a", live(1));
        first.started_at_ms = 5;
        let tasks = tasks_of(&[second, first], &[(call, Some(1))]);
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
    fn provider_task_entries_read_as_agent_rows() {
        let started = SessionEvent::AgentTaskStarted {
            task_id: "task-1".to_string(),
            title: Some("Find files".to_string()),
            subagent_type: Some("explorer".to_string()),
            tool_use_id: None,
            is_backgrounded: Some(false),
            spawn_depth: Some(1),
        };
        let tasks = tasks_of(&[], &[(started, Some(3))]);
        assert_eq!(tasks.len(), 1);
        let task = &tasks[0];
        assert_eq!(task.kind, SessionTaskKind::Agent);
        assert_eq!(task.title, "Find files");
        assert_eq!(task.state, SessionTaskState::Running);
        assert_eq!(task.child_session_id, None);
        assert_eq!(task.model, None);
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
        let tasks = tasks_of(&[], &[(changed, Some(3))]);
        assert_eq!(tasks.len(), 2);
        assert_eq!(tasks[0].kind, SessionTaskKind::Agent);
        assert_eq!(tasks[1].kind, SessionTaskKind::Command);
    }
}

mod transitions {
    use super::*;

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

    #[test]
    fn a_background_command_runs_then_ends_by_status() {
        for (status, end) in [
            ("completed", SessionTaskState::Finished),
            ("failed", SessionTaskState::Failed),
            ("cancelled", SessionTaskState::Cancelled),
            ("interrupted", SessionTaskState::Cancelled),
        ] {
            let tasks = tasks_of(
                &[],
                &[
                    (background_call("toolu_1"), Some(1)),
                    (update("toolu_1", status), Some(2)),
                ],
            );
            assert_eq!(tasks.len(), 1, "{status} must keep its row");
            assert_eq!(tasks[0].state, end, "{status}");
            assert_eq!(tasks[0].ended_at_ms, Some(2), "{status}");
        }
    }

    #[test]
    fn a_foreground_call_is_not_a_task() {
        let mut call = background_call("toolu_1");
        let SessionEvent::AgentToolCall { background, .. } = &mut call else {
            panic!("a tool call");
        };
        *background = None;
        let tasks = tasks_of(&[], &[(call, Some(1))]);
        assert!(tasks.is_empty());
    }

    #[test]
    fn an_update_for_an_unknown_call_changes_nothing() {
        let tasks = tasks_of(&[], &[(update("toolu_9", "completed"), Some(2))]);
        assert!(tasks.is_empty());
    }

    #[test]
    fn a_child_finish_overrules_the_exit_code_read() {
        // The human stopped it: cancelled, not failed, and the row keeps the
        // registry's start, not the finish row's time.
        let finished = SessionEvent::ChildFinished {
            message_id: None,
            child_session_id: "s.child".to_string(),
            display_name: "child".to_string(),
            state: AgentTaskState::Canceled,
            note: None,
            artifacts: Vec::new(),
        };
        let tasks = tasks_of(&[held("s.child", ended(Some(3)))], &[(finished, Some(99))]);
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].state, SessionTaskState::Cancelled);
        assert_eq!(tasks[0].started_at_ms, 10);
        assert_eq!(tasks[0].ended_at_ms, Some(99));
    }

    #[test]
    fn a_finish_row_names_a_closed_child_back_into_the_list() {
        let finished = SessionEvent::ChildFinished {
            message_id: None,
            child_session_id: "s.closed".to_string(),
            display_name: "archived work".to_string(),
            state: AgentTaskState::Completed,
            note: None,
            artifacts: Vec::new(),
        };
        let tasks = tasks_of(&[], &[(finished, Some(50))]);
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].title, "archived work");
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
        let tasks = tasks_of(&[], &[(started, Some(1)), (notified, Some(9))]);
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
        let finished = SessionEvent::ChildFinished {
            message_id: None,
            child_session_id: "s.child".to_string(),
            display_name: "child".to_string(),
            state: AgentTaskState::Completed,
            note: None,
            artifacts: Vec::new(),
        };
        let once = tasks_of(&[held("s.child", live(1))], &[(finished.clone(), Some(7))]);
        let twice = tasks_of(
            &[held("s.child", live(1))],
            &[(finished.clone(), Some(7)), (finished, Some(7))],
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
        let tasks = tasks_of(&[], &[(first, Some(1)), (second, Some(2))]);
        assert!(tasks.is_empty());
    }

    #[test]
    fn a_started_entry_survives_a_set_that_never_named_it() {
        // Foreground Task entries are not set members: the replacement rule
        // only drops what the set itself vouched for.
        let started = SessionEvent::AgentTaskStarted {
            task_id: "task-1".to_string(),
            title: Some("front work".to_string()),
            subagent_type: None,
            tool_use_id: None,
            is_backgrounded: Some(false),
            spawn_depth: None,
        };
        let changed = SessionEvent::AgentBackgroundTasksChanged { tasks: vec![] };
        let tasks = tasks_of(&[], &[(started, Some(1)), (changed, Some(2))]);
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].state, SessionTaskState::Running);
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
        let tasks = tasks_of(&[], &[(started, Some(1))]);
        assert_eq!(tasks[0].title, "Find the relevant files");
    }
}
