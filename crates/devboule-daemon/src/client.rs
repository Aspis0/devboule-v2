use std::collections::HashMap;
use std::fs::File;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use devboule_protocol::{
    ActiveTurnBehavior, AgentActivityState, AttachmentReference, ClientHello, ClientMessage,
    Cursor, DaemonHello, DaemonMessage, DaemonStatusBody, ErrorCode, JournalRetention,
    JournalUsage, OwnerId, PairingSecret, PeerRole, PeerRow, PermissionOutcome, Persistence,
    Project, PromptAttachment, ProviderInfo, ResumeResult, RetentionPatch, Session, SessionEvent,
    SessionEventEnvelope, SessionKind, SessionStateSnapshot, StoredAttachment, SubscriptionId,
    WireError, Workspace, WorkspaceDirectory, WorkspaceFileContent, WorkspaceFileMutation,
    WorkspaceFilePreview, WorkspaceGitFileDiff, WorkspaceGitStatus, WorkspaceIsolation,
};

use crate::diagnostics::DiagnosticsReport;
use crate::error::DaemonError;
use crate::framing::Framed;
use crate::paths::RuntimePaths;
use crate::spawn::{resolve_daemon_binary, spawn_daemon};
use crate::transport;

const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(2);
const RPC_TIMEOUT: Duration = Duration::from_secs(30);
const PROVIDER_UPDATE_RPC_TIMEOUT: Duration = Duration::from_secs(240);
const SPAWN_ATTEMPTS: u32 = 50;
const SPAWN_SLEEP: Duration = Duration::from_millis(100);
const JOIN_BUDGET: Duration = Duration::from_millis(500);

/// The budget `session_resume` carries, and why it is not [`RPC_TIMEOUT`].
///
/// The daemon answers this RPC only after it has run a provider startup
/// inline: `initialize` is awaited with
/// `session::acp_client::ACP_FIRST_RESPONSE_TIMEOUT` (120 s, the bound widened
/// for the measured 20.7 s cold `npx` start) and the `session/load` or
/// `session/new` behind it with `session::acp_client::ACP_RESPONSE_TIMEOUT`
/// (15 s). The recovery road adds the journal read and the replacement
/// session's own startup, which carries the same two bounds — so a resume the
/// provider refuses can cost **two** startups, and 2 × 135 s is what this
/// budget is sized against. Measured 2026-09-21 in the app: the window showed
/// `timed out: waiting for a daemon reply` at 30 s while the daemon answered
/// at 36 s — the client gave up on work the daemon was still doing and threw
/// the answer away. The same mistake one order of magnitude later is what the
/// 300 s here exists to prevent; it is deliberately no longer ranked against
/// [`PROVIDER_UPDATE_RPC_TIMEOUT`], whose work (a package install) is
/// unrelated to how many providers one resume starts.
///
/// The measurement supports 36 s for the single-startup road; 300 s is the
/// daemon's own two bounds declared as a ceiling, plus 30 s of journal and
/// queue work — not a measurement of its own.
const SESSION_RESUME_RPC_TIMEOUT: Duration = Duration::from_secs(300);

/// The budget `session_create` carries, and why it is not [`RPC_TIMEOUT`].
///
/// The daemon cannot answer `SessionCreate` before the provider startup it
/// runs inline has finished, and that startup can cross **five** awaited
/// replies: `initialize` under `ACP_FIRST_RESPONSE_TIMEOUT` (120 s), the
/// `session/new` behind it under `ACP_RESPONSE_TIMEOUT` (15 s), the
/// `session/set_mode` a creation with a mode owes when the agent declares
/// standard modes (15 s), and the delivery's confirmation — `confirm_switch`
/// reads the primary reply and, when the switch needs a follow-up, a second
/// one (15 s each). The ceiling those reads declare is 180 s; the
/// control-plane default gives up at 30, and the slowest cold `npx` start
/// measured in the house is 20.7 s — two thirds of it with nothing left for
/// a slower machine or an agent that answers slowly. 210 s is that 180 s
/// ceiling plus the same 30 s of journal and queue work
/// [`SESSION_RESUME_RPC_TIMEOUT`] counts, so the client does not surrender in
/// the instant the daemon's worst case ends. The measurement supports far
/// less, and this is a declared ceiling, not one.
const SESSION_CREATE_RPC_TIMEOUT: Duration = Duration::from_secs(210);

// One test's own deadline for the resume road, so the wiring can be proved
// without waiting out the production window. A thread-local rather than an
// environment variable: the test harness runs each test on its own thread, so
// one test's deadline cannot reach another's connection. `///` above the macro
// documents nothing, hence the `//` line.
#[cfg(test)]
thread_local! {
    static SESSION_RESUME_DEADLINE: std::cell::Cell<Option<Duration>> =
        const { std::cell::Cell::new(None) };
}

/// The deadline [`DaemonClient::session_resume`] sends with. Read here rather
/// than at the call site so a test can pull this one road's budget down and
/// watch the control plane keep [`RPC_TIMEOUT`]; nothing sets it outside a
/// test.
fn session_resume_deadline() -> Duration {
    #[cfg(test)]
    if let Some(deadline) = SESSION_RESUME_DEADLINE.with(std::cell::Cell::get) {
        return deadline;
    }
    SESSION_RESUME_RPC_TIMEOUT
}

// The create road's seam, for the reason the resume road's gives: one test
// pulls this budget down to watch the road use its own window instead of the
// control-plane default, without waiting out the production one.
#[cfg(test)]
thread_local! {
    static SESSION_CREATE_DEADLINE: std::cell::Cell<Option<Duration>> =
        const { std::cell::Cell::new(None) };
}

/// The deadline [`DaemonClient::session_create_with`] sends with. Read here
/// rather than at the call site so a test can pull this one road's budget
/// down; nothing sets it outside a test.
fn session_create_deadline() -> Duration {
    #[cfg(test)]
    if let Some(deadline) = SESSION_CREATE_DEADLINE.with(std::cell::Cell::get) {
        return deadline;
    }
    SESSION_CREATE_RPC_TIMEOUT
}

pub type EventHandler = Arc<dyn Fn(SessionEventEnvelope) + Send + Sync>;
pub type SessionStateHandler = Arc<dyn Fn(Vec<SessionStateSnapshot>) + Send + Sync>;
/// The daemon-pushed delegation switch (`DelegationChanged`): the stored
/// value and where it came from. Fired for a server-initiated broadcast, so
/// unlike an RPC reply it carries no request id.
pub type DelegationChangedHandler =
    Arc<dyn Fn(bool, devboule_protocol::DelegationSource) + Send + Sync>;

struct PendingSubscription {
    subscription_id: SubscriptionId,
    session_id: String,
    handler: EventHandler,
}

struct Subscription {
    session_id: String,
    handler: EventHandler,
}

struct ClientInner {
    framed: Framed,
    next_id: AtomicU64,
    next_subscription_id: AtomicU64,
    pending: Mutex<HashMap<u64, mpsc::Sender<DaemonMessage>>>,
    pending_subscriptions: Mutex<HashMap<u64, PendingSubscription>>,
    subscriptions: Mutex<HashMap<SubscriptionId, Subscription>>,
    #[cfg(feature = "server")]
    default_subscriptions: Mutex<HashMap<String, SubscriptionId>>,
    session_state_subscription: Mutex<Option<SessionStateHandler>>,
    delegation_subscription: Mutex<Option<DelegationChangedHandler>>,
    stop: AtomicBool,
    hello: DaemonHello,
    server_pid: Option<u32>,
}

pub struct DaemonClient {
    inner: Arc<ClientInner>,
    reader: Mutex<Option<JoinHandle<()>>>,
}

impl DaemonClient {
    pub fn hello(&self) -> &DaemonHello {
        &self.inner.hello
    }

    /// Refuse an RPC whose capability the connected daemon did not advertise.
    ///
    /// The agreed set is the intersection the daemon sent back in its hello,
    /// so a capability this client offered but the daemon does not know is
    /// absent here. An RPC gated on such a name must not leave this process:
    /// the older daemon's reader cannot deserialize the frame, and the
    /// connection would die on a request it never knew how to answer. The
    /// sentence matches the daemon's mirror case, a client sending an RPC that
    /// was not negotiated (`server.rs` `capability_not_supported`).
    fn require_agreed(&self, capability: &str) -> Result<(), DaemonError> {
        if self
            .inner
            .hello
            .capabilities
            .iter()
            .any(|agreed| agreed.as_str() == capability)
        {
            return Ok(());
        }
        Err(DaemonError::Handshake(WireError::new(
            ErrorCode::CapabilityNotSupported,
            format!("capability '{capability}' was not negotiated"),
        )))
    }

    pub fn ping(&self) -> Result<u64, DaemonError> {
        let id = self.alloc_id();
        match self.roundtrip(ClientMessage::Ping { id })? {
            DaemonMessage::Pong { ts_ms, .. } => Ok(ts_ms),
            other => unexpected(other),
        }
    }

    pub fn status(&self) -> Result<DaemonStatusBody, DaemonError> {
        let id = self.alloc_id();
        match self.roundtrip(ClientMessage::Status { id })? {
            DaemonMessage::Status { body, .. } => Ok(body),
            DaemonMessage::Error(error) if error.code == ErrorCode::ConnectionLost => {
                Err(DaemonError::ConnectionLost)
            }
            DaemonMessage::Error(error) => Err(DaemonError::Handshake(error)),
            other => unexpected(other),
        }
    }

