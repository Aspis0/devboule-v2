//! Connection layer — pass-3a split of `server.rs`: accepting and serving
//! one connection (`handle_client`, the Noise and pipe entrances both land
//! here), the hello handshake, event flushing/draining, and the reply
//! redaction.
//!
//! The answers that never reach a domain handler stay here by design:
//! shutdown refusals, the hello handshake and its repeated-`Hello` answer,
//! and the remote rate-limit drop — which writes an audit row
//! (`rate_limited`/`denied`) outside the gate's `peer_outcome` vocabulary.
//! That second audit writer is a fact, not a bypass: no domain handler runs
//! on those paths.

use super::*;

/// Serve one connection until it closes.
///
/// `conn_peer` is `Some(ConnPeer::Remote {..})` when this connection arrived
/// over Noise: its identity was authenticated before this call. A pipe
/// connection passes `None` and is identified by the kernel through the pipe
/// handle. The two are never mixed: a remote peer has no pipe handle at all.
pub(crate) fn handle_client(
    framed: Framed,
    state: Arc<ServerState>,
    conn_peer: Option<ConnPeer>,
) -> Result<(), DaemonError> {
    if state.is_shutting_down() {
        send_shutting_down(&framed, None)?;
        return Ok(());
    }
    let hello: ClientMessage = framed.recv_timeout(HANDSHAKE_TIMEOUT)?;
    let ClientMessage::Hello(client_hello) = hello else {
        framed.send(&DaemonMessage::Error(WireError::new(
            ErrorCode::InvalidRequest,
            "first frame must be hello",
        )))?;
        return Ok(());
    };
    if state.is_shutting_down() {
        send_shutting_down(&framed, None)?;
        return Ok(());
    }
    // `as_file()` is `Option` because a stream connection has no pipe handle;
    // that case is routine, so it is a branch and never an unwrap.
    #[cfg(windows)]
    let peer: Option<crate::agent_report::PeerIdentity> = match framed.as_file() {
        Some(file) => match transport::peer_identity(&file) {
            Ok(peer) => Some(peer),
            Err(error) => {
                eprintln!("could not derive named-pipe peer identity: {error}");
                let _ = framed.send(&DaemonMessage::Error(WireError::new(
                    ErrorCode::Unauthorized,
                    "Could not verify the daemon client identity.",
                )));
                return Err(DaemonError::Io(error));
            }
        },
        None => None,
    };
    #[cfg(not(windows))]
    let peer: Option<crate::agent_report::PeerIdentity> = None;

    // Authority is `OwnerId.user`: a kernel SID (starts with `S-` on Windows)
    // for a local client, `peer_<device_id>` for a remote one.
    let true_owner = match &conn_peer {
        Some(ConnPeer::Remote {
            device_id, role, ..
        }) => OwnerId::new(format!("peer_{device_id}"), role.as_str())
            .map_err(DaemonError::Protocol)?,
        _ => match &peer {
            Some(peer) => match OwnerId::new(peer.user.clone(), format!("process-{}", peer.pid)) {
                Ok(owner) => owner,
                Err(message) => {
                    let _ = framed.send(&DaemonMessage::Error(WireError::new(
                        ErrorCode::Unauthorized,
                        "Could not verify the daemon client identity.",
                    )));
                    return Err(DaemonError::Protocol(message));
                }
            },
            // No pipe identity on this platform: today's behaviour, the hello
            // owner label. The peer case is handled above.
            None => client_hello.owner.clone(),
        },
    };
    if client_hello.owner != true_owner {
        // Redacted, not printed: this line used to carry the user SID and, on
        // a peer connection, `peer_<device_id>` — both of which §8 R7 keeps out
        // of logs. The mismatch is still diagnosable; the identities are not in
        // the file.
        eprintln!(
            "client hello owner label {} did not match the connection peer {}",
            crate::device_identity::redact(&client_hello.owner.user),
            crate::device_identity::redact(&true_owner.user)
        );
    }
    let daemon_hello = daemon_hello(&state);
    let agreed = match negotiate(&client_hello, &daemon_hello) {
        Ok(agreed) => {
            // The client learns the usable capability set from this hello;
            // do not expose daemon-only capabilities as if they were agreed.
            let mut agreed_hello = daemon_hello.clone();
            agreed_hello.capabilities = agreed.capabilities.clone();
            framed.send(&DaemonMessage::Hello(agreed_hello))?;
            agreed
        }
        Err(error) => {
            framed.send(&DaemonMessage::Error(error))?;
            return Ok(());
        }
    };
    let sessions_ok = agreed
        .capabilities
        .iter()
        .any(|capability| capability.as_str() == caps::SESSIONS);
    let journal_ok = agreed
        .capabilities
        .iter()
        .any(|capability| capability.as_str() == caps::JOURNAL);
    let typed_permissions_ok = agreed
        .capabilities
        .iter()
        .any(|capability| capability.as_str() == caps::TYPED_PERMISSIONS);
    let devices_ok = agreed
        .capabilities
        .iter()
        .any(|capability| capability.as_str() == caps::DEVICES);
    // The hello owner is diagnostic only. All idempotency and session access
    // below use the identity decided above.
    //
    // A paired `Client` speaks for the person who paired it, so every session
    // request it makes is that user's request: the registry's owner-user filter
    // is then the whole scope (§8b A3), and it is also what makes a
    // peer-created session appear in the desktop's list. A `Daemon` peer keeps
    // the `peer_<device_id>` identity slice 1 gave it, so the same filter
    // scopes it to the sessions it created (R2).
    let owner = match &conn_peer {
        Some(ConnPeer::Remote {
            role: PeerRole::Client,
            paired_by_user: Some(paired),
            ..
        }) => OwnerId::new(paired.clone(), PeerRole::Client.as_str())
            .unwrap_or_else(|_| true_owner.clone()),
        _ => true_owner.clone(),
    };
    // The peer's capability set, resolved once from its `peers` row: the gate
    // reads it on every request, and a `PeerSetCaps` closes this connection, so
    // a running connection can never hold a capability the row dropped.
    let peer_caps = match &conn_peer {
        Some(ConnPeer::Remote { device_id, .. }) => state.peer_caps(device_id),
        _ => Vec::new(),
    };
    let conn = ConnHandle::with_peer_caps(state.alloc_conn(), peer, conn_peer.clone(), peer_caps);
    let (request_tx, request_rx) = mpsc::sync_channel(64);
    let reader_wake = Arc::clone(&conn.outbound);
    let reader_framed = framed.clone();
    let reader = std::thread::Builder::new()
        .name("daemon-client-request".into())
        .spawn(move || read_client_requests(reader_framed, request_tx, reader_wake))
        .map_err(DaemonError::from)?;
    let mut pending_events = VecDeque::new();
    let mut pending_state_events = VecDeque::new();
    let mut pending_replies = VecDeque::new();
    // Remote connections are rate limited; a local pipe is not (muse M1).
    let is_remote = matches!(conn.conn_peer, Some(ConnPeer::Remote { .. }));
    let mut bucket = TokenBucket::new(Instant::now());
    // A revocation must drop a live connection, so the connection registers
    // itself and polls the flag its own revoke sets.
    let close_requested = match &conn.conn_peer {
        Some(ConnPeer::Remote { device_id, .. }) => {
            Some(state.register_remote_conn(conn.id, device_id))
        }
        _ => None,
    };
    let loop_result = (|| -> Result<(), DaemonError> {
        loop {
            if state.stop.load(Ordering::SeqCst) {
                break;
            }
            if close_requested
                .as_ref()
                .is_some_and(|close| close.load(Ordering::SeqCst))
            {
                break;
            }
            let observed_generation = conn.outbound.wake_generation();
            if pending_replies.is_empty() {
                pending_replies.extend(conn.outbound.pull_replies());
            }
            if let Some(reply) = pending_replies.pop_front() {
                framed.send(&redact_for_conn(&conn, reply))?;
                continue;
            }
            let (request, request_channel_closed) = match request_rx.try_recv() {
                Ok(request) => (Some(request), false),
                Err(TryRecvError::Empty) => (None, false),
                Err(TryRecvError::Disconnected) => (None, true),
            };
            if request_channel_closed {
                break;
            }
            if let Some(request) = request {
                let request = match request {
                    Ok(request) => request,
                    Err(error) => {
                        if connection_closed(&error) || state.stop.load(Ordering::SeqCst) {
                            break;
                        }
                        return Err(error);
                    }
                };
                let close_request = matches!(&request, ClientMessage::SessionClose { .. });
                if is_remote && !bucket.take(Instant::now()) {
                    // Exactly one audit row, then close: the audit table must
                    // not amplify a flood.
                    if let Some(ConnPeer::Remote {
                        device_id, role, ..
                    }) = &conn.conn_peer
                    {
                        state.audit(AuditRecord {
                            device_id: device_id.clone(),
                            role: role.as_str().to_string(),
                            claimed_origin: None,
                            action: "rate_limited".to_string(),
                            session_id: None,
                            outcome: "denied".to_string(),
                        });
                    }
                    break;
                }
                if drains_events_before_dispatch(&request) {
                    // A close must leave the pull state alive for the
                    // post-dispatch pull: teardown_session joins the
                    // coalescer and may publish its final output there.
                    if !close_request {
                        refill_pending_events(&conn, &mut pending_events);
                        refill_pending_state_events(&conn, &mut pending_state_events);
                    }
                    drain_pending_events(&framed, &conn, &mut pending_events, &state.sessions)?;
                    drain_pending_state_events(&framed, &mut pending_state_events)?;
                } else {
                    // Give the event stream one turn before every ordinary
                    // request. This is deliberately one frame, not a bulk
                    // drain: a continuously replenished request stream cannot
                    // starve output, while a DSR/control request waits behind
                    // at most the single event write already in progress.
                    refill_pending_events(&conn, &mut pending_events);
                    refill_pending_state_events(&conn, &mut pending_state_events);
                    if let Some(event) = pending_events.pop_front() {
                        send_pending_event(&framed, &conn, event, &state.sessions)?;
                    } else if let Some(event) = pending_state_events.pop_front() {
                        send_state_event(&framed, event)?;
                    }
                }
                if let ClientMessage::Hello(_) = request {
                    let id = request.request_id();
                    let mut error =
                        WireError::new(ErrorCode::InvalidRequest, "hello already completed");
                    if let Some(id) = id {
                        error = error.with_id(id);
                    }
                    framed.send(&DaemonMessage::Error(error))?;
                    continue;
                }
                let Some(reply) = dispatch(
                    &state,
                    &owner,
                    request,
                    &conn,
                    sessions_ok,
                    journal_ok,
                    typed_permissions_ok,
                    devices_ok,
                ) else {
                    continue;
                };
                if close_request {
                    // SessionClose joins the coalescer and calls finish(),
                    // which can publish the teardown tail after the pre-drain.
                    refill_pending_events(&conn, &mut pending_events);
                    refill_pending_state_events(&conn, &mut pending_state_events);
                    drain_pending_events(&framed, &conn, &mut pending_events, &state.sessions)?;
                    drain_pending_state_events(&framed, &mut pending_state_events)?;
                }
                let shutting_down = matches!(reply, DaemonMessage::Shutdown { accepted: true, .. });
                // Control/lifecycle replies retain the flush barrier. It makes
                // the acknowledgement visible before teardown or a shutdown
                // disconnect; the event stream below must never use that
                // barrier per frame.
                framed.send(&redact_for_conn(&conn, reply))?;
                if shutting_down {
                    state.request_shutdown();
                    break;
                }
                continue;
            }

            if pending_events.is_empty() && pending_state_events.is_empty() {
                if pending_replies.is_empty() {
                    pending_replies.extend(conn.outbound.pull_replies());
                }
                if let Some(reply) = pending_replies.pop_front() {
                    framed.send(&redact_for_conn(&conn, reply))?;
                    continue;
                }
                refill_pending_events(&conn, &mut pending_events);
                refill_pending_state_events(&conn, &mut pending_state_events);
                if pending_events.is_empty() && pending_state_events.is_empty() {
                    if !conn
                        .outbound
                        .wait_for_notify_since(observed_generation, conn.next_exit_wake())
                    {
                        break;
                    }
                    continue;
                }
            }

            // Send at most one event before looking for control traffic again.
            // In particular, no bulk output batch can hold a DSR, resize, or
            // kill request behind a sequence of flushes.
            if let Some(event) = pending_events.pop_front() {
                send_pending_event(&framed, &conn, event, &state.sessions)?;
            } else {
                let event = pending_state_events
                    .pop_front()
                    .expect("state event queue was checked above");
                send_state_event(&framed, event)?;
            }
        }
        Ok(())
    })();

    // Stop the request reader before the final pull so no new request/error
    // can race connection cleanup. This path is shared by normal disconnects,
    // write/read errors, daemon shutdown, and idle exit.
    framed.cancel_read();
    conn.outbound.close();
    bounded_join(reader, JOIN_BUDGET);
    // A connection the user has just revoked (`PeerRevoke`, or a `PeerSetCaps`
    // that dropped a capability) gets nothing more — not even an event that was
    // already queued when the flag went up. Skipping the flush is what makes
    // "revoked" hold at the last place a frame could still leave, and the
    // detach below then runs with no write between it and the end of the
    // connection (H3, §8b A4).
    let revoked = close_requested
        .as_ref()
        .is_some_and(|close| close.load(Ordering::SeqCst));
    flush_final_events(
        &framed,
        &conn,
        &state.sessions,
        &mut pending_events,
        &mut pending_state_events,
        revoked,
    );
    state.sessions.detach_conn(&conn);
    state.unwatch_sessions(conn.id);
    state.sessions.clear_presence(conn.id);
    state.unregister_remote_conn(conn.id);
    // The device is gone, so the permission cards it was holding can no longer
    // be answered by it: whatever slots it still held go back to the
    // daemon-wide allowance (H2).
    if let Some(device_id) = conn.conn_peer.as_ref().and_then(ConnPeer::device_id) {
        crate::session::release_peer_cards(device_id);
    }
    loop_result
}

