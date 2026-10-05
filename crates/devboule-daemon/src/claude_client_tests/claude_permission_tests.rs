use super::ReaderDispatch;
use super::{attached, test_reader};
use super::{ClaudePendingControl, PermissionBroker, PermissionSender};
use devboule_protocol::{PermissionOutcome, PermissionRequestKind, SessionEvent};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

#[path = "plan_question_tests.rs"]
mod plan_question_tests;

mod plan_tests {
    use super::super::super::{
        claude_permission_sender_with_writer, control_response_frame, plan_target_mode,
    };
    use super::super::drain;
    use super::{
        attached, plan_line, set_runtime_mode, test_reader, PermissionBroker, PermissionOutcome,
        ReaderDispatch, SessionEvent,
    };
    use std::collections::HashMap;
    use std::sync::atomic::AtomicU64;
    use std::sync::{Arc, Mutex};

    #[test]
    fn plan_actions_select_one_mode_and_map_rejection_to_deny() {
        assert_eq!(
            plan_target_mode(true, "implement", None),
            Some("acceptEdits")
        );
        assert_eq!(
            plan_target_mode(true, "implement_bypass", Some("bypassPermissions")),
            Some("bypassPermissions")
        );
        assert_eq!(
            plan_target_mode(true, "implement_bypass", Some("acceptEdits")),
            Some("acceptEdits")
        );
        assert_eq!(
            plan_target_mode(true, "deny", Some("bypassPermissions")),
            None
        );
        assert_eq!(plan_target_mode(false, "implement", None), None);
        let ordinary_allow = control_response_frame(
            "ordinary-request",
            &serde_json::json!({"command": "true"}),
            false,
            false,
            None,
            &serde_json::json!({"outcome": {"outcome": "selected", "optionId": "implement"}}),
        );
        assert!(ordinary_allow["response"]["response"]["updatedPermissions"].is_null());
        let denied = control_response_frame(
            "plan-request",
            &serde_json::json!({"plan": "Keep this"}),
            false,
            true,
            Some("default"),
            &serde_json::json!({"outcome": {"outcome": "selected", "optionId": "deny"}}),
        );
        assert_eq!(denied["response"]["response"]["behavior"], "deny");
    }

    #[test]
    fn requested_bypass_not_reported_does_not_offer_bypass_implement() {
        let broker = PermissionBroker::for_test(Arc::new(|_, _| Ok(())));
        let mut reader = super::super::super::ClaudeReader::new(
            super::super::super::ClaudeView::new(None),
            Arc::clone(&broker),
            Arc::new(Mutex::new(HashMap::new())),
            Arc::new(Mutex::new(HashMap::new())),
            Arc::new(AtomicU64::new(2)),
        );
        let (runtime, connection) = attached(&broker);
        runtime.store_session_manifest(SessionEvent::SessionManifest {
            provider_id: Some("claude".to_string()),
            current_model_id: None,
            models: Vec::new(),
            modes: Some(devboule_protocol::SessionModeStateView {
                current_mode_id: "bypassPermissions".to_string(),
                available_modes: Vec::new(),
            }),
        });
        assert_eq!(runtime.mode_before_plan_id(), None);

        let init = include_str!("../../fixtures/wire/claude-exit-plan-mode.jsonl")
            .lines()
            .next()
            .expect("captured init frame");
        reader
            .feed(format!("{init}\n").as_bytes(), &runtime)
            .expect("feed reported plan mode");
        assert_eq!(runtime.current_mode_id().as_deref(), Some("plan"));
        assert_eq!(runtime.mode_before_plan_id(), None);

        let request = plan_line(serde_json::json!({"plan": "Implement safely"}));
        reader
            .feed(format!("{request}\n").as_bytes(), &runtime)
            .expect("dispatch plan request");
        let options = drain(&connection)
            .into_iter()
            .find_map(|event| match event {
                SessionEvent::PermissionRequest { options, .. } => Some(options),
                _ => None,
            })
            .expect("plan permission card");
        assert!(!options
            .iter()
            .any(|option| option.option_id == "implement_bypass"));
    }

    #[test]
    fn captured_exit_plan_status_updates_the_reported_mode() {
        let broker = PermissionBroker::for_test(Arc::new(|_, _| Ok(())));
        let mut reader = super::super::super::ClaudeReader::new(
            super::super::super::ClaudeView::new(None),
            Arc::clone(&broker),
            Arc::new(Mutex::new(HashMap::new())),
            Arc::new(Mutex::new(HashMap::new())),
            Arc::new(AtomicU64::new(2)),
        );
        let (runtime, _connection) = attached(&broker);
        let frames = include_str!("../../fixtures/wire/claude-exit-plan-mode.jsonl")
            .lines()
            .map(|line| serde_json::from_str::<serde_json::Value>(line).expect("captured frame"))
            .collect::<Vec<_>>();

        reader
            .feed(format!("{}\n", frames[0]).as_bytes(), &runtime)
            .expect("feed captured init frame");
        assert_eq!(runtime.current_mode_id().as_deref(), Some("plan"));
        reader
            .feed(format!("{}\n", frames[1]).as_bytes(), &runtime)
            .expect("feed captured ExitPlanMode response");
        assert_eq!(runtime.current_mode_id().as_deref(), Some("plan"));
        reader
            .feed(format!("{}\n", frames[2]).as_bytes(), &runtime)
            .expect("feed captured status frame");

        assert_eq!(runtime.current_mode_id().as_deref(), Some("acceptEdits"));
        assert_eq!(reader.view.current_mode_id(), Some("acceptEdits"));
        assert!(matches!(
            runtime.session_manifest(),
            Some(SessionEvent::SessionManifest {
                modes: Some(modes), ..
            }) if modes.current_mode_id == "acceptEdits"
        ));
    }