    pub fn daemon_diagnostics(&self) -> Result<DiagnosticsReport, DaemonError> {
        let id = self.alloc_id();
        match self.roundtrip(ClientMessage::DaemonDiagnostics { id })? {
            DaemonMessage::Diagnostics { report, .. } => {
                serde_json::from_value(report).map_err(|error| {
                    DaemonError::Protocol(format!("invalid diagnostics report: {error}"))
                })
            }
            DaemonMessage::Error(error) => Err(DaemonError::Handshake(error)),
            other => unexpected(other),
        }
    }

    pub fn shutdown(&self) -> Result<(), DaemonError> {
        let id = self.alloc_id();
        match self.roundtrip(ClientMessage::Shutdown { id })? {
            DaemonMessage::Shutdown { accepted, .. } if accepted => Ok(()),
            DaemonMessage::Error(error) => Err(DaemonError::Handshake(error)),
            other => unexpected(other),
        }
    }

    /// Kill the daemon at the server end of this connection. The pipe PID is
    /// captured at handshake and checked again immediately before termination;
    /// a changed identity is refused rather than risking a recycled PID.
    pub fn restart_daemon(&self) -> Result<(), DaemonError> {
        #[cfg(windows)]
        {
            let expected = self.inner.server_pid.ok_or_else(|| {
                DaemonError::Protocol(
                    "cannot prove the identity of the connected daemon".to_string(),
                )
            })?;
            crate::transport::terminate_server_process_if_identity_matches(
                self.inner
                    .framed
                    .as_file()
                    .ok_or_else(|| {
                        DaemonError::Protocol(
                            "this connection is not a named pipe; its daemon cannot be terminated \
                         by handle"
                                .to_string(),
                        )
                    })?
                    .as_ref(),
                expected,
            )
            .map_err(DaemonError::from)
        }
        #[cfg(not(windows))]
        {
            Err(DaemonError::UnsupportedPlatform)
        }
    }

    pub fn session_create(
        &self,
        workspace_id: Option<String>,
        kind: SessionKind,
        idempotency_key: Option<String>,
    ) -> Result<Session, DaemonError> {
        self.session_create_with(workspace_id, kind, None, None, idempotency_key)
    }

    pub fn session_create_with(
        &self,
        workspace_id: Option<String>,
        kind: SessionKind,
        provider: Option<String>,
        mode: Option<String>,
        idempotency_key: Option<String>,
    ) -> Result<Session, DaemonError> {
        let id = self.alloc_id();
        match self.roundtrip_with_deadline(
            ClientMessage::SessionCreate {
                id,
                workspace_id,
                kind,
                provider,
                mode,
                // A human-started session is named by the daemon's fallback in this
                // slice: nothing in the app asks for a name yet, and inventing one
                // here would put a second naming path beside the protocol field.
                display_name: None,
                idempotency_key,
            },
            session_create_deadline(),
        )? {
            DaemonMessage::Session { session, .. } => Ok(session),
            DaemonMessage::Error(error) => Err(DaemonError::Handshake(error)),
            other => unexpected(other),
        }
    }

    pub fn session_attach(
        &self,
        session_id: &str,
        from_cursor: Option<Cursor>,
        handler: EventHandler,
    ) -> Result<SubscriptionId, DaemonError> {
        let subscription_id = self.alloc_subscription_id();
        self.session_attach_with_subscription(subscription_id, session_id, from_cursor, handler)
    }

    pub fn session_attach_with_subscription(
        &self,
        subscription_id: SubscriptionId,
        session_id: &str,
        from_cursor: Option<Cursor>,
        handler: EventHandler,
    ) -> Result<SubscriptionId, DaemonError> {
        if subscription_id == 0 {
            return Err(DaemonError::Protocol(
                "subscription id must be non-zero".to_string(),
            ));
        }
        let id = self.alloc_id();
        {
            let mut subscriptions = self
                .inner
                .pending_subscriptions
                .lock()
                .unwrap_or_else(|err| err.into_inner());
            subscriptions.insert(
                id,
                PendingSubscription {
                    subscription_id,
                    session_id: session_id.to_string(),
                    handler,
                },
            );
        }
        let result = self.roundtrip(ClientMessage::SessionAttach {
            id,
            session_id: session_id.to_string(),
            subscription_id,
            from_cursor,
        });
        match result {
            Ok(DaemonMessage::SessionAttached {
                subscription_id: confirmed,
                ..
            }) if confirmed == subscription_id => Ok(confirmed),
            Ok(DaemonMessage::SessionAttached { .. }) => {
                self.remove_pending_subscription(id);
                Err(DaemonError::Protocol(
                    "daemon returned a different subscription id".to_string(),
                ))
            }
            Ok(DaemonMessage::Error(error)) => {
                self.remove_pending_subscription(id);
                Err(DaemonError::Handshake(error))
            }
            Ok(other) => {
                self.remove_pending_subscription(id);
                unexpected(other)
            }
            Err(error) => {
                self.remove_pending_subscription(id);
                Err(error)
            }
        }
    }

    // These session-id helpers exist only for the in-process server test harnesses. Client builds
    // use the subscription-bearing methods so two observers can never share an implicit default.
    #[cfg(feature = "server")]
    pub fn session_detach(&self, session_id: &str) -> Result<(), DaemonError> {
        let subscription_id = self
            .default_subscription(session_id)
            .ok_or_else(|| DaemonError::Protocol("session is not attached".to_string()))?;
        self.session_detach_with_subscription(session_id, subscription_id)
    }

    pub fn session_detach_with_subscription(
        &self,
        session_id: &str,
        subscription_id: SubscriptionId,
    ) -> Result<(), DaemonError> {
        let id = self.alloc_id();
        let result = self.roundtrip(ClientMessage::SessionDetach {
            id,
            session_id: session_id.to_string(),
            subscription_id,
        });
        match result? {
            DaemonMessage::Ok { .. } => {
                self.unsubscribe(subscription_id);
                self.remove_pending_subscription_for_id(subscription_id);
                Ok(())
            }
            DaemonMessage::Error(error) => Err(DaemonError::Handshake(error)),
            other => unexpected(other),
        }
    }

    #[cfg(feature = "server")]
    pub fn session_claim(&self, session_id: &str) -> Result<(), DaemonError> {
        self.session_claim_with_subscription(session_id, self.control_subscription_id(session_id)?)
    }

    pub fn session_claim_with_subscription(
        &self,
        session_id: &str,
        subscription_id: SubscriptionId,
    ) -> Result<(), DaemonError> {
        let id = self.alloc_id();
        match self.roundtrip(ClientMessage::SessionClaim {
            id,
            session_id: session_id.to_string(),
            subscription_id,
        })? {
            DaemonMessage::Ok { .. } => Ok(()),
            DaemonMessage::Error(error) => Err(DaemonError::Handshake(error)),
            other => unexpected(other),
        }
    }

    #[cfg(feature = "server")]
    pub fn session_close(&self, session_id: &str) -> Result<(), DaemonError> {
        let id = self.alloc_id();
        let result = self.roundtrip(ClientMessage::SessionClose {
            id,
            session_id: session_id.to_string(),
            idempotency_key: None,
        });
        if matches!(result.as_ref(), Ok(DaemonMessage::Ok { .. })) {
            self.unsubscribe_session(session_id);
        }
        match result? {
            DaemonMessage::Ok { .. } => Ok(()),
            DaemonMessage::Error(error) => Err(DaemonError::Handshake(error)),
            other => unexpected(other),
        }
    }

    pub fn session_close_with_subscription(
        &self,
        session_id: &str,
        subscription_id: SubscriptionId,
    ) -> Result<(), DaemonError> {
        let id = self.alloc_id();
        match self.roundtrip(ClientMessage::SessionClose {
            id,
            session_id: session_id.to_string(),
            idempotency_key: None,
        })? {
            DaemonMessage::Ok { .. } => {
                self.unsubscribe(subscription_id);
                self.remove_pending_subscription_for_id(subscription_id);
                Ok(())
            }
            DaemonMessage::Error(error) => Err(DaemonError::Handshake(error)),
            other => unexpected(other),
        }
    }

    #[cfg(feature = "server")]
    pub fn session_stop(&self, session_id: &str) -> Result<(), DaemonError> {
        self.session_stop_with_subscription(session_id, self.control_subscription_id(session_id)?)
    }

    pub fn session_stop_with_subscription(
        &self,
        session_id: &str,
        subscription_id: SubscriptionId,
    ) -> Result<(), DaemonError> {
        let id = self.alloc_id();
        match self.roundtrip(ClientMessage::SessionStop {
            id,
            session_id: session_id.to_string(),
            subscription_id,
        })? {
            DaemonMessage::Ok { .. } => Ok(()),
            DaemonMessage::Error(error) => Err(DaemonError::Handshake(error)),
            other => unexpected(other),
        }
    }

    #[cfg(feature = "server")]
    pub fn session_interrupt(&self, session_id: &str) -> Result<(), DaemonError> {
        self.session_interrupt_with_subscription(
            session_id,
            self.control_subscription_id(session_id)?,
        )
    }

