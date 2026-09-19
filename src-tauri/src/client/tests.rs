//! Tests for the daemon client: attachment cursors, reconnection and request routing.

use super::*;
use devboule_daemon::{DaemonError, EventHandler, SessionStateHandler};
use devboule_protocol::DaemonStatusBody;
use devboule_protocol::{SessionKind, SessionState, SessionStateSnapshot};
use std::collections::HashSet;
use std::sync::atomic::AtomicUsize;

#[test]
fn stop_attach_asks_for_nothing_after_everything_when_generation_known() {
    let cursor = stop_tail_cursor(Some(3)).expect("a known generation carries a cursor");
    assert_eq!(cursor.generation, 3);
    assert_eq!(cursor.seq, u64::MAX);
}

#[test]
fn stop_attach_stays_cursorless_without_a_generation() {
    // No generation, no gate: inventing one would fail the daemon's
    // generation check at best, so the attach pays the full replay.
    assert_eq!(stop_tail_cursor(None), None);
}

#[test]
fn delivered_history_leaves_the_client_cursor_at_its_real_position() {
    let registry = AttachmentRegistry::default();
    let sink: AttachmentSink = Arc::new(|_| {});
    let subscription = registry.insert("s.1", None, sink);
    // Pre-attach history is history: a cross-generation replay delivers
    // its rows with their own generation on the envelope. Such an
    // envelope is a record of what happened, not a position in the
    // current stream, however large the seq it carries.
    let history = SessionEventEnvelope {
        session_id: "s.1".to_string(),
        generation: 1,
        transcript_seq: None,
        event: SessionEvent::AgentReported {
            seq: 100,
            source: "devboule:stub".to_string(),
            agent: "stub".to_string(),
            state: devboule_protocol::AgentActivityState::Working,
            message: None,
            report_seq: Some(1),
            agent_session_id: None,
            agent_session_path: None,
            session_start_source: None,
        },
    };
    let mut state = registry.state.lock().unwrap();
    let entry = state.entries.get_mut(&subscription).unwrap();
    entry.cursor = Some(Cursor {
        generation: 2,
        seq: 5,
    });
    advance_cursor(entry, &history);
    let cursor = entry.cursor.expect("cursor kept");
    assert_eq!(
        cursor.generation, 2,
        "history must not move the cursor to its own generation"
    );
    assert_eq!(
        cursor.seq, 5,
        "history must not move the cursor past the reader's real position"
    );
}

#[test]
fn generation_for_prefers_the_roster_over_any_entry_cursor() {
    let registry = AttachmentRegistry::default();
    let sink: AttachmentSink = Arc::new(|_| {});
    registry.insert("s.1", None, Arc::clone(&sink));
    registry.insert("s.1", None, sink);
    // Mid-replay a cursor can legitimately name an older generation:
    // history is restamped to its own generation and never advances
    // cursors. The roster is the daemon's word on the current one.
    {
        let mut state = registry.state.lock().unwrap();
        for entry in state.entries.values_mut() {
            if entry.session_id == "s.1" {
                entry.cursor = Some(Cursor {
                    generation: 1,
                    seq: 50,
                });
            }
        }
    }
    set_roster(
        &registry,
        vec![stop_test_snapshot(
            "s.1",
            SessionState::Live { generation: 2 },
        )],
    );
    assert_eq!(
        registry.generation_for("s.1"),
        Some(2),
        "a stale entry cursor must not outrank the roster's generation"
    );
}

fn bind_attachment(registry: &AttachmentRegistry, subscription_id: SubscriptionId) {
    registry
        .state
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .entries
        .get_mut(&subscription_id)
        .expect("inserted attachment is registered")
        .binding = Some(1);
}

fn set_roster(registry: &AttachmentRegistry, snapshots: Vec<SessionStateSnapshot>) {
    registry
        .state
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .roster = Some(
        snapshots
            .into_iter()
            .map(|snapshot| (snapshot.id.clone(), snapshot))
            .collect(),
    );
}

fn stop_test_snapshot(id: &str, state: SessionState) -> SessionStateSnapshot {
    SessionStateSnapshot {
        id: id.to_string(),
        workspace_id: None,
        kind: SessionKind::Terminal,
        title: id.to_string(),
        state,
        elapsed_ms: None,
        attention: None,
        origin: devboule_protocol::SessionOrigin::local(),
        display_name: None,
        created_by: None,
        profile_id: None,
        context_id: None,
        unattended: devboule_protocol::UnattendedState::Unknown,
        labels: std::collections::BTreeMap::new(),
        delegation: None,
    }
}

#[test]
fn stop_reuse_prefers_the_newest_bound_attachment() {
    let registry = AttachmentRegistry::default();
    let sink: AttachmentSink = Arc::new(|_| {});
    let first = registry.insert("s.1", None, Arc::clone(&sink));
    let second = registry.insert("s.1", None, Arc::clone(&sink));
    registry.insert("s.2", None, sink);
    // Nothing bound yet: entries a deferred reattach left behind cannot
    // serve the daemon's observer check, whatever their age.
    assert_eq!(registry.bound_subscription_for_session("s.1"), None);
    bind_attachment(&registry, first);
    assert_eq!(registry.bound_subscription_for_session("s.1"), Some(first));
    bind_attachment(&registry, second);
    assert_eq!(registry.bound_subscription_for_session("s.1"), Some(second));
    assert_eq!(registry.bound_subscription_for_session("s.2"), None);
    assert_eq!(registry.bound_subscription_for_session("s.9"), None);
}

#[test]
fn stop_bind_race_against_a_terminal_roster_is_already_stopped() {
    let registry = AttachmentRegistry::default();
    set_roster(
        &registry,
        vec![stop_test_snapshot(
            "s.1",
            SessionState::Ended {
                generation: 1,
                code: Some(0),
                integrity: devboule_protocol::TranscriptIntegrity::Complete,
            },
        )],
    );
    // Terminal in the roster, or gone from it: the process is already
    // dead, so the stop's postcondition holds.
    assert!(BridgeInner::stop_already_achieved(&registry, "s.1"));
    assert!(BridgeInner::stop_already_achieved(&registry, "s.gone"));
}

