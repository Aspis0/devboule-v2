//! Peer gate — pass-3a split of `server.rs`: the refusal/audit helpers
//! behind the gate (`dispatch`'s first statement calls into these).
//! Not a leaf: `dispatch.rs`'s capability arms call
//! `capability_not_supported` too.

use super::*;

/// Proof that a request reached a domain handler **through the gate**.
///
/// The field is `()` and it is private to this module, so no code outside
/// `peer_gate.rs` can construct one — not the sibling domain children, not
/// the parent, not a test. `run_gate` below is the only place a `GatePassed`
/// comes into existence, which is what turns "the gate is the only door"
/// from a convention the next edit can break into a compile error.
///
/// It proves exactly one thing: this request passed the decision point. It is
/// **not** an authorisation — the authorisation is the gate's own refusals,
/// which are unchanged. A handler holding this token still enforces
/// everything it enforced before.
pub(super) struct GatePassed(());

/// The peer gate, lifted out of `dispatch` verbatim in pass 3b.
///
/// Nothing below the gate (not the provider spawns, not the readiness check)
/// runs for a remote connection before its request has a decision
/// (`DESIGN-remote-agents.md` §8b A1). The capability set is consulted first
/// of all: it is the whole permission model for a paired device (A9/A11), and
/// a request it does not open is refused before anything looks at what the
/// request would do.
///
/// A local connection has no peer to judge, so it reaches the end and takes a
/// token too: "local, therefore allowed" is still a decision, and it is still
/// made here. That is what keeps this function the single door rather than
/// the remote-only half of one.
/// The refusal is boxed: `DaemonMessage` is 416 bytes and clippy's
/// `result_large_err` is right that every caller would carry it. The box
/// is paid only on the refusal path, which already writes an audit row;
/// the allowed path returns a zero-sized token.
pub(super) fn run_gate(
    state: &Arc<ServerState>,
    owner: &OwnerId,
    request: &ClientMessage,
    conn: &Arc<ConnHandle>,
) -> Result<GatePassed, Box<DaemonMessage>> {
    if let Some(ConnPeer::Remote { role, .. }) = &conn.conn_peer {
        match peer_allows(*role, &conn.peer_caps, request) {
            PeerDecision::Deny(reason) => {
                audit_peer_request(state, &conn.conn_peer, request, "denied");
                return Err(Box::new(capability_not_supported(
                    request.request_id(),
                    reason,
                )));
            }
            PeerDecision::Allow => {
                // The refusals that come before the mode policy, in the order
                // they need: attachments first (they are refused before the
                // idempotency fingerprint decodes anything, H4), then the
                // ownership question for a request that names a session, so the
                // policy lookups below never answer "that session exists, and
                // it is this kind" to a peer that may not reach it (H6).
                if let Some(reply) =
                    peer_refusal_before_mode(state, owner, request, &conn.conn_peer)
                {
                    audit_peer_request(state, &conn.conn_peer, request, "denied");
                    return Err(Box::new(reply));
                }
                // §8b A4/A5/R3: an allowed request that would run a session
                // without asking this machine's user is refused here, and
                // recorded as such — a paired device asking for unattended
                // execution is a different event in the trail from a device
                // asking for something it may not have.
                if let Some(reason) = peer_mode_refusal_for_conn(state, request, &conn.conn_peer) {
                    audit_peer_request(state, &conn.conn_peer, request, reason);
                    return Err(Box::new(mode_refused(request.request_id(), reason)));
                }
                // Only state-changing requests audit on success. An allowed
                // read must never write a row: a `Ping` loop would fill the
                // disk (muse M1).
                if request.is_state_changing() {
                    audit_peer_request(state, &conn.conn_peer, request, "ok");
                }
            }
        }
    }
    Ok(GatePassed(()))
}

pub(super) fn capability_not_supported(id: Option<u64>, capability: &str) -> DaemonMessage {
    let mut error = WireError::new(
        ErrorCode::CapabilityNotSupported,
        format!("capability '{capability}' was not negotiated"),
    );
    if let Some(id) = id {
        error = error.with_id(id);
    }
    DaemonMessage::Error(error)
}