    pub fn session_interrupt_with_subscription(
        &self,
        session_id: &str,
        subscription_id: SubscriptionId,
    ) -> Result<(), DaemonError> {
        let id = self.alloc_id();
        match self.roundtrip(ClientMessage::SessionInterrupt {
            id,
            session_id: session_id.to_string(),
            subscription_id,
        })? {
            DaemonMessage::Ok { .. } => Ok(()),
            DaemonMessage::Error(error) => Err(DaemonError::Handshake(error)),
            other => unexpected(other),
        }
    }

    pub fn session_set_model(
        &self,
        session_id: &str,
        model_id: Option<&str>,
        effort: Option<&str>,
    ) -> Result<(), DaemonError> {
        let id = self.alloc_id();
        match self.roundtrip(ClientMessage::SessionSetModel {
            id,
            session_id: session_id.to_string(),
            model_id: model_id.map(str::to_string),
            effort: effort.map(str::to_string),
        })? {
            DaemonMessage::Ok { .. } => Ok(()),
            DaemonMessage::Error(error) => Err(DaemonError::Handshake(error)),
            other => unexpected(other),
        }
    }

    pub fn session_set_mode(&self, session_id: &str, mode_id: &str) -> Result<(), DaemonError> {
        let id = self.alloc_id();
        match self.roundtrip(ClientMessage::SessionSetMode {
            id,
            session_id: session_id.to_string(),
            mode_id: mode_id.to_string(),
        })? {
            DaemonMessage::Ok { .. } => Ok(()),
            DaemonMessage::Error(error) => Err(DaemonError::Handshake(error)),
            other => unexpected(other),
        }
    }

    #[cfg(feature = "server")]
    pub fn session_send(&self, session_id: &str, text: &str) -> Result<(), DaemonError> {
        self.session_send_with_subscription(
            session_id,
            self.control_subscription_id(session_id)?,
            text,
            &[],
            &[],
            None,
        )
    }

    /// Send one prompt with the files attached to it and the pages already
    /// deposited for it.
    ///
    /// `attachments` travels as bytes (base64 inside the message), never as a
    /// path: see [`PromptAttachment`] for why.
    ///
    /// `attachment_references` names bytes that travelled in earlier frames —
    /// one entry per page the composer deposited, in the order the pages appear
    /// in the composer. The daemon resolves each against this session's own
    /// store and refuses a reference that names another session, so an empty
    /// list is the honest value for a caller that holds no reference, and it is
    /// what every send before the composer had a deposit path passed.
    ///
    /// [`PromptAttachment`]: devboule_protocol::PromptAttachment
    pub fn session_send_with_subscription(
        &self,
        session_id: &str,
        subscription_id: SubscriptionId,
        text: &str,
        attachments: &[PromptAttachment],
        attachment_references: &[AttachmentReference],
        active_turn_behavior: Option<ActiveTurnBehavior>,
    ) -> Result<(), DaemonError> {
        let id = self.alloc_id();
        match self.roundtrip(ClientMessage::SessionSend {
            id,
            session_id: session_id.to_string(),
            subscription_id,
            text: text.to_string(),
            attachments: attachments.to_vec(),
            attachment_references: attachment_references.to_vec(),
            idempotency_key: None,
            active_turn_behavior,
        })? {
            DaemonMessage::Ok { .. } => Ok(()),
            DaemonMessage::Error(error) => Err(DaemonError::Handshake(error)),
            other => unexpected(other),
        }
    }

    /// Store one prompt attachment for a session and answer the reference the
    /// send that follows names it by.
    ///
    /// One attachment per frame, and one frame per call: a page is stored by
    /// itself, so a deck is a sequence of deposits rather than one frame the
    /// frame cap would have to hold. The caller is what makes that sequence
    /// sequential; nothing here batches.
    ///
    /// The reply is the store's own statement — the digest of the bytes **as
    /// stored** (the metadata strip runs before the hash) and their size on
    /// disk. Neither is something this side can compute, which is why a deposit
    /// answers with them instead of the caller keeping its own idea of what was
    /// written.
    ///
    /// Errors arrive as an `Error` frame on the correlation id, exactly as they
    /// do for a send: an id this daemon does not know, a session the caller does
    /// not own, an attachment over the wire's per-attachment ceiling, or a store
    /// at its owner budget. The frame itself is never formatted into an error
    /// string here, and it must not be: `unexpected` prints the *reply*, and the
    /// request that carries a page's base64 is built by the caller.
    pub fn session_deposit(
        &self,
        session_id: &str,
        attachment: &PromptAttachment,
    ) -> Result<AttachmentReference, DaemonError> {
        let id = self.alloc_id();
        match self.roundtrip(ClientMessage::SessionDeposit {
            id,
            session_id: session_id.to_string(),
            attachment: attachment.clone(),
        })? {
            DaemonMessage::SessionDeposited { reference, .. } => Ok(reference),
            DaemonMessage::Error(error) => Err(DaemonError::Handshake(error)),
            other => unexpected(other),
        }
    }

    /// Read back the bytes of one deposited attachment, by reference.
    ///
    /// One reference per call, like one attachment per deposit: the reply
    /// carries at most the artifact cap, well under the frame ceiling. The
    /// digest and size are the store's to state, so the reference this
    /// takes is the value a deposit answered with, verbatim.
    ///
    /// Errors arrive as an `Error` frame on the correlation id, exactly as
    /// they do for a deposit: an id this daemon does not know, a session
    /// the caller does not own, a reference naming another session, a file
    /// the store no longer holds, or bytes over the read cap.
    pub fn session_attachment_read(
        &self,
        reference: &AttachmentReference,
    ) -> Result<StoredAttachment, DaemonError> {
        let id = self.alloc_id();
        match self.roundtrip(ClientMessage::SessionAttachmentRead {
            id,
            reference: reference.clone(),
        })? {
            DaemonMessage::SessionAttachment { attachment, .. } => Ok(attachment),
            DaemonMessage::Error(error) => Err(DaemonError::Handshake(error)),
            other => unexpected(other),
        }
    }

    #[cfg(feature = "server")]
    pub fn session_resize(
        &self,
        session_id: &str,
        cols: u16,
        rows: u16,
    ) -> Result<(), DaemonError> {
        self.session_resize_with_subscription(
            session_id,
            self.control_subscription_id(session_id)?,
            cols,
            rows,
        )
    }