#[test]
fn stop_bind_race_without_evidence_stays_an_error() {
    let registry = AttachmentRegistry::default();
    // No roster yet is no evidence — never a guess.
    assert!(!BridgeInner::stop_already_achieved(&registry, "s.1"));
    set_roster(
        &registry,
        vec![stop_test_snapshot(
            "s.1",
            SessionState::Live { generation: 1 },
        )],
    );
    assert!(!BridgeInner::stop_already_achieved(&registry, "s.1"));
}

#[derive(Default)]
struct FakeAttachmentClient {
    calls: Mutex<Vec<(String, Option<devboule_protocol::Cursor>)>>,
    handlers: Mutex<HashMap<String, Vec<EventHandler>>>,
    failures: Mutex<HashSet<String>>,
    in_flight: AtomicUsize,
    max_in_flight: AtomicUsize,
}

impl FakeAttachmentClient {
    fn emit(&self, session_id: &str, envelope: devboule_protocol::SessionEventEnvelope) {
        let handlers = self
            .handlers
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .get(session_id)
            .cloned()
            .unwrap_or_default();
        for handler in handlers {
            handler(envelope.clone());
        }
    }

    fn emit_stale(&self, session_id: &str, envelope: devboule_protocol::SessionEventEnvelope) {
        if let Some(handler) = self
            .handlers
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .get(session_id)
            .and_then(|handlers| handlers.first().cloned())
        {
            handler(envelope);
        }
    }

    fn fail_for(&self, session_id: &str) {
        self.failures
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .insert(session_id.to_string());
    }

    fn allow_for(&self, session_id: &str) {
        self.failures
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .remove(session_id);
    }
}

impl SessionAttachmentClient for FakeAttachmentClient {
    fn session_attach(
        &self,
        subscription_id: SubscriptionId,
        session_id: &str,
        from_cursor: Option<devboule_protocol::Cursor>,
        handler: EventHandler,
    ) -> Result<SubscriptionId, DaemonError> {
        self.calls
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .push((session_id.to_string(), from_cursor));
        let active = self.in_flight.fetch_add(1, Ordering::SeqCst) + 1;
        self.max_in_flight.fetch_max(active, Ordering::SeqCst);
        let failed = self
            .failures
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .contains(session_id);
        self.in_flight.fetch_sub(1, Ordering::SeqCst);
        if failed {
            return Err(DaemonError::Protocol("fake attach failed".to_string()));
        }
        self.handlers
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .entry(session_id.to_string())
            .or_default()
            .push(handler);
        Ok(subscription_id)
    }
}

fn agent_envelope(
    session_id: &str,
    generation: u64,
    transcript_seq: Option<u64>,
    text: &str,
) -> devboule_protocol::SessionEventEnvelope {
    devboule_protocol::SessionEventEnvelope {
        session_id: session_id.to_string(),
        generation,
        transcript_seq,
        event: devboule_protocol::SessionEvent::AgentMessage {
            message_id: None,
            text: text.to_string(),
            parent_tool_use_id: None,
            spawn_depth: None,
        },
    }
}

fn output_envelope(
    session_id: &str,
    generation: u64,
    transcript_seq: Option<u64>,
    seq: u64,
    data: &str,
) -> devboule_protocol::SessionEventEnvelope {
    devboule_protocol::SessionEventEnvelope {
        session_id: session_id.to_string(),
        generation,
        transcript_seq,
        event: devboule_protocol::SessionEvent::Output {
            seq,
            data: data.to_string(),
        },
    }
}

#[test]
fn derived_rows_sharing_one_envelope_seq_all_reach_the_sink() {
    let registry = Arc::new(AttachmentRegistry::default());
    let received = Arc::new(Mutex::new(Vec::new()));
    let received_by_sink = Arc::clone(&received);
    let subscription_id = registry.insert(
        "session-shared-seq",
        None,
        Arc::new(move |event| {
            received_by_sink
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .push(event);
        }),
    );
    let client = FakeAttachmentClient::default();
    registry.bind(&client, subscription_id).expect("attach");

    client.emit(
        "session-shared-seq",
        agent_envelope("session-shared-seq", 1, Some(17), "thinking"),
    );
    client.emit(
        "session-shared-seq",
        agent_envelope("session-shared-seq", 1, Some(17), "answer"),
    );

    assert_eq!(
        received
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .len(),
        2,
        "one journal envelope may produce multiple rows"
    );
    let cursor = registry
        .state
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .entries
        .get(&subscription_id)
        .and_then(|entry| entry.cursor);
    assert_eq!(
        cursor,
        Some(Cursor {
            generation: 1,
            seq: 17,
        })
    );
}