/// The refusals that come *before* the mode policy, in the one order the three
/// of them need (H4, H6).
///
/// 1. A frame from a paired device that *carries* an attachment may not carry
///    one at all. It is refused here — before `session_send` builds the
///    idempotency fingerprint, which hashes every attachment by base64-decoding
///    it, and before anything looks at a session or a store. Refusing after the
///    fingerprint meant a large base64 field still cost decoder time and memory
///    on a request that could never succeed. A refusal is also not
///    idempotent-cached: nothing is remembered for it, so no fingerprint is
///    computed for it either. The rule is about what the frame carries, not
///    about the frame's name: `SessionSend` carries attachments only when it
///    names some (`attachments` may be empty), while a `SessionDeposit` carries
///    exactly one by construction (`attachment` is a `PromptAttachment`, never
///    an `Option`). One predicate, so both forms are refused with the same
///    sentence — `peer_policy::PEER_ATTACHMENTS_UNSUPPORTED` — and the refusal
///    reads identically whichever frame it arrives on.
/// 2. A request that names a session has its scope decided before the mode
///    lookup, so the policy gate can never answer "that session exists, and it
///    runs this provider" to a peer that may not reach it. An agent message
///    admits only the authenticated daemon peer's allowed local target; an
///    unknown target and a target owned by another peer receive the same scope
///    error, so the gate cannot become an existence oracle (§8b A1/A3).
///
/// Returns the frame to send, when there is one. `dispatch` records the audit
/// row, so the label stays in one place.
pub(super) fn peer_refusal_before_mode(
    state: &ServerState,
    owner: &OwnerId,
    request: &ClientMessage,
    conn_peer: &Option<ConnPeer>,
) -> Option<DaemonMessage> {
    // The predicate is about the frame's payload, so it is stated once for both
    // shapes: a send carries attachments only when it names some, a deposit by
    // construction. The id the refusal carries is the frame's own, exactly as
    // it was when this arm was `SessionSend`'s alone.
    let carries_attachment = match request {
        ClientMessage::SessionSend { attachments, .. } => !attachments.is_empty(),
        ClientMessage::SessionDeposit { .. } => true,
        _ => false,
    };
    if carries_attachment && !crate::session::session_origin_for(conn_peer).is_local() {
        let error = WireError::new(
            ErrorCode::InvalidRequest,
            crate::peer_policy::PEER_ATTACHMENTS_UNSUPPORTED,
        );
        return Some(DaemonMessage::Error(match request.request_id() {
            Some(id) => error.with_id(id),
            None => error,
        }));
    }
    let scope = match request {
        ClientMessage::SessionAttach { session_id, .. }
        | ClientMessage::SessionSend { session_id, .. }
        | ClientMessage::SessionSetMode { session_id, .. } => {
            state.sessions.session_scope(session_id, owner, conn_peer)
        }
        // An agent message names two sessions and its *target* is the one that
        // receives the prompt. The target classifier is the single scope rule:
        // only the allowed local target proceeds to mode policy, while a relay
        // is refused here exactly like an unknown id.
        ClientMessage::AgentMessageSend { to_session, .. } => state
            .sessions
            .agent_message_target_scope(to_session, owner, conn_peer),
        // Every other shape: no session to authorize before the mode gate,
        // which only reads a session for these four.
        _ => return None,
    };
    let error = scope.err()?;
    Some(match request.request_id() {
        Some(id) => DaemonMessage::Error(error.with_id(id)),
        None => DaemonMessage::Error(error),
    })
}

#[cfg(test)]
#[path = "peer_gate_tests.rs"]
mod tests;