    #[test]
    fn closed_stream_mode_ack_changes_neither_runtime_nor_view() {
        let (sender, receiver) = std::sync::mpsc::channel();
        let mode_responses = Arc::new(Mutex::new(HashMap::from([("m1".to_string(), sender)])));
        let broker = PermissionBroker::for_test(Arc::new(|_, _| Ok(())));
        let mut reader = super::super::super::ClaudeReader::new(
            super::super::super::ClaudeView::new(None),
            Arc::clone(&broker),
            Arc::new(Mutex::new(HashMap::new())),
            mode_responses,
            Arc::new(AtomicU64::new(2)),
        );
        let (runtime, _) = attached(&broker);
        set_runtime_mode(&runtime, "default");
        runtime.stream.lock().expect("stream lock").output_closed = true;
        let ack = include_str!("../../fixtures/wire/claude-set-mode.jsonl")
            .lines()
            .next()
            .expect("captured mode acknowledgement");

        reader
            .feed(format!("{ack}\n").as_bytes(), &runtime)
            .expect("feed captured mode acknowledgement");

        assert_eq!(
            receiver.recv().expect("mode result"),
            Err("Session event stream is unavailable; reported mode was not applied.".into())
        );
        assert_eq!(runtime.current_mode_id().as_deref(), Some("default"));
        assert_eq!(runtime.mode_before_plan_id(), None);
        assert_eq!(reader.view.current_mode_id(), None);
    }

    #[test]
    fn mode_ack_surfaces_missing_mode_state_to_the_session() {
        let (sender, receiver) = std::sync::mpsc::channel();
        let mode_responses = Arc::new(Mutex::new(HashMap::from([(
            "set-permission-mode-1".to_string(),
            sender,
        )])));
        let broker = PermissionBroker::for_test(Arc::new(|_, _| Ok(())));
        let mut reader = super::super::super::ClaudeReader::new(
            super::super::super::ClaudeView::new(None),
            Arc::clone(&broker),
            Arc::new(Mutex::new(HashMap::new())),
            mode_responses,
            Arc::new(AtomicU64::new(2)),
        );
        let (runtime, connection) = attached(&broker);
        let acknowledged = br#"{"type":"control_response","response":{"subtype":"success","request_id":"set-permission-mode-1","response":{"mode":"acceptEdits"}}}"#;
        reader
            .feed(&[acknowledged.as_slice(), b"\n"].concat(), &runtime)
            .expect("dispatch successful mode acknowledgement");

        assert_eq!(
            receiver.recv().expect("mode result"),
            Err("Claude mode manifest is missing; reported mode was not applied.".to_string())
        );
        assert!(super::super::drain(&connection)
                .iter()
                .any(|event| matches!(
                    event,
                    SessionEvent::AgentError { message }
                        if message.contains("Claude reported mode acceptEdits, but session state was not updated")
                )));
    }

    #[test]
    fn real_plan_sender_allows_and_switches_mode_in_one_control_response() {
        let controls = Arc::new(Mutex::new(HashMap::new()));
        let frames = Arc::new(Mutex::new(Vec::new()));
        let frames_for_writer = Arc::clone(&frames);
        let sender = claude_permission_sender_with_writer(
            Arc::clone(&controls),
            Arc::new(move |bytes| {
                frames_for_writer.lock().expect("captured frames").push(
                    serde_json::from_slice::<serde_json::Value>(
                        bytes.strip_suffix(b"\n").expect("line ending"),
                    )
                    .expect("control response"),
                );
                Ok(())
            }),
        );
        let broker = PermissionBroker::for_test(sender);
        let mut reader = test_reader(Arc::clone(&broker), Arc::clone(&controls));
        let (runtime, connection) = attached(&broker);
        set_runtime_mode(&runtime, "default");
        set_runtime_mode(&runtime, "plan");
        reader
            .feed(
                format!(
                    "{}\n",
                    plan_line(serde_json::json!({"plan": "Implement it"}))
                )
                .as_bytes(),
                &runtime,
            )
            .expect("read plan request");

        broker
            .test_answer("toolu_plan", PermissionOutcome::AllowOnce, "implement")
            .expect("approve plan");

        let frames = frames.lock().expect("captured frames");
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0]["response"]["response"]["behavior"], "allow");
        assert_eq!(frames[0]["response"]["request_id"], "plan-request");
        assert_eq!(
            frames[0]["response"]["response"]["updatedPermissions"],
            serde_json::json!([
                {"type": "setMode", "mode": "acceptEdits", "destination": "session"}
            ])
        );
        drop(frames);
        assert!(super::super::drain(&connection).iter().any(|event| matches!(event,
            SessionEvent::AgentToolUpdate { tool_call_id, status: Some(status), text: None, title: Some(title), kind: Some(kind), .. }
            if tool_call_id == "toolu_plan" && status == "completed" && title == "Approved" && kind == "plan"
        )));
    }
}