#[test]
fn reattach_redelivers_every_row_from_the_boundary_envelope() {
    let registry = Arc::new(AttachmentRegistry::default());
    let received = Arc::new(Mutex::new(Vec::new()));
    let received_by_sink = Arc::clone(&received);
    let subscription_id = registry.insert(
        "session-boundary",
        None,
        Arc::new(move |event| {
            received_by_sink
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .push(event);
        }),
    );
    let old_client = FakeAttachmentClient::default();
    let new_client = FakeAttachmentClient::default();
    registry
        .bind(&old_client, subscription_id)
        .expect("initial attach");

    for text in ["thinking", "answer"] {
        old_client.emit(
            "session-boundary",
            agent_envelope("session-boundary", 1, Some(17), text),
        );
    }

    registry.begin_replacement();
    registry.reattach_all(&new_client);

    assert_eq!(
        new_client
            .calls
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .as_slice(),
        &[(
            "session-boundary".to_string(),
            Some(devboule_protocol::Cursor {
                generation: 1,
                seq: 16,
            }),
        )],
        "reattach must back off one envelope so a boundary envelope is replayed whole"
    );

    for text in ["thinking", "answer"] {
        new_client.emit(
            "session-boundary",
            agent_envelope("session-boundary", 1, Some(17), text),
        );
    }

    let texts = received
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .iter()
        .filter_map(|event| match event {
            devboule_protocol::SessionEvent::AgentMessage { text, .. } => Some(text.clone()),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(texts, ["thinking", "answer", "thinking", "answer"]);
}

#[test]
fn live_terminal_output_cursor_survives_connection_replacement() {
    let registry = Arc::new(AttachmentRegistry::default());
    let old_client = FakeAttachmentClient::default();
    let new_client = FakeAttachmentClient::default();
    let subscription_id = registry.insert("session-terminal", None, Arc::new(|_| {}));
    registry
        .bind(&old_client, subscription_id)
        .expect("initial attach");

    old_client.emit(
        "session-terminal",
        output_envelope("session-terminal", 4, Some(17), 17, "before replacement"),
    );

    registry.begin_replacement();
    registry.reattach_all(&new_client);

    assert_eq!(
        new_client
            .calls
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .as_slice(),
        &[(
            "session-terminal".to_string(),
            Some(devboule_protocol::Cursor {
                generation: 4,
                seq: 16,
            }),
        )],
        "terminal reattach must ask only for output after the live cursor"
    );
}

#[test]
fn live_chat_cursor_survives_connection_replacement() {
    let registry = Arc::new(AttachmentRegistry::default());
    let old_client = FakeAttachmentClient::default();
    let new_client = FakeAttachmentClient::default();
    let subscription_id = registry.insert("session-chat", None, Arc::new(|_| {}));
    registry
        .bind(&old_client, subscription_id)
        .expect("initial attach");

    old_client.emit(
        "session-chat",
        agent_envelope("session-chat", 4, Some(17), "live chat"),
    );

    registry.begin_replacement();
    registry.reattach_all(&new_client);

    assert_eq!(
        new_client
            .calls
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .as_slice(),
        &[(
            "session-chat".to_string(),
            Some(devboule_protocol::Cursor {
                generation: 4,
                seq: 16,
            }),
        )],
        "reattach must send the cursor advanced by live chat"
    );
}

#[test]
fn unpositioned_envelope_is_forwarded_without_advancing() {
    let registry = Arc::new(AttachmentRegistry::default());
    let received = Arc::new(Mutex::new(Vec::new()));
    let received_by_sink = Arc::clone(&received);
    let subscription_id = registry.insert(
        "session-marker",
        None,
        Arc::new(move |event| {
            received_by_sink
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .push(event);
        }),
    );
    let client = FakeAttachmentClient::default();
    registry.bind(&client, subscription_id).expect("attach");

    client.emit(
        "session-marker",
        agent_envelope("session-marker", 1, None, "marker"),
    );

    assert_eq!(
        received
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .len(),
        1,
        "an unpositioned envelope must still reach the sink"
    );
    let cursor = registry
        .state
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .entries
        .get(&subscription_id)
        .and_then(|entry| entry.cursor);
    assert_eq!(cursor, None, "an absent position must not create progress");
}

#[test]
fn attached_session_reattaches_with_the_cursor_received_before_replacement() {
    let registry = Arc::new(AttachmentRegistry::default());
    let received = Arc::new(Mutex::new(Vec::new()));
    let received_by_handler = Arc::clone(&received);
    let sink: AttachmentSink = Arc::new(move |event| {
        received_by_handler
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .push(event);
    });
    let old_client = FakeAttachmentClient::default();
    let new_client = FakeAttachmentClient::default();

    let subscription_id = registry.insert("session-1", None, sink);
    registry
        .bind(&old_client, subscription_id)
        .expect("initial attach");
    old_client.emit(
        "session-1",
        devboule_protocol::SessionEventEnvelope {
            session_id: "session-1".to_string(),
            generation: 4,
            transcript_seq: Some(17),
            event: devboule_protocol::SessionEvent::Output {
                seq: 17,
                data: "before replacement".to_string(),
            },
        },
    );

    registry.begin_replacement();
    old_client.emit_stale(
        "session-1",
        devboule_protocol::SessionEventEnvelope {
            session_id: "session-1".to_string(),
            generation: 0,
            transcript_seq: None,
            event: devboule_protocol::SessionEvent::Exit { code: None },
        },
    );
    assert_eq!(
        received
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .len(),
        1,
        "connection-loss exit from the old client must not reach the tab"
    );
    registry.reattach_all(&new_client);

    assert_eq!(
        new_client
            .calls
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .as_slice(),
        &[(
            "session-1".to_string(),
            Some(devboule_protocol::Cursor {
                generation: 4,
                seq: 16,
            }),
        )]
    );
    new_client.emit(
        "session-1",
        devboule_protocol::SessionEventEnvelope {
            session_id: "session-1".to_string(),
            generation: 4,
            transcript_seq: Some(18),
            event: devboule_protocol::SessionEvent::Output {
                seq: 18,
                data: "after replacement".to_string(),
            },
        },
    );
    old_client.emit_stale(
        "session-1",
        devboule_protocol::SessionEventEnvelope {
            session_id: "session-1".to_string(),
            generation: 4,
            transcript_seq: Some(99),
            event: devboule_protocol::SessionEvent::Output {
                seq: 99,
                data: "late old-client event".to_string(),
            },
        },
    );
    assert!(received
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .iter()
        .any(|event| matches!(
            event,
            devboule_protocol::SessionEvent::Output { seq: 18, .. }
        )));
    assert!(!received
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .iter()
        .any(|event| matches!(
            event,
            devboule_protocol::SessionEvent::Output { seq: 99, .. }
        )));
}

#[test]
fn attachment_registry_keeps_same_session_subscriptions_independent() {
    let registry = Arc::new(AttachmentRegistry::default());
    let first_events = Arc::new(Mutex::new(Vec::<SessionEvent>::new()));
    let second_events = Arc::new(Mutex::new(Vec::<SessionEvent>::new()));
    let first_sink_events = Arc::clone(&first_events);
    let second_sink_events = Arc::clone(&second_events);
    let first = registry.insert(
        "shared",
        None,
        Arc::new(move |event| {
            first_sink_events
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .push(event);
        }),
    );
    let second = registry.insert(
        "shared",
        None,
        Arc::new(move |event| {
            second_sink_events
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .push(event);
        }),
    );
    let client = FakeAttachmentClient::default();
    registry.bind(&client, first).expect("first attach");
    registry.bind(&client, second).expect("second attach");

    let event = devboule_protocol::SessionEventEnvelope {
        session_id: "shared".to_string(),
        generation: 1,
        transcript_seq: None,
        event: SessionEvent::AgentMessage {
            message_id: None,
            text: "shared event".to_string(),
            parent_tool_use_id: None,
            spawn_depth: None,
        },
    };
    client.emit("shared", event.clone());
    assert_eq!(first_events.lock().unwrap().len(), 1);
    assert_eq!(second_events.lock().unwrap().len(), 1);

    registry.remove(first);
    assert_eq!(registry.len(), 1);
    assert_eq!(registry.session_id_for(second).as_deref(), Some("shared"));
    client.emit("shared", event);
    assert_eq!(first_events.lock().unwrap().len(), 1);
    assert_eq!(second_events.lock().unwrap().len(), 2);
}

#[test]
fn generation_bump_discards_the_old_sequence_but_keeps_the_session_binding() {
    let registry = Arc::new(AttachmentRegistry::default());
    let sink: AttachmentSink = Arc::new(|_| {});
    let old_client = FakeAttachmentClient::default();
    let new_client = FakeAttachmentClient::default();
    let subscription_id = registry.insert("session-2", None, sink);
    registry
        .bind(&old_client, subscription_id)
        .expect("initial attach");
    old_client.emit(
        "session-2",
        devboule_protocol::SessionEventEnvelope {
            session_id: "session-2".to_string(),
            generation: 4,
            transcript_seq: Some(17),
            event: devboule_protocol::SessionEvent::Output {
                seq: 17,
                data: "old generation".to_string(),
            },
        },
    );
    registry.begin_replacement();
    registry.observe_roster(&[SessionStateSnapshot {
        id: "session-2".to_string(),
        workspace_id: None,
        kind: SessionKind::Acp,
        title: "agent".to_string(),
        state: SessionState::Live { generation: 5 },
        elapsed_ms: None,
        attention: None,
        origin: devboule_protocol::SessionOrigin::local(),
        display_name: None,
        created_by: None,
        profile_id: None,
        context_id: None,
        unattended: devboule_protocol::UnattendedState::No,
        labels: Default::default(),
        delegation: None,
    }]);
    registry.reattach_all(&new_client);

    assert_eq!(
        new_client
            .calls
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .first()
            .and_then(|(_, cursor)| *cursor),
        Some(devboule_protocol::Cursor {
            generation: 5,
            seq: 0,
        })
    );
    assert!(registry.is_bound("session-2"));
}

#[test]
fn ended_while_disconnected_is_delivered_as_ended_without_an_attach() {
    let registry = Arc::new(AttachmentRegistry::default());
    let received = Arc::new(Mutex::new(Vec::new()));
    let received_by_handler = Arc::clone(&received);
    let sink: AttachmentSink = Arc::new(move |event| {
        received_by_handler
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .push(event);
    });
    let old_client = FakeAttachmentClient::default();
    let new_client = FakeAttachmentClient::default();
    let subscription_id = registry.insert("ended", None, sink);
    registry
        .bind(&old_client, subscription_id)
        .expect("initial attach");
    registry.begin_replacement();
    registry.observe_roster(&[SessionStateSnapshot {
        id: "ended".to_string(),
        workspace_id: None,
        kind: SessionKind::Terminal,
        title: "ended".to_string(),
        state: SessionState::Ended {
            generation: 4,
            code: Some(23),
            integrity: devboule_protocol::TranscriptIntegrity::Complete,
        },
        elapsed_ms: None,
        attention: None,
        origin: devboule_protocol::SessionOrigin::local(),
        display_name: None,
        created_by: None,
        profile_id: None,
        context_id: None,
        unattended: devboule_protocol::UnattendedState::No,
        labels: Default::default(),
        delegation: None,
    }]);
    registry.reattach_all(&new_client);

    assert!(new_client
        .calls
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .is_empty());
    assert!(received
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .iter()
        .any(|event| matches!(
            event,
            devboule_protocol::SessionEvent::Exit { code: Some(23) }
        )));
    assert_eq!(registry.len(), 0);
}

#[test]
fn a_failed_reattach_does_not_abort_the_remaining_tabs() {
    let registry = Arc::new(AttachmentRegistry::default());
    let received = Arc::new(Mutex::new(Vec::new()));
    let received_by_handler = Arc::clone(&received);
    let sink: AttachmentSink = Arc::new(move |event| {
        received_by_handler
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .push(event);
    });
    let second_sink = Arc::clone(&sink);
    let client = FakeAttachmentClient::default();
    client.fail_for("bad");
    registry.insert("bad", None, Arc::clone(&sink));
    registry.insert("good", None, second_sink);
    registry.begin_replacement();
    let failures = registry.reattach_all(&client);

    assert_eq!(failures.len(), 1);
    assert!(!registry.is_bound("bad"));
    assert!(registry.is_bound("good"));
    client.emit(
        "good",
        devboule_protocol::SessionEventEnvelope {
            session_id: "good".to_string(),
            generation: 1,
            transcript_seq: None,
            event: devboule_protocol::SessionEvent::AgentMessage {
                message_id: None,
                text: "still live".to_string(),
                parent_tool_use_id: None,
                spawn_depth: None,
            },
        },
    );
    assert!(received
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .iter()
            .any(|event| matches!(event, devboule_protocol::SessionEvent::AgentMessage { text, .. } if text == "still live")));
}

#[test]
fn a_failed_reattach_can_be_retried_for_a_later_user_action() {
    let registry = Arc::new(AttachmentRegistry::default());
    let received = Arc::new(Mutex::new(Vec::new()));
    let received_by_handler = Arc::clone(&received);
    let sink: AttachmentSink = Arc::new(move |event| {
        received_by_handler
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .push(event);
    });
    let client = FakeAttachmentClient::default();
    client.fail_for("retry");
    let subscription_id = registry.insert("retry", None, sink);
    registry.begin_replacement();
    assert_eq!(registry.reattach_all(&client).len(), 1);
    assert!(!registry.is_bound("retry"));

    client.allow_for("retry");
    registry
        .retry_one(&client, subscription_id)
        .expect("the later action retries the attachment");
    assert!(registry.is_bound("retry"));
    client.emit(
        "retry",
        devboule_protocol::SessionEventEnvelope {
            session_id: "retry".to_string(),
            generation: 1,
            transcript_seq: None,
            event: devboule_protocol::SessionEvent::AgentMessage {
                message_id: None,
                text: "recovered".to_string(),
                parent_tool_use_id: None,
                spawn_depth: None,
            },
        },
    );
    assert!(received
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .iter()
            .any(|event| matches!(event, devboule_protocol::SessionEvent::AgentMessage { text, .. } if text == "recovered")));
}

#[test]
fn reattach_worker_allows_only_one_attach_in_flight() {
    let registry = Arc::new(AttachmentRegistry::default());
    let client = FakeAttachmentClient::default();
    for index in 0..8 {
        registry.insert(&format!("session-{index}"), None, Arc::new(|_| {}));
    }
    registry.begin_replacement();
    registry.reattach_all(&client);

    assert_eq!(client.max_in_flight.load(Ordering::SeqCst), 1);
}

#[test]
fn closed_session_events_remove_the_registry_entry() {
    let registry = Arc::new(AttachmentRegistry::default());
    let client = FakeAttachmentClient::default();
    let subscription_id = registry.insert("closed", None, Arc::new(|_| {}));
    registry.bind(&client, subscription_id).expect("attach");
    client.emit(
        "closed",
        devboule_protocol::SessionEventEnvelope {
            session_id: "closed".to_string(),
            generation: 1,
            transcript_seq: None,
            event: devboule_protocol::SessionEvent::Exit { code: Some(0) },
        },
    );

    assert_eq!(registry.len(), 0);
}

#[test]
fn removing_one_session_subscription_keeps_the_other() {
    let registry = Arc::new(AttachmentRegistry::default());
    let first = registry.insert("shared", None, Arc::new(|_| {}));
    let second = registry.insert("shared", None, Arc::new(|_| {}));

    registry.remove(first);

    assert_eq!(registry.len(), 1);
    assert_eq!(registry.session_id_for(second).as_deref(), Some("shared"));
    assert_eq!(registry.session_id_for(first), None);
}

#[test]
fn forgetting_a_session_drops_only_that_sessions_attachments() {
    let registry = Arc::new(AttachmentRegistry::default());
    let first = registry.insert("shared", None, Arc::new(|_| {}));
    let second = registry.insert("shared", None, Arc::new(|_| {}));
    let other = registry.insert("kept", None, Arc::new(|_| {}));

    assert_eq!(
        registry.subscriptions_for_session("shared"),
        vec![first, second]
    );

    registry.forget_session("shared");

    assert_eq!(
        registry.subscriptions_for_session("shared"),
        Vec::<SubscriptionId>::new()
    );
    assert_eq!(registry.len(), 1);
    assert_eq!(registry.session_id_for(other).as_deref(), Some("kept"));
}

#[test]
fn retrying_an_unknown_subscription_is_rejected() {
    let registry = Arc::new(AttachmentRegistry::default());
    let client = FakeAttachmentClient::default();

    let error = registry
        .retry_one(&client, 41)
        .expect_err("unknown subscriptions must not pass command validation");
    match error {
        DaemonError::Protocol(message) => {
            assert_eq!(message, "session attachment is not registered")
        }
        other => panic!("unexpected error: {other:?}"),
    }
}

#[derive(Default)]
struct FakeRosterClient {
    handler: Mutex<Option<SessionStateHandler>>,
}

impl FakeRosterClient {
    fn emit(&self, snapshot: SessionStateSnapshot) {
        if let Some(handler) = self
            .handler
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .as_ref()
        {
            handler(vec![snapshot]);
        }
    }
}

impl SessionWatchClient for FakeRosterClient {
    fn sessions_watch(&self, handler: SessionStateHandler) -> Result<(), DaemonError> {
        *self
            .handler
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = Some(handler);
        Ok(())
    }

    fn sessions_unwatch(&self) -> Result<(), DaemonError> {
        self.handler
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .take();
        Ok(())
    }
}

fn roster_snapshot(id: &str) -> SessionStateSnapshot {
    SessionStateSnapshot {
        id: id.to_string(),
        workspace_id: None,
        kind: SessionKind::Terminal,
        title: "new daemon".to_string(),
        state: SessionState::Live { generation: 1 },
        elapsed_ms: Some(1),
        attention: None,
        origin: devboule_protocol::SessionOrigin::local(),
        display_name: None,
        created_by: None,
        profile_id: None,
        context_id: None,
        unattended: devboule_protocol::UnattendedState::No,
        labels: Default::default(),
        delegation: None,
    }
}

#[test]
fn roster_update_reaches_a_subscription_after_client_replacement() {
    let subscription = Arc::new(RosterSubscription::default());
    let received = Arc::new(Mutex::new(Vec::new()));
    let received_by_handler = Arc::clone(&received);
    let handler: SessionStateHandler = Arc::new(move |snapshots| {
        received_by_handler
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .extend(snapshots);
    });
    let old_client = FakeRosterClient::default();
    let new_client = FakeRosterClient::default();

    subscription
        .watch(Some(&old_client), handler)
        .expect("watch old client");
    subscription
        .rebind(&new_client)
        .expect("rebind replacement client");
    new_client.emit(roster_snapshot("new-daemon-session"));

    assert_eq!(
        received
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .iter()
            .map(|snapshot| snapshot.id.as_str())
            .collect::<Vec<_>>(),
        vec!["new-daemon-session"]
    );
}

#[test]
fn roster_subscription_registered_while_disconnected_binds_later() {
    let subscription = Arc::new(RosterSubscription::default());
    let received = Arc::new(Mutex::new(Vec::new()));
    let received_by_handler = Arc::clone(&received);
    let handler: SessionStateHandler = Arc::new(move |snapshots| {
        received_by_handler
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .extend(snapshots);
    });
    let new_client = FakeRosterClient::default();

    subscription
        .watch::<FakeRosterClient>(None, handler)
        .expect("save watch");
    subscription.rebind(&new_client).expect("bind new client");
    new_client.emit(roster_snapshot("connected-later"));

    assert_eq!(
        received
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .iter()
            .map(|snapshot| snapshot.id.as_str())
            .collect::<Vec<_>>(),
        vec!["connected-later"]
    );
}

#[test]
fn roster_unwatch_stays_unsubscribed_across_client_replacement() {
    let subscription = Arc::new(RosterSubscription::default());
    let received = Arc::new(Mutex::new(Vec::new()));
    let received_by_handler = Arc::clone(&received);
    let handler: SessionStateHandler = Arc::new(move |snapshots| {
        received_by_handler
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .extend(snapshots);
    });
    let old_client = FakeRosterClient::default();
    let new_client = FakeRosterClient::default();

    subscription
        .watch(Some(&old_client), handler)
        .expect("watch old client");
    subscription
        .unwatch(Some(&old_client))
        .expect("unwatch old client");
    subscription
        .rebind(&new_client)
        .expect("rebind replacement client");
    new_client.emit(roster_snapshot("must-not-arrive"));

    assert!(received
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .is_empty());
}

#[test]
fn an_old_roster_client_cannot_satisfy_the_new_client_snapshot_barrier() {
    let subscription = Arc::new(RosterSubscription::default());
    let received = Arc::new(Mutex::new(Vec::new()));
    let received_by_handler = Arc::clone(&received);
    let handler: SessionStateHandler = Arc::new(move |snapshots| {
        received_by_handler
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .extend(snapshots);
    });
    let old_client = FakeRosterClient::default();
    let new_client = FakeRosterClient::default();
    subscription
        .watch(Some(&old_client), handler)
        .expect("watch old client");
    let requested_epoch = subscription
        .begin_rebind()
        .expect("a desired roster watch exists");
    subscription.rebind(&new_client).expect("rebind new client");

    old_client.emit(roster_snapshot("old-daemon"));
    assert!(received
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .is_empty());
    assert_eq!(
        *subscription
            .snapshot_epoch
            .lock()
            .unwrap_or_else(|error| error.into_inner()),
        requested_epoch,
        "the old callback must not advance the snapshot barrier"
    );

    new_client.emit(roster_snapshot("new-daemon"));
    assert!(subscription.wait_for_snapshot(requested_epoch));
    assert_eq!(
        received
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .iter()
            .map(|snapshot| snapshot.id.as_str())
            .collect::<Vec<_>>(),
        vec!["new-daemon"]
    );
}

struct TimeoutStatusSource {
    calls: AtomicUsize,
}

impl StatusSource for TimeoutStatusSource {
    fn status(&self) -> Result<DaemonStatusBody, devboule_daemon::DaemonError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Err(devboule_daemon::DaemonError::timed_out("fake status"))
    }
}

struct LostStatusSource {
    calls: AtomicUsize,
}

impl StatusSource for LostStatusSource {
    fn status(&self) -> Result<DaemonStatusBody, devboule_daemon::DaemonError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Err(devboule_daemon::DaemonError::ConnectionLost)
    }
}