    pub fn session_resize_with_subscription(
        &self,
        session_id: &str,
        subscription_id: SubscriptionId,
        cols: u16,
        rows: u16,
    ) -> Result<(), DaemonError> {
        let id = self.alloc_id();
        match self.roundtrip(ClientMessage::SessionResize {
            id,
            session_id: session_id.to_string(),
            subscription_id,
            cols,
            rows,
        })? {
            DaemonMessage::Ok { .. } => Ok(()),
            DaemonMessage::Error(error) => Err(DaemonError::Handshake(error)),
            other => unexpected(other),
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn session_report_agent(
        &self,
        session_id: &str,
        source: &str,
        agent: &str,
        state: AgentActivityState,
        seq: Option<u64>,
        agent_session_id: Option<String>,
        agent_session_path: Option<String>,
        session_start_source: Option<String>,
        message: Option<String>,
    ) -> Result<(), DaemonError> {
        let id = self.alloc_id();
        match self.roundtrip(ClientMessage::SessionReportAgent {
            id,
            session_id: session_id.to_string(),
            source: source.to_string(),
            agent: agent.to_string(),
            state,
            message,
            seq,
            agent_session_id,
            agent_session_path,
            session_start_source,
        })? {
            DaemonMessage::Ok { .. } => Ok(()),
            DaemonMessage::Error(error) => Err(DaemonError::Handshake(error)),
            other => unexpected(other),
        }
    }

    #[cfg(feature = "server")]
    pub fn session_permission_respond(
        &self,
        session_id: &str,
        request_id: &str,
        outcome: PermissionOutcome,
    ) -> Result<(), DaemonError> {
        self.session_permission_respond_with_subscription(
            session_id,
            self.control_subscription_id(session_id)?,
            request_id,
            outcome,
            None,
        )
    }

    pub fn session_permission_respond_with_subscription(
        &self,
        session_id: &str,
        subscription_id: SubscriptionId,
        request_id: &str,
        outcome: PermissionOutcome,
        option_id: Option<&str>,
    ) -> Result<(), DaemonError> {
        let id = self.alloc_id();
        match self.roundtrip(ClientMessage::SessionPermissionRespond {
            id,
            session_id: session_id.to_string(),
            subscription_id,
            request_id: request_id.to_string(),
            outcome,
            option_id: option_id.map(str::to_string),
            idempotency_key: None,
        })? {
            DaemonMessage::Ok { .. } => Ok(()),
            DaemonMessage::Error(error) => Err(DaemonError::Handshake(error)),
            other => unexpected(other),
        }
    }

    pub fn session_resume(
        &self,
        persistence: Persistence,
        idempotency_key: Option<String>,
    ) -> Result<ResumeResult, DaemonError> {
        let id = self.alloc_id();
        match self.roundtrip_with_deadline(
            ClientMessage::SessionResume {
                id,
                persistence,
                idempotency_key,
            },
            session_resume_deadline(),
        )? {
            DaemonMessage::Resume { result, .. } => Ok(result),
            DaemonMessage::Error(error) => Err(DaemonError::Handshake(error)),
            other => unexpected(other),
        }
    }

    pub fn sessions_list(&self) -> Result<Vec<Session>, DaemonError> {
        let id = self.alloc_id();
        match self.roundtrip(ClientMessage::SessionsList { id })? {
            DaemonMessage::Sessions { sessions, .. } => Ok(sessions),
            DaemonMessage::Error(error) => Err(DaemonError::Handshake(error)),
            other => unexpected(other),
        }
    }

    pub fn projects_list(&self) -> Result<Vec<Project>, DaemonError> {
        let id = self.alloc_id();
        match self.roundtrip(ClientMessage::ProjectsList { id })? {
            DaemonMessage::Projects { projects, .. } => Ok(projects),
            DaemonMessage::Error(error) => Err(DaemonError::Handshake(error)),
            other => unexpected(other),
        }
    }

    pub fn project_add(&self, path: &str) -> Result<Project, DaemonError> {
        let id = self.alloc_id();
        match self.roundtrip(ClientMessage::ProjectAdd {
            id,
            path: path.to_string(),
        })? {
            DaemonMessage::Project { project, .. } => Ok(project),
            DaemonMessage::Error(error) => Err(DaemonError::Handshake(error)),
            other => unexpected(other),
        }
    }

    pub fn workspaces_list(&self, project_id: &str) -> Result<Vec<Workspace>, DaemonError> {
        let id = self.alloc_id();
        match self.roundtrip(ClientMessage::WorkspacesList {
            id,
            project_id: project_id.to_string(),
        })? {
            DaemonMessage::Workspaces { workspaces, .. } => Ok(workspaces),
            DaemonMessage::Error(error) => Err(DaemonError::Handshake(error)),
            other => unexpected(other),
        }
    }

    /// The uncommitted working-tree state of one workspace. The id is the
    /// whole argument: the daemon resolves the directory from it, because the
    /// `path` every `Workspace` carries is display-only on the wire.
    pub fn workspace_git_status(
        &self,
        workspace_id: &str,
    ) -> Result<WorkspaceGitStatus, DaemonError> {
        let id = self.alloc_id();
        match self.roundtrip(ClientMessage::WorkspaceGitStatus {
            id,
            workspace_id: workspace_id.to_string(),
        })? {
            DaemonMessage::WorkspaceGit { status, .. } => Ok(status),
            DaemonMessage::Error(error) => Err(DaemonError::Handshake(error)),
            other => unexpected(other),
        }
    }

    /// The diff of one workspace file. The id and a relative `path` are the
    /// whole argument: the daemon resolves the directory from the id — the
    /// `path` every `Workspace` carries is display-only — and confines the
    /// rest to a file inside that directory.
    pub fn workspace_git_diff(
        &self,
        workspace_id: &str,
        path: &str,
    ) -> Result<WorkspaceGitFileDiff, DaemonError> {
        let id = self.alloc_id();
        match self.roundtrip(ClientMessage::WorkspaceGitDiff {
            id,
            workspace_id: workspace_id.to_string(),
            path: path.to_string(),
        })? {
            DaemonMessage::WorkspaceGitFile { file, .. } => Ok(file),
            DaemonMessage::Error(error) => Err(DaemonError::Handshake(error)),
            other => unexpected(other),
        }
    }

    /// The entries of one workspace folder. The id and a relative `path` are
    /// the whole argument — the empty string is the folder itself — and the
    /// daemon confines the rest to a directory inside it before opening
    /// anything.
    pub fn workspace_files_list(
        &self,
        workspace_id: &str,
        path: &str,
    ) -> Result<WorkspaceDirectory, DaemonError> {
        let id = self.alloc_id();
        match self.roundtrip(ClientMessage::WorkspaceFilesList {
            id,
            workspace_id: workspace_id.to_string(),
            path: path.to_string(),
        })? {
            DaemonMessage::WorkspaceFiles { directory, .. } => Ok(directory),
            DaemonMessage::Error(error) => Err(DaemonError::Handshake(error)),
            other => unexpected(other),
        }
    }

    /// The content of one workspace file. The id and a relative `path` are
    /// the whole argument — of an ordinary file, never a folder — and the
    /// daemon confines the rest to a directory inside it before opening
    /// anything, caps the read, and classifies the bytes.
    pub fn workspace_file_read(
        &self,
        workspace_id: &str,
        path: &str,
    ) -> Result<WorkspaceFileContent, DaemonError> {
        let id = self.alloc_id();
        match self.roundtrip(ClientMessage::WorkspaceFileRead {
            id,
            workspace_id: workspace_id.to_string(),
            path: path.to_string(),
        })? {
            DaemonMessage::WorkspaceFileContent { file, .. } => Ok(file),
            DaemonMessage::Error(error) => Err(DaemonError::Handshake(error)),
            other => unexpected(other),
        }
    }

    /// Rename one workspace entry. The id, a relative `path` — of an entry
    /// the listing handed back, never the workspace's own folder — and the
    /// new name are the whole argument; the daemon confines the path,
    /// validates the name, and answers with the entry's new spelling (or
    /// the sentence the refusal stopped on).
    pub fn workspace_file_rename(
        &self,
        workspace_id: &str,
        path: &str,
        name: &str,
    ) -> Result<WorkspaceFileMutation, DaemonError> {
        let id = self.alloc_id();
        match self.roundtrip(ClientMessage::WorkspaceFileRename {
            id,
            workspace_id: workspace_id.to_string(),
            path: path.to_string(),
            name: name.to_string(),
            idempotency_key: None,
        })? {
            DaemonMessage::WorkspaceFileRenamed { change, .. } => Ok(change),
            DaemonMessage::Error(error) => Err(DaemonError::Handshake(error)),
            other => unexpected(other),
        }
    }

    /// Duplicate one workspace entry: the daemon picks the name (`a copy`,
    /// `a copy 2`, …) and answers with the copy's spelling.
    pub fn workspace_file_duplicate(
        &self,
        workspace_id: &str,
        path: &str,
    ) -> Result<WorkspaceFileMutation, DaemonError> {
        let id = self.alloc_id();
        match self.roundtrip(ClientMessage::WorkspaceFileDuplicate {
            id,
            workspace_id: workspace_id.to_string(),
            path: path.to_string(),
            idempotency_key: None,
        })? {
            DaemonMessage::WorkspaceFileDuplicated { change, .. } => Ok(change),
            DaemonMessage::Error(error) => Err(DaemonError::Handshake(error)),
            other => unexpected(other),
        }
    }

    /// Stage one workspace file for the panel's full-size preview. The id, a
    /// workspace id and a relative `path` are the whole argument; the daemon
    /// confines the path, refuses links, the repository's metadata and any
    /// extension the panel never draws, clears its `previews` folder and
    /// copies the file there — the answer carries the copy's absolute path.
    pub fn workspace_file_preview_stage(
        &self,
        workspace_id: &str,
        path: &str,
    ) -> Result<WorkspaceFilePreview, DaemonError> {
        let id = self.alloc_id();
        match self.roundtrip(ClientMessage::WorkspaceFilePreviewStage {
            id,
            workspace_id: workspace_id.to_string(),
            path: path.to_string(),
        })? {
            DaemonMessage::WorkspaceFilePreviewStaged { staged, .. } => Ok(staged),
            DaemonMessage::Error(error) => Err(DaemonError::Handshake(error)),
            other => unexpected(other),
        }
    }

    /// Revoke the staged preview: delete every copy the stage above left.
    /// The panel sends this when the selection leaves a staged file and
    /// when it closes — revoking is deleting the copy, never withdrawing a
    /// scope (a Tauri concession lives until the process restarts).
    pub fn workspace_file_preview_unstage(&self) -> Result<(), DaemonError> {
        let id = self.alloc_id();
        match self.roundtrip(ClientMessage::WorkspaceFilePreviewUnstage { id })? {
            DaemonMessage::Ok { .. } => Ok(()),
            DaemonMessage::Error(error) => Err(DaemonError::Handshake(error)),
            other => unexpected(other),
        }
    }

    pub fn workspace_create(
        &self,
        project_id: &str,
        isolation: WorkspaceIsolation,
        branch: Option<String>,
    ) -> Result<Workspace, DaemonError> {
        let id = self.alloc_id();
        match self.roundtrip(ClientMessage::WorkspaceCreate {
            id,
            project_id: project_id.to_string(),
            isolation,
            branch,
        })? {
            DaemonMessage::Workspace { workspace, .. } => Ok(workspace),
            DaemonMessage::Error(error) => Err(DaemonError::Handshake(error)),
            other => unexpected(other),
        }
    }

    pub fn workspace_delete(&self, workspace_id: &str, force: bool) -> Result<(), DaemonError> {
        let id = self.alloc_id();
        match self.roundtrip(ClientMessage::WorkspaceDelete {
            id,
            workspace_id: workspace_id.to_string(),
            force,
        })? {
            DaemonMessage::Ok { .. } => Ok(()),
            DaemonMessage::Error(error) => Err(DaemonError::Handshake(error)),
            other => unexpected(other),
        }
    }

    /// This device's identity plus every paired and pending peer, straight from
    /// the daemon's frame. The panel needs the frame's field names unchanged, so
    /// this method hands the reply on instead of re-shaping it.
    pub fn devices_list(&self) -> Result<DaemonMessage, DaemonError> {
        let id = self.alloc_id();
        match self.roundtrip(ClientMessage::DevicesList { id })? {
            reply @ DaemonMessage::Devices { .. } => Ok(reply),
            DaemonMessage::Error(error) => Err(DaemonError::Handshake(error)),
            // Not `unexpected`: a `pairing_code` frame misdelivered here would
            // otherwise be `Debug`-formatted into an error string, and that
            // string is rendered on screen and printable by any error log.
            _ => pairing_reply_mismatch(),
        }
    }

    /// Asks the daemon to display a fresh one-time pairing code for `role`.
    ///
    /// The code is a five-minute secret: it leaves here only inside the returned
    /// frame, which the panel puts on screen. This method formats no frame into
    /// a message, so no error path of it can carry the code.
    ///
    /// `role` is the typed enum, not a string: an unrecognised role string is
    /// refused where it is parsed (the Tauri command boundary) rather than
    /// travelling as a request the daemon would have to reinterpret.
    pub fn pairing_start(&self, role: PeerRole) -> Result<DaemonMessage, DaemonError> {
        let id = self.alloc_id();
        match self.roundtrip(ClientMessage::PairingStart { id, role })? {
            reply @ DaemonMessage::PairingCode { .. } => Ok(reply),
            DaemonMessage::Error(error) => Err(DaemonError::Handshake(error)),
            _ => pairing_reply_mismatch(),
        }
    }

    /// Types a code shown on another device. The daemon answers either with a
    /// parked pairing this device's user still has to confirm, or with the row
    /// it already wrote.
    ///
    /// `code` is the redacting wrapper, so it is structurally impossible to
    /// `Debug`-format it into an error string here.
    pub fn pairing_complete(
        &self,
        address: &str,
        code: PairingSecret,
        role: PeerRole,
    ) -> Result<DaemonMessage, DaemonError> {
        let id = self.alloc_id();
        match self.roundtrip(ClientMessage::PairingComplete {
            id,
            address: address.to_string(),
            code,
            role,
        })? {
            reply @ (DaemonMessage::PairingPending { .. } | DaemonMessage::PairingDone { .. }) => {
                Ok(reply)
            }
            DaemonMessage::Error(error) => Err(DaemonError::Handshake(error)),
            _ => pairing_reply_mismatch(),
        }
    }

    /// Answers a pending `client` pairing. `Some(row)` is the peer the daemon
    /// wrote for an accept; `None` is a decline, which is a success too — the
    /// daemon replies `PairingDeclined` because the parked pairing is gone, not
    /// because anything failed.
    pub fn pairing_confirm(
        &self,
        device_id: &str,
        accept: bool,
    ) -> Result<Option<PeerRow>, DaemonError> {
        let id = self.alloc_id();
        match self.roundtrip(ClientMessage::PairingConfirm {
            id,
            device_id: device_id.to_string(),
            accept,
        })? {
            DaemonMessage::PeerUpdated { peer, .. } => Ok(Some(peer)),
            DaemonMessage::PairingDeclined { .. } => Ok(None),
            DaemonMessage::Error(error) => Err(DaemonError::Handshake(error)),
            _ => pairing_reply_mismatch(),
        }
    }

    /// Revokes one peer: the row is stamped, live connections are dropped, and
    /// the audit table records it. Returns the row as it now stands.
    pub fn peer_revoke(&self, device_id: &str) -> Result<PeerRow, DaemonError> {
        let id = self.alloc_id();
        match self.roundtrip(ClientMessage::PeerRevoke {
            id,
            device_id: device_id.to_string(),
        })? {
            DaemonMessage::PeerUpdated { peer, .. } => Ok(peer),
            DaemonMessage::Error(error) => Err(DaemonError::Handshake(error)),
            _ => pairing_reply_mismatch(),
        }
    }

    /// Replaces a peer's whole capability list. The daemon stores what it is
    /// given, so `caps` is always the complete set, never a delta.
    pub fn peer_set_caps(
        &self,
        device_id: &str,
        caps: Vec<String>,
    ) -> Result<PeerRow, DaemonError> {
        let id = self.alloc_id();
        match self.roundtrip(ClientMessage::PeerSetCaps {
            id,
            device_id: device_id.to_string(),
            caps,
        })? {
            DaemonMessage::PeerUpdated { peer, .. } => Ok(peer),
            DaemonMessage::Error(error) => Err(DaemonError::Handshake(error)),
            _ => pairing_reply_mismatch(),
        }
    }

    /// Every stored per-provider tool policy, straight from the daemon's
    /// frame. A provider with no stored policy is absent from `policies` and
    /// reads as enabled, so the panel defaults it rather than guessing at a
    /// fabricated row.
    pub fn tool_policy_get(&self) -> Result<DaemonMessage, DaemonError> {
        self.require_agreed(devboule_protocol::caps::TOOL_POLICY)?;
        let id = self.alloc_id();
        match self.roundtrip(ClientMessage::ToolPolicyGet { id })? {
            reply @ DaemonMessage::ToolPolicy { .. } => Ok(reply),
            DaemonMessage::Error(error) => Err(DaemonError::Handshake(error)),
            other => unexpected(other),
        }
    }

    /// Replaces one provider's tool policy. `enabled: None` (the app's `null`)
    /// and `Some(true)` both mean enabled; `disabled_tools` is the complete
    /// per-tool set, never a delta.
    ///
    /// The reply is the daemon's frame rather than `()`: this call is the
    /// only place the write is acknowledged, so it hands the acknowledgement
    /// on instead of re-shaping it into a value the daemon did not send.
    pub fn tool_policy_set(
        &self,
        provider_id: &str,
        enabled: Option<bool>,
        disabled_tools: Vec<String>,
    ) -> Result<DaemonMessage, DaemonError> {
        self.require_agreed(devboule_protocol::caps::TOOL_POLICY)?;
        let id = self.alloc_id();
        match self.roundtrip(ClientMessage::ToolPolicySet {
            id,
            provider_id: provider_id.to_string(),
            enabled,
            disabled_tools,
        })? {
            reply @ DaemonMessage::ToolPolicySetOk { .. } => Ok(reply),
            DaemonMessage::Error(error) => Err(DaemonError::Handshake(error)),
            other => unexpected(other),
        }
    }

    /// The whole stored agent-profile document: the ordered profile list and the
    /// standing instructions, straight from the daemon's frame.
    ///
    /// One read for both halves, so the list a creation resolves and the
    /// instructions that travel with it cannot be read at two different
    /// moments.
    pub fn agent_profiles_get(&self) -> Result<DaemonMessage, DaemonError> {
        self.require_agreed(devboule_protocol::caps::AGENT_PROFILES)?;
        let id = self.alloc_id();
        match self.roundtrip(ClientMessage::AgentProfilesGet { id })? {
            reply @ DaemonMessage::AgentProfiles { .. } => Ok(reply),
            DaemonMessage::Error(error) => Err(DaemonError::Handshake(error)),
            other => unexpected(other),
        }
    }

    /// Replaces the whole agent-profile document — the ordered list **and** the
    /// standing instructions, never one half, and never a delta.
    ///
    /// The reply is the daemon's frame rather than `()`: this call is the only
    /// place the write is acknowledged, so it hands the acknowledgement on
    /// instead of re-shaping it into a value the daemon did not send. The ids
    /// the daemon minted for new profiles are read back with
    /// [`DaemonClient::agent_profiles_get`], exactly as a tool policy toggle is
    /// read back with `tool_policy_get`.
    pub fn agent_profiles_set(
        &self,
        document: devboule_protocol::AgentProfilesDocument,
    ) -> Result<DaemonMessage, DaemonError> {
        self.require_agreed(devboule_protocol::caps::AGENT_PROFILES)?;
        let id = self.alloc_id();
        match self.roundtrip(ClientMessage::AgentProfilesSet { id, document })? {
            reply @ DaemonMessage::AgentProfilesSetOk { .. } => Ok(reply),
            DaemonMessage::Error(error) => Err(DaemonError::Handshake(error)),
            other => unexpected(other),
        }
    }

    /// What one provider offers — its models and its modes — for the profile
    /// form. `refresh: false` is a cached read; `refresh: true` re-probes
    /// now, which briefly starts the provider's process (Claude usually
    /// costs a file scan; the one process it can start is the native
    /// version probe, and only while its installed version is unknown).
    ///
    /// Refused unless the handshake negotiated `provider_vocabulary`: a
    /// daemon without the capability predates the query, and asking it would
    /// fail on a frame it cannot read. That absence is a different fact from
    /// the query answering `absent`, and this gate is what keeps the two
    /// apart on the client side.
    ///
    /// The reply is the daemon's frame rather than a reshaped value, exactly
    /// as `agent_profiles_get` hands on the document frame: the caller
    /// matches `DaemonMessage::ProviderVocabulary` and reads the axes it
    /// needs.
    pub fn provider_vocabulary_get(
        &self,
        provider: &str,
        refresh: bool,
    ) -> Result<DaemonMessage, DaemonError> {
        self.require_agreed(devboule_protocol::caps::PROVIDER_VOCABULARY)?;
        let id = self.alloc_id();
        match self.roundtrip(ClientMessage::ProviderVocabularyGet {
            id,
            provider: provider.to_string(),
            refresh,
        })? {
            reply @ DaemonMessage::ProviderVocabulary { .. } => Ok(reply),
            DaemonMessage::Error(error) => Err(DaemonError::Handshake(error)),
            other => unexpected(other),
        }
    }

    /// The delegation switch, straight from the daemon's frame: the stored
    /// value and where the answer came from (`file`, `default`,
    /// `quarantined`).
    ///
    /// Refused unless the handshake negotiated `permission_delegation`, the
    /// same gate `agent_profiles_get` applies: a daemon without the capability
    /// predates the RPC, and asking it would fail on a frame it cannot read.
    pub fn delegation_get(&self) -> Result<DaemonMessage, DaemonError> {
        self.require_agreed(devboule_protocol::caps::PERMISSION_DELEGATION)?;
        let id = self.alloc_id();
        match self.roundtrip(ClientMessage::DelegationGet { id })? {
            reply @ DaemonMessage::DelegationState { .. } => Ok(reply),
            DaemonMessage::Error(error) => Err(DaemonError::Handshake(error)),
            other => unexpected(other),
        }
    }

    /// Sets the delegation switch. The reply is the daemon's frame carrying
    /// what was **stored** — not an echo of the argument — so the caller can
    /// hold the value the daemon actually has, the rule
    /// `NOTE-a-write-that-does-not-say-what-it-stored.md` argues for. The
    /// daemon also pushes `DelegationChanged` to every watching connection,
    /// which is how the other surfaces learn; this reply is the writer's own
    /// acknowledgement.
    pub fn delegation_set(&self, enabled: bool) -> Result<DaemonMessage, DaemonError> {
        self.require_agreed(devboule_protocol::caps::PERMISSION_DELEGATION)?;
        let id = self.alloc_id();
        match self.roundtrip(ClientMessage::DelegationSet { id, enabled })? {
            reply @ DaemonMessage::DelegationSetOk { .. } => Ok(reply),
            DaemonMessage::Error(error) => Err(DaemonError::Handshake(error)),
            other => unexpected(other),
        }
    }

    /// Install the handler for the daemon-pushed switch. The setting is
    /// global and the app reads `DelegationGet` once at mount, so a write
    /// from any surface — the Settings switch, the roster's take-back,
    /// another app instance — arrives here, not as a reply to anything this
    /// process asked.
    pub fn on_delegation_changed(&self, handler: DelegationChangedHandler) {
        *self
            .inner
            .delegation_subscription
            .lock()
            .unwrap_or_else(|err| err.into_inner()) = Some(handler);
    }

    pub fn journal_usage(&self) -> Result<JournalUsage, DaemonError> {
        let id = self.alloc_id();
        match self.roundtrip(ClientMessage::JournalUsage { id })? {
            DaemonMessage::JournalUsage { usage, .. } => Ok(usage),
            DaemonMessage::Error(error) => Err(DaemonError::Handshake(error)),
            other => unexpected(other),
        }
    }

    pub fn providers_list(&self) -> Result<(Vec<ProviderInfo>, u32), DaemonError> {
        let id = self.alloc_id();
        match self.roundtrip(ClientMessage::ProvidersList { id })? {
            DaemonMessage::Providers {
                providers,
                unreadable_dirs,
                ..
            } => Ok((providers, unreadable_dirs)),
            DaemonMessage::Error(error) => Err(DaemonError::Handshake(error)),
            other => unexpected(other),
        }
    }

    pub fn providers_refresh(&self) -> Result<(Vec<ProviderInfo>, u32), DaemonError> {
        let id = self.alloc_id();
        match self.roundtrip(ClientMessage::ProvidersRefresh { id })? {
            DaemonMessage::Providers {
                providers,
                unreadable_dirs,
                ..
            } => Ok((providers, unreadable_dirs)),
            DaemonMessage::Error(error) => Err(DaemonError::Handshake(error)),
            other => unexpected(other),
        }
    }

    pub fn provider_update(
        &self,
        provider_id: &str,
    ) -> Result<(bool, Option<i32>, String), DaemonError> {
        let id = self.alloc_id();
        match self.roundtrip_with_deadline(
            ClientMessage::ProviderUpdate {
                id,
                provider_id: provider_id.to_string(),
            },
            PROVIDER_UPDATE_RPC_TIMEOUT,
        )? {
            DaemonMessage::ProviderUpdated {
                ok, exit_code, log, ..
            } => Ok((ok, exit_code, log)),
            DaemonMessage::Error(error) => Err(DaemonError::Handshake(error)),
            other => unexpected(other),
        }
    }

    pub fn journal_retention_get(&self) -> Result<JournalRetention, DaemonError> {
        let id = self.alloc_id();
        match self.roundtrip(ClientMessage::JournalRetentionGet { id })? {
            DaemonMessage::JournalRetention { retention, .. } => Ok(retention),
            DaemonMessage::Error(error) => Err(DaemonError::Handshake(error)),
            other => unexpected(other),
        }
    }

    pub fn journal_retention_set(
        &self,
        patch: RetentionPatch,
    ) -> Result<JournalRetention, DaemonError> {
        let id = self.alloc_id();
        match self.roundtrip(ClientMessage::JournalRetentionSet {
            id,
            max_age_ms: patch.max_age_ms,
            max_bytes: patch.max_bytes,
            max_sessions: patch.max_sessions,
            session_max_bytes: patch.session_max_bytes,
            idempotency_key: None,
        })? {
            DaemonMessage::JournalRetention { retention, .. } => Ok(retention),
            DaemonMessage::Error(error) => Err(DaemonError::Handshake(error)),
            other => unexpected(other),
        }
    }

    pub fn session_delete(&self, session_id: &str) -> Result<(), DaemonError> {
        let id = self.alloc_id();
        match self.roundtrip(ClientMessage::SessionDelete {
            id,
            session_id: session_id.to_string(),
            idempotency_key: None,
        })? {
            DaemonMessage::Ok { .. } => Ok(()),
            DaemonMessage::Error(error) => Err(DaemonError::Handshake(error)),
            other => unexpected(other),
        }
    }

    /// Subscribe this connection to owner-scoped roster transitions. The
    /// handler is registered before the request so the initial snapshot and
    /// a transition racing it cannot be lost by the reader.
    pub fn sessions_watch(&self, handler: SessionStateHandler) -> Result<(), DaemonError> {
        {
            let mut subscription = self
                .inner
                .session_state_subscription
                .lock()
                .unwrap_or_else(|err| err.into_inner());
            *subscription = Some(handler);
        }
        let id = self.alloc_id();
        match self.roundtrip(ClientMessage::SessionsWatch { id }) {
            Ok(DaemonMessage::Ok { .. }) => Ok(()),
            Ok(DaemonMessage::Error(error)) => {
                self.unsubscribe_sessions_watch();
                Err(DaemonError::Handshake(error))
            }
            Ok(other) => {
                self.unsubscribe_sessions_watch();
                unexpected(other)
            }
            Err(error) => {
                self.unsubscribe_sessions_watch();
                Err(error)
            }
        }
    }

    pub fn sessions_unwatch(&self) -> Result<(), DaemonError> {
        let id = self.alloc_id();
        let result = self.roundtrip(ClientMessage::SessionsUnwatch { id });
        self.unsubscribe_sessions_watch();
        match result? {
            DaemonMessage::Ok { .. } => Ok(()),
            DaemonMessage::Error(error) => Err(DaemonError::Handshake(error)),
            other => unexpected(other),
        }
    }

    /// Report this connection's foreground presence to the daemon. The
    /// daemon keeps it per connection so another same-user window cannot
    /// accidentally suppress attention for this one.
    pub fn session_presence(
        &self,
        focused_session_id: Option<&str>,
        app_visible: bool,
    ) -> Result<(), DaemonError> {
        let id = self.alloc_id();
        match self.roundtrip(ClientMessage::SessionsPresence {
            id,
            focused_session_id: focused_session_id.map(str::to_string),
            app_visible,
        })? {
            DaemonMessage::Ok { .. } => Ok(()),
            DaemonMessage::Error(error) => Err(DaemonError::Handshake(error)),
            other => unexpected(other),
        }
    }

    pub fn roundtrip(&self, message: ClientMessage) -> Result<DaemonMessage, DaemonError> {
        self.roundtrip_with_deadline(message, RPC_TIMEOUT)
    }

    pub fn roundtrip_with_deadline(
        &self,
        message: ClientMessage,
        timeout: Duration,
    ) -> Result<DaemonMessage, DaemonError> {
        // One line at departure and one at arrival (or deadline) on the
        // calling thread: this wait *is* the window's wait when a non-async
        // command makes it. Spent unless DEVBOULE_RPC_TRACE names a sink.
        let trace =
            crate::rpc_trace::Roundtrip::begin(message.name(), message.request_id(), timeout);
        let result = self.roundtrip_inner(message, timeout);
        trace.finish(&result);
        result
    }

    fn roundtrip_inner(
        &self,
        message: ClientMessage,
        timeout: Duration,
    ) -> Result<DaemonMessage, DaemonError> {
        let Some(id) = message.request_id() else {
            self.write_frame(&message)?;
            return Err(DaemonError::Protocol(
                "roundtrip requires a request id".to_string(),
            ));
        };
        let (tx, rx) = mpsc::channel();
        {
            let mut pending = self
                .inner
                .pending
                .lock()
                .unwrap_or_else(|err| err.into_inner());
            pending.insert(id, tx);
        }
        let deadline = Instant::now() + timeout;
        if let Err(error) = self.inner.framed.send_until(&message, deadline) {
            self.inner
                .pending
                .lock()
                .unwrap_or_else(|err| err.into_inner())
                .remove(&id);
            return Err(error);
        }
        match rx.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
            Ok(message) => Ok(message),
            Err(RecvTimeoutError::Timeout) => {
                self.inner
                    .pending
                    .lock()
                    .unwrap_or_else(|err| err.into_inner())
                    .remove(&id);
                Err(DaemonError::timed_out("waiting for a daemon reply"))
            }
            Err(RecvTimeoutError::Disconnected) => Err(DaemonError::ConnectionLost),
        }
    }

    /// Write a frame without reading a reply. A client that stops reading
    /// uses this so we can prove other connections still make progress.
    pub fn write_frame(&self, message: &ClientMessage) -> Result<(), DaemonError> {
        if self.inner.stop.load(Ordering::SeqCst) {
            return Err(DaemonError::ConnectionLost);
        }
        self.inner.framed.send(message)
    }

    #[cfg(windows)]
    pub fn pipe_dacl_sddl(&self) -> std::io::Result<String> {
        let file = self.inner.framed.as_file().ok_or_else(|| {
            std::io::Error::other("this connection is not a named pipe and has no DACL")
        })?;
        crate::transport::inspect_pipe_dacl(&file)
    }

    fn alloc_id(&self) -> u64 {
        self.inner.next_id.fetch_add(1, Ordering::Relaxed)
    }

    fn alloc_subscription_id(&self) -> SubscriptionId {
        self.inner
            .next_subscription_id
            .fetch_add(1, Ordering::Relaxed)
    }

    #[cfg(feature = "server")]
    fn default_subscription(&self, session_id: &str) -> Option<SubscriptionId> {
        self.inner
            .default_subscriptions
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .get(session_id)
            .copied()
    }

    #[cfg(feature = "server")]
    fn control_subscription_id(&self, session_id: &str) -> Result<SubscriptionId, DaemonError> {
        self.default_subscription(session_id).ok_or_else(|| {
            DaemonError::Protocol(
                "Session is not attached; attach before sending session commands.".to_string(),
            )
        })
    }

    fn unsubscribe(&self, subscription_id: SubscriptionId) {
        let removed = self
            .inner
            .subscriptions
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .remove(&subscription_id);
        #[cfg(not(feature = "server"))]
        let _ = removed;
        #[cfg(feature = "server")]
        if let Some(removed) = removed {
            let mut defaults = self
                .inner
                .default_subscriptions
                .lock()
                .unwrap_or_else(|err| err.into_inner());
            if defaults.get(&removed.session_id) == Some(&subscription_id) {
                let replacement = self
                    .inner
                    .subscriptions
                    .lock()
                    .unwrap_or_else(|err| err.into_inner())
                    .iter()
                    .find(|(_, subscription)| subscription.session_id == removed.session_id)
                    .map(|(id, _)| *id);
                if let Some(replacement) = replacement {
                    defaults.insert(removed.session_id, replacement);
                } else {
                    defaults.remove(&removed.session_id);
                }
            }
        }
    }

    #[cfg(feature = "server")]
    fn unsubscribe_session(&self, session_id: &str) {
        let ids = self
            .inner
            .subscriptions
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .iter()
            .filter(|(_, subscription)| subscription.session_id == session_id)
            .map(|(id, _)| *id)
            .collect::<Vec<_>>();
        for id in ids {
            self.unsubscribe(id);
        }
    }

    fn remove_pending_subscription_for_id(&self, subscription_id: SubscriptionId) {
        self.inner
            .pending_subscriptions
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .retain(|_, pending| pending.subscription_id != subscription_id);
    }

    fn remove_pending_subscription(&self, request_id: u64) {
        self.inner
            .pending_subscriptions
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .remove(&request_id);
    }

    fn unsubscribe_sessions_watch(&self) {
        self.inner
            .session_state_subscription
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .take();
    }
}

impl Drop for DaemonClient {
    fn drop(&mut self) {
        self.inner.stop.store(true, Ordering::SeqCst);
        // Closing the write handle unblocks a reader parked on the pipe.
        // The reader thread then fails pending RPCs and notifies subscribers.
        let handle = self
            .reader
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .take();
        if let Some(handle) = handle {
            let deadline = std::time::Instant::now() + JOIN_BUDGET;
            while !handle.is_finished() && std::time::Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(10));
            }
            if handle.is_finished() {
                let _ = handle.join();
            }
        }
    }
}