fn question_line(tool_use_id: &str) -> serde_json::Value {
    serde_json::json!({
        "type": "control_request",
        "request_id": "ask-1",
        "request": {
            "subtype": "can_use_tool",
            "tool_name": "AskUserQuestion",
            "display_name": "AskUserQuestion",
            "input": {
                "questions": [{
                    "question": "Which colour should I paint the fence?",
                    "header": "Fence colour",
                    "multiSelect": false,
                    "options": [
                        {"label": "Forest green (Recommended)", "description": "Blends in."},
                        {"label": "Barn red", "description": "Classic."}
                    ]
                }]
            },
            "tool_use_id": tool_use_id
        }
    })
}

type Harness = (
    Arc<PermissionBroker>,
    Arc<Mutex<Vec<serde_json::Value>>>,
    Arc<Mutex<HashMap<u64, ClaudePendingControl>>>,
    Arc<super::SessionRuntime>,
    Arc<crate::session::ConnHandle>,
);

fn harness() -> Harness {
    harness_with(question_line("toolu_question"))
}

fn harness_with(line: serde_json::Value) -> Harness {
    let captured = Arc::new(Mutex::new(Vec::new()));
    let controls = Arc::new(Mutex::new(HashMap::new()));
    let captured_for_sender = Arc::clone(&captured);
    let controls_for_sender = Arc::clone(&controls);
    let sender: Arc<PermissionSender> = Arc::new(move |id, result| {
        let pending: ClaudePendingControl = controls_for_sender
            .lock()
            .expect("controls")
            .remove(&id)
            .expect("pending control");
        let frame = super::control_response_frame(
            &pending.request_id,
            &pending.input,
            pending.is_question,
            pending.is_plan,
            pending.pre_plan_mode.as_deref(),
            &result,
        );
        captured_for_sender.lock().expect("captured").push(frame);
        Ok(())
    });
    let broker = PermissionBroker::for_test(sender);
    let reader_controls = Arc::clone(&controls);
    let mut reader = test_reader(Arc::clone(&broker), reader_controls);
    let (runtime, conn) = attached(&broker);
    if line
        .pointer("/request/tool_name")
        .and_then(serde_json::Value::as_str)
        == Some("ExitPlanMode")
    {
        set_runtime_mode(&runtime, "default");
        set_runtime_mode(&runtime, "plan");
    }
    reader
        .feed(format!("{line}\n").as_bytes(), &runtime)
        .expect("feed question");
    // The harness reader is dropped; answering goes through the broker.
    let _ = reader;
    (broker, captured, controls, runtime, conn)
}

fn plan_line(input: serde_json::Value) -> serde_json::Value {
    serde_json::json!({
        "type": "control_request",
        "request_id": "plan-request",
        "request": {
            "subtype": "can_use_tool",
            "tool_name": "ExitPlanMode",
            "display_name": "ExitPlanMode",
            "input": input,
            "tool_use_id": "toolu_plan"
        }
    })
}

mod plan_card_tests {
    use super::{
        harness_with, harness_with_mode, plan_line, PermissionOutcome, PermissionRequestKind,
        SessionEvent,
    };

    #[test]
    fn exit_plan_mode_builds_a_plan_card_from_the_fixture_shape() {
        // This fixture follows the provider's can_use_tool shape; it is not captured live.
        let (broker, captured, _, _, conn) = harness_with(plan_line(serde_json::json!({
            "plan": "## Steps\n\n- Add the route\n- Verify it"
        })));
        let events = super::super::drain(&conn);
        let request = events
            .iter()
            .find_map(|event| match event {
                SessionEvent::PermissionRequest {
                    tool_call_id,
                    title,
                    kind,
                    plan,
                    options,
                    ..
                } if tool_call_id == "toolu_plan" => Some((title, kind, plan, options)),
                _ => None,
            })
            .expect("plan request event");
        assert_eq!(request.0, "Plan");
        assert_eq!(*request.1, Some(PermissionRequestKind::Plan));
        assert_eq!(
            request.2.as_deref(),
            Some("## Steps\n\n- Add the route\n- Verify it")
        );
        assert_eq!(
            request
                .3
                .iter()
                .map(|item| item.name.as_str())
                .collect::<Vec<_>>(),
            ["Reject", "Implement"]
        );
        broker
            .respond_with_option(
                "toolu_plan",
                PermissionOutcome::AllowOnce,
                Some("implement".to_string()),
                None,
            )
            .expect("implement");
        assert_eq!(
            captured.lock().expect("sender")[0]["response"]["response"]["behavior"],
            "allow"
        );
    }

