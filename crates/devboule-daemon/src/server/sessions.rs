//! Sessions domain — pass-3a split of `server.rs`: the `dispatch_session`
//! request handler, the create/send roads behind it, and the creation
//! idempotency seam (`mcp_broker.rs` reuses the three `pub(crate)` helpers).

use super::*;

pub(super) fn dispatch_session(
    state: &Arc<ServerState>,
    owner: &OwnerId,
    request: ClientMessage,
    conn: &Arc<ConnHandle>,
    typed_permissions_ok: bool,
    _passed: &GatePassed,
) -> DaemonMessage {
    match request {
        ClientMessage::SessionCreate {
            id,
            workspace_id,
            kind,
            provider,
            mode,
            display_name,
            idempotency_key,
        } => session_create(
            state,
            owner,
            &conn.conn_peer,
            id,
            workspace_id,
            kind,
            provider,
            mode,
            display_name,
            idempotency_key,
        ),
        ClientMessage::SessionAttach {
            id,
            session_id,
            subscription_id,
            from_cursor,
        } => reply_result(
            id,
            state
                .sessions
                .attach_with_subscription(
                    &session_id,
                    subscription_id,
                    from_cursor,
                    conn,
                    owner,
                    typed_permissions_ok,
                )
                .map(|()| DaemonMessage::SessionAttached {
                    id,
                    subscription_id,
                }),
        ),
        ClientMessage::SessionDetach {
            id,
            session_id,
            subscription_id,
        } => reply_result(
            id,
            state
                .sessions
                .detach_with_subscription(&session_id, subscription_id, conn, owner)
                .map(|()| DaemonMessage::Ok { id }),
        ),
        ClientMessage::SessionClaim {
            id,
            session_id,
            subscription_id,
        } => reply_result(
            id,
            state
                .sessions
                .claim_resize_with_subscription(&session_id, subscription_id, owner, conn)
                .map(|()| DaemonMessage::Ok { id }),
        ),
        ClientMessage::SessionClose {
            id,
            session_id,
            idempotency_key,
        } => {
            let fingerprint = format!("close:{session_id}");
            if let Some(reply) =
                idempotent_hit(state, owner, id, idempotency_key.as_deref(), &fingerprint)
            {
                return reply;
            }
            match state.sessions.close(&session_id, owner, &conn.conn_peer) {
                Ok(removed) => {
                    if removed {
                        state.session_finished();
                    }
                    let reply = DaemonMessage::Ok { id };
                    remember(
                        state,
                        owner,
                        idempotency_key.as_deref(),
                        &fingerprint,
                        &reply,
                    );
                    reply
                }
                Err(error) => DaemonMessage::Error(error.with_id(id)),
            }
        }
        ClientMessage::SessionsWatch { id } => {
            state.watch_sessions(owner, conn);
            DaemonMessage::Ok { id }
        }
        ClientMessage::SessionsUnwatch { id } => {
            state.unwatch_sessions(conn.id);
            conn.clear_state_events();
            DaemonMessage::Ok { id }
        }
        ClientMessage::SessionsPresence {
            id,
            focused_session_id,
            app_visible,
        } => reply_result(
            id,
            state
                .sessions
                .set_presence(conn.id, owner, focused_session_id, app_visible)
                .map(|()| DaemonMessage::Ok { id }),
        ),
        ClientMessage::SessionStop {
            id,
            session_id,
            subscription_id,
        } => reply_result(
            id,
            state
                .sessions
                .stop_with_subscription(&session_id, subscription_id, owner, conn)
                .map(|()| DaemonMessage::Ok { id }),
        ),
        ClientMessage::SessionSend {
            id,
            session_id,
            subscription_id,
            text,
            attachments,
            active_turn_behavior,
            attachment_references,
            idempotency_key,
        } => {
            // The refusal that stood here — a prompt naming stored references
            // was rejected rather than sent without them — went away with the
            // resolution path it was waiting for: `session_send` now resolves
            // every reference against the store, refuses the whole request if
            // one of them cannot be resolved, and puts the rest in the prompt
            // as paths. The guard existed so a deck could not vanish between
            // the deposit and the agent; what replaces it is the resolution,
            // not a quieter version of the same omission.
            let reply = session_send(
                state,
                owner,
                conn,
                id,
                session_id.clone(),
                subscription_id,
                text,
                attachments,
                attachment_references,
                active_turn_behavior,
                idempotency_key,
            );
            audit_peer_unauthorized(state, conn, "SessionSend", Some(session_id), &reply);
            reply
        }
        ClientMessage::AgentMessageSend {
            id,
            from_session,
            to_session,
            text,
            idempotency_key,
        } => {
            let fingerprint = format!("agent-message:{from_session}:{to_session}:{text}");
            if let Some(reply) =
                idempotent_hit(state, owner, id, idempotency_key.as_deref(), &fingerprint)
            {
                return reply;
            }
            let reply = match state.sessions.agent_message_send(
                &from_session,
                &to_session,
                &text,
                owner,
                conn,
            ) {
                Ok(()) => {
                    let reply = DaemonMessage::AgentMessageReceipt {
                        id,
                        state: AgentMessageState::Accepted,
                    };
                    remember(
                        state,
                        owner,
                        idempotency_key.as_deref(),
                        &fingerprint,
                        &reply,
                    );
                    reply
                }
                Err(error) if error.code == ErrorCode::SessionNotFound => {
                    let reply = DaemonMessage::AgentMessageReceipt {
                        id,
                        state: AgentMessageState::RejectedAbsent,
                    };
                    remember(
                        state,
                        owner,
                        idempotency_key.as_deref(),
                        &fingerprint,
                        &reply,
                    );
                    reply
                }
                Err(error) if error.code == ErrorCode::Unauthorized => {
                    // A2-07: an `Unauthorized` that reaches this dispatch is a
                    // *denied* caller, not an unpaired one. An unpaired
                    // connection never gets this far — the peer gate refuses a
                    // request the device's capability set does not open, before
                    // anything looks at what the request would do — so what
                    // arrives here is a paired device (or the person at this
                    // machine) refused the message itself, and the receipt must
                    // say that rather than blame the pairing.
                    let reply = DaemonMessage::AgentMessageReceipt {
                        id,
                        state: AgentMessageState::RejectedDenied,
                    };
                    remember(
                        state,
                        owner,
                        idempotency_key.as_deref(),
                        &fingerprint,
                        &reply,
                    );
                    reply
                }
                Err(error) => DaemonMessage::Error(error.with_id(id)),
            };
            // A receipt is not a trail: a paired device refused this message is
            // recorded like the gate's own denials (S4-01).
            audit_peer_unauthorized(state, conn, "AgentMessageSend", Some(to_session), &reply);
            reply
        }
        ClientMessage::SessionResize {
            id,
            session_id,
            subscription_id,
            cols,
            rows,
        } => reply_result(
            id,
            state
                .sessions
                .resize_with_subscription(&session_id, subscription_id, cols, rows, owner, conn)
                .map(|()| DaemonMessage::Ok { id }),
        ),
        ClientMessage::SessionsList { id } => match state.sessions.list(owner) {
            Ok(sessions) => DaemonMessage::Sessions { id, sessions },
            Err(error) => DaemonMessage::Error(error.with_id(id)),
        },
        ClientMessage::SessionReportAgent {
            id,
            session_id,
            source,
            agent,
            state: agent_state,
            message,
            seq,
            agent_session_id,
            agent_session_path,
            session_start_source,
        } => {
            let report = crate::agent_report::AgentReport {
                source,
                agent,
                state: agent_state,
                message,
                seq,
                agent_session_id,
                agent_session_path,
                session_start_source: crate::agent_report::normalize_session_start_source(
                    session_start_source,
                ),
            };
            reply_result(
                id,
                state
                    .sessions
                    .report_agent(&session_id, report, conn.peer.as_ref())
                    .map(|_| DaemonMessage::Ok { id }),
            )
        }
        ClientMessage::SessionResume {
            id, persistence, ..
        } => match persistence.kind {
            PersistenceKind::None => DaemonMessage::Resume {
                id,
                result: ResumeResult::NotSupported,
            },
            // Every resumable variant unwraps to the same handle: the tag
            // names the family that wrote the row, it never admits. Admission
            // is the journal row through `Provider::resumable()` inside
            // `resume`, so a tag/row mismatch still resumes what the row is.
            PersistenceKind::Acp { handle } | PersistenceKind::Claude { handle } => {
                match state.sessions.resume(state, &handle, owner, conn) {
                    Ok(session) => DaemonMessage::Resume {
                        id,
                        result: ResumeResult::Resumed {
                            session: Box::new(session),
                        },
                    },
                    Err(error) => DaemonMessage::Error(error.with_id(id)),
                }
            }
        },
        ClientMessage::SessionInterrupt {
            id,
            session_id,
            subscription_id,
        } => reply_result(
            id,
            state
                .sessions
                .interrupt_with_subscription(&session_id, subscription_id, owner, conn)
                .map(|()| DaemonMessage::Ok { id }),
        ),
        ClientMessage::SessionSetModel {
            id,
            session_id,
            model_id,
            effort,
        } => reply_result(
            id,
            state
                .sessions
                .set_model(&session_id, owner, model_id.as_deref(), effort.as_deref())
                .map(|()| DaemonMessage::Ok { id }),
        ),
        ClientMessage::SessionSetMode {
            id,
            session_id,
            mode_id,
        } => reply_result(
            id,
            state
                .sessions
                .set_mode(&session_id, owner, &mode_id, conn)
                .map(|()| DaemonMessage::Ok { id }),
        ),
        ClientMessage::SessionPermissionRespond {
            id,
            session_id,
            subscription_id,
            request_id,
            outcome,
            option_id,
            idempotency_key,
        } => {
            let fingerprint = format!(
                "permission:{session_id}:{request_id}:{outcome:?}:{}",
                option_id.as_deref().unwrap_or("")
            );
            if let Some(reply) =
                idempotent_hit(state, owner, id, idempotency_key.as_deref(), &fingerprint)
            {
                return reply;
            }
            match state.sessions.permission_respond_with_subscription(
                crate::session::PermissionResponse {
                    session_id: &session_id,
                    request_id: &request_id,
                    outcome,
                    option_id: option_id.as_deref(),
                },
                subscription_id,
                conn,
                owner,
            ) {
                Ok(()) => {
                    let reply = DaemonMessage::Ok { id };
                    remember(
                        state,
                        owner,
                        idempotency_key.as_deref(),
                        &fingerprint,
                        &reply,
                    );
                    reply
                }
                Err(error) => DaemonMessage::Error(error.with_id(id)),
            }
        }
        // The door to the deposit handler. `deposit` checks ownership, then the
        // wire's own limits, and only then asks the store to write — so a frame
        // this daemon refuses costs no decode and leaves no file (DEP-06).
        ClientMessage::SessionDeposit {
            id,
            session_id,
            attachment,
        } => {
            let reply = reply_result(
                id,
                state
                    .sessions
                    .deposit(&session_id, owner, conn, &attachment)
                    .map(|reference| DaemonMessage::SessionDeposited { id, reference }),
            );
            audit_peer_unauthorized(state, conn, "SessionDeposit", Some(session_id), &reply);
            reply
        }
        // Closed on purpose (`S5`, block 6): every frame that is not a
        // session-level one is named, so a new frame has to be classified here
        // rather than falling into a catch-all. The sentence is constant and
        // carries no echo of the frame; only the request id comes back.
        other @ (ClientMessage::Hello(_)
        | ClientMessage::Ping { .. }
        | ClientMessage::Status { .. }
        | ClientMessage::DaemonDiagnostics { .. }
        | ClientMessage::Shutdown { .. }
        | ClientMessage::JournalUsage { .. }
        | ClientMessage::JournalRetentionGet { .. }
        | ClientMessage::JournalRetentionSet { .. }
        | ClientMessage::SessionDelete { .. }
        | ClientMessage::ProjectsList { .. }
        | ClientMessage::ProjectAdd { .. }
        | ClientMessage::WorkspacesList { .. }
        | ClientMessage::WorkspaceCreate { .. }
        | ClientMessage::WorkspaceDelete { .. }
        | ClientMessage::ProvidersList { .. }
        | ClientMessage::ProvidersRefresh { .. }
        | ClientMessage::ProviderUpdate { .. }
        | ClientMessage::Invoke { .. }
        | ClientMessage::DevicesList { .. }
        | ClientMessage::PairingStart { .. }
        | ClientMessage::PairingComplete { .. }
        | ClientMessage::PairingConfirm { .. }
        | ClientMessage::PeerRevoke { .. }
        | ClientMessage::PeerSetCaps { .. }
        | ClientMessage::ToolPolicyGet { .. }
        | ClientMessage::ToolPolicySet { .. }
        | ClientMessage::AgentProfilesGet { .. }
        | ClientMessage::AgentProfilesSet { .. }
        | ClientMessage::ProviderVocabularyGet { .. }
        | ClientMessage::DelegationGet { .. }
        | ClientMessage::DelegationSet { .. }) => unexpected_session_frame(&other),
    }
}