/// §8b A4/A5/R3: would this request run a session without asking this machine's
/// user, or name a mode this daemon cannot vet?
///
/// `SessionCreate` names its own kind and mode in the frame, so no lookup is
/// needed — and for ACP any named mode is refused, because ACP mode ids are the
/// agent's own (`peer_policy::mode_refusal`). `SessionSetMode`, `SessionSend`
/// and `SessionAttach` name a session, and the registry answers with that
/// session's kind and the mode it is in now
/// (`SessionRegistry::session_mode_guard`): a session sitting in a
/// prompt-skipping mode refuses a remote send, and a peer refuses to switch any
/// session into one.
///
/// The answer is the audit label the refusal is recorded under, so the trail
/// says which rule fired: A5's prompt-skipping list, or R3's ACP modes.
///
/// A session this daemon does not know is not a refusal here: the request still
/// has to pass the scope check, and the answer for an unknown id is the scope
/// denial rather than a policy verdict. Scope-denied agent-message targets skip
/// this mode lookup so neither their provider mode nor their existence is
/// disclosed (§8b A1, H6).
///
/// The match below is closed over `ClientMessage` with no `_` arm: a new
/// variant does not compile until it says whether it carries a mode, which is
/// what keeps this gate from silently ignoring one. `SessionCreate` takes two
/// arms because its `mode` is optional; `AgentMessageSend` takes its own arm
/// because the session it names is its *target*, not a `session_id` field.
pub(super) fn peer_mode_refusal_for_conn(
    state: &ServerState,
    request: &ClientMessage,
    conn_peer: &Option<ConnPeer>,
) -> Option<&'static str> {
    match request {
        // A create names its own kind and mode in the frame, and ACP mode ids
        // are the agent's own: `peer_policy::mode_refusal` vets the pair.
        ClientMessage::SessionCreate {
            kind,
            mode: Some(mode),
            ..
        } => crate::peer_policy::mode_refusal(kind.clone(), mode),
        // A create that names no mode has nothing to vet: one variant, two
        // arms, because `Some(mode)` is a mode question and `None` is not.
        ClientMessage::SessionCreate { mode: None, .. } => None,
        // The profile frames are not session frames at all: they name no mode,
        // no session and no create, so there is nothing here to vet. Their
        // refusal for a peer is `peer_allows`'s, and their validation is the
        // store's.
        ClientMessage::AgentProfilesGet { .. } | ClientMessage::AgentProfilesSet { .. } => None,
        // The delegation pair is the same shape: no session, no mode, and its
        // peer refusal is `peer_allows`'s (both roles, always), its validation
        // the one-boolean store's.
        ClientMessage::DelegationGet { .. } | ClientMessage::DelegationSet { .. } => None,
        // The vocabulary query is the profile store's companion read and
        // carries no mode either; its peer refusal is `peer_allows`'s, and
        // what it reads is discovery plus the catalog, never a session.
        ClientMessage::ProviderVocabularyGet { .. } => None,
        // A set-mode asks to *switch* a session into a mode, so the session's
        // kind decides whether this daemon lets a peer name that mode at all.
        ClientMessage::SessionSetMode {
            session_id,
            mode_id,
            ..
        } => state
            .sessions
            .session_mode_guard(session_id)
            .and_then(|(kind, _)| crate::peer_policy::mode_refusal(kind, mode_id)),
        // A send puts a prompt into a session, so the mode that session is in
        // *now* is what decides.
        ClientMessage::SessionSend { session_id, .. }
        | ClientMessage::SessionAttach { session_id, .. } => {
            prompt_into_session_refusal(state, session_id)
        }
        // An agent message is a send whose session is its *target*: the target
        // receives the prompt, so the target is the session this gate vets,
        // exactly as `SessionSend`'s own session is. Spelled out rather than
        // folded into the arm above: it is the decision this slice adds.
        ClientMessage::AgentMessageSend { to_session, .. }
            if state
                .sessions
                .agent_message_target_is_mode_visible(to_session, conn_peer) =>
        {
            prompt_into_session_refusal(state, to_session)
        }
        ClientMessage::AgentMessageSend { .. } => None,
        // Every other frame carries no mode, so this gate has no verdict for it
        // — one arm per variant and no `_` arm, because a new `ClientMessage`
        // variant is a decision here. A frame that names a session but no mode
        // is the registry's business, not this gate's.
        // A read returns bytes and never reaches the agent: nothing is
        // prompted, so there is no mode to vet — the same reason the
        // deposit below is not vetted either.
        ClientMessage::SessionAttachmentRead { .. } => None,
        ClientMessage::Hello(_) => None,
        // A deposit writes bytes into a session's folder and never reaches the
        // agent: nothing is prompted, so there is no mode to vet. The send that
        // later names the deposited reference is the frame this gate stops, and
        // it is already covered by the `SessionSend` arm above.
        ClientMessage::SessionDeposit { .. } => None,
        ClientMessage::Ping { .. } => None,
        ClientMessage::Status { .. } => None,
        ClientMessage::DaemonDiagnostics { .. } => None,
        ClientMessage::Shutdown { .. } => None,
        ClientMessage::SessionDetach { .. } => None,
        ClientMessage::SessionClaim { .. } => None,
        ClientMessage::SessionClose { .. } => None,
        ClientMessage::SessionStop { .. } => None,
        ClientMessage::SessionResize { .. } => None,
        ClientMessage::SessionInterrupt { .. } => None,
        ClientMessage::SessionSetModel { .. } => None,
        ClientMessage::SessionPermissionRespond { .. } => None,
        ClientMessage::SessionReportAgent { .. } => None,
        ClientMessage::SessionsList { .. } => None,
        ClientMessage::SessionsWatch { .. } => None,
        ClientMessage::SessionsUnwatch { .. } => None,
        ClientMessage::SessionsPresence { .. } => None,
        ClientMessage::SessionResume { .. } => None,
        ClientMessage::JournalUsage { .. } => None,
        ClientMessage::JournalRetentionGet { .. } => None,
        ClientMessage::JournalRetentionSet { .. } => None,
        ClientMessage::SessionDelete { .. } => None,
        ClientMessage::ProjectsList { .. } => None,
        ClientMessage::ProjectAdd { .. } => None,
        ClientMessage::WorkspacesList { .. } => None,
        ClientMessage::WorkspaceGitStatus { .. } => None,
        // A read like the status list: it reaches no agent, so there is no
        // mode to vet here either.
        ClientMessage::WorkspaceGitDiff { .. } => None,
        // The four git writes act on this machine's disk through git —
        // same reason as the file writes below: no mode to vet (the
        // capability table is what guards them).
        ClientMessage::WorkspaceGitStage { .. } => None,
        ClientMessage::WorkspaceGitUnstage { .. } => None,
        ClientMessage::WorkspaceGitDiscard { .. } => None,
        ClientMessage::WorkspaceGitCommit { .. } => None,
        ClientMessage::WorkspaceFilesList { .. } => None,
        ClientMessage::WorkspaceFileRead { .. } => None,
        // The write acts act on this machine's own disk, never on an
        // agent: there is no mode here to vet (the capability table below
        // is what guards them). The preview's stage and unstage write and
        // delete bytes in the daemon's own previews folder for the same
        // reason.
        ClientMessage::WorkspaceFileRename { .. } => None,
        ClientMessage::WorkspaceFileDuplicate { .. } => None,
        ClientMessage::WorkspaceFileDelete { .. } => None,
        ClientMessage::WorkspaceFilePreviewStage { .. } => None,
        ClientMessage::WorkspaceFilePreviewUnstage { .. } => None,
        ClientMessage::WorkspaceCreate { .. } => None,
        ClientMessage::WorkspaceDelete { .. } => None,
        ClientMessage::ProvidersList { .. } => None,
        ClientMessage::ProvidersRefresh { .. } => None,
        ClientMessage::ProviderUpdate { .. } => None,
        ClientMessage::Invoke { .. } => None,
        ClientMessage::DevicesList { .. } => None,
        // A roster read names a device, never a mode: nothing to vet.
        ClientMessage::PeerAgentsList { .. } => None,
        ClientMessage::PairingStart { .. } => None,
        ClientMessage::PairingComplete { .. } => None,
        ClientMessage::PairingConfirm { .. } => None,
        ClientMessage::PeerRevoke { .. } => None,
        ClientMessage::PeerSetCaps { .. } => None,
        ClientMessage::ToolPolicyGet { .. } => None,
        ClientMessage::ToolPolicySet { .. } => None,
    }
}