    #[test]
    fn plan_cap_is_shared_by_the_permission_card_and_allow_response() {
        let text = format!("{}{}", "ab", "🧭".repeat(crate::plan_text::MAX_PLAN_BYTES));
        let (broker, captured, _, _, conn) =
            harness_with(plan_line(serde_json::json!({"plan": text})));
        let plan = super::super::drain(&conn)
            .into_iter()
            .find_map(|event| match event {
                SessionEvent::PermissionRequest { plan, .. } => plan,
                _ => None,
            })
            .expect("permission card plan");
        assert!(plan.len() <= crate::plan_text::MAX_PLAN_BYTES);
        assert!(plan.ends_with("[Plan truncated.]"));

        broker
            .respond_with_option(
                "toolu_plan",
                PermissionOutcome::AllowOnce,
                Some("implement".to_string()),
                None,
            )
            .expect("allow plan");
        assert_eq!(
            captured.lock().expect("response")[0]["response"]["response"]["updatedInput"]["plan"],
            plan
        );
    }

    #[test]
    fn plan_card_handles_missing_text_and_remembers_bypass_before_plan() {
        let (broker, _, _, _, conn) = harness_with(plan_line(serde_json::json!({})));
        let event = super::super::drain(&conn)
            .into_iter()
            .find(|event| matches!(event, SessionEvent::PermissionRequest { tool_call_id, .. } if tool_call_id == "toolu_plan"))
            .expect("plan card");
        assert!(
            matches!(event, SessionEvent::PermissionRequest { plan: Some(ref body), kind: Some(PermissionRequestKind::Plan), .. } if body == "No plan text was provided.")
        );
        drop(broker);

        let (_broker, _, _, _, conn) = harness_with_mode(
            plan_line(serde_json::json!({ "plan": "Do it" })),
            "bypassPermissions",
        );
        let event = super::super::drain(&conn)
            .into_iter()
            .find(|event| matches!(event, SessionEvent::PermissionRequest { tool_call_id, .. } if tool_call_id == "toolu_plan"))
            .expect("bypass plan card");
        assert!(
            matches!(event, SessionEvent::PermissionRequest { options, .. } if options.iter().any(|option| option.option_id == "implement_bypass" && option.name == "Implement with bypass"))
        );
    }

    #[test]
    fn closing_with_an_unanswered_plan_publishes_withdrawn() {
        let (broker, captured, _, _, connection) =
            harness_with(plan_line(serde_json::json!({"plan": "Wait for me"})));

        broker.close();

        assert!(super::super::drain(&connection).iter().any(|event| matches!(event,
            SessionEvent::AgentToolUpdate { tool_call_id, status: Some(status), text: None, title: Some(title), kind: Some(kind), .. }
            if tool_call_id == "toolu_plan" && status == "cancelled" && title == "Withdrawn" && kind == "plan"
        )));
        assert_eq!(
            captured.lock().expect("captured response")[0]["response"]["response"]["behavior"],
            "deny"
        );
    }
}

fn harness_with_mode(line: serde_json::Value, mode: &str) -> Harness {
    let captured = Arc::new(Mutex::new(Vec::new()));
    let controls = Arc::new(Mutex::new(HashMap::new()));
    let captured_for_sender = Arc::clone(&captured);
    let controls_for_sender = Arc::clone(&controls);
    let sender: Arc<PermissionSender> = Arc::new(move |id, result| {
        let pending: ClaudePendingControl = controls_for_sender
            .lock()
            .expect("controls")
            .remove(&id)
            .expect("pending");
        captured_for_sender
            .lock()
            .expect("captured")
            .push(super::control_response_frame(
                &pending.request_id,
                &pending.input,
                pending.is_question,
                pending.is_plan,
                pending.pre_plan_mode.as_deref(),
                &result,
            ));
        Ok(())
    });
    let broker = PermissionBroker::for_test(sender);
    let mut reader = test_reader(Arc::clone(&broker), Arc::clone(&controls));
    let (runtime, conn) = attached(&broker);
    set_runtime_mode(&runtime, mode);
    set_runtime_mode(&runtime, "plan");
    reader
        .feed(format!("{line}\n").as_bytes(), &runtime)
        .expect("feed plan");
    (broker, captured, controls, runtime, conn)
}

fn set_runtime_mode(runtime: &super::SessionRuntime, mode: &str) {
    runtime.store_session_manifest(SessionEvent::SessionManifest {
        provider_id: Some("claude".to_string()),
        current_model_id: None,
        models: Vec::new(),
        modes: Some(devboule_protocol::SessionModeStateView {
            current_mode_id: mode.to_string(),
            available_modes: Vec::new(),
        }),
    });
    runtime
        .record_claude_mode_report(mode)
        .expect("record provider-reported mode");
}

fn multi_select_line(tool_use_id: &str) -> serde_json::Value {
    let mut line = question_line(tool_use_id);
    line["request"]["input"]["questions"][0]["multiSelect"] = serde_json::Value::Bool(true);
    line
}