pub fn connect(paths: &RuntimePaths, hello: ClientHello) -> Result<DaemonClient, DaemonError> {
    let file = transport::connect(paths)?;
    handshake(file, hello)
}

/// Connect, spawning the daemon binary if the pipe is not up yet. Racing
/// callers converge on one daemon because the loser of the file lock exits.
pub fn connect_or_spawn(
    paths: &RuntimePaths,
    hello: ClientHello,
    daemon_binary: Option<&Path>,
) -> Result<DaemonClient, DaemonError> {
    let binary = match daemon_binary {
        Some(path) => path.to_path_buf(),
        None => resolve_daemon_binary()?,
    };
    connect_or_spawn_with(paths, hello, &binary, connect, |binary, paths| {
        spawn_daemon(binary, paths).map(|_| ())
    })
}

fn connect_or_spawn_with<T, Connect, Spawn>(
    paths: &RuntimePaths,
    hello: ClientHello,
    daemon_binary: &Path,
    mut connect_fn: Connect,
    mut spawn_fn: Spawn,
) -> Result<T, DaemonError>
where
    Connect: FnMut(&RuntimePaths, ClientHello) -> Result<T, DaemonError>,
    Spawn: FnMut(&Path, &RuntimePaths) -> Result<(), DaemonError>,
{
    let mut spawned = false;
    for attempt in 0..SPAWN_ATTEMPTS {
        match connect_fn(paths, hello.clone()) {
            Ok(client) => return Ok(client),
            Err(error) => {
                if attempt + 1 == SPAWN_ATTEMPTS {
                    return Err(error);
                }
            }
        }
        if !spawned {
            match spawn_fn(daemon_binary, paths) {
                Ok(()) => {
                    spawned = true;
                }
                Err(error) => {
                    if attempt + 1 == SPAWN_ATTEMPTS {
                        return Err(error);
                    }
                }
            }
        }
        std::thread::sleep(SPAWN_SLEEP);
    }
    Err(DaemonError::timed_out("connecting to the daemon"))
}