/// Whatever is still queued for this connection, on its way out — unless the
/// connection was revoked while it was queued.
///
/// `PeerRevoke` and a capability-dropping `PeerSetCaps` raise the close flag
/// and the loop breaks at its next iteration boundary; this is the last place a
/// frame could still leave afterwards. Refilling and draining here would hand a
/// device the user has just disowned every event that was pending at the moment
/// of revocation, so a revoked connection sends none of them: the queues die
/// with the connection (H3, `DESIGN-remote-agents.md` §8b A4).
pub(super) fn flush_final_events(
    framed: &Framed,
    conn: &ConnHandle,
    sessions: &SessionRegistry,
    pending_events: &mut VecDeque<PendingEvent>,
    pending_state_events: &mut VecDeque<SessionEventEnvelope>,
    revoked: bool,
) {
    if revoked {
        return;
    }
    refill_pending_events(conn, pending_events);
    refill_pending_state_events(conn, pending_state_events);
    if let Err(error) = drain_pending_events(framed, conn, pending_events, sessions) {
        eprintln!("daemon connection final event drain failed: {error}");
    }
    if let Err(error) = drain_pending_state_events(framed, pending_state_events) {
        eprintln!("daemon connection final state event drain failed: {error}");
    }
    // This is the deliberate teardown-only pipe barrier: it makes every frame
    // accepted above client-readable before the server drops this connection.
    // FlushFileBuffers stays out of the per-frame event path because it waits
    // for the client to consume the pipe. A revoked connection skips it too:
    // there is nothing to make readable.
    let _ = framed.flush_pipe();
}

