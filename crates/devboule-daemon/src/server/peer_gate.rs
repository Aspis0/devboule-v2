//! Peer gate: the refusal/audit helpers behind the gate (`dispatch`'s
//! first statement calls into these).
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
    if let Some(ConnPeer::Remote { .. }) = &conn.conn_peer {
        match peer_allows(&conn.peer_caps, request) {
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
                // disk.
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
///    one at all — unless it is the pairing human moving their own bytes
///    into their own session (P1-3/P2-6: human scope). A human send travels
///    like a local one: the same size/EXIF/limit validation at the handler
///    and the same owner budget at the store. Anything else is refused here —
///    before `session_send` builds the idempotency fingerprint, which hashes
///    every attachment by base64-decoding it, and before anything looks at a
///    session or a store. Refusing after the fingerprint meant a large base64
///    field still cost decoder time and memory on a request that could never
///    succeed. A refusal is also not idempotent-cached: nothing is remembered
///    for it, so no fingerprint is computed for it either. The rule is about
///    what the frame carries, not about the frame's name: `SessionSend`
///    carries attachments only when it names some (`attachments` may be
///    empty), while a `SessionDeposit` carries exactly one by construction
///    (`attachment` is a `PromptAttachment`, never an `Option`). One
///    predicate, so both forms are refused with the same sentence —
///    `peer_policy::PEER_ATTACHMENTS_UNSUPPORTED` — and the refusal reads
///    identically whichever frame it arrives on. Staging new bytes
///    (`SessionDeposit`, the upload frames, the delete) stays refused for
///    every peer: no remote UI stages them, and the deposit counter that
///    would account them is still the recorded TODO.
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
    // The predicate is about the frame's payload, so it is stated once for every
    // shape that can carry one: a send and a queue add carry attachments only
    // when they name some, a deposit by construction. The id the refusal carries
    // is the frame's own, exactly as it was when this arm was `SessionSend`'s
    // alone.
    let carries_attachment = match request {
        ClientMessage::SessionSend { attachments, .. }
        | ClientMessage::SessionQueueAdd { attachments, .. } => !attachments.is_empty(),
        ClientMessage::SessionDeposit { .. } => true,
        // Every upload frame belongs to the flow that carries one, including
        // the ones that carry no bytes themselves: refusing only the chunk
        // would leave an open upload record and a part file behind.
        ClientMessage::SessionUploadBegin { .. }
        | ClientMessage::SessionUploadStatus { .. }
        | ClientMessage::SessionUploadChunk { .. }
        | ClientMessage::SessionUploadFinish { .. }
        | ClientMessage::SessionUploadAbort { .. } => true,
        // The delete removes bytes the same road carried in; a paired device
        // gets the same refusal the upload frames get.
        ClientMessage::SessionAttachmentDelete { .. } => true,
        _ => false,
    };
    if carries_attachment && !crate::session::session_origin_for(conn_peer).is_local() {
        // Human scope: the pairing human's own send — inline images and
        // references alike — travels like a local one. Machine-peer scope
        // only (client devices keep the refusal); the session lookup below
        // still runs, so an unreachable target answers the scope denial,
        // never an existence oracle, exactly as without attachments.
        let machine_peer = matches!(
            conn_peer,
            Some(ConnPeer::Remote {
                scope: crate::peer_policy::PeerScope::PeerDevice,
                ..
            })
        );
        let human = machine_peer
            && match request {
                ClientMessage::SessionSend { session_id, .. }
                | ClientMessage::SessionQueueAdd { session_id, .. } => state
                    .sessions
                    .session_owner_user(session_id)
                    .is_some_and(|owner| {
                        crate::peer_policy::paired_human_scope(conn_peer, owner.as_str())
                    }),
                _ => false,
            };
        if !human {
            let error = WireError::new(
                ErrorCode::InvalidRequest,
                crate::peer_policy::PEER_ATTACHMENTS_UNSUPPORTED,
            );
            return Some(DaemonMessage::Error(match request.request_id() {
                Some(id) => error.with_id(id),
                None => error,
            }));
        }
    }
    // File content writes author bytes anywhere in a checkout — new power
    // beside the file mutations the grant already opens. This side
    // checks only the link: a machine peer (`PeerDevice` scope) to a
    // device a local user paired. That is NOT proof the paired human
    // sent the bytes — no frame carries who on the far machine acted —
    // it is the strongest statement the receiving side can make. The
    // sending side confines relay initiation to its local app (the
    // `app_only` door on every `RemoteHost*` frame); this side verifies
    // the link, never the payload. Client-scoped devices, unknown links and (by the policy
    // above) every non-`admin` grant keep the refusal. Reads stay on
    // the grant alone.
    if let ClientMessage::WorkspaceFileWrite { .. } = request {
        if let Some(ConnPeer::Remote {
            scope,
            paired_by_user,
            ..
        }) = conn_peer
        {
            let relay =
                *scope == crate::peer_policy::PeerScope::PeerDevice && paired_by_user.is_some();
            if !relay {
                let error = WireError::new(
                    ErrorCode::CapabilityNotSupported,
                    crate::peer_policy::PEER_FILE_WRITE_HUMAN_ONLY,
                );
                return Some(DaemonMessage::Error(match request.request_id() {
                    Some(id) => error.with_id(id),
                    None => error,
                }));
            }
        }
    }
    let scope = match request {
        // Attaching is reading: a machine peer reaches the pairing user's
        // sessions here, and only here.
        ClientMessage::SessionAttach { session_id, .. } => state
            .sessions
            .session_scope_observing(session_id, owner, conn_peer),
        ClientMessage::SessionSend { session_id, .. }
        | ClientMessage::SessionQueueAdd { session_id, .. }
        | ClientMessage::SessionQueueEdit { session_id, .. }
        | ClientMessage::SessionQueueRemove { session_id, .. }
        | ClientMessage::SessionQueueMove { session_id, .. }
        | ClientMessage::SessionQueueSendNow { session_id, .. }
        | ClientMessage::SessionSetMode { session_id, .. }
        | ClientMessage::SessionSetName { session_id, .. }
        | ClientMessage::SessionSetFeature { session_id, .. } => {
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
    // Human scope (P1-3): the pairing human choosing a mode for their own
    // session — at create or by switch — is the human acting, like a local
    // mode choice. Session frames key on the machine-peer scope (this
    // slice's topology: the human's other PC); client-scoped devices keep
    // every peer rule, including R3's ACP vetting. The scope gate already
    // ordered refusals before this lookup, so an unreachable target never
    // reaches the comparison below. Agent/tool commands that run unattended
    // stay carded at the broker, which vets the run, not the frame.
    let human_session = |session_id: &str| {
        matches!(
            conn_peer,
            Some(ConnPeer::Remote {
                scope: crate::peer_policy::PeerScope::PeerDevice,
                ..
            })
        ) && state
            .sessions
            .session_owner_user(session_id)
            .is_some_and(|owner| crate::peer_policy::paired_human_scope(conn_peer, owner.as_str()))
    };
    // A create has no session yet: human means the pairing is this user's
    // own on a machine peer — exactly when the create will be human-owned
    // (P1-2). Client-scoped devices keep the vetting even same-user: the
    // comment above promises it, and R3's ACP rule pins it.
    let human_create = match conn_peer {
        Some(ConnPeer::Remote {
            scope: crate::peer_policy::PeerScope::PeerDevice,
            paired_by_user: Some(paired),
            ..
        }) => state.local_user_sid().as_deref() == Some(paired.as_str()),
        _ => false,
    };
    match request {
        // A create names its own kind and mode in the frame, and ACP mode ids
        // are the agent's own: `peer_policy::mode_refusal` vets the pair —
        // unless the pairing human names it for their own session.
        ClientMessage::SessionCreate {
            kind,
            mode: Some(mode),
            ..
        } if !human_create => crate::peer_policy::mode_refusal(kind.clone(), mode),
        ClientMessage::SessionCreate { .. } => None,
        // The profile frames are not session frames at all: they name no mode,
        // no session and no create, so there is nothing here to vet. Their
        // refusal for a peer is `peer_allows`'s, and their validation is the
        // store's.
        ClientMessage::AgentProfilesGet { .. } | ClientMessage::AgentProfilesSet { .. } => None,
        // The delegation pair is the same shape: no session, no mode, and its
        // peer refusal is `peer_allows`'s (every capability set), its validation
        // the one-boolean store's.
        ClientMessage::DelegationGet { .. } | ClientMessage::DelegationSet { .. } => None,
        // The vocabulary query is the profile store's companion read and
        // carries no mode either; its peer refusal is `peer_allows`'s, and
        // what it reads is discovery plus the catalog, never a session.
        ClientMessage::ProviderVocabularyGet { .. } => None,
        // A set-mode asks to *switch* a session into a mode, so the session's
        // kind decides whether this daemon lets a peer name that mode at all —
        // unless the pairing human switches their own session.
        ClientMessage::SessionSetMode {
            session_id,
            mode_id,
            ..
        } if !human_session(session_id) => state
            .sessions
            .session_mode_guard(session_id)
            .and_then(|(kind, _)| crate::peer_policy::mode_refusal(kind, mode_id)),
        ClientMessage::SessionSetMode { .. } => None,
        // A send puts a prompt into a session, so the mode that session is in
        // *now* is what decides.
        ClientMessage::SessionSend { session_id, .. }
        // A queued message is that session's next prompt, so the mode it sits
        // in now is what decides — except send-now, which interrupts before it
        // writes and therefore reaches a session whose mode would have refused
        // the write. `SessionInterrupt` itself is not vetted here, and send-now
        // is its twin, so it is not either.
        | ClientMessage::SessionQueueAdd { session_id, .. }
        | ClientMessage::SessionQueueEdit { session_id, .. }
        | ClientMessage::SessionQueueRemove { session_id, .. }
        | ClientMessage::SessionQueueMove { session_id, .. }
        | ClientMessage::SessionAttach { session_id, .. }
            if !human_session(session_id) =>
        {
            prompt_into_session_refusal(state, session_id)
        }
        ClientMessage::SessionSend { .. }
        | ClientMessage::SessionQueueAdd { .. }
        | ClientMessage::SessionQueueEdit { .. }
        | ClientMessage::SessionQueueRemove { .. }
        | ClientMessage::SessionQueueMove { .. }
        | ClientMessage::SessionAttach { .. } => None,
        // The twin of `SessionInterrupt`: it takes a running turn away and
        // writes a new one, and interrupting is the act that gate does not
        // vet by mode.
        ClientMessage::SessionQueueSendNow { .. } => None,
        // An agent message is a send whose session is its *target*: the target
        // receives the prompt, so the target is the session this gate vets,
        // exactly as `SessionSend`'s own session is, spelled out rather than
        // folded into the arm above. No human bypass here: peer
        // `AgentMessageSend` frames are always agent-originated (the
        // one-shot `call_peer` dials — the human has no remote
        // agent-message road), so they stay fully vetted even for the
        // pairing human's own sessions.
        ClientMessage::AgentMessageSend { to_session, .. }
            if state
                .sessions
                .agent_message_target_is_mode_visible(to_session, conn_peer) =>
        {
            prompt_into_session_refusal(state, to_session)
        }
        ClientMessage::AgentMessageSend { .. } => None,
        // A task-list read prompts nothing and switches nothing: no mode to
        // vet, like the attachment read below it.
        ClientMessage::SessionTasksGet { .. } => None,
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
        // The upload frames write bytes into a folder and never reach the
        // agent: nothing is prompted, so there is no mode to vet.
        ClientMessage::SessionUploadBegin { .. }
        | ClientMessage::SessionUploadStatus { .. }
        | ClientMessage::SessionUploadChunk { .. }
        | ClientMessage::SessionUploadFinish { .. }
        | ClientMessage::SessionUploadAbort { .. } => None,
        // The delete removes a file and never reaches the agent either.
        ClientMessage::SessionAttachmentDelete { .. } => None,
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
        // A rename names a session and a name, never a mode: nothing to vet.
        ClientMessage::SessionSetName { .. } => None,
        ClientMessage::SessionSetFeature { .. } => None,
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
        // The commit-history read: a read like the two above.
        ClientMessage::WorkspaceGitLog { .. } => None,
        // The four git writes act on this machine's disk through git —
        // same reason as the file writes below: no mode to vet (the
        // capability table is what guards them).
        ClientMessage::WorkspaceGitStage { .. } => None,
        ClientMessage::WorkspaceGitUnstage { .. } => None,
        ClientMessage::WorkspaceGitDiscard { .. } => None,
        ClientMessage::WorkspaceGitCommit { .. } => None,
        ClientMessage::WorkspaceFilesList { .. } => None,
        ClientMessage::WorkspaceFileRead { .. } => None,
        // The editor's open and version poll are reads like the windowed
        // read above; the write acts on this machine's own disk, never on
        // an agent — no mode to vet in either case.
        ClientMessage::WorkspaceFileOpen { .. } => None,
        ClientMessage::WorkspaceFileVersion { .. } => None,
        ClientMessage::WorkspaceFileWrite { .. } => None,
        ClientMessage::AppFileOpen { .. } => None,
        ClientMessage::AppFileVersion { .. } => None,
        ClientMessage::AppFileWrite { .. } => None,
        // The open root is a folder lookup and reaches no agent — nothing
        // to vet here either.
        ClientMessage::WorkspaceOpenRoot { .. } => None,
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
        // A title and an id, never a session: nothing to vet here either.
        ClientMessage::WorkspaceSetTitle { .. } => None,
        ClientMessage::ProvidersList { .. } => None,
        ClientMessage::ProvidersAuthCheck { .. } => None,
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
        // A provider switch names a provider, never a mode: nothing to vet.
        ClientMessage::ProviderSetEnabled { .. } => None,
        // The host frames name another machine, not a session or a mode here.
        ClientMessage::RemoteHostWatch { .. } => None,
        ClientMessage::RemoteHostUnwatch { .. } => None,
        ClientMessage::RemoteHostList { .. } => None,
        ClientMessage::RemoteHostAttach { .. } => None,
        ClientMessage::RemoteHostDetach { .. } => None,
        ClientMessage::RemoteHostCreate { .. } => None,
        ClientMessage::RemoteHostSend { .. } => None,
        ClientMessage::RemoteHostResize { .. } => None,
        ClientMessage::RemoteHostClaim { .. } => None,
        ClientMessage::RemoteHostInterrupt { .. } => None,
        ClientMessage::RemoteHostPermissionRespond { .. } => None,
        ClientMessage::RemoteHostClose { .. } => None,
        ClientMessage::RemoteHostStop { .. } => None,
        ClientMessage::RemoteHostProviders { .. } => None,
        ClientMessage::RemoteHostSetModel { .. } => None,
        ClientMessage::RemoteHostSetMode { .. } => None,
        ClientMessage::RemoteHostFileOpen { .. } => None,
        ClientMessage::RemoteHostFileVersion { .. } => None,
        ClientMessage::RemoteHostFileWrite { .. } => None,
        ClientMessage::RemoteHostFilesList { .. } => None,
        ClientMessage::RemoteHostGitStatus { .. } => None,
        // The browser-host frames name no session or mode either; a peer is
        // refused them by `peer_allows` before this is asked.
        ClientMessage::BrowserHostRegister { .. } => None,
        ClientMessage::BrowserHostUnregister { .. } => None,
        ClientMessage::BrowserExecuteResponse { .. } => None,
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
/// the trail exists for, and a receipt sent back to the device is not a
/// trail.
pub(super) fn audit_peer_unauthorized(
    state: &Arc<ServerState>,
    conn: &ConnHandle,
    action: &str,
    session_id: Option<String>,
    reply: &DaemonMessage,
) {
    let Some(ConnPeer::Remote { device_id, .. }) = &conn.conn_peer else {
        return;
    };
    let refused = match reply {
        DaemonMessage::Error(error) => error.code == ErrorCode::Unauthorized,
        // The receipt that names a *denied* caller is what this audit row
        // records. The daemon does not answer `RejectedUnpaired` from this
        // dispatch: an unpaired source that is not a pairing candidate is
        // closed at the accept layer, before the Noise handshake, so the
        // denial is this state and only this one.
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
        role: crate::peer_policy::PEER_AUDIT_ROLE.to_string(),
        claimed_origin: None,
        action: action.to_string(),
        session_id,
        outcome: "denied".to_string(),
    });
}

/// Audit one request that came from a remote peer.
///
/// `device_id` comes from the Noise-authenticated `ConnPeer`, never from the
/// frame, and the action is the variant name. The outcome is refined by
/// [`peer_outcome`].
pub(super) fn audit_peer_request(
    state: &Arc<ServerState>,
    conn_peer: &Option<ConnPeer>,
    request: &ClientMessage,
    outcome: &str,
) {
    let Some(ConnPeer::Remote { device_id, .. }) = conn_peer else {
        return;
    };
    state.audit(AuditRecord {
        device_id: device_id.clone(),
        role: crate::peer_policy::PEER_AUDIT_ROLE.to_string(),
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
/// A *closed* match (`S5`, block 6): every `ClientMessage` is named, so a
/// new frame has to be classified here rather than falling silently into
/// `None`.
/// The variants that name no session are listed one by one for the same reason.
pub(super) fn request_session_id(request: &ClientMessage) -> Option<String> {
    match request {
        ClientMessage::SessionAttach { session_id, .. }
        | ClientMessage::SessionDetach { session_id, .. }
        | ClientMessage::SessionClaim { session_id, .. }
        | ClientMessage::SessionClose { session_id, .. }
        | ClientMessage::SessionStop { session_id, .. }
        | ClientMessage::SessionSend { session_id, .. }
        | ClientMessage::SessionQueueAdd { session_id, .. }
        | ClientMessage::SessionQueueEdit { session_id, .. }
        | ClientMessage::SessionQueueRemove { session_id, .. }
        | ClientMessage::SessionQueueMove { session_id, .. }
        | ClientMessage::SessionQueueSendNow { session_id, .. }
        | ClientMessage::SessionTasksGet { session_id, .. }
        | ClientMessage::SessionDeposit { session_id, .. }
        | ClientMessage::SessionUploadBegin { session_id, .. }
        | ClientMessage::SessionUploadStatus { session_id, .. }
        | ClientMessage::SessionUploadChunk { session_id, .. }
        | ClientMessage::SessionUploadFinish { session_id, .. }
        | ClientMessage::SessionUploadAbort { session_id, .. }
        | ClientMessage::AgentMessageSend {
            to_session: session_id,
            ..
        }
        | ClientMessage::SessionResize { session_id, .. }
        | ClientMessage::SessionInterrupt { session_id, .. }
        | ClientMessage::SessionSetModel { session_id, .. }
        | ClientMessage::SessionSetMode { session_id, .. }
        | ClientMessage::SessionSetName { session_id, .. }
        | ClientMessage::SessionSetFeature { session_id, .. }
        | ClientMessage::SessionPermissionRespond { session_id, .. }
        | ClientMessage::SessionReportAgent { session_id, .. }
        | ClientMessage::SessionDelete { session_id, .. } => Some(session_id.clone()),
        // The reference names its session the way a frame names one: the
        // digest resolves only inside it.
        ClientMessage::SessionAttachmentRead { reference, .. }
        | ClientMessage::SessionAttachmentDelete { reference, .. } => {
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
        | ClientMessage::WorkspaceGitLog { .. }
        | ClientMessage::WorkspaceGitStage { .. }
        | ClientMessage::WorkspaceGitUnstage { .. }
        | ClientMessage::WorkspaceGitDiscard { .. }
        | ClientMessage::WorkspaceGitCommit { .. }
        | ClientMessage::WorkspaceFilesList { .. }
        | ClientMessage::WorkspaceFileRead { .. }
        | ClientMessage::WorkspaceOpenRoot { .. }
        | ClientMessage::WorkspaceFileRename { .. }
        | ClientMessage::WorkspaceFileDuplicate { .. }
        | ClientMessage::WorkspaceFileDelete { .. }
        | ClientMessage::WorkspaceFilePreviewStage { .. }
        | ClientMessage::WorkspaceFilePreviewUnstage { .. }
        | ClientMessage::WorkspaceFileOpen { .. }
        | ClientMessage::WorkspaceFileVersion { .. }
        | ClientMessage::WorkspaceFileWrite { .. }
        | ClientMessage::AppFileOpen { .. }
        | ClientMessage::AppFileVersion { .. }
        | ClientMessage::AppFileWrite { .. }
        | ClientMessage::WorkspaceCreate { .. }
        | ClientMessage::WorkspaceDelete { .. }
        | ClientMessage::WorkspaceSetTitle { .. }
        | ClientMessage::ProvidersList { .. }
        | ClientMessage::ProvidersAuthCheck { .. }
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
        | ClientMessage::ProviderSetEnabled { .. }
        | ClientMessage::AgentProfilesGet { .. }
        | ClientMessage::AgentProfilesSet { .. }
        | ClientMessage::ProviderVocabularyGet { .. }
        | ClientMessage::DelegationGet { .. }
        | ClientMessage::DelegationSet { .. }
        // The host frames name no session on this device at all: they ask
        // about another machine's. Nothing to vet here; their peer refusal is
        // `peer_allows`'s.
        | ClientMessage::RemoteHostWatch { .. }
        | ClientMessage::RemoteHostUnwatch { .. }
        | ClientMessage::RemoteHostList { .. }
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
        | ClientMessage::RemoteHostSetModel { .. }
        | ClientMessage::RemoteHostSetMode { .. }
        | ClientMessage::RemoteHostFileOpen { .. }
        | ClientMessage::RemoteHostFileVersion { .. }
        | ClientMessage::RemoteHostFileWrite { .. }
        | ClientMessage::RemoteHostFilesList { .. }
        | ClientMessage::RemoteHostGitStatus { .. }
        // Likewise the browser host's three: they name this machine's own
        // browser, never a session; their peer refusal is `peer_allows`'s.
        | ClientMessage::BrowserHostRegister { .. }
        | ClientMessage::BrowserHostUnregister { .. }
        | ClientMessage::BrowserExecuteResponse { .. } => None,
    }
}