pub fn handshake(file: File, hello: ClientHello) -> Result<DaemonClient, DaemonError> {
    #[cfg(windows)]
    let server_pid = crate::transport::server_process_id(&file).ok();
    #[cfg(not(windows))]
    let server_pid = None;
    let framed = Framed::new(file);
    framed.send(&ClientMessage::Hello(hello))?;
    let reply: DaemonMessage = framed.recv_timeout(HANDSHAKE_TIMEOUT)?;
    match reply {
        DaemonMessage::Hello(daemon_hello) => {
            let inner = Arc::new(ClientInner {
                framed,
                next_id: AtomicU64::new(1),
                next_subscription_id: AtomicU64::new(1),
                pending: Mutex::new(HashMap::new()),
                pending_subscriptions: Mutex::new(HashMap::new()),
                subscriptions: Mutex::new(HashMap::new()),
                #[cfg(feature = "server")]
                default_subscriptions: Mutex::new(HashMap::new()),
                session_state_subscription: Mutex::new(None),
                delegation_subscription: Mutex::new(None),
                stop: AtomicBool::new(false),
                hello: daemon_hello,
                server_pid,
            });
            let reader_inner = Arc::clone(&inner);
            let reader = std::thread::Builder::new()
                .name("daemon-client-read".into())
                .spawn(move || client_read_loop(reader_inner))
                .map_err(DaemonError::from)?;
            Ok(DaemonClient {
                inner,
                reader: Mutex::new(Some(reader)),
            })
        }
        DaemonMessage::Error(error) => Err(DaemonError::Handshake(error)),
        other => unexpected(other),
    }
}