fn two_question_line(tool_use_id: &str) -> serde_json::Value {
    serde_json::json!({
        "type": "control_request",
        "request_id": "ask-2",
        "request": {
            "subtype": "can_use_tool",
            "tool_name": "AskUserQuestion",
            "display_name": "AskUserQuestion",
            "input": {
                "questions": [{
                    "question": "Which colour should I paint the fence?",
                    "header": "Fence colour",
                    "multiSelect": false,
                    "options": [
                        {"label": "Forest green (Recommended)"},
                        {"label": "Barn red"}
                    ]
                }, {
                    "question": "Which stain finish?",
                    "header": "Finish",
                    "multiSelect": false,
                    "options": [
                        {"label": "Matte"},
                        {"label": "Satin"}
                    ]
                }]
            },
            "tool_use_id": tool_use_id
        }
    })
}

fn tool_line_with_questions(tool_use_id: &str) -> serde_json::Value {
    // An ordinary tool whose input merely looks like a question: the
    // card is a tool card, and the reply must stay one too.
    serde_json::json!({
        "type": "control_request",
        "request_id": "tool-q",
        "request": {
            "subtype": "can_use_tool",
            "tool_name": "Bash",
            "display_name": "Bash",
            "input": {
                "command": "echo survey",
                "description": "Run the survey tool",
                "questions": [{
                    "question": "Which colour?",
                    "options": [{"label": "Green"}]
                }]
            },
            "tool_use_id": tool_use_id
        }
    })
}

fn asked(events: &[SessionEvent]) -> SessionEvent {
    events
        .iter()
        .find(|event| {
            matches!(event, SessionEvent::PermissionRequest { tool_call_id, .. }
                    if tool_call_id == "toolu_question")
        })
        .expect("question card")
        .clone()
}

#[test]
fn ask_user_question_builds_a_question_card() {
    let (broker, _, _, _, conn) = harness();
    let events = super::drain(&conn);
    match asked(&events) {
        SessionEvent::PermissionRequest {
            title,
            description,
            options,
            kind,
            questions,
            ..
        } => {
            assert_eq!(kind, Some(PermissionRequestKind::Question));
            assert_eq!(title, "Which colour should I paint the fence?");
            assert_eq!(
                description.as_deref(),
                Some("Forest green (Recommended) / Barn red")
            );
            assert_eq!(
                options
                    .iter()
                    .map(|option| option.option_id.as_str())
                    .collect::<Vec<_>>(),
                vec!["q0o0", "q0o1"]
            );
            let questions = questions.expect("question items");
            assert_eq!(questions.len(), 1);
            assert_eq!(
                questions[0].question,
                "Which colour should I paint the fence?"
            );
            assert_eq!(questions[0].header.as_deref(), Some("Fence colour"));
            assert!(!questions[0].multi_select);
            assert_eq!(questions[0].options[0].label, "Forest green (Recommended)");
            assert_eq!(
                questions[0].options[0].description.as_deref(),
                Some("Blends in.")
            );
        }
        _ => panic!("expected a permission request"),
    }
    assert_eq!(broker.pending_len(), 1);
}

#[test]
fn ask_user_question_option_pick_answers_by_full_text() {
    let (broker, captured, _, _, conn) = harness();
    let _ = super::drain(&conn);
    broker
        .respond_with_option(
            "toolu_question",
            PermissionOutcome::AllowOnce,
            Some("q0o1".to_string()),
            None,
        )
        .expect("option pick");
    let frames = captured.lock().expect("captured");
    assert_eq!(frames.len(), 1);
    let reply = &frames[0]["response"]["response"];
    assert_eq!(reply["behavior"], "allow");
    assert_eq!(
        reply["updatedInput"]["answers"]["Which colour should I paint the fence?"],
        "Barn red"
    );
    // The provider's own input travels on, with the answers added.
    assert!(reply["updatedInput"]["questions"].is_array());
}

#[test]
fn ask_user_question_other_answer_maps_by_full_text() {
    let (broker, captured, _, _, conn) = harness();
    let _ = super::drain(&conn);
    broker
        .respond_with_option(
            "toolu_question",
            PermissionOutcome::AllowOnce,
            None,
            Some("Teal, obviously".to_string()),
        )
        .expect("Other answer");
    let frames = captured.lock().expect("captured");
    let reply = &frames[0]["response"]["response"];
    assert_eq!(reply["behavior"], "allow");
    assert_eq!(
        reply["updatedInput"]["answers"]["Which colour should I paint the fence?"],
        "Teal, obviously"
    );
}

#[test]
fn ask_user_question_multi_select_answer_travels_verbatim() {
    // The card joins several picks the provider's own way (`", "`);
    // the daemon maps the joined text verbatim under the full-text key.
    // The fixture really is multi-select: a `false` flag here would
    // prove nothing about the multi path.
    let (broker, captured, _, _, conn) = harness_with(multi_select_line("toolu_question"));
    let events = super::drain(&conn);
    match asked(&events) {
        SessionEvent::PermissionRequest { questions, .. } => {
            let questions = questions.expect("question items");
            assert!(
                questions[0].multi_select,
                "the fixture must be multi-select"
            );
        }
        _ => panic!("expected a permission request"),
    }
    broker
        .respond_with_option(
            "toolu_question",
            PermissionOutcome::AllowOnce,
            None,
            Some("Forest green (Recommended), Barn red".to_string()),
        )
        .expect("multi-select answer");
    let frames = captured.lock().expect("captured");
    let reply = &frames[0]["response"]["response"];
    assert_eq!(reply["behavior"], "allow");
    assert_eq!(
        reply["updatedInput"]["answers"]["Which colour should I paint the fence?"],
        "Forest green (Recommended), Barn red"
    );
}