#[test]
fn supervisor_reconnects_after_a_connected_loop_reports_connection_loss() {
    let stop = AtomicBool::new(false);
    let mut connect_attempts = 0;
    let mut connected_runs = 0;
    let outcome = run_supervisor_loop(
        &stop,
        || {
            connect_attempts += 1;
            Ok(connect_attempts)
        },
        |_| {
            connected_runs += 1;
            if connected_runs == 1 {
                StatusLoopExit::ConnectionLost
            } else {
                StatusLoopExit::Stopped
            }
        },
        |_: Duration, _| true,
        Instant::now,
    );

    assert_eq!(outcome, SupervisorLoopExit::Stopped);
    assert_eq!(connect_attempts, 2);
    assert_eq!(connected_runs, 2);
}

#[test]
fn retry_status_message_joins_a_complete_cause_sentence() {
    let cause = "Devboule daemon not found. Set DEVBOULE_DAEMON or install devboule-daemon.exe beside the app.";

    assert_eq!(
            retry_status_message(Duration::from_secs(60), Some(cause)).as_deref(),
            Some("Devboule daemon not found. Set DEVBOULE_DAEMON or install devboule-daemon.exe beside the app. Retrying in 60s")
        );
    assert_eq!(
        retry_status_message(Duration::from_secs(60), None).as_deref(),
        Some("the daemon keeps stopping right after starting; retrying in 60s")
    );
    assert_eq!(retry_status_message(PING_PERIOD, Some(cause)), None);
}