/// The §8b A4/A5 answer for a frame that puts a prompt into `session_id`: the
/// mode that session is in *now* decides, and a session sitting in a mode that
/// skips the permission prompt refuses the prompt. `None` when the daemon has
/// no mode for the session yet, or does not know the session at all — an
/// unknown id is the ownership path's answer, not this gate's.
pub(super) fn prompt_into_session_refusal(
    state: &ServerState,
    session_id: &str,
) -> Option<&'static str> {
    state
        .sessions
        .session_mode_guard(session_id)
        .and_then(|(kind, mode)| {
            mode.map(|mode| crate::peer_policy::prompt_skipping_mode(kind, &mode))
        })
        .and_then(|skipping| skipping.then_some(crate::peer_policy::PROMPT_SKIPPING_REFUSED))
}

/// The frame a peer gets when §8b A4/A5/R3 refuses its mode choice. The label
/// picks the sentence, so what the peer reads and what the audit row says are
/// the same decision.
pub(super) fn mode_refused(id: Option<u64>, reason: &'static str) -> DaemonMessage {
    let message = if reason == crate::peer_policy::ACP_MODES_UNVETTED_REFUSED {
        crate::peer_policy::ACP_MODES_UNVETTED_MESSAGE
    } else {
        "Modes that skip the permission prompt are not available to a paired device."
    };
    let mut error = WireError::new(ErrorCode::CapabilityNotSupported, message);
    if let Some(id) = id {
        error = error.with_id(id);
    }
    DaemonMessage::Error(error)
}