#[test]
fn ask_user_question_two_questions_answer_as_one_map() {
    let (broker, captured, _, _, conn) = harness_with(two_question_line("toolu_question"));
    let events = super::drain(&conn);
    match asked(&events) {
        SessionEvent::PermissionRequest {
            questions, options, ..
        } => {
            assert_eq!(questions.expect("items").len(), 2);
            assert_eq!(options.len(), 4);
        }
        _ => panic!("expected a permission request"),
    }
    let answer = serde_json::json!({
        "Which colour should I paint the fence?": "Barn red",
        "Which stain finish?": "Satin"
    })
    .to_string();
    broker
        .respond_with_option(
            "toolu_question",
            PermissionOutcome::AllowOnce,
            None,
            Some(answer),
        )
        .expect("two-question answer");
    let frames = captured.lock().expect("captured");
    let reply = &frames[0]["response"]["response"];
    assert_eq!(reply["behavior"], "allow");
    assert_eq!(
        reply["updatedInput"]["answers"]["Which colour should I paint the fence?"],
        "Barn red"
    );
    assert_eq!(
        reply["updatedInput"]["answers"]["Which stain finish?"],
        "Satin"
    );
}

#[test]
fn tool_input_with_questions_stays_an_ordinary_tool() {
    // The recorded kind decides the reply's shape, never the input's:
    // an allowed tool whose input carries `questions` is echoed as a
    // tool, not decoded as a pick (which would deny a granted Allow).
    let (broker, captured, _, _, conn) = harness_with(tool_line_with_questions("toolu_question"));
    let events = super::drain(&conn);
    match asked(&events) {
        SessionEvent::PermissionRequest { kind, options, .. } => {
            assert_eq!(kind, None);
            assert_eq!(options.len(), 2);
        }
        _ => panic!("expected a permission request"),
    }
    broker
        .respond("toolu_question", PermissionOutcome::AllowOnce)
        .expect("allow the tool");
    let frames = captured.lock().expect("captured");
    let reply = &frames[0]["response"]["response"];
    assert_eq!(reply["behavior"], "allow");
    assert_eq!(reply["updatedInput"]["command"], "echo survey");
    assert!(reply["updatedInput"].get("answers").is_none());
}

#[test]
fn ask_user_question_dismissal_denies() {
    let (broker, captured, _, _, conn) = harness();
    let _ = super::drain(&conn);
    broker
        .respond_with_option("toolu_question", PermissionOutcome::Deny, None, None)
        .expect("dismissal");
    let frames = captured.lock().expect("captured");
    assert_eq!(frames[0]["response"]["response"]["behavior"], "deny");
}

#[test]
fn ask_user_question_without_items_stays_a_tool_card() {
    let broker = PermissionBroker::for_test(Arc::new(|_, _| Ok(())));
    let mut reader = test_reader(Arc::clone(&broker), Arc::new(Mutex::new(HashMap::new())));
    let (runtime, conn) = attached(&broker);
    let line = serde_json::json!({
        "type": "control_request",
        "request_id": "ask-empty",
        "request": {
            "subtype": "can_use_tool",
            "tool_name": "AskUserQuestion",
            "display_name": "AskUserQuestion",
            "input": {"questions": []},
            "tool_use_id": "toolu_empty"
        }
    });
    reader
        .feed(format!("{line}\n").as_bytes(), &runtime)
        .expect("feed");
    let events = super::drain(&conn);
    match events
        .iter()
        .find(|event| {
            matches!(event, SessionEvent::PermissionRequest { tool_call_id, .. }
                    if tool_call_id == "toolu_empty")
        })
        .expect("fallback card")
    {
        SessionEvent::PermissionRequest {
            kind,
            questions,
            options,
            ..
        } => {
            assert_eq!(*kind, None);
            assert_eq!(*questions, None);
            assert_eq!(options.len(), 2);
        }
        _ => panic!("expected a permission request"),
    }
}

#[allow(clippy::type_complexity)]
fn open_harness(
    label: &str,
) -> (
    Arc<PermissionBroker>,
    Arc<Mutex<Vec<serde_json::Value>>>,
    Arc<super::SessionRuntime>,
    Arc<crate::journal::Journal>,
    std::path::PathBuf,
    super::ClaudeReader,
) {
    let path = crate::session::permission_broker::permission_path(label);
    let _ = std::fs::remove_file(&path);
    let journal = Arc::new(crate::journal::Journal::open(&path).expect("journal"));
    let session_id = "s.claude.transcript".to_string();
    journal
        .upsert_blocking(crate::journal::new_session_record(
            session_id.clone(),
            "owner",
            None,
            devboule_protocol::SessionKind::Claude,
            "claude transcript test",
        ))
        .expect("session row");
    let captured = Arc::new(Mutex::new(Vec::new()));
    let controls = Arc::new(Mutex::new(HashMap::new()));
    let captured_for_sender = Arc::clone(&captured);
    let controls_for_sender = Arc::clone(&controls);
    let sender: Arc<super::PermissionSender> = Arc::new(move |id, result| {
        let pending: ClaudePendingControl = controls_for_sender
            .lock()
            .expect("controls")
            .remove(&id)
            .expect("pending control");
        let frame = super::control_response_frame(
            &pending.request_id,
            &pending.input,
            pending.is_question,
            pending.is_plan,
            pending.pre_plan_mode.as_deref(),
            &result,
        );
        captured_for_sender.lock().expect("captured").push(frame);
        Ok(())
    });
    let broker = PermissionBroker::for_test(sender);
    let runtime =
        super::SessionRuntime::for_acp(session_id, Some(Arc::clone(&journal)), Arc::clone(&broker));
    let reader = test_reader(Arc::clone(&broker), Arc::clone(&controls));
    (broker, captured, runtime, journal, path, reader)
}