#[test]
fn a_failed_connect_passes_its_cause_to_backoff_sleep() {
    let stop = AtomicBool::new(false);
    let cause = "distinctive connect failure";
    let mut delays = Vec::new();
    let mut received_cause = None;
    let outcome = run_supervisor_loop(
        &stop,
        || Err::<(), _>(cause.to_string()),
        |_| StatusLoopExit::Stopped,
        |delay, error| {
            if delay > PING_PERIOD {
                received_cause = error.map(|error| error.to_string());
            }
            delays.push(delay);
            delays.len() < 5
        },
        Instant::now,
    );

    assert_eq!(outcome, SupervisorLoopExit::Stopped);
    assert_eq!(received_cause.as_deref(), Some(cause));
}

#[test]
fn an_immediate_connected_loss_passes_no_cause_to_backoff_sleep() {
    let stop = AtomicBool::new(false);
    let mut delays = Vec::new();
    let mut received_cause = None;
    let outcome = run_supervisor_loop(
        &stop,
        || Ok::<(), String>(()),
        |_| StatusLoopExit::ConnectionLost,
        |delay, error| {
            if delay > PING_PERIOD {
                received_cause = error.map(|error| error.to_string());
            }
            delays.push(delay);
            delays.len() < 5
        },
        Instant::now,
    );

    assert_eq!(outcome, SupervisorLoopExit::Stopped);
    assert!(received_cause.is_none());
}

