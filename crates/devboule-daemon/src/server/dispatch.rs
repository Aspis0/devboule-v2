//! Dispatch — `dispatch` (whose first statement is the peer gate) and the
//! routing skeleton of `dispatch_immediate` (the store domains live in
//! `stores.rs`; the vocabulary arm stays a routing call).

use super::*;

/// The eighth argument is whether the `devices` capability was negotiated.
/// `session_send` in this file already declines a parameter object for the
/// same reason: one call shape, one place to read.
#[allow(clippy::too_many_arguments)]
pub(super) fn dispatch(
    state: &Arc<ServerState>,
    owner: &OwnerId,
    request: ClientMessage,
    conn: &Arc<ConnHandle>,
    sessions_ok: bool,
    journal_ok: bool,
    typed_permissions_ok: bool,
    devices_ok: bool,
) -> Option<DaemonMessage> {
    // The peer gate is the first statement, and `run_gate` is the only place
    // a `GatePassed` can be minted (its field is private to `peer_gate.rs`).
    // Every domain handler below requires one, so an edit that routes a
    // request past this line does not compile — the property stops being a
    // convention the next change can break.
    let passed = match run_gate(state, owner, &request, conn) {
        Ok(passed) => passed,
        Err(reply) => return Some(*reply),
    };
    // A remote peer's session list is a projection, not the local list, and it
    // is derived from the *connection* rather than from the `owner` this call
    // was handed: a caller that passes something else cannot widen the
    // projection. A `Client` sees the sessions of the user it was paired by
    // (what `handle_client` computes for every other request too, §8b A3); a
    // `Daemon` sees the sessions its own device created (§8 R2); the local pipe
    // sees its own list. In all three cases the registry's single owner-user
    // filter is the whole rule.
    if let Some(ConnPeer::Remote { .. }) = &conn.conn_peer {
        if let ClientMessage::SessionsList { id } = &request {
            let projected = session_list_owner(&conn.conn_peer, owner);
            return Some(match state.sessions.list(&projected) {
                Ok(sessions) => DaemonMessage::Sessions { id: *id, sessions },
                Err(error) => DaemonMessage::Error(error.with_id(*id)),
            });
        }
    }
    if state.is_shutting_down() && !matches!(request, ClientMessage::Shutdown { .. }) {
        let mut error = WireError::new(ErrorCode::ShuttingDown, "daemon is shutting down");
        if let Some(id) = request.request_id() {
            error = error.with_id(id);
        }
        return Some(DaemonMessage::Error(error));
    }
    if let ClientMessage::ProvidersAuthCheck { id, force } = &request {
        let id = *id;
        let force = *force;
        let worker_state = Arc::clone(state);
        let outbound = Arc::clone(&conn.outbound);
        let failure_outbound = Arc::clone(&outbound);
        let spawn = std::thread::Builder::new()
            .name("daemon-provider-auth-check".to_string())
            .spawn(move || {
                // A panic in discovery or a check must still answer the
                // caller, the way the SessionCreate worker does: an
                // unanswered frame leaves the client on the full RPC
                // timeout while the panel's catch swallows the symptom.
                let reply = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    providers_reply(&worker_state, id, false, true, force)
                }))
                .unwrap_or_else(|_| {
                    DaemonMessage::Error(
                        WireError::new(ErrorCode::Io, "provider status check failed").with_id(id),
                    )
                });
                outbound.enqueue_reply(reply);
            });
        if spawn.is_err() {
            failure_outbound.enqueue_reply(DaemonMessage::Error(
                WireError::new(ErrorCode::Io, "could not start provider status check").with_id(id),
            ));
        }
        return None;
    }
    if let ClientMessage::ProvidersRefresh { id } = request {
        let worker_state = Arc::clone(state);
        let outbound = Arc::clone(&conn.outbound);
        let failure_outbound = Arc::clone(&outbound);
        let spawn = std::thread::Builder::new()
            .name("daemon-providers-refresh".to_string())
            .spawn(move || {
                let reply = providers_reply(&worker_state, id, true, false, false);
                outbound.enqueue_reply(reply);
            });
        if spawn.is_err() {
            failure_outbound.enqueue_reply(DaemonMessage::Error(
                WireError::new(ErrorCode::Io, "could not start provider refresh").with_id(id),
            ));
        }
        return None;
    }
    if matches!(&request, ClientMessage::SessionCreate { .. }) {
        let request_id = request.request_id();
        let worker_state = Arc::clone(state);
        let worker_owner = owner.clone();
        let worker_request = request;
        let worker_conn = Arc::clone(conn);
        let outbound = Arc::clone(&conn.outbound);
        let failure_outbound = Arc::clone(&outbound);
        let spawn = std::thread::Builder::new()
            .name("daemon-session-create".to_string())
            .spawn(move || {
                let worker_started = Instant::now();
                #[cfg(test)]
                if let Some((entered, release)) = worker_state
                    .session_create_test_gate
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .take()
                {
                    let _ = entered.send(());
                    let _ = release.recv_timeout(Duration::from_secs(10));
                }
                let reply = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    let _create_guard = worker_conn
                        .session_create_lock
                        .lock()
                        .unwrap_or_else(|error| error.into_inner());
                    dispatch_immediate(
                        &worker_state,
                        &worker_owner,
                        worker_request,
                        &worker_conn,
                        sessions_ok,
                        journal_ok,
                        typed_permissions_ok,
                        devices_ok,
                        &passed,
                        false,
                    )
                }))
                .unwrap_or_else(|_| {
                    DaemonMessage::Error(
                        WireError::new(ErrorCode::Io, "session creation failed")
                            .with_id(request_id.unwrap_or_default()),
                    )
                });
                outbound.enqueue_reply(reply);
                let took_ms = worker_started.elapsed().as_millis().to_string();
                crate::rpc_trace::daemon_event(
                    "dispatch_worker_end",
                    "SessionCreate",
                    request_id,
                    worker_conn.id,
                    &[("took_ms", took_ms.as_str())],
                );
            });
        if spawn.is_err() {
            if let Some(id) = request_id {
                failure_outbound.enqueue_reply(DaemonMessage::Error(
                    WireError::new(ErrorCode::Io, "could not start session creation").with_id(id),
                ));
            }
        }
        return None;
    }
    // The held remote-host link waits on a peer, so it leaves the loop the
    // same way the git-backed arms do: the request's own worker answers, and
    // the reply goes out through this connection's normal writer. A reader
    // that waited on a peer would be waiting on a reply that only this same
    // reader can deliver.
    if peer_link_dispatch::is_remote_host(&request) {
        if !conn.remote_hosts_negotiated() {
            return Some(capability_not_supported(
                request.request_id(),
                caps::REMOTE_HOSTS,
            ));
        }
        let worker_state = Arc::clone(state);
        let worker_conn = Arc::clone(conn);
        let outbound = Arc::clone(&conn.outbound);
        let failure_outbound = Arc::clone(&outbound);
        let worker_request = request;
        let spawn = std::thread::Builder::new()
            .name("daemon-remote-host".to_string())
            .spawn(move || {
                let reply = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    dispatch_remote_host(&worker_state, &worker_conn, worker_request)
                }))
                .unwrap_or_else(|_| {
                    DaemonMessage::Error(WireError::new(
                        ErrorCode::Io,
                        "the remote host read failed",
                    ))
                });
                outbound.enqueue_reply(reply);
            });
        if spawn.is_err() {
            failure_outbound.enqueue_reply(DaemonMessage::Error(WireError::new(
                ErrorCode::Io,
                "could not start the remote host read",
            )));
        }
        return None;
    }
    // Deliberately do not serialize concurrent updates: this pipe is single-user,
    // the frontend runs one npm update at a time, and npm's global lockfile
    // serializes racers.
    if let ClientMessage::ProviderUpdate { id, provider_id } = request {
        let worker_state = Arc::clone(state);
        let outbound = Arc::clone(&conn.outbound);
        let failure_outbound = Arc::clone(&outbound);
        let spawn = std::thread::Builder::new()
            .name("daemon-provider-update".to_string())
            .spawn(move || {
                let reply = provider_update_reply(&worker_state, id, &provider_id);
                outbound.enqueue_reply(reply);
            });
        if spawn.is_err() {
            failure_outbound.enqueue_reply(DaemonMessage::Error(
                WireError::new(ErrorCode::Io, "could not start provider update").with_id(id),
            ));
        }
        return None;
    }
    // The git-backed workspace arms: each runs git — a workspace arm behind
    // a 10 s probe for up to several 60 s-capped commands, `ProjectAdd` a
    // 10 s probe alone — which must never hold the loop that writes replies
    // and drains session events. They answer through the worker road: one
    // serialized queue per repository root, reads and writes on separate
    // permit lanes, queued reads coalesced and capped. The capability
    // decision stays on the loop, ahead of the key resolution, exactly
    // where dispatch_immediate made it before the offload existed.
    if is_git_backed(&request) {
        if !journal_ok {
            return Some(capability_not_supported(
                request.request_id(),
                caps::JOURNAL,
            ));
        }
        return offload(
            state,
            owner,
            request,
            conn,
            passed,
            sessions_ok,
            journal_ok,
            typed_permissions_ok,
            devices_ok,
        );
    }
    Some(dispatch_immediate(
        state,
        owner,
        request,
        conn,
        sessions_ok,
        journal_ok,
        typed_permissions_ok,
        devices_ok,
        &passed,
        false,
    ))
}