/// The owner whose sessions a connection may read.
///
/// Derived from the connection's peer identity rather than from whatever owner
/// the caller passed, so a caller cannot widen the projection: a `Client` peer
/// reads the paired user's sessions, a `Daemon` peer reads what its own device
/// created, and the local pipe reads its own. `handle_client` builds the same
/// owner for a `Client` peer on every other request, so this restates the rule
/// where the reply is built instead of trusting the argument
/// (`DESIGN-remote-agents.md` §8b A3, §8 R2).
pub(super) fn session_list_owner(conn_peer: &Option<ConnPeer>, caller: &OwnerId) -> OwnerId {
    let projected = match conn_peer {
        Some(ConnPeer::Remote {
            role: PeerRole::Client,
            paired_by_user: Some(paired),
            ..
        }) => OwnerId::new(paired.clone(), PeerRole::Client.as_str()),
        Some(ConnPeer::Remote {
            role: PeerRole::Daemon,
            device_id,
            ..
        }) => OwnerId::new(format!("peer_{device_id}"), PeerRole::Daemon.as_str()),
        _ => return caller.clone(),
    };
    projected.unwrap_or_else(|_| caller.clone())
}

/// The one gate every reply passes on its way out to a peer connection.
///
/// A `DaemonMessage::Error` carries text written for the person at this
/// machine: absolute paths, this device's own id, key fingerprints. A remote
/// reader gets the same error with those facts replaced (`DESIGN-remote-agents.md`
/// §8 R7); a local connection gets it untouched, because
/// `WireError::redacted_for(None)` is the identity. It is deliberately not
/// applied to the event stream: a permission card's text is the owner's own
/// screen, shown to whoever is driving the session (§8b A14).
pub(super) fn redact_for_conn(conn: &ConnHandle, reply: DaemonMessage) -> DaemonMessage {
    let role = conn.conn_peer.as_ref().and_then(|peer| peer.role());
    match reply {
        DaemonMessage::Error(error) => DaemonMessage::Error(error.redacted_for(role.as_ref())),
        other => other,
    }
}