/// A clock the test advances by a whole number of seconds per call, so
/// "served time" is deterministic and nothing sleeps in real time.
struct FakeClock {
    step_seconds: std::cell::Cell<u64>,
    elapsed_seconds: std::cell::Cell<u64>,
}

impl FakeClock {
    fn now(&self) -> Instant {
        self.elapsed_seconds
            .set(self.elapsed_seconds.get() + self.step_seconds.get());
        // A real instant carries a fake offset: two reads differ by
        // exactly the steps taken, which is all the loop can see, and
        // nothing sleeps in real time.
        Instant::now() + Duration::from_secs(self.elapsed_seconds.get())
    }
}

/// Healthy means the **connected phase** lasted, not the attempt. A slow
/// spawn followed by an instant death is still a crash loop, and timing
/// from before `connect` would count the spawn as service, reset the
/// brake every round and spin forever. The fake connect below burns one
/// clock step; with the timer started before it, `served` would be two
/// steps (12s >= HEALTHY_CONNECTED) and no delay would ever appear.
#[test]
fn a_slow_spawn_that_dies_instantly_is_not_a_healthy_connection() {
    let stop = AtomicBool::new(false);
    let clock = FakeClock {
        step_seconds: std::cell::Cell::new(6),
        elapsed_seconds: std::cell::Cell::new(0),
    };
    let mut connected_rounds = 0;
    let mut delays: Vec<Duration> = Vec::new();
    let outcome = run_supervisor_loop(
        &stop,
        || {
            // The spawn takes time: the clock moves while connecting.
            let _ = clock.now();
            Ok(())
        },
        |_| {
            connected_rounds += 1;
            if connected_rounds >= 40 {
                StatusLoopExit::Stopped
            } else {
                StatusLoopExit::ConnectionLost
            }
        },
        |delay, _| {
            delays.push(delay);
            delays.len() < 2
        },
        || clock.now(),
    );

    assert_eq!(outcome, SupervisorLoopExit::Stopped);
    assert_eq!(
        delays,
        vec![Duration::from_secs(2), Duration::from_secs(4)],
        "the brake engages: a 6s connected phase is not healthy, however slow the spawn was"
    );
}

