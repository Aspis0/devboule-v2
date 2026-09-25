//! Tests for the session protocol types: state machines, kinds and message kinds.

use std::{collections::BTreeSet, path::PathBuf};

use super::*;

#[test]
fn transcript_integrity_variants_round_trip_with_exact_wire_shape() {
    let cases = [
        (TranscriptIntegrity::Complete, r#"{"kind":"complete"}"#),
        (
            TranscriptIntegrity::Truncated {
                dropped_frames: 3,
                dropped_bytes: 4096,
                trimmed_bytes: 2048,
            },
            r#"{"kind":"truncated","droppedFrames":3,"droppedBytes":4096,"trimmedBytes":2048}"#,
        ),
        (
            TranscriptIntegrity::Unverifiable {
                dropped_frames: 3,
                dropped_bytes: 4096,
                trimmed_bytes: 2048,
            },
            r#"{"kind":"unverifiable","droppedFrames":3,"droppedBytes":4096,"trimmedBytes":2048}"#,
        ),
    ];

    for (value, expected_json) in cases {
        let encoded = serde_json::to_string(&value).expect("json");
        assert_eq!(encoded, expected_json);
        let decoded: TranscriptIntegrity = serde_json::from_str(&encoded).expect("round trip");
        assert_eq!(decoded, value);
    }
}

#[test]
fn session_event_uses_a_type_tag() {
    let output = serde_json::to_value(SessionEvent::Output {
        seq: 7,
        data: "hi".to_string(),
    })
    .expect("json");
    assert_eq!(output["type"], "output");
    assert_eq!(output["seq"], 7);
    let exit = serde_json::to_value(SessionEvent::Exit { code: Some(0) }).expect("json");
    assert_eq!(exit["type"], "exit");
    assert_eq!(exit["code"], 0);
}

#[test]
fn attention_uses_camel_case_and_omits_absent_snapshot_value() {
    let attention = Attention {
        reason: AttentionReason::Permission,
        at_ms: 42,
    };
    let encoded = serde_json::to_value(attention).expect("attention json");
    assert_eq!(encoded["reason"], "permission");
    assert_eq!(encoded["atMs"], 42);

    let snapshot = SessionStateSnapshot {
        id: "s.client.1".to_string(),
        workspace_id: Some("ws-1".to_string()),
        kind: SessionKind::Acp,
        title: "Agent".to_string(),
        state: SessionState::Live { generation: 1 },
        elapsed_ms: None,
        attention: None,
        origin: SessionOrigin::local(),
        display_name: None,
        created_by: None,
        profile_id: None,
        context_id: None,
        unattended: UnattendedState::No,
        labels: Default::default(),
        delegation: None,

        activity: None,
    };
    let encoded = serde_json::to_value(snapshot).expect("snapshot json");
    assert_eq!(encoded["workspaceId"], "ws-1");
    assert_eq!(encoded["kind"], "acp");
    assert!(encoded.get("attention").is_none());
    assert_eq!(encoded["origin"]["kind"], "local");
    // A local origin names no device: the two optional fields stay off the
    // wire rather than travelling as `null`.
    assert!(encoded["origin"].get("deviceId").is_none());
    assert!(encoded["origin"].get("role").is_none());
}

/// The two fields a push-only row needs to be readable (S5-09, S5-04): the
/// name the child was created under and the session that created it. Both
/// spell camelCase on the wire and both stay off it when absent, so an
/// older dialect sees the frame it saw before.
#[test]
fn snapshot_carries_the_display_name_and_the_creator_in_camel_case() {
    let snapshot = SessionStateSnapshot {
        id: "s.client.1".to_string(),
        workspace_id: Some("ws-1".to_string()),
        kind: SessionKind::Acp,
        title: "Agent".to_string(),
        state: SessionState::Live { generation: 1 },
        elapsed_ms: Some(42),
        attention: None,
        origin: SessionOrigin::local(),
        display_name: Some("worker".to_string()),
        created_by: Some("s.parent.1".to_string()),
        profile_id: Some("profile-1".to_string()),
        context_id: Some("s.root.1".to_string()),
        unattended: UnattendedState::Yes,
        labels: Default::default(),
        delegation: Some(DelegationState {
            answered: 3,
            state: DelegationRunState::Active,
        }),

        activity: None,
    };
    let encoded = serde_json::to_value(&snapshot).expect("snapshot json");
    assert_eq!(encoded["displayName"], "worker");
    assert_eq!(encoded["createdBy"], "s.parent.1");
    let bytes = serde_json::to_string(&snapshot).expect("snapshot json");
    let back: SessionStateSnapshot = serde_json::from_str(&bytes).expect("round trip");
    assert_eq!(back, snapshot, "the spelling holds in both directions");

    // A session with neither says so by leaving both keys out, rather than
    // travelling as `null`.
    let plain = SessionStateSnapshot {
        display_name: None,
        created_by: None,
        profile_id: None,
        context_id: None,
        unattended: UnattendedState::No,
        labels: Default::default(),
        ..snapshot
    };
    let encoded = serde_json::to_value(&plain).expect("snapshot json");
    assert!(encoded.get("displayName").is_none());
    assert!(encoded.get("createdBy").is_none());
}

#[test]
fn session_uses_camel_case_workspace_id() {
    let session = Session {
        id: "session-1-1".to_string(),
        workspace_id: Some("ws-1".to_string()),
        cwd: None,
        kind: SessionKind::Terminal,
        title: "Terminal".to_string(),
        state: SessionState::Live { generation: 1 },
        elapsed_ms: None,
        provider: None,
        peer_session_id: None,
        created_at_ms: 1,
        origin: SessionOrigin::local(),
        display_name: None,
        created_by: None,
        profile_id: None,
        context_id: None,
        unattended: UnattendedState::No,
        labels: Default::default(),
        resumable: false,
    };
    let value = serde_json::to_value(&session).expect("json");
    assert_eq!(value["workspaceId"], "ws-1");
    assert_eq!(value["createdAtMs"], 1);
    assert_eq!(value["kind"], "terminal");
    assert!(value.get("generation").is_none());
    assert_eq!(value["state"]["type"], "live");
    assert_eq!(value["state"]["generation"], 1);
}

/// The app reads `session.origin?.kind === "peer"` to badge a session and
/// names the device from `deviceId`. The wire spelling is the contract.
#[test]
fn a_peer_origin_session_round_trips_and_a_missing_origin_reads_as_local() {
    let session = Session {
        id: "s.peer.1".to_string(),
        workspace_id: None,
        cwd: None,
        kind: SessionKind::Claude,
        title: "Agent".to_string(),
        state: SessionState::Live { generation: 1 },
        elapsed_ms: None,
        provider: Some("claude".to_string()),
        peer_session_id: None,
        created_at_ms: 1,
        origin: SessionOrigin::peer("device-phone", PeerRole::Client),
        display_name: None,
        created_by: None,
        profile_id: None,
        context_id: None,
        unattended: UnattendedState::No,
        labels: Default::default(),
        resumable: false,
    };
    let value = serde_json::to_value(&session).expect("json");
    assert_eq!(value["origin"]["kind"], "peer");
    assert_eq!(value["origin"]["deviceId"], "device-phone");
    assert_eq!(value["origin"]["role"], "client");
    let decoded: Session = serde_json::from_value(value).expect("session");
    assert_eq!(decoded.origin, session.origin);

    // Every row written before the origin existed is the local person's.
    let legacy = serde_json::json!({
        "id": "s.client.1",
        "workspaceId": null,
        "kind": "terminal",
        "title": "Terminal",
        "state": { "type": "live", "generation": 1 },
        "createdAtMs": 1
    });
    let decoded: Session = serde_json::from_value(legacy).expect("legacy session");
    assert_eq!(decoded.origin, SessionOrigin::local());
    assert!(decoded.origin.is_local());
}

#[test]
fn session_json_without_created_at_ms_fails_to_deserialize() {
    let session = Session {
        id: "session-1-1".to_string(),
        workspace_id: None,
        cwd: None,
        kind: SessionKind::Terminal,
        title: "Terminal".to_string(),
        state: SessionState::Live { generation: 1 },
        elapsed_ms: None,
        provider: None,
        peer_session_id: None,
        created_at_ms: 1,
        origin: SessionOrigin::local(),
        display_name: None,
        created_by: None,
        profile_id: None,
        context_id: None,
        unattended: UnattendedState::No,
        labels: Default::default(),
        resumable: false,
    };
    let mut value = serde_json::to_value(&session).expect("json");
    value
        .as_object_mut()
        .expect("session object")
        .remove("createdAtMs");
    let error = serde_json::from_value::<Session>(value)
        .expect_err("missing createdAtMs must not default to 0");
    let message = error.to_string();
    assert!(
        message.contains("createdAtMs") || message.contains("created_at_ms"),
        "error must name the missing field, got {message}"
    );
}

#[test]
fn silent_session_carries_elapsed_age_and_event_is_distinct() {
    let session = Session {
        id: "session-1-1".to_string(),
        workspace_id: None,
        cwd: None,
        kind: SessionKind::Terminal,
        title: "Terminal".to_string(),
        state: SessionState::Silent { generation: 1 },
        elapsed_ms: Some(300_042),
        provider: None,
        peer_session_id: None,
        created_at_ms: 1,
        origin: SessionOrigin::local(),
        display_name: None,
        created_by: None,
        profile_id: None,
        context_id: None,
        unattended: UnattendedState::No,
        labels: Default::default(),
        resumable: false,
    };
    let encoded = serde_json::to_value(&session).expect("session json");
    assert_eq!(encoded["state"]["type"], "silent");
    assert_eq!(encoded["elapsedMs"], 300_042);

    let event = serde_json::to_value(SessionEvent::Silent {
        elapsed_ms: 300_042,
    })
    .expect("event json");
    assert_eq!(event["type"], "silent");
    assert_eq!(event["elapsedMs"], 300_042);
}

#[test]
fn resumable_session_metadata_uses_camel_case_wire_names() {
    let session = Session {
        id: "session-1-1".to_string(),
        workspace_id: None,
        cwd: None,
        kind: SessionKind::Acp,
        title: "Agent".to_string(),
        state: SessionState::Recovered {
            generation: 1,
            integrity: TranscriptIntegrity::Unverifiable {
                dropped_frames: 0,
                dropped_bytes: 0,
                trimmed_bytes: 0,
            },
        },
        elapsed_ms: None,
        provider: Some("grok".to_string()),
        peer_session_id: Some("peer-session-1".to_string()),
        created_at_ms: 1,
        origin: SessionOrigin::local(),
        display_name: None,
        created_by: None,
        profile_id: None,
        context_id: None,
        unattended: UnattendedState::No,
        labels: Default::default(),
        resumable: false,
    };
    let value = serde_json::to_value(&session).expect("json");
    assert_eq!(value["provider"], "grok");
    assert_eq!(value["peerSessionId"], "peer-session-1");
    let decoded: Session = serde_json::from_value(value).expect("round trip");
    assert_eq!(decoded, session);
}

#[test]
fn recovered_is_a_different_wire_type_from_live_and_ended() {
    let live = serde_json::to_value(SessionState::Live { generation: 1 }).expect("json");
    let ended = serde_json::to_value(SessionState::Ended {
        generation: 1,
        code: Some(0),
        integrity: TranscriptIntegrity::Complete,
    })
    .expect("json");
    let recovered = serde_json::to_value(SessionState::Recovered {
        generation: 1,
        integrity: TranscriptIntegrity::Unverifiable {
            dropped_frames: 0,
            dropped_bytes: 0,
            trimmed_bytes: 0,
        },
    })
    .expect("json");
    assert_eq!(live["type"], "live");
    assert_eq!(ended["type"], "ended");
    assert_eq!(recovered["type"], "recovered");
    assert_eq!(recovered["integrity"]["kind"], "unverifiable");
    assert_ne!(live["type"], recovered["type"]);
    assert_ne!(ended["type"], recovered["type"]);
}

#[test]
fn recovered_event_is_distinct_from_exit() {
    let recovered = serde_json::to_value(SessionEvent::Recovered {
        integrity: TranscriptIntegrity::Unverifiable {
            dropped_frames: 2,
            dropped_bytes: 12,
            trimmed_bytes: 0,
        },
    })
    .expect("json");
    let exit = serde_json::to_value(SessionEvent::Exit { code: None }).expect("json");
    assert_eq!(recovered["type"], "recovered");
    assert_eq!(recovered["integrity"]["kind"], "unverifiable");
    assert_eq!(recovered["integrity"]["droppedBytes"], 12);
    assert_eq!(exit["type"], "exit");
    assert_ne!(recovered["type"], exit["type"]);
}

#[test]
fn journal_degraded_event_round_trips() {
    let event = SessionEvent::JournalDegraded {
        dropped_frames: 3,
        dropped_bytes: 4096,
    };
    let encoded = serde_json::to_value(&event).expect("json");
    assert_eq!(encoded["type"], "journal_degraded");
    assert_eq!(encoded["droppedFrames"], 3);
    assert_eq!(encoded["droppedBytes"], 4096);
    let decoded: SessionEvent = serde_json::from_value(encoded).expect("event");
    assert_eq!(decoded, event);
}

#[test]
fn session_notice_round_trips_with_severity() {
    let event = SessionEvent::SessionNotice {
        text: "Codex declined an out-of-scope request.".to_string(),
        severity: NoticeSeverity::Warning,
    };
    let encoded = serde_json::to_value(&event).expect("event json");
    assert_eq!(
        encoded,
        serde_json::json!({
            "type": "session_notice",
            "text": "Codex declined an out-of-scope request.",
            "severity": "warning"
        })
    );
    let decoded: SessionEvent = serde_json::from_value(encoded).expect("event");
    assert_eq!(decoded, event);
}

#[test]
fn agent_user_message_kind_defaults_for_legacy_rows_and_serializes_current_rows() {
    let legacy = serde_json::json!({
        "type": "agent_user_message",
        "messageId": "m-legacy",
        "text": "old row",
        "author": "agent"
    });
    let decoded: SessionEvent = serde_json::from_value(legacy).expect("legacy event");
    assert!(matches!(
        decoded,
        SessionEvent::AgentUserMessage {
            message_kind: UserMessageKind::Unknown,
            ..
        }
    ));

    let current = SessionEvent::AgentUserMessage {
        message_id: Some("m-current".to_string()),
        text: "outgoing words".to_string(),
        author: UserMessageAuthor::Agent,
        message_kind: UserMessageKind::OutgoingA2a,
    };
    let encoded = serde_json::to_value(&current).expect("current event");
    assert_eq!(encoded["messageKind"], "outgoing_a2a");
}

#[test]
fn user_message_kind_matches_frontend_union() {
    let path = frontend_ipc_ts_path();
    if !path.is_file() {
        panic!(
                "TypeScript UserMessageKind union not found at {}. \
                 Refusing to skip: this test is the guard that keeps UserMessageKind aligned with src/types/ipc.ts.",
                path.display()
            );
    }
    let source = std::fs::read_to_string(&path).unwrap_or_else(|err| {
        panic!("failed to read {}: {err}", path.display());
    });
    let ts_names = user_message_kinds_in_typescript_union(&source);

    let mut rust_names = BTreeSet::new();
    for kind in every_user_message_kind() {
        let value = serde_json::to_value(kind).expect("json");
        let Some(name) = value.as_str() else {
            panic!("{kind:?} serialized to {value}, expected a string");
        };
        rust_names.insert(name.to_owned());
    }

    assert_eq!(
            rust_names, ts_names,
            "UserMessageKind serde names and the TypeScript UserMessageKind union in src/types/ipc.ts drifted"
        );
}

fn frontend_ipc_ts_path() -> PathBuf {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.pop();
    path.pop();
    path.push("src");
    path.push("types");
    path.push("ipc.ts");
    path
}

fn user_message_kinds_in_typescript_union(source: &str) -> BTreeSet<String> {
    const MARKER: &str = "export type UserMessageKind";
    let Some(marker_at) = source.find(MARKER) else {
        panic!(
            "src/types/ipc.ts has no `{MARKER}` alias; cannot check alignment with UserMessageKind"
        );
    };
    let after_marker = &source[marker_at + MARKER.len()..];
    let Some(eq_at) = after_marker.find('=') else {
        panic!("`{MARKER}` has no `=`");
    };
    let after_eq = &after_marker[eq_at + 1..];
    let Some(semi_at) = after_eq.find(';') else {
        panic!("`{MARKER}` has no terminating `;`");
    };
    let body = &after_eq[..semi_at];

    let mut names = BTreeSet::new();
    let mut rest = body;
    while let Some(start) = rest.find('"') {
        rest = &rest[start + 1..];
        let Some(end) = rest.find('"') else {
            panic!("unterminated string in `{MARKER}` union");
        };
        names.insert(rest[..end].to_owned());
        rest = &rest[end + 1..];
    }
    if names.is_empty() {
        panic!("`{MARKER}` union contains no string literals");
    }
    names
}

fn every_user_message_kind() -> Vec<UserMessageKind> {
    macro_rules! variants {
            ($($variant:ident),+ $(,)?) => {{
                let kinds = vec![$(UserMessageKind::$variant),+];
                for kind in &kinds {
                    match kind {
                        $(UserMessageKind::$variant => {})+
                    }
                }
                kinds
            }};
        }
    variants!(
        Unknown,
        Composer,
        OutgoingA2a,
        IncomingA2a,
        SystemNotice,
        Creation
    )
}

#[test]
fn permission_request_round_trips_with_tool_call_correlation() {
    let event = SessionEvent::PermissionRequest {
        tool_call_id: "call-17".to_string(),
        title: "Run command".to_string(),
        description: Some("The agent wants to run a build.".to_string()),
        command: Some("cargo test".to_string()),
        args: Some(vec!["--offline".to_string(), "gate".to_string()]),
        cwd: Some("C:\\worktree".to_string()),
        env: Some(vec![PermissionEnvVar {
            name: "DB_GATE".to_string(),
            value: "SAFE".to_string(),
        }]),
        options: vec![PermissionOption {
            option_id: "allow".to_string(),
            name: "Allow once".to_string(),
            kind: "allow_once".to_string(),
        }],
        is_chooser: None,
        origin: SessionOrigin::peer("device-phone", PeerRole::Client),
        create_agent: None,
    };
    let encoded = serde_json::to_value(&event).expect("json");
    assert_eq!(encoded["type"], "permission_request");
    assert_eq!(encoded["toolCallId"], "call-17");
    assert_eq!(encoded["args"][0], "--offline");
    assert_eq!(encoded["args"][1], "gate");
    assert_eq!(encoded["env"][0]["name"], "DB_GATE");
    assert_eq!(encoded["env"][0]["value"], "SAFE");
    assert_eq!(encoded["options"][0]["optionId"], "allow");
    assert_eq!(encoded["options"][0]["name"], "Allow once");
    assert_eq!(encoded["options"][0]["kind"], "allow_once");
    assert_eq!(encoded["origin"]["kind"], "peer");
    assert_eq!(encoded["origin"]["deviceId"], "device-phone");
    assert_eq!(encoded["origin"]["role"], "client");
    let decoded: SessionEvent = serde_json::from_value(encoded).expect("event");
    assert_eq!(decoded, event);
}

/// The two shapes the app's `SessionOrigin` type names, in the protocol's
/// own words, plus the one thing the field must refuse: absence.
#[test]
fn a_permission_request_origin_round_trips_and_absence_is_a_wire_error() {
    let local: SessionOrigin = serde_json::from_str(r#"{"kind":"local"}"#).expect("local origin");
    assert_eq!(local, SessionOrigin::local());
    assert_eq!(
        serde_json::to_string(&local).expect("json"),
        r#"{"kind":"local"}"#
    );

    let peer: SessionOrigin =
        serde_json::from_str(r#"{"kind":"peer","deviceId":"device-phone","role":"client"}"#)
            .expect("peer origin");
    assert_eq!(peer, SessionOrigin::peer("device-phone", PeerRole::Client));
    assert_eq!(
        serde_json::to_string(&peer).expect("json"),
        r#"{"kind":"peer","deviceId":"device-phone","role":"client"}"#
    );

    let request = SessionEvent::PermissionRequest {
        tool_call_id: "call-18".to_string(),
        title: "Run command".to_string(),
        description: None,
        command: None,
        args: None,
        cwd: None,
        env: None,
        options: Vec::new(),
        is_chooser: None,
        origin: SessionOrigin::local(),
        create_agent: None,
    };
    let encoded = serde_json::to_value(&request).expect("json");
    assert_eq!(encoded["origin"]["kind"], "local");
    assert!(encoded["origin"].get("deviceId").is_none());
    assert!(encoded["origin"].get("role").is_none());
    let decoded: SessionEvent = serde_json::from_value(encoded).expect("event");
    assert_eq!(decoded, request);

    // Absence is not "local": a card that does not say where it came from
    // is a wire error, because "absent" would be read as this machine's own
    // session on a device that asked for it.
    let absent = serde_json::json!({
        "type": "permission_request",
        "toolCallId": "call-19",
        "title": "Run command",
        "options": []
    });
    let error = serde_json::from_value::<SessionEvent>(absent)
        .expect_err("a permission request without an origin must not parse");
    assert!(
        error.to_string().contains("origin"),
        "the error must name the missing field: {error}"
    );
}

/// P1: a creation card frame from a daemon that never heard of `ToolsState`
/// — no `tools` key — decodes, and lands on the third state, never on the
/// benign "no tools". A required field would have failed the decode and
/// closed a mixed pair's connection; the addition with the honest default
/// does not, which is why this is not a version bump.
#[test]
fn a_creation_card_without_the_tools_word_decodes_as_unverified() {
    let frame = serde_json::json!({
        "creatorSessionId": "s.1.1",
        "provider": "pi",
        "profile": "worker",
        "title": "worker",
        "caps": {
            "liveChildren": 0,
            "maxLiveChildren": 3,
            "creationsThisHour": 0,
            "maxCreationsPerHour": 10,
            "depth": 1,
            "maxDepth": 2,
            "liveAgentSessions": 1,
            "maxLiveAgentSessions": 8,
        },
    });
    let card: super::CreateAgentCard =
        serde_json::from_value(frame).expect("an old card frame decodes");
    assert_eq!(
        card.tools, "unverified",
        "absent renders as not-established, never as no-tools"
    );
    // And the current shape round-trips with its word intact.
    let current = super::CreateAgentCard {
        creator_session_id: "s.1.1".to_string(),
        provider: "claude".to_string(),
        profile: "worker".to_string(),
        title: "worker".to_string(),
        tools: "hosted".to_string(),
        caps: card.caps.clone(),
    };
    let encoded = serde_json::to_value(&current).expect("json");
    assert_eq!(encoded["tools"], "hosted");
    let decoded: super::CreateAgentCard = serde_json::from_value(encoded).expect("event");
    assert_eq!(decoded, current);
}

#[test]
fn session_manifest_round_trips_with_camel_case_wire_names() {
    let event = SessionEvent::SessionManifest {
        provider_id: Some("grok".to_string()),
        current_model_id: Some("grok-4.6".to_string()),
        models: vec![SessionModel {
            model_id: "grok-4.6".to_string(),
            name: "Grok 4.6".to_string(),
            description: Some("SpaceXAI's latest frontier model".to_string()),
            context_tokens: Some(500_000),
            current_effort: Some("xhigh".to_string()),
            efforts: Some(vec![SessionModelEffort {
                id: "xhigh".to_string(),
                label: "Extra High Effort".to_string(),
                description: Some("Highest effort and reasoning level".to_string()),
                default: Some(false),
            }]),
        }],
        modes: Some(SessionModeStateView {
            current_mode_id: "ask".to_string(),
            available_modes: vec![SessionModeView {
                id: "ask".to_string(),
                name: "Always ask".to_string(),
                description: Some("Ask before every tool call.".to_string()),
            }],
        }),
    };
    let encoded = serde_json::to_value(&event).expect("json");
    assert_eq!(encoded["type"], "session_manifest");
    assert_eq!(encoded["providerId"], "grok");
    assert_eq!(encoded["currentModelId"], "grok-4.6");
    assert_eq!(encoded["models"][0]["modelId"], "grok-4.6");
    assert_eq!(encoded["models"][0]["contextTokens"], 500_000);
    assert_eq!(encoded["models"][0]["currentEffort"], "xhigh");
    assert_eq!(encoded["models"][0]["efforts"][0]["id"], "xhigh");
    assert_eq!(encoded["modes"]["currentModeId"], "ask");
    assert_eq!(encoded["modes"]["availableModes"][0]["name"], "Always ask");
    let decoded: SessionEvent = serde_json::from_value(encoded).expect("event");
    assert_eq!(decoded, event);
}

#[test]
fn snapshot_event_round_trips_with_camel_case_wire_names() {
    let event = SessionEvent::Snapshot {
        as_of_seq: 41,
        cols: 200,
        rows: 50,
        data: "\u{1b}[2J\u{1b}[H$ ls\r\nsrc  target\r\n".to_string(),
        cursor: ScreenCursor {
            row: 12,
            col: 34,
            visible: true,
            shape: CursorShape::Block,
            blinking: true,
        },
        alternate_screen: false,
        bracketed_paste: true,
        line_wrap: true,
        title: Some("devboule - pwsh".to_string()),
    };
    let encoded = serde_json::to_value(&event).expect("json");
    assert_eq!(encoded["type"], "snapshot");
    assert_eq!(encoded["asOfSeq"], 41);
    assert_eq!(encoded["cols"], 200);
    assert_eq!(encoded["rows"], 50);
    assert_eq!(encoded["cursor"]["row"], 12);
    assert_eq!(encoded["cursor"]["col"], 34);
    assert_eq!(encoded["cursor"]["visible"], true);
    assert_eq!(encoded["cursor"]["shape"], "block");
    assert_eq!(encoded["cursor"]["blinking"], true);
    assert_eq!(encoded["alternateScreen"], false);
    assert_eq!(encoded["bracketedPaste"], true);
    assert_eq!(encoded["lineWrap"], true);
    assert_eq!(encoded["title"], "devboule - pwsh");
    // A rename here silently breaks the client, so pin the exact key
    // set, not just the individual names.
    let mut keys: Vec<&str> = encoded
        .as_object()
        .expect("object")
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        [
            "alternateScreen",
            "asOfSeq",
            "bracketedPaste",
            "cols",
            "cursor",
            "data",
            "lineWrap",
            "rows",
            "title",
            "type"
        ]
    );
    let decoded: SessionEvent = serde_json::from_value(encoded).expect("event");
    assert_eq!(decoded, event);
}

#[test]
fn snapshot_title_is_absent_when_none() {
    let event = SessionEvent::Snapshot {
        as_of_seq: 0,
        cols: 80,
        rows: 24,
        data: "\u{1b}[Hready".to_string(),
        cursor: ScreenCursor {
            row: 0,
            col: 5,
            visible: true,
            shape: CursorShape::Underline,
            blinking: false,
        },
        alternate_screen: false,
        bracketed_paste: false,
        line_wrap: false,
        title: None,
    };
    let encoded = serde_json::to_value(&event).expect("json");
    assert!(encoded.get("title").is_none());
    let decoded: SessionEvent = serde_json::from_value(encoded).expect("event");
    assert_eq!(decoded, event);
}

#[test]
fn cursor_shapes_have_stable_wire_values() {
    assert_eq!(
        serde_json::to_value(CursorShape::Block).expect("json"),
        "block"
    );
    assert_eq!(
        serde_json::to_value(CursorShape::Underline).expect("json"),
        "underline"
    );
    assert_eq!(serde_json::to_value(CursorShape::Bar).expect("json"), "bar");
    for shape in [CursorShape::Block, CursorShape::Underline, CursorShape::Bar] {
        let decoded: CursorShape =
            serde_json::from_value(serde_json::to_value(shape).expect("json")).expect("shape");
        assert_eq!(decoded, shape);
    }
}

#[test]
fn snapshot_screen_data_is_ndjson_safe() {
    let event = SessionEvent::Snapshot {
        as_of_seq: 7,
        cols: 80,
        rows: 24,
        data: "row1\r\nrow2\n\u{1b}[K".to_string(),
        cursor: ScreenCursor {
            row: 1,
            col: 0,
            visible: true,
            shape: CursorShape::Block,
            blinking: true,
        },
        alternate_screen: false,
        bracketed_paste: false,
        line_wrap: true,
        title: None,
    };
    let encoded = serde_json::to_string(&event).expect("json");
    assert!(
        !encoded.contains('\n'),
        "compact JSON must not contain a raw newline or NDJSON framing splits the event"
    );
    let decoded: SessionEvent = serde_json::from_str(&encoded).expect("parse");
    assert_eq!(decoded, event);
}

#[test]
fn dense_worst_case_snapshot_still_fits_the_frame_cap() {
    // Heaviest plausible snapshot: a 200x50 screen where every cell
    // repaints foreground and background in its own 24-bit colour, so
    // no pair of SGR sequences can collapse. Each cell costs 49 bytes
    // of escaped JSON (each ESC becomes \u001b), about 490 KiB total -
    // heavier than the ~410 KiB worst case measured during design, and
    // still under the 1 MiB NDJSON frame cap. A typical
    // snapshot is 8-30 KiB: never assume snapshots are always small.
    let mut data = String::with_capacity(200 * 50 * 39);
    for cell in 0..(200u32 * 50) {
        // 100..=255 so every colour component is three digits long.
        let fg = (100 + (cell % 156)) as u8;
        let bg = (100 + (cell * 7 % 156)) as u8;
        data.push_str(&format!(
            "\u{1b}[38;2;{fg};{fg};{fg}m\u{1b}[48;2;{bg};{bg};{bg}mX"
        ));
    }
    let event = SessionEvent::Snapshot {
        as_of_seq: 4096,
        cols: 200,
        rows: 50,
        data,
        cursor: ScreenCursor {
            row: 49,
            col: 199,
            visible: true,
            shape: CursorShape::Bar,
            blinking: false,
        },
        alternate_screen: true,
        bracketed_paste: true,
        line_wrap: true,
        title: None,
    };
    let encoded = serde_json::to_vec(&event).expect("json");
    assert!(
        encoded.len() > 400_000,
        "expected a dense worst-case payload, got {} bytes",
        encoded.len()
    );
    assert!(
        encoded.len() < crate::MAX_FRAME_BYTES,
        "worst-case snapshot serialises to {} bytes, frame cap is {}",
        encoded.len(),
        crate::MAX_FRAME_BYTES
    );
}

#[test]
fn cursor_generation_mismatch_is_an_error() {
    let err = cursor_replay_ok(
        2,
        Cursor {
            generation: 1,
            seq: 9,
        },
    )
    .unwrap_err();
    assert_eq!(err.code, ErrorCode::SessionGenerationMismatch);
    cursor_replay_ok(
        2,
        Cursor {
            generation: 2,
            seq: 9,
        },
    )
    .expect("same generation");
}

#[test]
fn resume_not_supported_is_an_explicit_variant() {
    let value = serde_json::to_value(ResumeResult::NotSupported).expect("json");
    assert_eq!(value["type"], "not_supported");
}

#[test]
fn persistence_kind_claude_round_trips_with_its_own_tag() {
    let encoded = serde_json::to_value(Persistence {
        kind: PersistenceKind::Claude {
            handle: "s.client.9".to_string(),
        },
    })
    .expect("json");
    assert_eq!(encoded["kind"]["type"], "claude");
    assert_eq!(encoded["kind"]["handle"], "s.client.9");
    let back: Persistence = serde_json::from_value(encoded).expect("round trip");
    assert_eq!(
        back.kind,
        PersistenceKind::Claude {
            handle: "s.client.9".to_string()
        }
    );
}

#[test]
fn session_without_resumable_reads_back_as_not_resumable() {
    // A frame from a daemon that predates the field must parse, and must
    // not offer a resume on a guess.
    let session: Session = serde_json::from_str(
        r#"{"id":"s.client.1","kind":"acp","title":"t",
                "state":{"type":"ended","generation":1,"code":0,
                "integrity":{"kind":"complete"}},
                "createdAtMs":1,"origin":{"kind":"local"},"unattended":"unknown"}"#,
    )
    .expect("old frame parses");
    assert!(!session.resumable);
}

#[test]
fn resume_resumed_and_failed_round_trip_on_the_wire() {
    let resumed = ResumeResult::Resumed {
        session: Box::new(Session {
            id: "s.client.1".to_string(),
            workspace_id: None,
            cwd: None,
            kind: SessionKind::Acp,
            title: "t".to_string(),
            state: SessionState::Live { generation: 2 },
            elapsed_ms: None,
            provider: Some("grok".to_string()),
            peer_session_id: Some("peer-1".to_string()),
            created_at_ms: 1,
            origin: SessionOrigin::local(),
            display_name: None,
            created_by: None,
            profile_id: None,
            context_id: None,
            unattended: UnattendedState::No,
            labels: Default::default(),
            resumable: false,
        }),
    };
    let value = serde_json::to_value(&resumed).expect("json");
    assert_eq!(value["type"], "resumed");
    assert_eq!(value["session"]["id"], "s.client.1");
    assert_eq!(value["session"]["peerSessionId"], "peer-1");
    let back: ResumeResult = serde_json::from_value(value).expect("round trip");
    assert!(matches!(back, ResumeResult::Resumed { .. }));

    let failed = ResumeResult::Failed {
        message: "no".to_string(),
    };
    let value = serde_json::to_value(&failed).expect("json");
    assert_eq!(value["type"], "failed");
    assert_eq!(value["message"], "no");
    let back: ResumeResult = serde_json::from_value(value).expect("round trip");
    assert!(matches!(back, ResumeResult::Failed { message } if message == "no"));
}

#[test]
fn agent_reported_round_trips_with_herdr_shaped_fields() {
    let event = SessionEvent::AgentReported {
        seq: 4,
        source: "devboule:stub".to_string(),
        agent: "stub".to_string(),
        state: AgentActivityState::Working,
        message: Some("turn started".to_string()),
        report_seq: Some(7),
        agent_session_id: Some("agent-session-1".to_string()),
        agent_session_path: Some(r"C:\tmp\session.json".to_string()),
        session_start_source: Some("startup".to_string()),
    };
    let encoded = serde_json::to_value(&event).expect("json");
    assert_eq!(encoded["type"], "agent_reported");
    assert_eq!(encoded["seq"], 4);
    assert_eq!(encoded["source"], "devboule:stub");
    assert_eq!(encoded["agent"], "stub");
    assert_eq!(encoded["state"], "working");
    assert_eq!(encoded["message"], "turn started");
    assert_eq!(encoded["reportSeq"], 7);
    assert_eq!(encoded["agentSessionId"], "agent-session-1");
    assert_eq!(encoded["agentSessionPath"], r"C:\tmp\session.json");
    assert_eq!(encoded["sessionStartSource"], "startup");
    let decoded: SessionEvent = serde_json::from_value(encoded).expect("event");
    assert_eq!(decoded, event);
    assert_eq!(
        serde_json::to_value(AgentActivityState::Idle).expect("json"),
        "idle"
    );
    assert_eq!(
        serde_json::to_value(AgentActivityState::Blocked).expect("json"),
        "blocked"
    );
    assert_eq!(
        serde_json::to_value(AgentActivityState::Unknown).expect("json"),
        "unknown"
    );
}

#[test]
fn claude_session_kind_is_the_wire_string_claude() {
    assert_eq!(
        serde_json::to_value(SessionKind::Claude).expect("json"),
        "claude"
    );
    let decoded: SessionKind = serde_json::from_str("\"claude\"").expect("kind");
    assert_eq!(decoded, SessionKind::Claude);
    assert!(SessionKind::Claude.is_agent());
    assert!(SessionKind::Acp.is_agent());
    assert!(SessionKind::Pi.is_agent());
    assert!(SessionKind::Codex.is_agent());
    assert!(!SessionKind::Terminal.is_agent());
}

#[test]
fn pi_session_kind_is_the_wire_string_pi() {
    assert_eq!(serde_json::to_value(SessionKind::Pi).expect("json"), "pi");
    let decoded: SessionKind = serde_json::from_str("\"pi\"").expect("kind");
    assert_eq!(decoded, SessionKind::Pi);
}

#[test]
fn codex_session_kind_is_the_wire_string_codex() {
    assert_eq!(
        serde_json::to_value(SessionKind::Codex).expect("json"),
        "codex"
    );
    let decoded: SessionKind = serde_json::from_str("\"codex\"").expect("kind");
    assert_eq!(decoded, SessionKind::Codex);
}

#[test]
fn tool_call_kind_and_locations_are_camel_case_and_optional() {
    let with = SessionEvent::AgentToolCall {
        tool_call_id: "toolu_1".to_string(),
        title: "Read src/lib.rs".to_string(),
        status: "pending".to_string(),
        kind: Some("read".to_string()),
        locations: Some(vec![ToolLocation {
            path: "src/lib.rs".to_string(),
            line: Some(12),
        }]),
        subagent_type: None,
        parent_tool_use_id: None,
        spawn_depth: None,
    };
    let encoded = serde_json::to_value(&with).expect("json");
    assert_eq!(encoded["type"], "agent_tool_call");
    assert_eq!(encoded["toolCallId"], "toolu_1");
    assert_eq!(encoded["kind"], "read");
    assert_eq!(encoded["locations"][0]["path"], "src/lib.rs");
    assert_eq!(encoded["locations"][0]["line"], 12);
    let decoded: SessionEvent = serde_json::from_value(encoded).expect("event");
    assert_eq!(decoded, with);

    let without = SessionEvent::AgentToolCall {
        tool_call_id: "t".to_string(),
        title: "x".to_string(),
        status: "pending".to_string(),
        kind: None,
        locations: None,
        subagent_type: None,
        parent_tool_use_id: None,
        spawn_depth: None,
    };
    let encoded = serde_json::to_value(&without).expect("json");
    assert!(encoded.get("kind").is_none());
    assert!(encoded.get("locations").is_none());
    let decoded: SessionEvent = serde_json::from_value(encoded).expect("event");
    assert_eq!(decoded, without);

    let update = SessionEvent::AgentToolUpdate {
        tool_call_id: "t".to_string(),
        status: Some("completed".to_string()),
        text: Some("ok".to_string()),
        title: None,
        kind: Some("edit".to_string()),
        locations: Some(vec![ToolLocation {
            path: "src/main.rs".to_string(),
            line: None,
        }]),
        parent_tool_use_id: None,
        spawn_depth: None,
    };
    let encoded = serde_json::to_value(&update).expect("json");
    assert_eq!(encoded["type"], "agent_tool_update");
    assert_eq!(encoded["kind"], "edit");
    assert_eq!(encoded["locations"][0]["path"], "src/main.rs");
    assert!(encoded["locations"][0].get("line").is_none());
}

/// `Session.displayName` and `Session.createdBy` are camelCase on the wire
/// and absent when the daemon has nothing to say (S5-09).
#[test]
fn session_display_name_and_created_by_are_camel_case_and_optional() {
    let session = Session {
        id: "s.parent.2".to_string(),
        workspace_id: None,
        cwd: None,
        kind: SessionKind::Acp,
        title: "Agent".to_string(),
        state: SessionState::Live { generation: 1 },
        elapsed_ms: None,
        provider: Some("claude".to_string()),
        peer_session_id: None,
        created_at_ms: 7,
        origin: SessionOrigin::local(),
        display_name: Some("worker".to_string()),
        created_by: Some("s.parent.1".to_string()),
        profile_id: Some("profile-1".to_string()),
        context_id: Some("s.root.1".to_string()),
        unattended: UnattendedState::Yes,
        labels: Default::default(),
        resumable: false,
    };
    let value = serde_json::to_value(&session).expect("json");
    assert_eq!(value["displayName"], "worker");
    assert_eq!(value["createdBy"], "s.parent.1");
    assert!(value.get("display_name").is_none());
    let back: Session = serde_json::from_value(value).expect("round trip");
    assert_eq!(back, session);

    // A human-started session (and every row written before the fields
    // existed) carries neither key rather than a null one.
    let unnamed = Session {
        display_name: None,
        created_by: None,
        profile_id: None,
        context_id: None,
        unattended: UnattendedState::No,
        labels: Default::default(),
        resumable: false,
        ..session
    };
    let value = serde_json::to_value(&unnamed).expect("json");
    assert!(value.get("displayName").is_none());
    assert!(value.get("createdBy").is_none());
    let legacy = serde_json::json!({
        "id": "s.client.1",
        "workspaceId": null,
        "kind": "terminal",
        "title": "Terminal",
        "state": { "type": "live", "generation": 1 },
        "createdAtMs": 1,
    });
    let decoded: Session = serde_json::from_value(legacy).expect("older row");
    assert_eq!(decoded.display_name, None);
    assert_eq!(decoded.created_by, None);
}

/// Both creation events, on the wire, with the fields the app reads.
#[test]
fn the_creation_events_carry_their_camel_case_facts() {
    let created = SessionEvent::AgentCreated {
        message_id: Some("m1".to_string()),
        child_session_id: "s.parent.2".to_string(),
        display_name: "worker".to_string(),
        provider: "claude".to_string(),
        profile: "worker".to_string(),
    };
    let value = serde_json::to_value(&created).expect("json");
    assert_eq!(value["type"], "agent_created");
    assert_eq!(value["messageId"], "m1");
    assert_eq!(value["childSessionId"], "s.parent.2");
    assert_eq!(value["displayName"], "worker");
    assert_eq!(value["provider"], "claude");
    assert_eq!(value["profile"], "worker");
    assert_eq!(
        serde_json::from_value::<SessionEvent>(value).expect("round trip"),
        created
    );

    // `state` is one of A2A's words, not the session-state tag: an app that
    // read `live` here would be reading a different vocabulary.
    let finished = SessionEvent::ChildFinished {
        message_id: Some("m2".to_string()),
        child_session_id: "s.parent.2".to_string(),
        display_name: "worker".to_string(),
        state: AgentTaskState::Canceled,
        note: None,
        artifacts: vec![FinishArtifact {
            artifact_id: "devboule-attachment:s.parent.2/abc".to_string(),
            parts: vec![FinishArtifactPart {
                url: "devboule-attachment:s.parent.2/abc".to_string(),
                mime_type: "text/markdown".to_string(),
                metadata: Some(FinishArtifactPartMetadata { stored_bytes: 42 }),
            }],
        }],
    };
    let value = serde_json::to_value(&finished).expect("json");
    assert_eq!(value["type"], "child_finished");
    assert_eq!(value["state"], "canceled");
    assert!(value.get("note").is_none(), "an absent note is absent");
    assert_eq!(
        value["artifacts"][0]["artifactId"],
        "devboule-attachment:s.parent.2/abc"
    );
    assert_eq!(
        value["artifacts"][0]["parts"][0]["mimeType"],
        "text/markdown"
    );
    assert_eq!(
        value["artifacts"][0]["parts"][0]["metadata"]["storedBytes"],
        42
    );
    assert_eq!(
        serde_json::from_value::<SessionEvent>(value).expect("round trip"),
        finished
    );

    // The whole vocabulary, spelled the way A2A spells it.
    for (state, word) in [
        (AgentTaskState::Submitted, "submitted"),
        (AgentTaskState::Working, "working"),
        (AgentTaskState::Completed, "completed"),
        (AgentTaskState::Failed, "failed"),
        (AgentTaskState::Canceled, "canceled"),
        (AgentTaskState::InputRequired, "input_required"),
        (AgentTaskState::Rejected, "rejected"),
    ] {
        assert_eq!(serde_json::to_value(state).expect("json"), word);
    }
}

/// An `agent_created` row written before journal v11 carries `preset`, not
/// `profile`, and the replay paths deserialize the payload in one tolerant
/// read — a row that fails to parse is dropped without a counter. The alias
/// is what keeps those rows hydrating instead of vanishing.
///
/// The value under the old spelling is the **preset** the child was created
/// under (`worker`, `design`), so the assertion pins the preset word into
/// `profile` exactly as stored: nothing translates or rewrites it.
#[test]
fn a_preset_shaped_agent_created_row_still_hydrates() {
    // The shape a pre-v11 daemon wrote, field for field.
    let legacy = serde_json::json!({
        "type": "agent_created",
        "messageId": "devboule-agent-created-1-128",
        "childSessionId": "s.process-30252.00000002",
        "displayName": "Poster",
        "provider": "claude",
        "preset": "design",
    });
    match serde_json::from_value::<SessionEvent>(legacy).expect("legacy row hydrates") {
        SessionEvent::AgentCreated {
            message_id,
            child_session_id,
            display_name,
            provider,
            profile,
        } => {
            assert_eq!(message_id.as_deref(), Some("devboule-agent-created-1-128"));
            assert_eq!(child_session_id, "s.process-30252.00000002");
            assert_eq!(display_name, "Poster");
            assert_eq!(provider, "claude");
            assert_eq!(
                profile, "design",
                "the preset value is kept as it was written, not translated"
            );
        }
        other => panic!("expected AgentCreated, got {other:?}"),
    }

    // The current spelling is unaffected: a new row serializes `profile`
    // and round-trips without the alias being consulted.
    let current = SessionEvent::AgentCreated {
        message_id: Some("m1".to_string()),
        child_session_id: "s.parent.2".to_string(),
        display_name: "worker".to_string(),
        provider: "claude".to_string(),
        profile: "reviewer".to_string(),
    };
    let value = serde_json::to_value(&current).expect("json");
    assert!(value.get("preset").is_none(), "new rows write `profile`");
    assert_eq!(value["profile"], "reviewer");
    assert_eq!(
        serde_json::from_value::<SessionEvent>(value).expect("round trip"),
        current
    );
}

/// The session-state → A2A word mapping, including the two that are easy to
/// get wrong: `Recovered` is not `completed`, and a live session with no
/// turn is `submitted` and not `working`.
#[test]
fn session_state_maps_to_the_a2a_words() {
    let live = SessionState::Live { generation: 1 };
    assert_eq!(live.task_state(true), AgentTaskState::Working);
    assert_eq!(live.task_state(false), AgentTaskState::Submitted);
    assert_eq!(
        SessionState::Silent { generation: 1 }.task_state(true),
        AgentTaskState::Working
    );
    let integrity = TranscriptIntegrity::Complete;
    assert_eq!(
        SessionState::Ended {
            generation: 1,
            code: Some(0),
            integrity,
        }
        .task_state(false),
        AgentTaskState::Completed
    );
    assert_eq!(
        SessionState::Ended {
            generation: 1,
            code: Some(2),
            integrity,
        }
        .task_state(false),
        AgentTaskState::Failed
    );
    // No status at all: the process is gone and nothing said it succeeded.
    assert_eq!(
        SessionState::Ended {
            generation: 1,
            code: None,
            integrity,
        }
        .task_state(false),
        AgentTaskState::Failed
    );
    assert_eq!(
        SessionState::Recovered {
            generation: 1,
            integrity: TranscriptIntegrity::Unverifiable {
                dropped_frames: 0,
                dropped_bytes: 0,
                trimmed_bytes: 0,
            },
        }
        .task_state(true),
        AgentTaskState::Canceled,
        "a transcript whose daemon died did not complete anything"
    );
}

/// The frozen wire contract (slice-5b app-delegation report §6.3): the
/// marker crosses as `unattended: "yes" | "no" | "unknown"`, is a **key on
/// every write** (the collapsed `bool` it replaces skipped `false`, which
/// made absent and false the same value), and a frame without the key
/// reads as `unknown` — the daemon has not said — never as `no`, which
/// would claim a human is watching when the truth is that nobody knows.
#[test]
fn unattended_is_required_on_every_write_and_absent_reads_as_unknown() {
    let session = Session {
        id: "s.child.1".to_string(),
        workspace_id: None,
        cwd: None,
        kind: SessionKind::Claude,
        title: "Agent".to_string(),
        state: SessionState::Live { generation: 1 },
        elapsed_ms: None,
        provider: Some("claude".to_string()),
        peer_session_id: None,
        created_at_ms: 1,
        origin: SessionOrigin::local(),
        display_name: None,
        created_by: None,
        profile_id: None,
        context_id: None,
        unattended: UnattendedState::Yes,
        labels: Default::default(),
        resumable: false,
    };
    let value = serde_json::to_value(&session).expect("session json");
    assert_eq!(
        value["unattended"], "yes",
        "the wire words are the frozen contract's, and the key is present on a write"
    );

    // `no` is written too — a marker that only ever said `yes` would be
    // the assertion of the negative the contract forbids.
    let asking = Session {
        unattended: UnattendedState::No,
        ..session.clone()
    };
    let value = serde_json::to_value(&asking).expect("session json");
    assert_eq!(value["unattended"], "no", "the key is never skipped");

    let frame = serde_json::json!({
        "id": "s.old.1",
        "workspaceId": null,
        "kind": "terminal",
        "title": "Terminal",
        "state": { "type": "live", "generation": 1 },
        "createdAtMs": 1
    });
    let decoded = serde_json::from_value::<Session>(frame).expect("older frame");
    assert_eq!(
        decoded.unattended,
        UnattendedState::Unknown,
        "a row the daemon has not said anything about is unknown, never no"
    );
}

/// The type change the 4 → 5 protocol bump exists for (audit R2b-1 §7).
/// `unattended` crossed the wire as an optional JSON boolean and now
/// crosses as a lowercase string written on every frame. `#[serde(default)]`
/// covers an absent key — the test above — and has no effect on a key
/// present with the wrong type, so an old writer's `"unattended": true`
/// must **fail** to decode. That failure is why the handshake itself must
/// refuse a 4-speaking peer: a decode failure is terminal for the
/// connection, and reading it as a reconnect handoff is the silent churn
/// the bump removes. A bool-tolerant shim added in the future turns this
/// red — which is correct, because such a shim invalidates the premise
/// the bump documents.
#[test]
fn an_old_boolean_unattended_key_fails_to_decode() {
    let frame = serde_json::json!({
        "id": "s.bool.1",
        "workspaceId": null,
        "kind": "terminal",
        "title": "Terminal",
        "state": { "type": "live", "generation": 1 },
        "createdAtMs": 1,
        "unattended": true
    });
    assert!(
        serde_json::from_value::<Session>(frame).is_err(),
        "a JSON boolean is not a dialect this crate speaks"
    );

    // The same frame without the key still decodes, as `unknown` — the
    // absence `#[serde(default)]` is actually for.
    let frame = serde_json::json!({
        "id": "s.bool.1",
        "workspaceId": null,
        "kind": "terminal",
        "title": "Terminal",
        "state": { "type": "live", "generation": 1 },
        "createdAtMs": 1
    });
    let decoded = serde_json::from_value::<Session>(frame).expect("key-absent frame");
    assert_eq!(decoded.unattended, UnattendedState::Unknown);
}

/// The roster surface (audit R2b-1 §6.2): the pushed
/// [`SessionStateSnapshot`] carries the marker as a **key on every push**
/// — for all three answers — and a push without the key reads `unknown`.
/// The attributes on the two structs are symmetric by eye, but nothing
/// pinned the snapshot's half: a `skip_serializing_if` added to this
/// struct alone would have passed every `Session` test and silenced the
/// one row a push-only child arrives as.
#[test]
fn the_roster_snapshot_carries_the_marker_on_every_push() {
    let snapshot = SessionStateSnapshot {
        id: "s.push.1".to_string(),
        workspace_id: None,
        kind: SessionKind::Pi,
        title: "Agent".to_string(),
        state: SessionState::Live { generation: 1 },
        elapsed_ms: Some(5),
        attention: None,
        origin: SessionOrigin::local(),
        display_name: None,
        created_by: None,
        profile_id: None,
        context_id: None,
        unattended: UnattendedState::Yes,
        labels: Default::default(),
        delegation: None,

        activity: None,
    };
    for (state, word) in [
        (UnattendedState::Yes, "yes"),
        (UnattendedState::No, "no"),
        (UnattendedState::Unknown, "unknown"),
    ] {
        let value = serde_json::to_value(&SessionStateSnapshot {
            unattended: state,
            ..snapshot.clone()
        })
        .expect("snapshot json");
        assert_eq!(
            value["unattended"], word,
            "the key is present on every push, whatever the answer"
        );
    }

    // And an older push without the key reads `unknown`, never `no`.
    let frame = serde_json::json!({
        "id": "s.push.1",
        "workspaceId": null,
        "kind": "pi",
        "title": "Agent",
        "state": { "type": "live", "generation": 1 },
        "elapsedMs": 5
    });
    let decoded = serde_json::from_value::<SessionStateSnapshot>(frame).expect("older push");
    assert_eq!(
        decoded.unattended,
        UnattendedState::Unknown,
        "a push the daemon has not said anything about is unknown, never no"
    );
}