fn refill_pending_events(conn: &ConnHandle, pending_events: &mut VecDeque<PendingEvent>) {
    // pull_events() starts at the last successfully written sequence. A
    // non-empty queue already owns every event after that cursor, so pulling
    // again would append the same envelopes and duplicate them on the wire.
    if pending_events.is_empty() {
        pending_events.extend(conn.pull_events());
    }
}

fn drain_pending_events(
    framed: &Framed,
    conn: &ConnHandle,
    pending_events: &mut VecDeque<PendingEvent>,
    sessions: &SessionRegistry,
) -> Result<(), DaemonError> {
    while let Some(event) = pending_events.pop_front() {
        send_pending_event(framed, conn, event, sessions)?;
    }
    Ok(())
}

fn refill_pending_state_events(
    conn: &ConnHandle,
    pending_events: &mut VecDeque<SessionEventEnvelope>,
) {
    if pending_events.is_empty() {
        pending_events.extend(conn.pull_state_events());
    }
}

fn drain_pending_state_events(
    framed: &Framed,
    pending_events: &mut VecDeque<SessionEventEnvelope>,
) -> Result<(), DaemonError> {
    while let Some(event) = pending_events.pop_front() {
        send_state_event(framed, event)?;
    }
    Ok(())
}

fn send_state_event(framed: &Framed, event: SessionEventEnvelope) -> Result<(), DaemonError> {
    framed.send_unflushed(&DaemonMessage::Event(event))
}

