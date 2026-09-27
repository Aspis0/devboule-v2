use super::{
    open_harness, plan_line, question_line, question_tool_result, replayed_tool_rows,
    set_runtime_mode, PermissionOutcome, PermissionRequestKind, ReaderDispatch, SessionEvent,
};
use crate::session::ConnHandle;
use std::sync::Arc;

#[test]
fn plan_and_question_rows_answer_and_replay_in_one_claude_session() {
    let (broker, captured, runtime, journal, path, mut reader) =
        open_harness("claude-plan-question-replay");
    let conn = ConnHandle::new(1);
    let attached = runtime
        .try_attach_with_replay(None, &conn, true)
        .expect("attach live transcript");
    conn.track_with_agent_replay(
        "s.claude.transcript",
        Arc::clone(&runtime),
        false,
        None,
        attached.generation,
        attached.live_agent_replay,
    );
    set_runtime_mode(&runtime, "default");
    set_runtime_mode(&runtime, "plan");

    let assistant = serde_json::json!({
        "type": "assistant",
        "message": {
            "id": "msg-plan-question",
            "content": [
                {
                    "type": "tool_use",
                    "id": "toolu_plan",
                    "name": "ExitPlanMode",
                    "input": {"plan": "1. Review changes\n2. Verify both rows"}
                },
                {
                    "type": "tool_use",
                    "id": "toolu_question",
                    "name": "AskUserQuestion",
                    "input": {"questions": [{"question": "Which colour?"}]}
                }
            ]
        }
    });
    reader
        .feed(format!("{assistant}\n").as_bytes(), &runtime)
        .expect("feed both provider tool uses");
    reader
        .feed(
            format!(
                "{}\n",
                plan_line(serde_json::json!({
                    "plan": "1. Review changes\n2. Verify both rows"
                }))
            )
            .as_bytes(),
            &runtime,
        )
        .expect("feed plan request");
    reader
        .feed(
            format!("{}\n", question_line("toolu_question")).as_bytes(),
            &runtime,
        )
        .expect("feed question request");
    let cards = super::super::drain(&conn);
    assert!(cards.iter().any(|event| matches!(event,
        SessionEvent::PermissionRequest { tool_call_id, kind: Some(PermissionRequestKind::Plan), .. }
        if tool_call_id == "toolu_plan"
    )));
    assert!(cards.iter().any(|event| matches!(event,
        SessionEvent::PermissionRequest { tool_call_id, kind: Some(PermissionRequestKind::Question), .. }
        if tool_call_id == "toolu_question"
    )));

    broker
        .respond_with_option(
            "toolu_plan",
            PermissionOutcome::AllowOnce,
            Some("implement".to_string()),
            None,
        )
        .expect("approve plan");
    broker
        .respond_with_option(
            "toolu_question",
            PermissionOutcome::AllowOnce,
            Some("q0o1".to_string()),
            None,
        )
        .expect("answer question");
    let mut live_rows: Vec<_> = cards
        .into_iter()
        .chain(super::super::drain(&conn))
        .filter(|event| {
            matches!(
                event,
                SessionEvent::AgentToolCall { .. } | SessionEvent::AgentToolUpdate { .. }
            )
        })
        .collect();

    reader
        .feed(
            format!(
                "{}\n",
                serde_json::json!({
                    "type": "user",
                    "message": {"content": [{
                        "type": "tool_result",
                        "tool_use_id": "toolu_plan",
                        "content": [{"type": "text", "text": "plan echo"}],
                        "is_error": false
                    }]}
                })
            )
            .as_bytes(),
            &runtime,
        )
        .expect("feed plan result");
    let plan_result_events = super::super::drain(&conn);
    assert!(!plan_result_events.iter().any(|event| matches!(event,
        SessionEvent::AgentToolUpdate { tool_call_id, .. } if tool_call_id == "toolu_plan"
    )));
    reader
        .feed(
            format!("{}\n", question_tool_result("toolu_question")).as_bytes(),
            &runtime,
        )
        .expect("feed question result");
    let question_result_events = super::super::drain(&conn);
    assert!(question_result_events.iter().any(|event| matches!(event,
        SessionEvent::AgentToolUpdate { tool_call_id, text: None, .. }
        if tool_call_id == "toolu_question"
    )));
    live_rows.extend(
        plan_result_events
            .into_iter()
            .chain(question_result_events)
            .filter(|event| {
                matches!(
                    event,
                    SessionEvent::AgentToolCall { .. } | SessionEvent::AgentToolUpdate { .. }
                )
            }),
    );

    for events in [
        &live_rows,
        &replayed_tool_rows(&journal, &path, "s.claude.transcript"),
    ] {
        assert!(events.iter().any(|event| matches!(event,
            SessionEvent::AgentToolUpdate {
                tool_call_id, text: Some(text), kind: Some(kind), ..
            } if tool_call_id == "toolu_plan" && text == "1. Review changes\n2. Verify both rows" && kind == "plan"
        )), "plan body is present");
        assert!(events.iter().any(|event| matches!(event,
            SessionEvent::AgentToolUpdate {
                tool_call_id, status: Some(status), title: Some(title), kind: Some(kind), ..
            } if tool_call_id == "toolu_plan" && status == "completed" && title == "Approved" && kind == "plan"
        )), "plan answer is present");
        assert!(events.iter().any(|event| matches!(event,
            SessionEvent::AgentToolUpdate {
                tool_call_id, text: Some(text), kind: Some(kind), ..
            } if tool_call_id == "toolu_question" && text.contains("Question: Which colour should I paint the fence?") && text.contains("Answer: Barn red") && kind == "question"
        )), "question and selected answer are present: {events:#?}");
    }

    let responses = captured.lock().expect("responses");
    assert_eq!(responses.len(), 2);
    assert!(responses
        .iter()
        .any(|frame| frame["response"]["request_id"] == "plan-request"
            && frame["response"]["response"]["behavior"] == "allow"));
    assert!(responses
        .iter()
        .any(|frame| frame["response"]["request_id"] == "ask-1"
            && frame["response"]["response"]["updatedInput"]["answers"]
                ["Which colour should I paint the fence?"]
                == "Barn red"));
    drop(responses);
    journal.shutdown();
    let _ = std::fs::remove_file(path);
}