/// A request a paired device was allowed to make, and that the session layer
/// then refused as unauthorized, recorded like the gate's own denials.
///
/// The peer gate writes `ok` for an allowed state-changing request *before* the
/// handler runs, so a refusal raised inside the handler would otherwise leave
/// only "the capability opened it" in the trail. "This paired device asked to
/// take a running turn away from an agent, and was refused" is exactly the event
/// the trail exists for (S4-01) — and a receipt sent back to the device is not a
/// trail.
pub(super) fn audit_peer_unauthorized(
    state: &Arc<ServerState>,
    conn: &ConnHandle,
    action: &str,
    session_id: Option<String>,
    reply: &DaemonMessage,
) {
    let Some(ConnPeer::Remote {
        device_id, role, ..
    }) = &conn.conn_peer
    else {
        return;
    };
    let refused = match reply {
        DaemonMessage::Error(error) => error.code == ErrorCode::Unauthorized,
        // A2-07: the receipt that names a *denied* caller is what this audit row
        // records. The daemon no longer answers `RejectedUnpaired` from this
        // dispatch — an unpaired connection is refused at the peer gate, before
        // any message is looked at — so the denial is this state and only this
        // one.
        DaemonMessage::AgentMessageReceipt {
            state: AgentMessageState::RejectedDenied,
            ..
        } => true,
        _ => false,
    };
    if !refused {
        return;
    }
    state.audit(AuditRecord {
        device_id: device_id.clone(),
        role: role.as_str().to_string(),
        claimed_origin: None,
        action: action.to_string(),
        session_id,
        outcome: "denied".to_string(),
    });
}

/// Audit one request that came from a remote peer.
///
/// `device_id` and `role` come from the Noise-authenticated `ConnPeer`, never
/// from the frame, and the action is the variant name. The outcome is refined
/// by [`peer_outcome`].
pub(super) fn audit_peer_request(
    state: &Arc<ServerState>,
    conn_peer: &Option<ConnPeer>,
    request: &ClientMessage,
    outcome: &str,
) {
    let Some(ConnPeer::Remote {
        device_id, role, ..
    }) = conn_peer
    else {
        return;
    };
    state.audit(AuditRecord {
        device_id: device_id.clone(),
        role: role.as_str().to_string(),
        claimed_origin: None,
        action: request.name().to_string(),
        session_id: request_session_id(request),
        outcome: peer_outcome(request, outcome).to_string(),
    });
}