fn send_pending_event(
    framed: &Framed,
    conn: &ConnHandle,
    event: PendingEvent,
    sessions: &SessionRegistry,
) -> Result<(), DaemonError> {
    if !conn.event_is_current(event.subscription_id, event.attachment_generation) {
        let sequence = match &event.envelope.event {
            SessionEvent::Output { seq, .. } => format!(" seq={seq}"),
            SessionEvent::Exit { .. } => " exit".to_string(),
            SessionEvent::Recovered { .. } => " recovered".to_string(),
            SessionEvent::Silent { .. } => " silent".to_string(),
            SessionEvent::JournalDegraded { .. } => " journal_degraded".to_string(),
            SessionEvent::SessionsSnapshot { .. } => " sessions_snapshot".to_string(),
            // A snapshot is screen state and has no replay sequence.
            SessionEvent::Snapshot { .. } => " snapshot".to_string(),
            SessionEvent::AgentMessage { .. } => " agent_message".to_string(),
            SessionEvent::AgentUserMessage { .. } => " agent_user_message".to_string(),
            SessionEvent::Steered { .. } => " steered".to_string(),
            SessionEvent::AgentThought { .. } => " agent_thought".to_string(),
            SessionEvent::AvailableCommands { .. } => " available_commands".to_string(),
            SessionEvent::AgentToolCall { .. } => " agent_tool_call".to_string(),
            SessionEvent::AgentToolUpdate { .. } => " agent_tool_update".to_string(),
            SessionEvent::AgentFinished { .. } => " agent_finished".to_string(),
            SessionEvent::AgentTaskStarted { .. } => " agent_task_started".to_string(),
            SessionEvent::AgentTaskNotification { .. } => " agent_task_notification".to_string(),
            SessionEvent::AgentBackgroundTasksChanged { .. } => {
                " agent_background_tasks_changed".to_string()
            }
            SessionEvent::AgentError { .. } => " agent_error".to_string(),
            SessionEvent::AgentStderr { .. } => " agent_stderr".to_string(),
            SessionEvent::PermissionRequest { .. } => " permission_request".to_string(),
            SessionEvent::PermissionResolved { .. } => " permission_resolved".to_string(),
            SessionEvent::PermissionAnswered { .. } => " permission_answered".to_string(),
            SessionEvent::SessionManifest { .. } => " session_manifest".to_string(),
            SessionEvent::SessionNotice { .. } => " session_notice".to_string(),
            SessionEvent::AgentReported { .. } => " agent_reported".to_string(),
            SessionEvent::AgentCreated { .. } => " agent_created".to_string(),
            SessionEvent::ChildFinished { .. } => " child_finished".to_string(),
            SessionEvent::Detached => " detached".to_string(),
        };
        eprintln!(
            "discarded stale pending event for session {} generation {}{}",
            event.session_id, event.attachment_generation, sequence
        );
        return Ok(());
    }
    framed.send_unflushed(&DaemonMessage::SubscriptionEvent {
        subscription_id: event.subscription_id,
        envelope: event.envelope.clone(),
    })?;
    // The cursor is advanced after the complete frame has been written. The
    // clone above is only for the serialized message; the original envelope
    // retains the acknowledgement metadata.
    if let Some(session_id) = conn.event_sent(&event) {
        sessions.subscription_event_sent(&session_id);
    }
    Ok(())
}

