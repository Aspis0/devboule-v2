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
    quit_intent: QuitIntent,
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
    // `as_file()` is `Option` because a stream connection has no file
    // handle; that case is routine, so it is a branch and never an unwrap.
    // The accept layer already refused a foreign uid, so a local peer here
    // is this user; the owner below is still derived from the kernel
    // identity, never from the hello's label.
    #[cfg(any(windows, unix))]
    let peer: Option<crate::agent_report::PeerIdentity> = match framed.as_file() {
        Some(file) => match transport::peer_identity(&file) {
            Ok(peer) => Some(peer),
            Err(error) => {
                #[cfg(windows)]
                eprintln!("could not derive named-pipe peer identity: {error}");
                #[cfg(unix)]
                eprintln!("could not derive socket peer identity: {error}");
                let _ = framed.send(&DaemonMessage::Error(WireError::new(
                    ErrorCode::Unauthorized,
                    "Could not verify the daemon client identity.",
                )));
                return Err(DaemonError::Io(error));
            }
        },
        None => None,
    };
    #[cfg(not(any(windows, unix)))]
    let peer: Option<crate::agent_report::PeerIdentity> = None;

    // Authority is `OwnerId.user`: a kernel SID (starts with `S-` on Windows)
    // or uid (a decimal string on Unix) for a local client, `peer_<device_id>` for a remote one.
    let true_owner = match &conn_peer {
        Some(ConnPeer::Remote { device_id, .. }) => OwnerId::new(
            format!("peer_{device_id}"),
            crate::peer_policy::PEER_OWNER_TAG,
        )
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
            // No kernel identity for this connection (a remote stream, or a
            // platform without one): fall back to the hello owner label.
            // The peer case is handled above.
            None => client_hello.owner.clone(),
        },
    };
    if client_hello.owner != true_owner {
        super::hello_owner_log::log_mismatch_once(&client_hello.owner, &true_owner);
    }
    let mut daemon_hello = daemon_hello(&state);
    // A peer connection is the one conversation where this daemon's own
    // workspace-host presence belongs in the hello: it is what the far side
    // binds to this device id to pick the session scope it grants us. The
    // builder keeps the bit and the advertised service in step. An app's hello
    // carries no such claim, and never gets one.
    if conn_peer.is_some() {
        daemon_hello = daemon_hello.with_workspace_host(state.has_hosted_workspace());
    }
    let agreed = match negotiate(&client_hello, &daemon_hello) {
        Ok(agreed) => agreed,
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
    // The shared queue rides this connection rather than a dispatch argument:
    // both the attach that registers an observer and the queue frames that
    // mutate one have to know it, and the observer outlives the request that
    // created it. Recorded on the connection below, once the connection is
    // built.
    let queue_ok = agreed
        .capabilities
        .iter()
        .any(|capability| capability.as_str() == caps::SESSION_QUEUE);
    // The held link to another machine rides this connection too: a client that
    // never agreed the name gets its `RemoteHost*` frames refused, so an older
    // app cannot ask this daemon to open sockets it has no way to close.
    let remote_hosts_ok = agreed
        .capabilities
        .iter()
        .any(|capability| capability.as_str() == caps::REMOTE_HOSTS);
    // A v32 peer states its workspace-host presence: the field is required on
    // every dialect from 32 on, and absence is legal only for a negotiated
    // v30/v31 hello. Accepting an absent bit would leave the scope to the
    // record alone with no way to learn the peer's transition.
    if conn_peer.is_some() && agreed.protocol_version >= 32 && client_hello.workspace_host.is_none()
    {
        framed.send(&DaemonMessage::Error(WireError::new(
            ErrorCode::ProtocolVersionMismatch,
            "a v32 peer hello must state its workspace presence",
        )))?;
        return Ok(());
    }
    // The session register this connection reaches comes from the peer's own
    // authenticated presence pair — its `workspaceHost` word and whether the
    // same hello advertised the hosted-workspace service. The two are one fact
    // computed from one workspace database, so a hello where they disagree is
    // refused: a device that hosts workspaces cannot claim "no workspace" and
    // keep a broader recorded scope, and a device that advertises the service
    // cannot deny hosting. The stored record is updated in both directions so
    // a legitimate host whose last workspace is gone becomes a client again.
    // A v30/v31 hello carries no presence at all and keeps the pairing record,
    // which is the only fact the older wire has. See `peer_policy::peer_scope`.
    let advertises_service = client_hello
        .capabilities
        .iter()
        .any(|capability| capability.as_str() == caps::HOSTED_WORKSPACES);
    let conn_peer = if conn_peer.is_some() && agreed.protocol_version >= 32 {
        let Some(scope) =
            crate::peer_policy::peer_scope(client_hello.workspace_host, advertises_service)
        else {
            framed.send(&DaemonMessage::Error(WireError::new(
                ErrorCode::InvalidRequest,
                "the peer's workspace-host presence and its advertised service disagree",
            )))?;
            return Ok(());
        };
        if let Some(ConnPeer::Remote { device_id, .. }) = &conn_peer {
            if let Err(error) = state.peer_set_hosts_workspaces(
                device_id,
                scope == crate::peer_policy::PeerScope::PeerDevice,
            ) {
                eprintln!("daemon could not record a peer's workspace presence: {error}");
            }
        }
        conn_peer.map(|peer| peer.with_scope(scope))
    } else {
        conn_peer
    };
    // The connection is admissible; only now does this daemon's own hello
    // leave, with the negotiated capability set and its own presence already
    // in step. A peer refused above never learns which daemon it reached.
    let mut agreed_hello = daemon_hello.clone();
    agreed_hello.capabilities = agreed.capabilities.clone();
    framed.send(&DaemonMessage::Hello(agreed_hello))?;
    // The hello owner is diagnostic only. All idempotency and session access
    // below use the identity decided above.
    //
    // A paired device with no hosted workspace speaks for the person who paired
    // it, so every session request it makes is that user's request: the
    // registry's owner-user filter is then the whole scope (§8b A3), and it is
    // also what makes a peer-created session appear in the desktop's list. A
    // machine peer keeps the `peer_<device_id>` identity, so the same filter
    // scopes it to the sessions it created.
    let owner = match &conn_peer {
        Some(ConnPeer::Remote {
            scope: crate::peer_policy::PeerScope::PairedUser,
            paired_by_user: Some(paired),
            ..
        }) => OwnerId::new(paired.clone(), crate::peer_policy::PEER_OWNER_TAG)
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
    let conn = ConnHandle::with_peer_caps(
        state.alloc_conn(),
        peer,
        conn_peer.clone(),
        peer_caps,
        quit_intent,
    );
    conn.set_session_queue_negotiated(queue_ok);
    // The dialect decides whether a devices-family reply carries the v30 role
    // projection; recorded here, before the connection reads a request.
    conn.set_negotiated_protocol(agreed.protocol_version);
    conn.set_session_tasks_negotiated(
        agreed
            .capabilities
            .iter()
            .any(|capability| capability.as_str() == caps::SESSION_TASKS),
    );
    conn.set_agent_resumed_negotiated(
        agreed
            .capabilities
            .iter()
            .any(|capability| capability.as_str() == caps::AGENT_RESUMED),
    );
    let resume_outcomes_ok = agreed
        .capabilities
        .iter()
        .any(|capability| capability.as_str() == caps::SESSION_RESUME_OUTCOMES);
    conn.set_resume_outcomes_negotiated(resume_outcomes_ok);
    conn.set_remote_hosts_negotiated(remote_hosts_ok);
    conn.set_browser_host_negotiated(
        agreed
            .capabilities
            .iter()
            .any(|capability| capability.as_str() == caps::BROWSER_HOST),
    );
    conn.set_plan_usage_live_negotiated(
        agreed
            .capabilities
            .iter()
            .any(|capability| capability.as_str() == caps::SESSION_PLAN_USAGE),
    );
    // Remote connections are rate limited; a local pipe is not. Computed
    // before the reader so a peer's silent socket gets an idle deadline.
    let is_remote = matches!(conn.conn_peer, Some(ConnPeer::Remote { .. }));
    let (request_tx, request_rx) = mpsc::sync_channel(64);
    let reader_wake = Arc::clone(&conn.outbound);
    let reader_framed = framed.clone();
    let reader_conn_id = conn.id;
    let reader_idle = is_remote.then(|| state.peer_idle_timeout());
    let reader = std::thread::Builder::new()
        .name("daemon-client-request".into())
        .spawn(move || {
            read_client_requests(
                reader_framed,
                request_tx,
                reader_wake,
                reader_conn_id,
                reader_idle,
            )
        })
        .map_err(DaemonError::from)?;
    let mut pending_events = VecDeque::new();
    let mut pending_state_events = VecDeque::new();
    let mut pending_replies = VecDeque::new();
    let mut bucket = TokenBucket::new(Instant::now());
    // A revocation must drop a live connection, so the connection registers
    // itself and polls the flag its own revoke sets.
    let close_requested = match &conn.conn_peer {
        Some(ConnPeer::Remote { .. }) => Some(state.register_remote_conn(Arc::clone(&conn))),
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
                    // Exactly one audit row, then close: the audit table must not amplify a flood.
                    if let Some(ConnPeer::Remote { device_id, .. }) = &conn.conn_peer {
                        state.audit(AuditRecord {
                            device_id: device_id.clone(),
                            role: crate::peer_policy::PEER_AUDIT_ROLE.to_string(),
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
                let trace_name = request.name();
                let trace_id = request.request_id();
                crate::rpc_trace::daemon_event(
                    "dispatch_start",
                    trace_name,
                    trace_id,
                    conn.id,
                    &[],
                );
                let dispatch_clock = Instant::now();
                let dispatched = dispatch(
                    &state,
                    &owner,
                    request,
                    &conn,
                    sessions_ok,
                    journal_ok,
                    typed_permissions_ok,
                    devices_ok,
                );
                // This measures dispatch work on the connection thread; async
                // creates have their full duration in `dispatch_worker_end`.
                let took_ms = dispatch_clock.elapsed().as_millis().to_string();
                crate::rpc_trace::daemon_event(
                    "dispatch_end",
                    trace_name,
                    trace_id,
                    conn.id,
                    &[("took_ms", took_ms.as_str())],
                );
                let Some(reply) = dispatched else {
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
                // disconnect; the event stream below must never use that barrier per frame.
                framed.send(&redact_for_conn(&conn, reply))?;
                crate::rpc_trace::daemon_event("reply", trace_name, trace_id, conn.id, &[]);
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
    // connection (`DESIGN-remote-agents.md` §8b A4).
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
    // A dropped window takes its host leases with it; the links linger only for
    // the grace, so a reopen reuses them.
    state.peer_links.release_connection(conn.id);
    // A host that leaves, however it leaves, fails the calls waiting on it.
    state.browser.connection_closed(conn.id);
    state.sessions.clear_presence(conn.id);
    state.unregister_remote_conn(conn.id);
    // The device is gone, so the permission cards it was holding can no longer
    // be answered by it: whatever slots it still held go back to the
    // daemon-wide allowance.
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
/// with the connection (`DESIGN-remote-agents.md` §8b A4).
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

/// The owner a machine-peer create writes: the pairing human when the
/// pairing is this user's own, else the peer identity as before.
///
/// A session created from the paired human's other PC belongs to that
/// human — not to a hidden `peer_<device>` owner — so the hosting machine
/// lists it in its own roster and can stop, drive and archive it, and the
/// session survives a later revoke of the device instead of orphaning.
/// The origin tag still records the creating device for attribution; only
/// the owner moves. Multi-user machines and rows that predate the pairing
/// user keep the peer identity (the conservative arm: no local SID match,
/// no human owner).
pub(super) fn session_create_owner(
    state: &Arc<ServerState>,
    conn_peer: &Option<ConnPeer>,
    caller: &OwnerId,
) -> OwnerId {
    let human = match conn_peer {
        Some(ConnPeer::Remote {
            scope: crate::peer_policy::PeerScope::PeerDevice,
            paired_by_user: Some(paired),
            ..
        }) if state.local_user_sid().as_deref() == Some(paired.as_str()) => paired.clone(),
        _ => return caller.clone(),
    };
    OwnerId::new(human, crate::peer_policy::PEER_OWNER_TAG).unwrap_or_else(|_| caller.clone())
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
    match reply {
        DaemonMessage::Error(error) => {
            DaemonMessage::Error(error.redacted_for(conn.conn_peer.is_some()))
        }
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
            SessionEvent::ContextUsage { .. } => " context_usage".to_string(),
            SessionEvent::PlanUsage { .. } => " plan_usage".to_string(),
            SessionEvent::AgentTaskStarted { .. } => " agent_task_started".to_string(),
            SessionEvent::AgentTaskNotification { .. } => " agent_task_notification".to_string(),
            SessionEvent::AgentBackgroundTasksChanged { .. } => {
                " agent_background_tasks_changed".to_string()
            }
            SessionEvent::AgentTasks { .. } => " agent_tasks".to_string(),
            SessionEvent::GoalChanged { .. } => " goal_changed".to_string(),
            SessionEvent::AgentError { .. } => " agent_error".to_string(),
            SessionEvent::AgentStderr { .. } => " agent_stderr".to_string(),
            SessionEvent::PermissionRequest { .. } => " permission_request".to_string(),
            SessionEvent::PermissionResolved { .. } => " permission_resolved".to_string(),
            SessionEvent::PermissionAnswered { .. } => " permission_answered".to_string(),
            SessionEvent::SessionManifest { .. } => " session_manifest".to_string(),
            SessionEvent::SessionFeatureState { .. } => " session_feature_state".to_string(),
            SessionEvent::SessionNotice { .. } => " session_notice".to_string(),
            SessionEvent::AgentReported { .. } => " agent_reported".to_string(),
            SessionEvent::AgentCreated { .. } => " agent_created".to_string(),
            SessionEvent::AgentResumed { .. } => " agent_resumed".to_string(),
            SessionEvent::ChildFinished { .. } => " child_finished".to_string(),
            SessionEvent::Detached => " detached".to_string(),
            SessionEvent::QueueSnapshot { .. } => " queue_snapshot".to_string(),
            SessionEvent::TasksSnapshot { .. } => " tasks_snapshot".to_string(),
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
    conn_id: u64,
    idle: Option<Duration>,
) {
    loop {
        // A peer link pings on its own cadence; silence past the deadline is a
        // half-open socket, and closing the reader is what removes the
        // registry entry that would otherwise keep the device online.
        let request = match idle {
            Some(deadline) => framed.recv_timeout::<ClientMessage>(deadline),
            None => framed.recv::<ClientMessage>(),
        };
        // Arrival on disk before the request enters the queue: an arrival
        // between two dispatch markers is the time it stood in line.
        if let Ok(message) = &request {
            crate::rpc_trace::daemon_event(
                "arrival",
                message.name(),
                message.request_id(),
                conn_id,
                &[],
            );
        }
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
    // A reader deadline that passes is an idle link ending, which is a clean
    // close for the same reason a hang-up is: nothing is left to read.
    if matches!(error, DaemonError::TimedOut(_)) {
        return true;
    }
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

/// Refuse a connection the daemon is too far gone to admit, with a frame
/// rather than a silent close.
///
/// The peer path admits after the Noise handshake but before the hello, so a
/// shutdown that lands in between must still answer: the hello is read (and
/// discarded) first, then `ShuttingDown` leaves. Reading first is what keeps
/// the close a FIN — dropping the socket with the peer's hello still unread
/// resets the connection on every platform instead, and a peer that redials
/// on transport errors would never learn the daemon is going away.
pub(crate) fn refuse_shutting_down(framed: &Framed) -> Result<(), DaemonError> {
    let _ = framed.recv_timeout::<ClientMessage>(HANDSHAKE_TIMEOUT);
    send_shutting_down(framed, None)
}

fn daemon_hello(state: &ServerState) -> DaemonHello {
    DaemonHello {
        protocol_version: PROTOCOL_VERSION,
        min_protocol_version: PROTOCOL_MIN_VERSION,
        daemon_version: env!("CARGO_PKG_VERSION").to_string(),
        instance_id: state.instance_id.clone(),
        pid: std::process::id(),
        capabilities: m3a_daemon_capabilities(),
        // A hello with no workspace database to speak for: the peer entrance
        // overwrites this with the daemon's own presence before it leaves.
        workspace_host: None,
    }
}

#[cfg(test)]
#[path = "connection_tests.rs"]
mod tests;
