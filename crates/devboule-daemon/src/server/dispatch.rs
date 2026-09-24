//! Dispatch — pass-3a split of `server.rs`: `dispatch` (whose first
//! statement is the peer gate) and the routing skeleton of
//! `dispatch_immediate` (the store domains moved to `stores.rs`; the
//! vocabulary arm stays a routing call). No body changes.

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
    if let ClientMessage::ProvidersRefresh { id } = request {
        let worker_state = Arc::clone(state);
        let outbound = Arc::clone(&conn.outbound);
        let failure_outbound = Arc::clone(&outbound);
        let spawn = std::thread::Builder::new()
            .name("daemon-providers-refresh".to_string())
            .spawn(move || {
                let reply = providers_reply(&worker_state, id, true);
                outbound.enqueue_reply(reply);
            });
        if spawn.is_err() {
            failure_outbound.enqueue_reply(DaemonMessage::Error(
                WireError::new(ErrorCode::Io, "could not start provider refresh").with_id(id),
            ));
        }
        return None;
    }
    // Deliberately do not serialize concurrent updates: this pipe is single-user,
    // the frontend runs one npm update at a time, and npm's global lockfile
    // serializes racers. Revisit if the daemon becomes multi-client.
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
) -> DaemonMessage {
    if state.is_shutting_down() && !matches!(request, ClientMessage::Shutdown { .. }) {
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
        ClientMessage::SessionPermissionRespond { .. } if !typed_permissions_ok => {
            capability_not_supported(request.request_id(), caps::TYPED_PERMISSIONS)
        }
        ClientMessage::Ping { id } => DaemonMessage::Pong {
            id,
            ts_ms: unix_millis(),
        },
        ClientMessage::Status { id } => state.status_body(id),
        ClientMessage::DaemonDiagnostics { id } => diagnostics_reply(state, id, owner),
        ClientMessage::Shutdown { id } => {
            // The quit handshake (`request_local_shutdown`): accept and
            // enter shutdown in one atomic step, or refuse and remember.
            // Peers never count — a phone neither stops the daemon out from
            // under an app nor blocks the last app out.
            match state.request_local_shutdown() {
                Ok(()) => {
                    // The reply is the app's last chance to know the journal
                    // is on disk. Flush before accepting so a follow-up
                    // kill/restart cannot race the shutdown path.
                    state.sessions.flush_journal();
                    DaemonMessage::Shutdown {
                        id,
                        accepted: true,
                        reason: None,
                    }
                }
                Err(local_clients) => {
                    let reason = format!(
                        "{local_clients} local app client(s) still connected; the daemon keeps                          running for them"
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
        // A workspace's own frames resolve the journal's workspace row by id —
        // the four reads (the git status, one file's diff, one folder's
        // entries, one file's content) and the three write acts of the Files
        // panel (a rename, a duplicate, the delete) — so all seven ride the
        // journal capability with the rest of the workspace inventory: the id
        // is the door, read or write. The preview's stage resolves the row the
        // same way; its unstage resolves none (it deletes copies the daemon
        // wrote itself) and rides the same grant as the panel that serves.
        | ClientMessage::WorkspaceGitStatus { .. }
        | ClientMessage::WorkspaceGitDiff { .. }
        | ClientMessage::WorkspaceGitStage { .. }
        | ClientMessage::WorkspaceGitUnstage { .. }
        | ClientMessage::WorkspaceGitDiscard { .. }
        | ClientMessage::WorkspaceGitCommit { .. }
        | ClientMessage::WorkspaceFilesList { .. }
        | ClientMessage::WorkspaceFileRead { .. }
        | ClientMessage::WorkspaceFileRename { .. }
        | ClientMessage::WorkspaceFileDuplicate { .. }
        | ClientMessage::WorkspaceFileDelete { .. }
        | ClientMessage::WorkspaceFilePreviewStage { .. }
        | ClientMessage::WorkspaceFilePreviewUnstage { .. }
        | ClientMessage::WorkspaceCreate { .. }
        | ClientMessage::WorkspaceDelete { .. } => {
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
        | ClientMessage::AgentMessageSend { .. }
        | ClientMessage::SessionDeposit { .. }
        | ClientMessage::SessionAttachmentRead { .. }
        | ClientMessage::SessionResize { .. }
        | ClientMessage::SessionInterrupt { .. }
        | ClientMessage::SessionSetModel { .. }
        | ClientMessage::SessionSetMode { .. }
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
        ClientMessage::ProvidersList { id } => providers_reply(state, id, false),
        ClientMessage::ToolPolicyGet { id } => tool_policy_get(state, id, passed),
        ClientMessage::ToolPolicySet {
            id,
            provider_id,
            enabled,
            disabled_tools,
        } => tool_policy_set(state, id, provider_id, enabled, disabled_tools, passed),
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
            refresh,
        } => crate::provider_vocabulary::provider_vocabulary_reply(state, id, &provider, refresh),
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
        ClientMessage::ProvidersRefresh { .. } => {
            unreachable!("ProvidersRefresh is dispatched by the async wrapper")
        }
        ClientMessage::ProviderUpdate { .. } => {
            unreachable!("ProviderUpdate is dispatched by the async wrapper")
        }
        ClientMessage::Invoke { id, method, .. } => DaemonMessage::Error(
            WireError::new(
                ErrorCode::Unimplemented,
                format!("this daemon is not a plugin backend; invoke '{method}' is refused"),
            )
            .with_id(id),
        ),
    }
}