/// M-a's target: losses that come soon after each spawn are a crash loop,
/// and the delay between attempts grows — three fast retries first, then
/// exponential, ceilinged (M-d's loop-level half: never above
/// `MAX_BACKOFF`).
#[test]
fn a_crash_loop_backs_off_across_repeated_immediate_losses() {
    let stop = AtomicBool::new(false);
    let clock = FakeClock {
        step_seconds: std::cell::Cell::new(1),
        elapsed_seconds: std::cell::Cell::new(0),
    };
    let mut connect_attempts = 0;
    let mut connected_rounds = 0;
    let mut delays: Vec<Duration> = Vec::new();
    let outcome = run_supervisor_loop(
        &stop,
        || {
            connect_attempts += 1;
            Ok(connect_attempts)
        },
        |_| {
            connected_rounds += 1;
            // The cap only exists so a broken brake (one that never
            // sleeps) cannot spin this test forever; the green run exits
            // long before it.
            if connected_rounds >= 40 {
                StatusLoopExit::Stopped
            } else {
                StatusLoopExit::ConnectionLost
            }
        },
        |delay, _| {
            delays.push(delay);
            delays.len() < 8
        },
        || clock.now(),
    );

    assert_eq!(outcome, SupervisorLoopExit::Stopped);
    assert_eq!(
        delays,
        vec![
            Duration::from_secs(2),
            Duration::from_secs(4),
            Duration::from_secs(8),
            Duration::from_secs(16),
            Duration::from_secs(32),
            Duration::from_secs(60),
            Duration::from_secs(60),
            Duration::from_secs(60),
        ],
        "three fast retries, then exponential, ceilinged at MAX_BACKOFF"
    );
    assert_eq!(connect_attempts, 11);
}

/// M-b's target: a connection that served at least `HEALTHY_CONNECTED`
/// is a genuinely healthy one — it resets the brake, so the losses after
/// it get the fast path again and the schedule restarts at the base
/// instead of continuing to climb.
#[test]
fn a_healthy_connection_restores_the_fast_path() {
    let stop = AtomicBool::new(false);
    let clock = FakeClock {
        step_seconds: std::cell::Cell::new(1),
        elapsed_seconds: std::cell::Cell::new(0),
    };
    let mut connect_attempts = 0;
    let mut connected_rounds = 0;
    let mut delays: Vec<Duration> = Vec::new();
    let outcome = run_supervisor_loop(
        &stop,
        || {
            connect_attempts += 1;
            Ok(connect_attempts)
        },
        |_| {
            connected_rounds += 1;
            // Round five is the healthy one: the clock's step is raised
            // so the connected phase served well over `HEALTHY_CONNECTED`.
            // Round ten is the deliberate stop that ends the loop.
            clock
                .step_seconds
                .set(if connected_rounds == 5 { 10 } else { 1 });
            if connected_rounds == 10 {
                StatusLoopExit::Stopped
            } else {
                StatusLoopExit::ConnectionLost
            }
        },
        |delay, _| {
            delays.push(delay);
            true
        },
        || clock.now(),
    );

    // Four fast failures: three free, the fourth sleeps at the base.
    // Round five is healthy and resets; the next three losses are free
    // again and the fifth sleeps at the base — restarted, not continued.
    assert_eq!(outcome, SupervisorLoopExit::Stopped);
    assert_eq!(connected_rounds, 10, "round ten is the deliberate stop");
    assert_eq!(
        delays,
        vec![Duration::from_secs(2), Duration::from_secs(2)],
        "a healthy connection restores the fast path and the base delay"
    );
}