#[allow(clippy::too_many_arguments)]
pub(super) fn dispatch_immediate(
    state: &Arc<ServerState>,
    owner: &OwnerId,
    request: ClientMessage,
    conn: &Arc<ConnHandle>,
    sessions_ok: bool,
    journal_ok: bool,
    typed_permissions_ok: bool,
    devices_ok: bool,
    passed: &GatePassed,
    queued_write: bool,
) -> DaemonMessage {
    // A write the queue accepted before the flag went up must run to its
    // end: a delete stopped here would strand a half-removed checkout with
    // its row still in the journal. New requests never reach this guard —
    // dispatch refuses them on the loop before anything is queued.
    if state.is_shutting_down()
        && !matches!(request, ClientMessage::Shutdown { .. })
        && !queued_write
    {
        return DaemonMessage::Error({
            let mut error = WireError::new(ErrorCode::ShuttingDown, "daemon is shutting down");
            if let Some(id) = request.request_id() {
                error = error.with_id(id);
            }
            error
        });
    }
    match request {
        ClientMessage::Hello(_) => DaemonMessage::Error(WireError::new(
            ErrorCode::InvalidRequest,
            "hello already completed",
        )),
        // The three remote-host frames never reach here: `dispatch` hands them
        // to the link worker before this function is called. The arms below
        // exist so a future call site that does reach them gets a refusal with
        // an id rather than a panic.
        ClientMessage::RemoteHostWatch { .. }
        | ClientMessage::RemoteHostUnwatch { .. }
        | ClientMessage::RemoteHostAttach { .. }
        | ClientMessage::RemoteHostDetach { .. }
        | ClientMessage::RemoteHostCreate { .. }
        | ClientMessage::RemoteHostSend { .. }
        | ClientMessage::RemoteHostResize { .. }
        | ClientMessage::RemoteHostClaim { .. }
        | ClientMessage::RemoteHostInterrupt { .. }
        | ClientMessage::RemoteHostPermissionRespond { .. }
        | ClientMessage::RemoteHostClose { .. }
        | ClientMessage::RemoteHostStop { .. }
        | ClientMessage::RemoteHostProviders { .. }
        | ClientMessage::RemoteHostList { .. } => DaemonMessage::Error(WireError::new(
            ErrorCode::InvalidRequest,
            "remote host frames are dispatched by the async wrapper",
        )),
        // The browser host is a local app's job: the peer gate already refused
        // every peer connection, and a local connection that did not negotiate
        // `browser.host` is refused here, before the broker sees the frame.
        ClientMessage::BrowserHostRegister { .. }
        | ClientMessage::BrowserHostUnregister { .. }
        | ClientMessage::BrowserExecuteResponse { .. } => {
            if !conn.browser_host_negotiated() {
                return capability_not_supported(request.request_id(), caps::BROWSER_HOST);
            }
            dispatch_browser_host(state, conn, request)
        }
        ClientMessage::SessionPermissionRespond { .. } if !typed_permissions_ok => {
            capability_not_supported(request.request_id(), caps::TYPED_PERMISSIONS)
        }
        // A connection whose hello did not agree `session.queue` is not sent
        // the queue's events, so it must not be allowed to write one either:
        // a client that cannot read the result of a frame must not send it.
        // The gate's own per-frame refusals (the peer's capability, the
        // session's mode) run after this and still apply.
        ClientMessage::SessionQueueAdd { .. }
        | ClientMessage::SessionQueueEdit { .. }
        | ClientMessage::SessionQueueRemove { .. }
        | ClientMessage::SessionQueueMove { .. }
        | ClientMessage::SessionQueueSendNow { .. } if !conn.session_queue_negotiated() => {
            capability_not_supported(request.request_id(), caps::SESSION_QUEUE)
        }
        // The task list rides its own name the same way: a client that did
        // not agree `session.tasks` cannot parse the reply, so it must not
        // send the request either.
        ClientMessage::SessionTasksGet { .. } if !conn.session_tasks_negotiated() => {
            capability_not_supported(request.request_id(), caps::SESSION_TASKS)
        }
        ClientMessage::Ping { id } => DaemonMessage::Pong {
            id,
            ts_ms: unix_millis(),
        },
        ClientMessage::Status { id } => state.status_body(id),
        ClientMessage::DaemonDiagnostics { id } => diagnostics_reply(state, id, owner),
        ClientMessage::Shutdown { id } => {
            // The quit handshake (`request_local_shutdown`): accept and
            // enter shutdown in one atomic step, or refuse and let this
            // connection remember its own intent. Peers never count — a
            // phone neither stops the daemon out from under an app nor
            // blocks the last app out.
            match state.request_local_shutdown() {
                Ok(()) => {
                    // Paid only by a quit that was actually accepted: the
                    // checkpoint pushes coalesced frames to the disk
                    // without closing the writer, and it is one round trip
                    // to the writer bounded by the journal's RPC_WAIT —
                    // up to 10 s on this loop, not a formality. The
                    // terminal close — flush plus writer join — runs once,
                    // on the shutdown path, after the bounded drain, so a
                    // write the queue accepted (a delete's row among
                    // them) can still land its journal write. A refused
                    // quit touches no journal and replies at once.
                    state.sessions.checkpoint_journal();
                    DaemonMessage::Shutdown {
                        id,
                        accepted: true,
                        reason: None,
                    }
                }
                Err(local_clients) => {
                    // The intent to quit belongs to the connection that
                    // asked, and only a local app connection can hold it: a
                    // peer's refused shutdown memorizes nothing.
                    if conn.conn_peer.is_none() {
                        conn.mark_quit_refused();
                    }
                    let reason = format!(
                        "{local_clients} local app client(s) still connected; the daemon keeps running for them"
                    );
                    eprintln!("daemon refused Shutdown: {reason}");
                    DaemonMessage::Shutdown {
                        id,
                        accepted: false,
                        reason: Some(reason),
                    }
                }
            }
        }
        ClientMessage::JournalUsage { .. }
        | ClientMessage::JournalRetentionGet { .. }
        | ClientMessage::JournalRetentionSet { .. }
        | ClientMessage::SessionDelete { .. }
        | ClientMessage::ProjectsList { .. }
        | ClientMessage::ProjectAdd { .. }
        | ClientMessage::WorkspacesList { .. }
        // A workspace's own frames ride the journal capability with the rest
        // of the workspace inventory: the workspace id is the door, read or
        // write. The git frames, the Files panel's tree and the workspace
        // create and delete all go through the journal's workspace row; the
        // preview's stage resolves the row the same way, and its unstage
        // resolves none (it deletes copies the daemon wrote itself) and rides
        // the same grant as the panel that serves; the open root resolves
        // the row the same way and answers with the folder only.
        | ClientMessage::WorkspaceGitStatus { .. }
        | ClientMessage::WorkspaceGitDiff { .. }
        | ClientMessage::WorkspaceGitStage { .. }
        | ClientMessage::WorkspaceGitUnstage { .. }
        | ClientMessage::WorkspaceGitDiscard { .. }
        | ClientMessage::WorkspaceGitCommit { .. }
        | ClientMessage::WorkspaceGitLog { .. }
        | ClientMessage::WorkspaceFilesList { .. }
        | ClientMessage::WorkspaceFileRead { .. }
        | ClientMessage::WorkspaceOpenRoot { .. }
        | ClientMessage::WorkspaceFileRename { .. }
        | ClientMessage::WorkspaceFileDuplicate { .. }
        | ClientMessage::WorkspaceFileDelete { .. }
        | ClientMessage::WorkspaceFilePreviewStage { .. }
        | ClientMessage::WorkspaceFilePreviewUnstage { .. }
        | ClientMessage::WorkspaceCreate { .. }
        | ClientMessage::WorkspaceDelete { .. }
        | ClientMessage::WorkspaceSetTitle { .. } => {
            if !journal_ok {
                return capability_not_supported(request.request_id(), caps::JOURNAL);
            }
            dispatch_journal(state, owner, request, passed)
        }
        ClientMessage::SessionCreate { .. }
        | ClientMessage::SessionAttach { .. }
        | ClientMessage::SessionDetach { .. }
        | ClientMessage::SessionClaim { .. }
        | ClientMessage::SessionClose { .. }
        | ClientMessage::SessionStop { .. }
        | ClientMessage::SessionSend { .. }
        | ClientMessage::SessionQueueAdd { .. }
        | ClientMessage::SessionQueueEdit { .. }
        | ClientMessage::SessionQueueRemove { .. }
        | ClientMessage::SessionQueueMove { .. }
        | ClientMessage::SessionQueueSendNow { .. }
        | ClientMessage::SessionTasksGet { .. }
        | ClientMessage::AgentMessageSend { .. }
        | ClientMessage::SessionDeposit { .. }
        | ClientMessage::SessionAttachmentRead { .. }
        | ClientMessage::SessionAttachmentDelete { .. }
        | ClientMessage::SessionUploadBegin { .. }
        | ClientMessage::SessionUploadStatus { .. }
        | ClientMessage::SessionUploadChunk { .. }
        | ClientMessage::SessionUploadFinish { .. }
        | ClientMessage::SessionUploadAbort { .. }
        | ClientMessage::SessionResize { .. }
        | ClientMessage::SessionInterrupt { .. }
        | ClientMessage::SessionSetModel { .. }
        | ClientMessage::SessionSetMode { .. }
        | ClientMessage::SessionSetName { .. }
        | ClientMessage::SessionSetFeature { .. }
        | ClientMessage::SessionPermissionRespond { .. }
        | ClientMessage::SessionsList { .. }
        | ClientMessage::SessionsWatch { .. }
        | ClientMessage::SessionsUnwatch { .. }
        | ClientMessage::SessionsPresence { .. }
        | ClientMessage::PeerAgentsList { .. }
        | ClientMessage::SessionResume { .. }
        | ClientMessage::SessionReportAgent { .. } => {
            if !sessions_ok {
                return capability_not_supported(request.request_id(), caps::SESSIONS);
            }
            dispatch_session(state, owner, request, conn, typed_permissions_ok, passed)
        }
        ClientMessage::ProvidersList { id } => providers_reply(state, id, false, false, false),
        ClientMessage::ProvidersAuthCheck { id, .. } => DaemonMessage::Error(
            WireError::new(
                ErrorCode::Unimplemented,
                "ProvidersAuthCheck is dispatched by the async wrapper",
            )
            .with_id(id),
        ),
        ClientMessage::ToolPolicyGet { id } => tool_policy_get(state, id, passed),
        ClientMessage::ToolPolicySet {
            id,
            provider_id,
            enabled,
            disabled_tools,
        } => tool_policy_set(state, id, provider_id, enabled, disabled_tools, passed),
        ClientMessage::ProviderSetEnabled {
            id,
            provider_id,
            enabled,
        } => provider_set_enabled(state, id, provider_id, enabled, passed),
        ClientMessage::AgentProfilesGet { id } => agent_profiles_get(state, id, passed),
        ClientMessage::AgentProfilesSet { id, document } => {
            agent_profiles_set(state, id, document, passed)
        }
        ClientMessage::DelegationGet { id } => delegation_get(state, id, passed),
        ClientMessage::DelegationSet { id, enabled } => {
            delegation_set(state, id, enabled, passed, conn)
        }
        ClientMessage::ProviderVocabularyGet {
            id,
            provider,
            model,
            refresh,
        } => crate::provider_vocabulary::provider_vocabulary_reply(
            state,
            id,
            &provider,
            model.as_deref(),
            refresh,
        ),
        ClientMessage::DevicesList { .. }
        | ClientMessage::PairingStart { .. }
        | ClientMessage::PairingComplete { .. }
        | ClientMessage::PairingConfirm { .. }
        | ClientMessage::PeerRevoke { .. }
        | ClientMessage::PeerSetCaps { .. } => {
            if !devices_ok {
                return capability_not_supported(request.request_id(), caps::DEVICES);
            }
            dispatch_devices(state, conn, request, passed)
        }
        ClientMessage::ProvidersRefresh { id } => DaemonMessage::Error(
            WireError::new(
                ErrorCode::Unimplemented,
                "ProvidersRefresh is dispatched by the async wrapper",
            )
            .with_id(id),
        ),
        ClientMessage::ProviderUpdate { id, .. } => DaemonMessage::Error(
            WireError::new(
                ErrorCode::Unimplemented,
                "ProviderUpdate is dispatched by the async wrapper",
            )
            .with_id(id),
        ),
        ClientMessage::Invoke { id, method, .. } => DaemonMessage::Error(
            WireError::new(
                ErrorCode::Unimplemented,
                format!("this daemon is not a plugin backend; invoke '{method}' is refused"),
            )
            .with_id(id),
        ),
    }
}