/// The audit outcome for a peer request: the decision, refined by whether the
/// request named a prompt-skipping mode.
///
/// `ClientMessage::SessionCreate` is the only request in the protocol carrying
/// both a session kind and a mode, so it is the only one this can classify
/// without a session lookup — and this gate runs before any session is
/// touched. A `SessionSetMode` or `SessionSend` refusal is classified by the
/// decision that made it (`peer_mode_refusal_for_conn`), which has the registry in hand,
/// and reaches the trail through this function unchanged. The request is
/// refused either way; the two outcomes are worth distinguishing because "a
/// paired machine asked for unattended execution" is a different event in the
/// trail from "a paired machine asked for something it may not have" (design
/// §8b A5).
pub(super) fn peer_outcome(request: &ClientMessage, outcome: &str) -> &'static str {
    const REFUSED: &str = crate::peer_policy::PROMPT_SKIPPING_REFUSED;
    const ACP_REFUSED: &str = crate::peer_policy::ACP_MODES_UNVETTED_REFUSED;
    match outcome {
        "ok" => "ok",
        // The refusal already names itself: one label, one vocabulary.
        REFUSED => REFUSED,
        // Same for the ACP rule: R3's refusal is not a plain denial.
        ACP_REFUSED => ACP_REFUSED,
        "denied" => match request {
            ClientMessage::SessionCreate {
                kind,
                mode: Some(mode),
                ..
            } if crate::peer_policy::prompt_skipping_mode(kind.clone(), mode) => REFUSED,
            // A caps-denied ACP create that named a mode is the same rule: the
            // trail says why the mode was refused, not merely that the device
            // lacked `create_sessions` (R3).
            ClientMessage::SessionCreate {
                kind: SessionKind::Acp,
                mode: Some(_),
                ..
            } => ACP_REFUSED,
            _ => "denied",
        },
        // Any other word is not a decision this gate produces; the trail says
        // `denied` rather than echoing it.
        _ => "denied",
    }
}

/// The session id a request names, when it names one.
///
/// A *closed* match (`S5`, block 6): every `ClientMessage` is named, so a new
/// frame has to be classified here rather than falling silently into `None`.
/// The variants that name no session are listed one by one for the same reason.
pub(super) fn request_session_id(request: &ClientMessage) -> Option<String> {
    match request {
        ClientMessage::SessionAttach { session_id, .. }
        | ClientMessage::SessionDetach { session_id, .. }
        | ClientMessage::SessionClaim { session_id, .. }
        | ClientMessage::SessionClose { session_id, .. }
        | ClientMessage::SessionStop { session_id, .. }
        | ClientMessage::SessionSend { session_id, .. }
        | ClientMessage::SessionDeposit { session_id, .. }
        | ClientMessage::AgentMessageSend {
            to_session: session_id,
            ..
        }
        | ClientMessage::SessionResize { session_id, .. }
        | ClientMessage::SessionInterrupt { session_id, .. }
        | ClientMessage::SessionSetModel { session_id, .. }
        | ClientMessage::SessionSetMode { session_id, .. }
        | ClientMessage::SessionPermissionRespond { session_id, .. }
        | ClientMessage::SessionReportAgent { session_id, .. }
        | ClientMessage::SessionDelete { session_id, .. } => Some(session_id.clone()),
        // The reference names its session the way a frame names one: the
        // digest resolves only inside it.
        ClientMessage::SessionAttachmentRead { reference, .. } => {
            Some(reference.session_id.clone())
        }
        // No session is named: a connection-level frame, a daemon-level one, a
        // workspace or provider one, or a pairing one. Named individually on
        // purpose — a new frame must be classified here, not inherit `None`.
        ClientMessage::Hello(_)
        | ClientMessage::Ping { .. }
        | ClientMessage::Status { .. }
        | ClientMessage::DaemonDiagnostics { .. }
        | ClientMessage::Shutdown { .. }
        | ClientMessage::SessionCreate { .. }
        | ClientMessage::SessionsList { .. }
        | ClientMessage::SessionsWatch { .. }
        | ClientMessage::SessionsUnwatch { .. }
        | ClientMessage::SessionsPresence { .. }
        | ClientMessage::SessionResume { .. }
        | ClientMessage::JournalUsage { .. }
        | ClientMessage::JournalRetentionGet { .. }
        | ClientMessage::JournalRetentionSet { .. }
        | ClientMessage::ProjectsList { .. }
        | ClientMessage::ProjectAdd { .. }
        | ClientMessage::WorkspacesList { .. }
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
        | ClientMessage::WorkspaceDelete { .. }
        | ClientMessage::ProvidersList { .. }
        | ClientMessage::ProvidersRefresh { .. }
        | ClientMessage::ProviderUpdate { .. }
        | ClientMessage::Invoke { .. }
        | ClientMessage::DevicesList { .. }
        // Names a device, not a session on this device.
        | ClientMessage::PeerAgentsList { .. }
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
        | ClientMessage::DelegationSet { .. } => None,
    }
}