pub fn test_owner(client: &str) -> Result<OwnerId, DaemonError> {
    #[cfg(windows)]
    {
        let user = crate::security::current_user_sid()?;
        OwnerId::new(user, client).map_err(DaemonError::Protocol)
    }
    #[cfg(not(windows))]
    {
        OwnerId::new("unix", client).map_err(DaemonError::Protocol)
    }
}

fn client_read_loop(inner: Arc<ClientInner>) {
    loop {
        if inner.stop.load(Ordering::SeqCst) {
            fail_connection(
                &inner,
                DaemonError::Protocol("daemon connection was closed".to_string()),
            );
            return;
        }
        match inner
            .framed
            .recv_timeout::<DaemonMessage>(Duration::from_millis(100))
        {
            Ok(DaemonMessage::Event(envelope)) => {
                if let SessionEvent::SessionsSnapshot { sessions } = envelope.event {
                    let handler = inner
                        .session_state_subscription
                        .lock()
                        .unwrap_or_else(|err| err.into_inner())
                        .clone();
                    if let Some(handler) = handler {
                        handler(sessions);
                    }
                }
            }
            Ok(DaemonMessage::SubscriptionEvent {
                subscription_id,
                envelope,
            }) => {
                let handler = inner
                    .subscriptions
                    .lock()
                    .unwrap_or_else(|err| err.into_inner())
                    .get(&subscription_id)
                    .filter(|subscription| subscription.session_id == envelope.session_id)
                    .map(|subscription| Arc::clone(&subscription.handler));
                let handler = handler.or_else(|| {
                    inner
                        .pending_subscriptions
                        .lock()
                        .unwrap_or_else(|err| err.into_inner())
                        .values()
                        .find(|pending| {
                            pending.subscription_id == subscription_id
                                && pending.session_id == envelope.session_id
                        })
                        .map(|pending| Arc::clone(&pending.handler))
                });
                if let Some(handler) = handler {
                    handler(envelope);
                }
            }
            Ok(message) => {
                // The daemon-pushed switch answers no request, so it never
                // enters the pending table below; it goes to its handler the
                // way a session snapshot does.
                if let DaemonMessage::DelegationChanged { enabled, source } = &message {
                    let handler = inner
                        .delegation_subscription
                        .lock()
                        .unwrap_or_else(|err| err.into_inner())
                        .clone();
                    if let Some(handler) = handler {
                        handler(*enabled, *source);
                    }
                    continue;
                }
                if let Some(id) = daemon_message_id(&message) {
                    if let DaemonMessage::SessionAttached {
                        subscription_id, ..
                    } = &message
                    {
                        // Keep the token in the pending table until the daemon
                        // confirms it, while subscription events can still be
                        // routed by that same token if they arrive first.
                        let pending_subscription = inner
                            .pending_subscriptions
                            .lock()
                            .unwrap_or_else(|err| err.into_inner())
                            .remove(&id);
                        if let Some(PendingSubscription {
                            subscription_id: _,
                            session_id,
                            handler,
                        }) = pending_subscription
                            .filter(|pending| pending.subscription_id == *subscription_id)
                        {
                            inner
                                .subscriptions
                                .lock()
                                .unwrap_or_else(|err| err.into_inner())
                                .insert(
                                    *subscription_id,
                                    Subscription {
                                        session_id: session_id.clone(),
                                        handler,
                                    },
                                );
                            #[cfg(feature = "server")]
                            {
                                // A reattach can follow a resume that replaced the runtime, so
                                // the old test helper token may no longer be current.
                                inner
                                    .default_subscriptions
                                    .lock()
                                    .unwrap_or_else(|err| err.into_inner())
                                    .insert(session_id, *subscription_id);
                            }
                        }
                    }
                    let tx = inner
                        .pending
                        .lock()
                        .unwrap_or_else(|err| err.into_inner())
                        .remove(&id);
                    if let Some(tx) = tx {
                        let _ = tx.send(message);
                    }
                }
            }
            Err(DaemonError::TimedOut(_)) => continue,
            Err(_) => {
                fail_connection(&inner, DaemonError::ConnectionLost);
                return;
            }
        }
    }
}