#[allow(clippy::type_complexity)]
fn journaled_harness(
    label: &str,
    line: serde_json::Value,
) -> (
    Arc<PermissionBroker>,
    Arc<Mutex<Vec<serde_json::Value>>>,
    Arc<super::SessionRuntime>,
    Arc<crate::journal::Journal>,
    std::path::PathBuf,
) {
    let (broker, captured, runtime, journal, path, mut reader) = open_harness(label);
    reader
        .feed(format!("{line}\n").as_bytes(), &runtime)
        .expect("feed question");
    (broker, captured, runtime, journal, path)
}

fn assistant_tool_use(tool_use_id: &str) -> serde_json::Value {
    serde_json::json!({
        "type": "assistant",
        "message": {
            "id": "msg-merge-1",
            "content": [{
                "type": "tool_use",
                "id": tool_use_id,
                "name": "AskUserQuestion",
                "input": {
                    "questions": [{"question": "Which colour should I paint the fence?"}]
                }
            }]
        }
    })
}

fn question_tool_result(tool_use_id: &str) -> serde_json::Value {
    serde_json::json!({
        "type": "user",
        "message": {
            "content": [{
                "type": "tool_result",
                "tool_use_id": tool_use_id,
                "content": [{"type": "text", "text": "Barn red, obviously"}],
                "is_error": false
            }]
        }
    })
}

/// This card's tool row, rebuilt through the history replay reader.
fn replayed_tool_rows(
    journal: &crate::journal::Journal,
    path: &std::path::Path,
    session_id: &str,
) -> Vec<SessionEvent> {
    journal.flush().expect("journal flush");
    let conn = rusqlite::Connection::open(path).expect("inspect journal");
    crate::journal::replay_session(&conn, session_id)
        .expect("history replay")
        .events
        .into_iter()
        .filter(|event| {
            matches!(
                event,
                SessionEvent::AgentToolCall { .. } | SessionEvent::AgentToolUpdate { .. }
            )
        })
        .collect()
}

/// Claude, end to end: an answered question leaves exactly one tool row
/// under the card id — the provider's `tool_use_id` — with the
/// question's words and the picked label, and the history reader
/// rebuilds the same row.
#[test]
fn answered_question_leaves_one_transcript_row() {
    let (broker, _, runtime, journal, path) =
        journaled_harness("claude-transcript-pick", question_line("toolu_question"));
    let _runtime = Arc::clone(&runtime);
    broker
        .respond_with_option(
            "toolu_question",
            PermissionOutcome::AllowOnce,
            Some("q0o1".to_string()),
            None,
        )
        .expect("option pick");
    let rows = replayed_tool_rows(&journal, &path, "s.claude.transcript");
    assert_eq!(rows.len(), 2, "one answered card leaves one row");
    match &rows[0] {
        SessionEvent::AgentToolCall {
            tool_call_id,
            title,
            ..
        } => {
            assert_eq!(tool_call_id, "toolu_question");
            assert_eq!(title, "Which colour should I paint the fence?");
        }
        _ => panic!("expected the tool call"),
    }
    match &rows[1] {
        SessionEvent::AgentToolUpdate {
            tool_call_id, text, ..
        } => {
            assert_eq!(tool_call_id, "toolu_question");
            let text = text.as_deref().unwrap_or_default();
            assert!(text.contains("Which colour should I paint the fence?"));
            assert!(text.contains("Barn red"));
        }
        _ => panic!("expected the tool update"),
    }
    journal.shutdown();
    let _ = std::fs::remove_file(path);
}

#[test]
fn dismissed_question_leaves_no_transcript_row() {
    let (broker, _, runtime, journal, path) =
        journaled_harness("claude-transcript-dismiss", question_line("toolu_question"));
    let _runtime = Arc::clone(&runtime);
    broker
        .respond_with_option("toolu_question", PermissionOutcome::Deny, None, None)
        .expect("dismissal");
    assert!(replayed_tool_rows(&journal, &path, "s.claude.transcript").is_empty());
    journal.shutdown();
    let _ = std::fs::remove_file(path);
}

fn tool_ids(events: &[SessionEvent]) -> Vec<String> {
    events
        .iter()
        .filter_map(|event| match event {
            SessionEvent::AgentToolCall { tool_call_id, .. }
            | SessionEvent::AgentToolUpdate { tool_call_id, .. } => Some(tool_call_id.clone()),
            _ => None,
        })
        .collect()
}