/// M-c's target: a backoff that ignored `stop` would make shutdown wait
/// out the whole delay. The sleep closure answering `false` — what
/// `sleep_interruptible` returns when `stop` is set — must exit the loop
/// promptly, with no further connect attempt.
#[test]
fn stopping_mid_backoff_exits_without_another_connect() {
    let stop = AtomicBool::new(false);
    let clock = FakeClock {
        step_seconds: std::cell::Cell::new(1),
        elapsed_seconds: std::cell::Cell::new(0),
    };
    let mut connect_attempts = 0;
    let mut connected_rounds = 0;
    let mut sleep_calls = 0;
    let outcome = run_supervisor_loop(
        &stop,
        || {
            connect_attempts += 1;
            Ok(connect_attempts)
        },
        |_| {
            connected_rounds += 1;
            // The cap only exists so a backoff that ignores `stop`
            // cannot spin this test forever; the green run exits long
            // before it.
            if connected_rounds >= 40 {
                StatusLoopExit::Stopped
            } else {
                StatusLoopExit::ConnectionLost
            }
        },
        |_, _| {
            sleep_calls += 1;
            false
        },
        || clock.now(),
    );

    assert_eq!(outcome, SupervisorLoopExit::Stopped);
    assert_eq!(
        connect_attempts, 4,
        "the interrupted backoff reconnects nothing"
    );
    assert_eq!(
        sleep_calls, 1,
        "the first backed-off sleep is the last wait"
    );
}

#[test]
fn connected_timeout_source_reaches_unresponsive_without_reconnect() {
    let source = TimeoutStatusSource {
        calls: AtomicUsize::new(0),
    };
    let stop = AtomicBool::new(false);
    let mut tracker = StatusFailureTracker::default();
    let mut updates = Vec::new();
    let mut sleeps = 0;
    let outcome = run_status_loop(
        &source,
        &mut tracker,
        &stop,
        || {
            sleeps += 1;
            sleeps < 3
        },
        |update| updates.push(update),
    );

    assert_eq!(outcome, StatusLoopExit::Stopped);
    assert_eq!(source.calls.load(Ordering::SeqCst), 3);
    assert!(updates.iter().any(|update| matches!(
        update,
        StatusUpdate::Failure(signal) if signal.state == "unresponsive"
    )));
}

#[test]
fn typed_connection_loss_exits_the_status_loop_for_reconnect() {
    let source = LostStatusSource {
        calls: AtomicUsize::new(0),
    };
    let stop = AtomicBool::new(false);
    let mut tracker = StatusFailureTracker::default();
    let mut updates = Vec::new();
    let outcome = run_status_loop(
        &source,
        &mut tracker,
        &stop,
        || true,
        |update| updates.push(update),
    );

    assert_eq!(outcome, StatusLoopExit::ConnectionLost);
    assert_eq!(source.calls.load(Ordering::SeqCst), 1);
    assert!(matches!(updates.as_slice(), [StatusUpdate::Failure(_)]));
}

#[test]
fn two_status_failures_do_not_raise_unresponsive_but_three_do() {
    let first_attempt = Instant::now();
    let mut tracker = StatusFailureTracker::default();
    let first = tracker.record_failure(
        first_attempt,
        first_attempt + Duration::from_secs(30),
        "timed out",
    );
    assert_eq!(first.state, "error");
    let second = tracker.record_failure(
        first_attempt + Duration::from_secs(30),
        first_attempt + Duration::from_secs(60),
        "timed out",
    );
    assert_eq!(second.state, "error");
    let third = tracker.record_failure(
        first_attempt + Duration::from_secs(60),
        first_attempt + Duration::from_secs(90),
        "timed out",
    );
    assert_eq!(third.state, "unresponsive");
    assert!(third
        .message
        .as_deref()
        .is_some_and(|message| message.contains("90 seconds")));
}

#[test]
fn a_success_resets_the_failure_count() {
    let first_attempt = Instant::now();
    let mut tracker = StatusFailureTracker::default();
    tracker.record_failure(
        first_attempt,
        first_attempt + Duration::from_secs(30),
        "timed out",
    );
    tracker.record_success();
    let first_after_success = tracker.record_failure(
        first_attempt + Duration::from_secs(60),
        first_attempt + Duration::from_secs(90),
        "timed out",
    );
    let second_after_success = tracker.record_failure(
        first_attempt + Duration::from_secs(90),
        first_attempt + Duration::from_secs(120),
        "timed out",
    );
    assert_eq!(first_after_success.state, "error");
    assert_eq!(second_after_success.state, "error");
}

#[test]
fn reconnect_does_not_reset_failures_when_status_keeps_failing() {
    let first_attempt = Instant::now();
    let mut tracker = StatusFailureTracker::default();
    tracker.record_failure(
        first_attempt,
        first_attempt + Duration::from_secs(30),
        "timed out",
    );
    assert!(tracker
        .connection_status(first_attempt + Duration::from_secs(30))
        .is_none());
    tracker.record_failure(
        first_attempt + Duration::from_secs(30),
        first_attempt + Duration::from_secs(60),
        "timed out",
    );
    let status = tracker.record_failure(
        first_attempt + Duration::from_secs(60),
        first_attempt + Duration::from_secs(90),
        "timed out",
    );
    assert_eq!(status.state, "unresponsive");
}

#[test]
fn daemon_restart_has_the_frozen_tauri_signature() {
    let _: fn(State<'_, DaemonBridge>) -> Result<(), crate::backend::error::CommandError> =
        daemon_restart;
}