fn read_client_requests(
    framed: Framed,
    inbox: SyncSender<Result<ClientMessage, DaemonError>>,
    wake: Arc<ConnOut>,
) {
    loop {
        let request = framed.recv::<ClientMessage>();
        let finished = request.is_err();
        if inbox.send(request).is_err() {
            break;
        }
        wake.notify();
        if finished {
            break;
        }
    }
}

fn connection_closed(error: &DaemonError) -> bool {
    matches!(
        error,
        DaemonError::Io(error)
            if error.kind() == std::io::ErrorKind::UnexpectedEof
                || error.kind() == std::io::ErrorKind::BrokenPipe
                || error.kind() == std::io::ErrorKind::ConnectionReset
                || error.raw_os_error() == Some(995)
    )
}

fn drains_events_before_dispatch(request: &ClientMessage) -> bool {
    matches!(
        request,
        ClientMessage::Shutdown { .. }
            | ClientMessage::SessionClose { .. }
            | ClientMessage::SessionsUnwatch { .. }
    )
}

pub(super) fn reject_shutting_down(stream: std::fs::File) {
    let framed = Framed::new(stream);
    let _ = send_shutting_down(&framed, None);
}

fn send_shutting_down(framed: &Framed, id: Option<u64>) -> Result<(), DaemonError> {
    let mut error = WireError::new(ErrorCode::ShuttingDown, "daemon is shutting down");
    if let Some(id) = id {
        error = error.with_id(id);
    }
    framed.send(&DaemonMessage::Error(error))
}

fn daemon_hello(state: &ServerState) -> DaemonHello {
    DaemonHello {
        protocol_version: PROTOCOL_VERSION,
        min_protocol_version: PROTOCOL_MIN_VERSION,
        daemon_version: env!("CARGO_PKG_VERSION").to_string(),
        instance_id: state.instance_id.clone(),
        pid: std::process::id(),
        capabilities: m3a_daemon_capabilities(),
    }
}