/// The one sentence a frame that is not a session frame gets from the session
/// dispatcher (`S5`, block 6). Constant, and deliberately free of the frame.
pub(super) const UNEXPECTED_SESSION_FRAME: &str = "unexpected session frame";

/// The answer a frame that reached the session dispatcher without being one
/// gets (`S5`, block 6): one constant sentence and the request id.
pub(super) fn unexpected_session_frame(request: &ClientMessage) -> DaemonMessage {
    let error = WireError::new(ErrorCode::InvalidRequest, UNEXPECTED_SESSION_FRAME);
    DaemonMessage::Error(match request.request_id() {
        Some(id) => error.with_id(id),
        None => error,
    })
}

#[allow(clippy::too_many_arguments)]
fn session_create(
    state: &Arc<ServerState>,
    owner: &OwnerId,
    conn_peer: &Option<ConnPeer>,
    id: u64,
    workspace_id: Option<String>,
    kind: SessionKind,
    provider: Option<String>,
    mode: Option<String>,
    display_name: Option<String>,
    idempotency_key: Option<String>,
) -> DaemonMessage {
    // The name is checked, and trimmed, before anything is touched: the value
    // the daemon stores is the value it judged, and a name it refused is
    // refused before the idempotency table sees the request (S5-09).
    let display_name = match display_name
        .as_deref()
        .map(devboule_protocol::validate_display_name)
    {
        Some(Ok(name)) => Some(name),
        Some(Err(message)) => {
            return DaemonMessage::Error(
                WireError::new(ErrorCode::InvalidRequest, message).with_id(id),
            )
        }
        None => None,
    };
    let fingerprint = format!(
        "create:{}:{}:{}:{}:{}",
        match kind {
            SessionKind::Terminal => "terminal",
            SessionKind::Acp => "acp",
            SessionKind::Claude => "claude",
            SessionKind::Pi => "pi",
            SessionKind::Codex => "codex",
        },
        provider.as_deref().unwrap_or(""),
        workspace_id.as_deref().unwrap_or(""),
        mode.as_deref().unwrap_or(""),
        // The name is part of the payload, so a retry that changed it is a
        // different request and not the same one answered twice.
        display_name.as_deref().unwrap_or("")
    );
    if let Some(reply) = idempotent_hit(state, owner, id, idempotency_key.as_deref(), &fingerprint)
    {
        return reply;
    }
    if !state.session_started() {
        return DaemonMessage::Error(
            WireError::new(ErrorCode::ShuttingDown, "daemon is shutting down").with_id(id),
        );
    }
    match state.sessions.create(
        state,
        owner,
        workspace_id,
        kind,
        provider,
        mode,
        display_name,
        conn_peer,
    ) {
        Ok(session) => {
            let reply = DaemonMessage::Session { id, session };
            remember(
                state,
                owner,
                idempotency_key.as_deref(),
                &fingerprint,
                &reply,
            );
            reply
        }
        Err(error) => {
            state.session_finished();
            DaemonMessage::Error(error.with_id(id))
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn session_send(
    state: &Arc<ServerState>,
    owner: &OwnerId,
    conn: &ConnHandle,
    id: u64,
    session_id: String,
    subscription_id: u64,
    text: String,
    attachments: Vec<PromptAttachment>,
    attachment_references: Vec<AttachmentReference>,
    active_turn_behavior: Option<devboule_protocol::ActiveTurnBehavior>,
    idempotency_key: Option<String>,
) -> DaemonMessage {
    let fingerprint = send_fingerprint(
        &session_id,
        &text,
        &attachments,
        &attachment_references,
        active_turn_behavior,
    );
    if let Some(reply) = idempotent_hit(state, owner, id, idempotency_key.as_deref(), &fingerprint)
    {
        return reply;
    }
    // The remote-attachment refusal is not here: `dispatch` refuses an
    // attachment-carrying send from a paired device before this function runs,
    // which is what keeps the fingerprint above (and the base64 decode inside
    // it) from ever seeing those bytes (H4). A local send is unchanged.
    // The public entry point is the one that builds the `SendRequest`; it is
    // called from here rather than the private one-argument form, which would
    // leave this wrapper dead in a non-test build.
    match state.sessions.send_with_subscription_behavior(
        &session_id,
        subscription_id,
        &text,
        &attachments,
        &attachment_references,
        owner,
        conn,
        active_turn_behavior,
    ) {
        Ok(()) => {
            let reply = DaemonMessage::Ok { id };
            remember(
                state,
                owner,
                idempotency_key.as_deref(),
                &fingerprint,
                &reply,
            );
            reply
        }
        Err(error) => DaemonMessage::Error(error.with_id(id)),
    }
}

/// The idempotency fingerprint of one send.
///
/// The attachment digests belong in it because the text is not the whole
/// payload. Without them, two sends with the same text and different images
/// share a fingerprint, and the second comes back as an idempotent replay of
/// the first: the user swaps the picture, presses Generate, and gets the
/// previous answer. The count fixes how many digests follow so the text cannot
/// be mistaken for one of them.
///
/// The references' digests belong in it for the same reason and against the
/// same failure: a client that reuses one key while pointing at a different
/// stored deck would otherwise be answered from the first send's receipt and
/// the second deck would never reach the agent. Each half carries its own
/// count, so neither list can be read as the other and the text cannot be read
/// as either.
///
/// The digests are sha256 hex, not the encoded bytes. This string is stored
/// beside every key the daemon has seen and must not weigh as much as the
/// images it identifies — and a reference's digest is already the stored
/// spelling, so it is appended as it arrived rather than re-hashed.
pub(super) fn send_fingerprint(
    session_id: &str,
    text: &str,
    attachments: &[PromptAttachment],
    attachment_references: &[AttachmentReference],
    active_turn_behavior: Option<devboule_protocol::ActiveTurnBehavior>,
) -> String {
    let mut fingerprint = format!(
        "send:{session_id}:{}:{active_turn_behavior:?}",
        attachments.len()
    );
    for attachment in attachments {
        fingerprint.push(':');
        fingerprint.push_str(&crate::attachment_store::attachment_digest(attachment));
    }
    fingerprint.push(':');
    fingerprint.push_str(&attachment_references.len().to_string());
    for reference in attachment_references {
        fingerprint.push(':');
        fingerprint.push_str(&reference.digest);
    }
    fingerprint.push(':');
    fingerprint.push_str(text);
    fingerprint
}

pub(super) fn idempotent_hit(
    state: &ServerState,
    owner: &OwnerId,
    request_id: u64,
    key: Option<&str>,
    fingerprint: &str,
) -> Option<DaemonMessage> {
    let key = key?;
    if let Err(message) = validate_idempotency_key(key) {
        return Some(DaemonMessage::Error(
            WireError::new(ErrorCode::InvalidRequest, message).with_id(request_id),
        ));
    }
    let owner_key = format!("{}.{}", owner.user, owner.client);
    let mut store = state
        .idempotency
        .lock()
        .unwrap_or_else(|err| err.into_inner());
    match store.check(&owner_key, key, fingerprint, Instant::now()) {
        IdempotencyOutcome::Hit(message) => Some(rewrite_id(message, request_id)),
        IdempotencyOutcome::Conflict => Some(DaemonMessage::Error(
            WireError::new(
                ErrorCode::IdempotencyConflict,
                "idempotency key reused with a different payload",
            )
            .with_id(request_id),
        )),
        IdempotencyOutcome::Miss => None,
    }
}

/// The creation idempotency door (`S5` block 7).
///
/// Keyed on the caller's own JSON-RPC id, because §2's schema is closed and
/// defines no idempotency parameter: the frame's id is the only retry identity
/// an MCP `tools/call` has. The stored reply is the session the first call
/// created.
///
/// `None` covers a miss and a conflict: a key reused with a different payload is
/// not a retry, and answering it with the first session would be a lie about
/// what was created.
pub(crate) fn idempotent_creation_session(
    state: &ServerState,
    owner: &OwnerId,
    key: &str,
    fingerprint: &str,
) -> Option<devboule_protocol::Session> {
    let owner_key = format!("{}.{}", owner.user, owner.client);
    let mut store = state
        .idempotency
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    match store.check(&owner_key, key, fingerprint, Instant::now()) {
        IdempotencyOutcome::Hit(DaemonMessage::Session { session, .. }) => Some(session),
        _ => None,
    }
}

/// Remember the session a creation answered with, under the same key the retry
/// will arrive on.
pub(crate) fn remember_creation_session(
    state: &ServerState,
    owner: &OwnerId,
    key: &str,
    fingerprint: &str,
    session: &devboule_protocol::Session,
) {
    let owner_key = format!("{}.{}", owner.user, owner.client);
    let mut store = state
        .idempotency
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    store.remember(
        owner_key,
        key.to_string(),
        fingerprint.to_string(),
        DaemonMessage::Session {
            id: 0,
            session: session.clone(),
        },
        Instant::now(),
    );
}

/// The retry identity of one `tools/call` (`S5` block 7).
///
/// `None` when there is nothing to key on: a notification (no id), or an id
/// whose spelling cannot be an idempotency key. The caller then behaves exactly
/// as it did before this door existed, which is honest — an MCP frame offers no
/// other retry identity than its own id.
pub(crate) fn creation_retry_key(
    creator_session_id: &str,
    id: &serde_json::Value,
) -> Option<String> {
    let text = match id {
        serde_json::Value::Number(number) => number.to_string(),
        serde_json::Value::String(text) => text.clone(),
        _ => return None,
    };
    let key = format!("mcp-create-{creator_session_id}-{text}");
    devboule_protocol::validate_idempotency_key(&key)
        .ok()
        .map(|()| key)
}

pub(super) fn remember(
    state: &ServerState,
    owner: &OwnerId,
    key: Option<&str>,
    fingerprint: &str,
    reply: &DaemonMessage,
) {
    let Some(key) = key else {
        return;
    };
    if validate_idempotency_key(key).is_err() {
        return;
    }
    let owner_key = format!("{}.{}", owner.user, owner.client);
    let mut store = state
        .idempotency
        .lock()
        .unwrap_or_else(|err| err.into_inner());
    store.remember(
        owner_key,
        key.to_string(),
        fingerprint.to_string(),
        reply.clone(),
        Instant::now(),
    );
}

fn rewrite_id(message: DaemonMessage, id: u64) -> DaemonMessage {
    match message {
        DaemonMessage::Session { session, .. } => DaemonMessage::Session { id, session },
        DaemonMessage::Ok { .. } => DaemonMessage::Ok { id },
        DaemonMessage::Sessions { sessions, .. } => DaemonMessage::Sessions { id, sessions },
        DaemonMessage::Projects { projects, .. } => DaemonMessage::Projects { id, projects },
        DaemonMessage::Project { project, .. } => DaemonMessage::Project { id, project },
        DaemonMessage::Workspaces { workspaces, .. } => {
            DaemonMessage::Workspaces { id, workspaces }
        }
        DaemonMessage::Workspace { workspace, .. } => DaemonMessage::Workspace { id, workspace },
        DaemonMessage::SessionAttached {
            subscription_id, ..
        } => DaemonMessage::SessionAttached {
            id,
            subscription_id,
        },
        DaemonMessage::JournalRetention { retention, .. } => {
            DaemonMessage::JournalRetention { id, retention }
        }
        DaemonMessage::ProviderUpdated {
            ok, exit_code, log, ..
        } => DaemonMessage::ProviderUpdated {
            id,
            ok,
            exit_code,
            log,
        },
        DaemonMessage::Error(error) => DaemonMessage::Error(error.with_id(id)),
        other => other,
    }
}

fn reply_result(id: u64, result: Result<DaemonMessage, WireError>) -> DaemonMessage {
    match result {
        Ok(message) => message,
        Err(error) => DaemonMessage::Error(error.with_id(id)),
    }
}

pub(crate) fn unix_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| u64::try_from(duration.as_millis()).unwrap_or(0))
        .unwrap_or(0)
}

pub(super) fn bounded_join(handle: JoinHandle<()>, budget: Duration) {
    let deadline = Instant::now() + budget;
    while !handle.is_finished() && Instant::now() < deadline {
        std::thread::sleep(JOIN_SLICE);
    }
    if handle.is_finished() {
        let _ = handle.join();
    }
}