fn fail_connection(inner: &ClientInner, error: DaemonError) {
    inner.stop.store(true, Ordering::SeqCst);
    inner
        .session_state_subscription
        .lock()
        .unwrap_or_else(|err| err.into_inner())
        .take();
    let subscriptions: Vec<(SubscriptionId, String, EventHandler)> = inner
        .subscriptions
        .lock()
        .unwrap_or_else(|err| err.into_inner())
        .drain()
        .map(|(subscription_id, subscription)| {
            (
                subscription_id,
                subscription.session_id,
                subscription.handler,
            )
        })
        .collect();
    let pending_subscriptions = inner
        .pending_subscriptions
        .lock()
        .unwrap_or_else(|err| err.into_inner())
        .drain()
        .map(|(_, pending)| (pending.session_id, pending.handler))
        .collect::<Vec<_>>();
    for (_, session_id, handler) in subscriptions {
        handler(SessionEventEnvelope {
            session_id,
            generation: 0,
            transcript_seq: None,
            event: SessionEvent::Exit { code: None },
        });
    }
    for (session_id, handler) in pending_subscriptions {
        handler(SessionEventEnvelope {
            session_id,
            generation: 0,
            transcript_seq: None,
            event: SessionEvent::Exit { code: None },
        });
    }
    #[cfg(feature = "server")]
    inner
        .default_subscriptions
        .lock()
        .unwrap_or_else(|err| err.into_inner())
        .clear();
    let pending: Vec<mpsc::Sender<DaemonMessage>> = inner
        .pending
        .lock()
        .unwrap_or_else(|err| err.into_inner())
        .drain()
        .map(|(_, tx)| tx)
        .collect();
    let error = DaemonMessage::Error(WireError::new(ErrorCode::ConnectionLost, error.to_string()));
    for tx in pending {
        let _ = tx.send(error.clone());
    }
}

fn daemon_message_id(message: &DaemonMessage) -> Option<u64> {
    match message {
        DaemonMessage::Hello(_)
        | DaemonMessage::Event(_)
        | DaemonMessage::SubscriptionEvent { .. }
        // A server-initiated broadcast, not a reply: it answers no request,
        // so it must never be matched against the pending table (a reply
        // answered `None` here is never delivered to its caller). The reader
        // loop hands it to the delegation handler before this match runs.
        | DaemonMessage::DelegationChanged { .. } => None,
        DaemonMessage::Error(error) => error.id,
        // Every request-shaped reply carries its id. The device RPCs are
        // listed rather than swept into a wildcard: this match is exhaustive on
        // purpose, so a new reply variant is a compile error here until it is
        // given a decision. `DelegationState`/`DelegationSetOk` are
        // request-shaped and sit in this arm — putting one in the arm above
        // would silence the compiler and hang the caller forever.
        DaemonMessage::ProviderVocabulary { id, .. }
        | DaemonMessage::Devices { id, .. }
        | DaemonMessage::PeerAgents { id, .. }
        | DaemonMessage::PairingCode { id, .. }
        | DaemonMessage::PairingPending { id, .. }
        | DaemonMessage::PairingDone { id, .. }
        | DaemonMessage::PeerUpdated { id, .. }
        | DaemonMessage::PairingDeclined { id, .. }
        | DaemonMessage::ToolPolicy { id, .. }
        | DaemonMessage::ToolPolicySetOk { id }
        | DaemonMessage::AgentProfiles { id, .. }
        | DaemonMessage::AgentProfilesSetOk { id }
        | DaemonMessage::DelegationState { id, .. }
        | DaemonMessage::DelegationSetOk { id, .. }
        | DaemonMessage::Pong { id, .. }
        | DaemonMessage::Status { id, .. }
        | DaemonMessage::Diagnostics { id, .. }
        | DaemonMessage::Shutdown { id, .. }
        | DaemonMessage::Session { id, .. }
        | DaemonMessage::Sessions { id, .. }
        | DaemonMessage::Projects { id, .. }
        | DaemonMessage::Project { id, .. }
        | DaemonMessage::Workspaces { id, .. }
        | DaemonMessage::Workspace { id, .. }
        | DaemonMessage::WorkspaceGit { id, .. }
        | DaemonMessage::WorkspaceGitFile { id, .. }
        | DaemonMessage::WorkspaceFiles { id, .. }
        | DaemonMessage::WorkspaceFileContent { id, .. }
        | DaemonMessage::WorkspaceFileRenamed { id, .. }
        | DaemonMessage::WorkspaceFileDuplicated { id, .. }
        | DaemonMessage::WorkspaceFilePreviewStaged { id, .. }
        | DaemonMessage::SessionAttached { id, .. }
        | DaemonMessage::JournalUsage { id, .. }
        | DaemonMessage::JournalRetention { id, .. }
        | DaemonMessage::Providers { id, .. }
        | DaemonMessage::ProviderUpdated { id, .. }
        | DaemonMessage::Ok { id }
        | DaemonMessage::AgentMessageReceipt { id, .. }
        | DaemonMessage::Resume { id, .. }
        | DaemonMessage::SessionDeposited { id, .. }
        | DaemonMessage::SessionAttachment { id, .. }
        | DaemonMessage::InvokeResult { id, .. } => Some(*id),
    }
}

fn unexpected<T>(message: DaemonMessage) -> Result<T, DaemonError> {
    Err(DaemonError::Protocol(format!(
        "unexpected daemon frame: {message:?}"
    )))
}

/// A reply the pairing and peer-management methods did not expect. Unlike
/// [`unexpected`] it never formats the frame: a `pairing_code` frame carries
/// the live code, and the code must not reach a log or an error string through
/// a `Debug` of a reply, whichever request the frame was misdelivered to.
fn pairing_reply_mismatch<T>() -> Result<T, DaemonError> {
    Err(DaemonError::Protocol(
        "unexpected daemon frame on a pairing or peer request".to_string(),
    ))
}

#[cfg(all(test, feature = "server"))]
#[path = "client_tests.rs"]
mod tests;