/// The load-bearing merge: a provider `tool_use` and a `can_use_tool`
/// carrying the same `tool_use_id` become one row — the broker's pair
/// reuses the card id, live and after replay. If the card were filed
/// under the control `request_id` instead, the ids split and this fails.
#[test]
fn provider_tool_use_and_card_share_one_row() {
    let (broker, _, runtime, journal, path, mut reader) = open_harness("claude-transcript-merge");
    let _runtime = Arc::clone(&runtime);
    let conn = super::ConnHandle::new(1);
    let outcome = runtime
        .try_attach_with_replay(None, &conn, true)
        .expect("attach");
    conn.track_with_agent_replay(
        "s.claude.transcript",
        Arc::clone(&runtime),
        false,
        None,
        outcome.generation,
        outcome.live_agent_replay,
    );
    reader
        .feed(
            format!("{}\n", assistant_tool_use("toolu_merge")).as_bytes(),
            &runtime,
        )
        .expect("feed tool_use");
    reader
        .feed(
            format!("{}\n", question_line("toolu_merge")).as_bytes(),
            &runtime,
        )
        .expect("feed permission");
    let live: Vec<SessionEvent> = super::drain(&conn)
        .into_iter()
        .filter(|event| {
            matches!(
                event,
                SessionEvent::AgentToolCall { .. } | SessionEvent::AgentToolUpdate { .. }
            )
        })
        .collect();
    assert_eq!(tool_ids(&live), vec!["toolu_merge".to_string()]);
    broker
        .respond_with_option(
            "toolu_merge",
            PermissionOutcome::AllowOnce,
            Some("q0o1".to_string()),
            None,
        )
        .expect("option pick");
    let live: Vec<SessionEvent> = super::drain(&conn)
        .into_iter()
        .filter(|event| {
            matches!(
                event,
                SessionEvent::AgentToolCall { .. } | SessionEvent::AgentToolUpdate { .. }
            )
        })
        .collect();
    assert_eq!(
        tool_ids(&live),
        vec!["toolu_merge".to_string(), "toolu_merge".to_string()]
    );
    // The provider's own echo arrives last and adds no text: the row's
    // final text is exactly ours.
    reader
        .feed(
            format!("{}\n", question_tool_result("toolu_merge")).as_bytes(),
            &runtime,
        )
        .expect("feed tool_result");
    let live: Vec<SessionEvent> = super::drain(&conn)
        .into_iter()
        .filter(|event| {
            matches!(
                event,
                SessionEvent::AgentToolCall { .. } | SessionEvent::AgentToolUpdate { .. }
            )
        })
        .collect();
    assert_eq!(live.len(), 1);
    match &live[0] {
        SessionEvent::AgentToolUpdate {
            tool_call_id, text, ..
        } => {
            assert_eq!(tool_call_id, "toolu_merge");
            assert_eq!(text.as_deref(), None);
        }
        _ => panic!("expected the provider update"),
    }
    let replayed = replayed_tool_rows(&journal, &path, "s.claude.transcript");
    assert_eq!(replayed.len(), 4);
    assert!(tool_ids(&replayed).iter().all(|id| id == "toolu_merge"));
    match &replayed[2] {
        SessionEvent::AgentToolUpdate { text, .. } => {
            assert_eq!(
                text.as_deref(),
                Some("Question: Which colour should I paint the fence?\nAnswer: Barn red")
            );
        }
        _ => panic!("expected our update"),
    }
    match &replayed[3] {
        SessionEvent::AgentToolUpdate { text, .. } => {
            assert_eq!(text.as_deref(), None);
        }
        _ => panic!("expected the provider update"),
    }
    journal.shutdown();
    let _ = std::fs::remove_file(path);
}

/// Without a `tool_use_id` the card falls back to the control
/// `request_id`, and the broker's row follows it there: the provider's
/// row under another id stays a separate row. The merge holds only
/// when the request carries the tool id.
#[test]
fn missing_tool_use_id_falls_back_to_request_id() {
    let (broker, _, runtime, journal, path, mut reader) =
        open_harness("claude-transcript-fallback");
    let _runtime = Arc::clone(&runtime);
    let mut line = question_line("toolu_detached");
    line["request"]
        .as_object_mut()
        .expect("request")
        .remove("tool_use_id");
    reader
        .feed(
            format!("{}\n", assistant_tool_use("toolu_detached")).as_bytes(),
            &runtime,
        )
        .expect("feed tool_use");
    reader
        .feed(format!("{line}\n").as_bytes(), &runtime)
        .expect("feed permission");
    broker
        .respond_with_option(
            "ask-1",
            PermissionOutcome::AllowOnce,
            Some("q0o1".to_string()),
            None,
        )
        .expect("option pick");
    let replayed = replayed_tool_rows(&journal, &path, "s.claude.transcript");
    assert_eq!(replayed.len(), 3);
    assert_eq!(
        tool_ids(&replayed),
        vec![
            "toolu_detached".to_string(),
            "ask-1".to_string(),
            "ask-1".to_string()
        ]
    );
    journal.shutdown();
    let _ = std::fs::remove_file(path);
}
