use std::collections::HashMap;
use std::fs::File;
#[cfg(unix)]
use std::io;
use std::path::Path;
#[cfg(unix)]
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use devboule_protocol::{
    ActiveTurnBehavior, AgentActivityState, AttachmentReference, BrowserExecuteRequest,
    ClientHello, ClientMessage, Cursor, DaemonHello, DaemonMessage, DaemonStatusBody, ErrorCode,
    JournalRetention, JournalUsage, OwnerId, PairingSecret, PeerRow, PermissionOutcome,
    Persistence, Project, PromptAttachment, ProviderInfo, RemoteHostList, RemoteHostListBody,
    RemoteHostStatus, ResumeResult, RetentionPatch, Session, SessionEvent, SessionEventEnvelope,
    SessionKind, SessionResumeInfo, SessionResumeOutcome, SessionStateSnapshot, SessionTask,
    StoredAttachment, SubscriptionId, WireError, Workspace, WorkspaceDirectory,
    WorkspaceFileContent, WorkspaceFileMutation, WorkspaceFilePreview, WorkspaceGitFileDiff,
    WorkspaceGitLog, WorkspaceGitStatus, WorkspaceIsolation,
};

use crate::diagnostics::DiagnosticsReport;
use crate::error::DaemonError;
use crate::framing::Framed;
use crate::paths::RuntimePaths;
use crate::spawn::{reap_spawned_daemon, resolve_daemon_binary, spawn_daemon};
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
/// The attach reply's `resume`, for the subscriber that can read it. Called on
/// the connection's reader thread, at the frame where the reply is matched,
/// and only for a `reset`: a `resumed` outcome names no tail and has nothing
/// to hand over. Ordering is the contract — that thread is also the only thing
/// that dispatches the envelopes following the reply.
pub type SessionResetHandler = Arc<dyn Fn(SessionResumeInfo) + Send + Sync>;
/// The daemon-pushed delegation switch (`DelegationChanged`): the stored
/// value and where it came from. Fired for a server-initiated broadcast, so
/// unlike an RPC reply it carries no request id.
pub type DelegationChangedHandler =
    Arc<dyn Fn(bool, devboule_protocol::DelegationSource) + Send + Sync>;
/// The daemon-pushed host state (`remote_host_status`): the host's device id,
/// its state, and the one sentence that goes with it. Fired for a
/// server-initiated broadcast, so unlike an RPC reply it carries no request id,
/// and it arrives only for the hosts this process is watching.
pub type RemoteHostStatusHandler = Arc<dyn Fn(RemoteHostStatus) + Send + Sync>;

struct PendingSubscription {
    subscription_id: SubscriptionId,
    session_id: String,
    handler: EventHandler,
    /// Absent for an attach whose caller cannot read the outcome; then the
    /// reply's `resume` is dropped exactly as it was before it existed.
    reset: Option<SessionResetHandler>,
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
    remote_host_status_handler: Mutex<Option<RemoteHostStatusHandler>>,
    /// The sending half of the browser-host queue (`client_browser.rs`). The
    /// reader only `try_send`s into it; emptied on connection failure so the
    /// host's receiver sees the end.
    browser_requests: Mutex<Option<mpsc::SyncSender<BrowserExecuteRequest>>>,
    /// The receiving half, handed out once.
    browser_request_inbox: Mutex<Option<mpsc::Receiver<BrowserExecuteRequest>>>,
    /// Feeds the thread that writes `browser_busy` refusals for commands the
    /// reader could not queue. Started by the first registration; emptied on
    /// connection failure so the thread ends.
    browser_rejects: Mutex<Option<mpsc::Sender<ClientMessage>>>,
    stop: AtomicBool,
    hello: DaemonHello,
    server_pid: Option<u32>,
    /// The runtime this connection was opened on, for restart's wait and
    /// re-spawn. Raw `handshake` clients have none and cannot restart.
    /// Read on Unix only; the Windows restart kills by handle instead.
    #[cfg_attr(not(unix), allow(dead_code))]
    runtime: Option<RuntimePaths>,
}

pub struct DaemonClient {
    inner: Arc<ClientInner>,
    reader: Mutex<Option<JoinHandle<()>>>,
}

/// The daemon's answer to a quit request. `Refused` is an answer, not a
/// broken call: the daemon chose to outlive this client (another local app
/// window is still connected), and the caller decides what "just exit" looks
/// like without treating the refusal as unreachable.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ShutdownAnswer {
    Accepted,
    Refused(String),
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

    /// Refuse a GIF or a WebP for a daemon that did not agree
    /// `attachments.gif_webp`.
    ///
    /// No frame is unknown to an older daemon here — it would answer with an
    /// ordinary rejection — but the app is meant to know before it sends, so
    /// the refusal is the client's, with the sentence of every other gate.
    fn require_gif_webp_agreed(&self, attachments: &[PromptAttachment]) -> Result<(), DaemonError> {
        if attachments
            .iter()
            .any(|attachment| devboule_protocol::is_gif_webp_mime(&attachment.mime_type))
        {
            self.require_agreed(devboule_protocol::caps::ATTACHMENTS_GIF_WEBP)?;
        }
        Ok(())
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
        match self.request_shutdown()? {
            ShutdownAnswer::Accepted => Ok(()),
            // A refusal is an answer, not a broken call: the daemon is saying
            // it must outlive this client (another local app window is still
            // connected). The caller decides what "just exit" looks like.
            ShutdownAnswer::Refused(reason) => Err(DaemonError::Protocol(reason)),
        }
    }

    /// The daemon's answer to a quit request, kept apart from `shutdown`'s
    /// error: a refusal arrived, an error did not.
    pub fn request_shutdown(&self) -> Result<ShutdownAnswer, DaemonError> {
        let id = self.alloc_id();
        match self.roundtrip(ClientMessage::Shutdown { id })? {
            DaemonMessage::Shutdown { accepted: true, .. } => Ok(ShutdownAnswer::Accepted),
            DaemonMessage::Shutdown {
                accepted: false,
                reason,
                ..
            } => Ok(ShutdownAnswer::Refused(reason.unwrap_or_default())),
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
        #[cfg(not(any(windows, unix)))]
        {
            Err(DaemonError::UnsupportedPlatform)
        }
        #[cfg(unix)]
        {
            self.restart_daemon_unix()
        }
    }

    /// Graceful shutdown first, then the guarded kill as the fallback: a
    /// daemon that refuses (other clients hold it) is never killed, and an
    /// accepted stop is waited out on the record before any signal, because a
    /// kill mid-drain leaves that record reading live for `STALE_AFTER`.
    #[cfg(unix)]
    fn restart_daemon_unix(&self) -> Result<(), DaemonError> {
        let paths = self.inner.runtime.clone().ok_or_else(|| {
            DaemonError::Protocol("restart needs the runtime this connection opened on".to_string())
        })?;
        let old_instance = self.inner.hello.instance_id.clone();
        let expected = self.inner.server_pid.ok_or_else(|| {
            DaemonError::Protocol("cannot prove the identity of the connected daemon".to_string())
        })?;
        let graceful = match self.request_shutdown() {
            Ok(ShutdownAnswer::Accepted) => wait_while_present(&paths, &old_instance),
            Ok(ShutdownAnswer::Refused(reason)) => {
                return Err(DaemonError::Protocol(format!(
                    "daemon refused shutdown: {reason}"
                )));
            }
            // A lost reply or a racing restart can hide an accepted drain:
            // give it the same wait before the kill can cut it short.
            Err(error) => {
                eprintln!("daemon shutdown request failed, checking the process: {error}");
                wait_while_present(&paths, &old_instance)
            }
        };
        if !graceful && daemon_present(&paths, &old_instance) {
            kill_verified_daemon(&paths, expected)?;
        }
        // The goodbye record is written while the singleton lock is still
        // held, so the process itself is what says the slot is free.
        if !wait_while_alive(expected) {
            return Err(DaemonError::timed_out("waiting for the daemon to leave"));
        }
        let binary = resolve_daemon_binary()?;
        let child = spawn_daemon(&binary, &paths)?;
        reap_spawned_daemon(child);
        wait_for_instance(&paths, &old_instance)
    }

    pub fn session_create(
        &self,
        workspace_id: Option<String>,
        kind: SessionKind,
        idempotency_key: Option<String>,
    ) -> Result<Session, DaemonError> {
        self.session_create_with(workspace_id, kind, None, None, None, None, idempotency_key)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn session_create_with(
        &self,
        workspace_id: Option<String>,
        kind: SessionKind,
        provider: Option<String>,
        mode: Option<String>,
        cols: Option<u16>,
        rows: Option<u16>,
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
                // A human-started session is named by the daemon's fallback:
                // nothing in the app asks for a name yet, and inventing one
                // here would put a second naming path beside the protocol field.
                display_name: None,
                idempotency_key,
                // Absent, not zero, when the caller measured nothing: the
                // daemon reads an absent field as "no size asked" and spawns
                // at its default.
                cols,
                rows,
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
        self.session_attach_with_subscription(
            subscription_id,
            session_id,
            from_cursor,
            handler,
            None,
        )
    }

    /// `reset` is the caller's reader for the reply's resume outcome. Pass
    /// `None` and a reset is dropped with the rest of the reply's extras; pass
    /// a reader and it is called on the reader thread before the reply
    /// resolves, so the caller learns about a replaced timeline before any
    /// frame of this attach reaches it.
    pub fn session_attach_with_subscription(
        &self,
        subscription_id: SubscriptionId,
        session_id: &str,
        from_cursor: Option<Cursor>,
        handler: EventHandler,
        reset: Option<SessionResetHandler>,
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
                    reset,
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
        self.unsubscribe(subscription_id);
        self.remove_pending_subscription_for_id(subscription_id);
        match result? {
            DaemonMessage::Ok { .. } => Ok(()),
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

    /// Rename a session: the daemon validates the name, stores it on the
    /// session record and the journal row, and pushes the roster.
    pub fn session_set_name(
        &self,
        session_id: &str,
        display_name: &str,
    ) -> Result<(), DaemonError> {
        let id = self.alloc_id();
        match self.roundtrip(ClientMessage::SessionSetName {
            id,
            session_id: session_id.to_string(),
            display_name: display_name.to_string(),
        })? {
            DaemonMessage::Ok { .. } => Ok(()),
            DaemonMessage::Error(error) => Err(DaemonError::Handshake(error)),
            other => unexpected(other),
        }
    }

    pub fn session_set_feature(
        &self,
        session_id: &str,
        feature_id: &str,
        enabled: bool,
    ) -> Result<(), DaemonError> {
        let id = self.alloc_id();
        match self.roundtrip(ClientMessage::SessionSetFeature {
            id,
            session_id: session_id.to_string(),
            feature_id: feature_id.to_string(),
            enabled,
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
            None,
        )
        .map(|_| ())
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
    /// Send one prompt, naming the send's retry identity.
    ///
    /// `idempotency_key` is `None` for every send the app does not intend to
    /// repeat — a composer prompt is one keystroke — and the queued item's own
    /// id names the ones it does, so a rung of that queue's retry ladder that
    /// the daemon already took is answered from its receipt instead of running
    /// the same prompt a second time (`server/sessions.rs::send_fingerprint`
    /// is what refuses a key reused with different bytes).
    ///
    /// [`PromptAttachment`]: devboule_protocol::PromptAttachment
    // The frame carries seven fields and the client names them one for one, as
    // the daemon's own `session_send` dispatcher does (`server/sessions.rs`,
    // same allow there): a params struct invented to quiet the lint would be a
    // second shape for one wire message, with nothing in it that the frame does
    // not already name.
    #[allow(clippy::too_many_arguments)]
    pub fn session_send_with_subscription(
        &self,
        session_id: &str,
        subscription_id: SubscriptionId,
        text: &str,
        attachments: &[PromptAttachment],
        attachment_references: &[AttachmentReference],
        active_turn_behavior: Option<ActiveTurnBehavior>,
        idempotency_key: Option<String>,
    ) -> Result<bool, DaemonError> {
        self.require_gif_webp_agreed(attachments)?;
        let id = self.alloc_id();
        match self.roundtrip(ClientMessage::SessionSend {
            id,
            session_id: session_id.to_string(),
            subscription_id,
            text: text.to_string(),
            attachments: attachments.to_vec(),
            attachment_references: attachment_references.to_vec(),
            idempotency_key,
            active_turn_behavior,
        })? {
            DaemonMessage::SessionSend { turn_active, .. } => Ok(turn_active),
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
        self.require_gif_webp_agreed(std::slice::from_ref(attachment))?;
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

    /// Delete one stored attachment and release the bytes it held.
    ///
    /// The reference is the value a deposit or an upload finish answered with,
    /// verbatim. The daemon removes the content-addressed file and charges its
    /// bytes back to the owner budget; a reference whose file is already gone is
    /// `Ok`, because there is nothing left to release.
    pub fn session_attachment_delete(
        &self,
        reference: &AttachmentReference,
    ) -> Result<(), DaemonError> {
        self.require_agreed(devboule_protocol::caps::ATTACHMENTS_DELETE)?;
        let id = self.alloc_id();
        match self.roundtrip(ClientMessage::SessionAttachmentDelete {
            id,
            reference: reference.clone(),
        })? {
            DaemonMessage::Ok { .. } => Ok(()),
            DaemonMessage::Error(error) => Err(DaemonError::Handshake(error)),
            other => unexpected(other),
        }
    }

    /// Open one chunked file upload, or adopt the one already in progress under
    /// `upload_id`, and answer the offset it stands at.
    ///
    /// The upload id is the caller's, because a reconnecting client has to
    /// name the same upload to resume it. The daemon answers zero for a fresh
    /// one and the bytes already received when the same id and declaration are
    /// opened again, so a retry after a lost acknowledgement continues instead
    /// of starting over.
    pub fn session_upload_begin(
        &self,
        session_id: &str,
        upload_id: &str,
        name: &str,
        total_bytes: u64,
    ) -> Result<u64, DaemonError> {
        self.require_agreed(devboule_protocol::caps::ATTACHMENTS_UPLOAD)?;
        let id = self.alloc_id();
        match self.roundtrip(ClientMessage::SessionUploadBegin {
            id,
            session_id: session_id.to_string(),
            upload_id: upload_id.to_string(),
            name: name.to_string(),
            total_bytes,
        })? {
            DaemonMessage::SessionUploadProgress { received_bytes, .. } => Ok(received_bytes),
            DaemonMessage::Error(error) => Err(DaemonError::Handshake(error)),
            other => unexpected(other),
        }
    }

    /// How many bytes of one upload the daemon holds.
    pub fn session_upload_status(
        &self,
        session_id: &str,
        upload_id: &str,
    ) -> Result<u64, DaemonError> {
        self.require_agreed(devboule_protocol::caps::ATTACHMENTS_UPLOAD)?;
        let id = self.alloc_id();
        match self.roundtrip(ClientMessage::SessionUploadStatus {
            id,
            session_id: session_id.to_string(),
            upload_id: upload_id.to_string(),
        })? {
            DaemonMessage::SessionUploadProgress { received_bytes, .. } => Ok(received_bytes),
            DaemonMessage::Error(error) => Err(DaemonError::Handshake(error)),
            other => unexpected(other),
        }
    }

    /// Append one chunk at exactly the offset the upload stands at, and answer
    /// the new offset.
    pub fn session_upload_chunk(
        &self,
        session_id: &str,
        upload_id: &str,
        offset: u64,
        data: &str,
    ) -> Result<u64, DaemonError> {
        self.require_agreed(devboule_protocol::caps::ATTACHMENTS_UPLOAD)?;
        let id = self.alloc_id();
        match self.roundtrip(ClientMessage::SessionUploadChunk {
            id,
            session_id: session_id.to_string(),
            upload_id: upload_id.to_string(),
            offset,
            data: data.to_string(),
        })? {
            DaemonMessage::SessionUploadProgress { received_bytes, .. } => Ok(received_bytes),
            DaemonMessage::Error(error) => Err(DaemonError::Handshake(error)),
            other => unexpected(other),
        }
    }

    /// Close one fully received upload and answer the reference a send names.
    pub fn session_upload_finish(
        &self,
        session_id: &str,
        upload_id: &str,
    ) -> Result<AttachmentReference, DaemonError> {
        self.require_agreed(devboule_protocol::caps::ATTACHMENTS_UPLOAD)?;
        let id = self.alloc_id();
        match self.roundtrip(ClientMessage::SessionUploadFinish {
            id,
            session_id: session_id.to_string(),
            upload_id: upload_id.to_string(),
        })? {
            DaemonMessage::SessionDeposited { reference, .. } => Ok(reference),
            DaemonMessage::Error(error) => Err(DaemonError::Handshake(error)),
            other => unexpected(other),
        }
    }

    /// Discard one upload and the bytes received so far.
    pub fn session_upload_abort(
        &self,
        session_id: &str,
        upload_id: &str,
    ) -> Result<(), DaemonError> {
        self.require_agreed(devboule_protocol::caps::ATTACHMENTS_UPLOAD)?;
        let id = self.alloc_id();
        match self.roundtrip(ClientMessage::SessionUploadAbort {
            id,
            session_id: session_id.to_string(),
            upload_id: upload_id.to_string(),
        })? {
            DaemonMessage::Ok { .. } => Ok(()),
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

    /// The shared follow-up queue (protocol 22, the `session.queue`
    /// capability). All five frames name the caller's `client_operation_id`,
    /// and the daemon answers a repeat of one it has already answered from its
    /// ledger instead of queueing or sending a second time; a repeat with
    /// different bytes is refused as `operation_conflict`. `replayed` is
    /// therefore not news to a caller — an answered operation is applied once
    /// however many times it is asked — so these five resolve to nothing.
    ///
    /// The capability gate is first in every one of them, for the reason
    /// [`DaemonClient::require_agreed`] gives: a daemon from before protocol 22
    /// cannot deserialize these variants, and the connection would die on a
    /// frame it had no answer for.
    pub fn session_queue_add(
        &self,
        session_id: &str,
        client_operation_id: &str,
        text: &str,
        attachments: &[PromptAttachment],
        attachment_references: &[AttachmentReference],
    ) -> Result<(), DaemonError> {
        self.require_agreed(devboule_protocol::caps::SESSION_QUEUE)?;
        self.require_gif_webp_agreed(attachments)?;
        let id = self.alloc_id();
        match self.roundtrip(ClientMessage::SessionQueueAdd {
            id,
            session_id: session_id.to_string(),
            client_operation_id: client_operation_id.to_string(),
            text: text.to_string(),
            attachments: attachments.to_vec(),
            attachment_references: attachment_references.to_vec(),
        })? {
            DaemonMessage::QueueAccepted { .. } => Ok(()),
            DaemonMessage::Error(error) => Err(DaemonError::Handshake(error)),
            other => unexpected(other),
        }
    }

    /// Replace one queued row's text where it stands. See
    /// [`DaemonClient::session_queue_add`] for what the operation id and the
    /// capability gate are for.
    pub fn session_queue_edit(
        &self,
        session_id: &str,
        client_operation_id: &str,
        item_id: &str,
        text: &str,
    ) -> Result<(), DaemonError> {
        self.require_agreed(devboule_protocol::caps::SESSION_QUEUE)?;
        let id = self.alloc_id();
        match self.roundtrip(ClientMessage::SessionQueueEdit {
            id,
            session_id: session_id.to_string(),
            client_operation_id: client_operation_id.to_string(),
            item_id: item_id.to_string(),
            text: text.to_string(),
        })? {
            DaemonMessage::QueueAccepted { .. } => Ok(()),
            DaemonMessage::Error(error) => Err(DaemonError::Handshake(error)),
            other => unexpected(other),
        }
    }

    /// Take one queued row out. See [`DaemonClient::session_queue_add`] for the
    /// rest.
    pub fn session_queue_remove(
        &self,
        session_id: &str,
        client_operation_id: &str,
        item_id: &str,
    ) -> Result<(), DaemonError> {
        self.require_agreed(devboule_protocol::caps::SESSION_QUEUE)?;
        let id = self.alloc_id();
        match self.roundtrip(ClientMessage::SessionQueueRemove {
            id,
            session_id: session_id.to_string(),
            client_operation_id: client_operation_id.to_string(),
            item_id: item_id.to_string(),
        })? {
            DaemonMessage::QueueAccepted { .. } => Ok(()),
            DaemonMessage::Error(error) => Err(DaemonError::Handshake(error)),
            other => unexpected(other),
        }
    }

    /// Move one queued row, counting `to_index` in the queue the row has
    /// already left. See [`DaemonClient::session_queue_add`] for the rest.
    pub fn session_queue_move(
        &self,
        session_id: &str,
        client_operation_id: &str,
        item_id: &str,
        to_index: usize,
    ) -> Result<(), DaemonError> {
        self.require_agreed(devboule_protocol::caps::SESSION_QUEUE)?;
        let id = self.alloc_id();
        match self.roundtrip(ClientMessage::SessionQueueMove {
            id,
            session_id: session_id.to_string(),
            client_operation_id: client_operation_id.to_string(),
            item_id: item_id.to_string(),
            to_index,
        })? {
            DaemonMessage::QueueAccepted { .. } => Ok(()),
            DaemonMessage::Error(error) => Err(DaemonError::Handshake(error)),
            other => unexpected(other),
        }
    }

    /// Send one queued row now, which is a send and therefore needs the
    /// subscription a send has. See [`DaemonClient::session_queue_add`] for
    /// the rest.
    pub fn session_queue_send_now(
        &self,
        session_id: &str,
        client_operation_id: &str,
        subscription_id: SubscriptionId,
        item_id: &str,
    ) -> Result<(), DaemonError> {
        self.require_agreed(devboule_protocol::caps::SESSION_QUEUE)?;
        let id = self.alloc_id();
        match self.roundtrip(ClientMessage::SessionQueueSendNow {
            id,
            session_id: session_id.to_string(),
            client_operation_id: client_operation_id.to_string(),
            subscription_id,
            item_id: item_id.to_string(),
        })? {
            DaemonMessage::QueueAccepted { .. } => Ok(()),
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
        answer: Option<&str>,
    ) -> Result<(), DaemonError> {
        let id = self.alloc_id();
        match self.roundtrip(ClientMessage::SessionPermissionRespond {
            id,
            session_id: session_id.to_string(),
            subscription_id,
            request_id: request_id.to_string(),
            outcome,
            option_id: option_id.map(str::to_string),
            answer: answer.map(str::to_string),
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

    pub fn session_tasks(&self, session_id: &str) -> Result<(Vec<SessionTask>, u32), DaemonError> {
        // The capability gate is first, for the reason
        // [`DaemonClient::require_agreed`] gives: a daemon from before
        // protocol 28 cannot deserialize this variant, and the connection
        // would die on a frame it had no answer for.
        self.require_agreed(devboule_protocol::caps::SESSION_TASKS)?;
        let id = self.alloc_id();
        match self.roundtrip(ClientMessage::SessionTasksGet {
            id,
            session_id: session_id.to_string(),
        })? {
            DaemonMessage::SessionTasks { tasks, omitted, .. } => Ok((tasks, omitted)),
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

    /// One git write's shared shape: send the frame, take the one reply
    /// the four share, and hand back `null` (landed) or the refusing
    /// sentence — the wire's own, never composed here. The key rides as
    /// `None` the way the file writes send it: this road is the local
    /// app's, and the protocol's key exists for a retrying peer.
    fn workspace_git_write(
        &self,
        request: impl FnOnce(u64, Option<String>) -> ClientMessage,
    ) -> Result<Option<String>, DaemonError> {
        let id = self.alloc_id();
        match self.roundtrip(request(id, None))? {
            DaemonMessage::WorkspaceGitWrite { error, .. } => Ok(error),
            DaemonMessage::Error(error) => Err(DaemonError::Handshake(error)),
            other => unexpected(other),
        }
    }

    /// Stage paths in one workspace's index — `git add` over a confined
    /// selection, the daemon judging every path before it spawns. `null`
    /// is the act landed; a string is the refusal.
    pub fn workspace_git_stage(
        &self,
        workspace_id: &str,
        paths: &[String],
    ) -> Result<Option<String>, DaemonError> {
        self.workspace_git_write(|id, idempotency_key| ClientMessage::WorkspaceGitStage {
            id,
            workspace_id: workspace_id.to_string(),
            paths: paths.to_vec(),
            idempotency_key,
        })
    }

    /// Unstage paths in one workspace's index — the index entry returns to
    /// `HEAD`, the worktree keeps its bytes.
    pub fn workspace_git_unstage(
        &self,
        workspace_id: &str,
        paths: &[String],
    ) -> Result<Option<String>, DaemonError> {
        self.workspace_git_write(|id, idempotency_key| ClientMessage::WorkspaceGitUnstage {
            id,
            workspace_id: workspace_id.to_string(),
            paths: paths.to_vec(),
            idempotency_key,
        })
    }

    /// Discard paths in one workspace — the act that loses data: the
    /// selection returns to `HEAD` and untracked paths are deleted. The
    /// confirmation is the sending screen's own, never this road's.
    pub fn workspace_git_discard(
        &self,
        workspace_id: &str,
        paths: &[String],
    ) -> Result<Option<String>, DaemonError> {
        self.workspace_git_write(|id, idempotency_key| ClientMessage::WorkspaceGitDiscard {
            id,
            workspace_id: workspace_id.to_string(),
            paths: paths.to_vec(),
            idempotency_key,
        })
    }

    /// Commit what is staged in one workspace — never `add -A`: the
    /// daemon runs `git commit` over the index exactly as it stands, with
    /// the caller's own message (empty refused before anything spawns).
    pub fn workspace_git_commit(
        &self,
        workspace_id: &str,
        message: &str,
    ) -> Result<Option<String>, DaemonError> {
        self.workspace_git_write(|id, idempotency_key| ClientMessage::WorkspaceGitCommit {
            id,
            workspace_id: workspace_id.to_string(),
            message: message.to_string(),
            idempotency_key,
        })
    }

    /// The commit history of one workspace — the branch's own commits and
    /// the base branch's recent history, split at the fork point. The id is
    /// the whole argument: the daemon resolves the directory from it, like
    /// the status read. Refused unless the handshake negotiated
    /// `workspace.git_log`: a daemon that predates the frame cannot
    /// deserialize it and would kill the connection on a request it never
    /// knew.
    pub fn workspace_git_log(&self, workspace_id: &str) -> Result<WorkspaceGitLog, DaemonError> {
        self.require_agreed(devboule_protocol::caps::WORKSPACE_GIT_LOG)?;
        let id = self.alloc_id();
        match self.roundtrip(ClientMessage::WorkspaceGitLog {
            id,
            workspace_id: workspace_id.to_string(),
        })? {
            DaemonMessage::WorkspaceGitLog { log, .. } => Ok(log),
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

    /// One window of one workspace file: the id and a relative `path` are
    /// the whole argument for the first window — of an ordinary file,
    /// never a folder — and `from_line`/`line_count` address a later one,
    /// `None`/`None` being the default the first window needs. The daemon
    /// confines the path to a directory inside the workspace before
    /// opening anything, reads at most one window's cap, and classifies
    /// the bytes.
    pub fn workspace_file_read(
        &self,
        workspace_id: &str,
        path: &str,
        from_line: Option<u64>,
        line_count: Option<u64>,
    ) -> Result<WorkspaceFileContent, DaemonError> {
        let id = self.alloc_id();
        match self.roundtrip(ClientMessage::WorkspaceFileRead {
            id,
            workspace_id: workspace_id.to_string(),
            path: path.to_string(),
            from_line,
            line_count,
        })? {
            DaemonMessage::WorkspaceFileContent { file, .. } => Ok(file),
            DaemonMessage::Error(error) => Err(DaemonError::Handshake(error)),
            other => unexpected(other),
        }
    }

    /// The folder a workspace's open resolves to — the root the local
    /// desktop app joins the file against and re-validates immediately
    /// before it spawns an editor. The id is the whole argument; no file
    /// path travels this wire. Sent only under `workspace.open`: a daemon
    /// that predates the frame cannot deserialize it, and the negotiated
    /// name is how this client knows not to ask.
    pub fn workspace_open_root(&self, workspace_id: &str) -> Result<String, DaemonError> {
        self.require_agreed(devboule_protocol::caps::WORKSPACE_OPEN)?;
        let id = self.alloc_id();
        match self.roundtrip(ClientMessage::WorkspaceOpenRoot {
            id,
            workspace_id: workspace_id.to_string(),
        })? {
            DaemonMessage::WorkspaceOpenRoot { root, .. } => Ok(root),
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

    /// Delete one workspace entry: the act that loses data. No confirmation
    /// exists on this road — the confirmation is the caller's screen (the
    /// Files panel asks before it calls), never the wire's — and the daemon
    /// re-judges the path with every guard the reads and the other writes
    /// use before removing anything.
    pub fn workspace_file_delete(
        &self,
        workspace_id: &str,
        path: &str,
    ) -> Result<WorkspaceFileMutation, DaemonError> {
        let id = self.alloc_id();
        match self.roundtrip(ClientMessage::WorkspaceFileDelete {
            id,
            workspace_id: workspace_id.to_string(),
            path: path.to_string(),
            idempotency_key: None,
        })? {
            DaemonMessage::WorkspaceFileDeleted { change, .. } => Ok(change),
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

    /// Store a workspace's new title; the reply is the row as it now stands,
    /// so the caller shows the stored title rather than the one it sent.
    pub fn workspace_set_title(
        &self,
        workspace_id: &str,
        title: &str,
    ) -> Result<Workspace, DaemonError> {
        self.require_agreed(devboule_protocol::caps::WORKSPACE_RENAME)?;
        let id = self.alloc_id();
        match self.roundtrip(ClientMessage::WorkspaceSetTitle {
            id,
            workspace_id: workspace_id.to_string(),
            title: title.to_string(),
        })? {
            DaemonMessage::Workspace { workspace, .. } => Ok(workspace),
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

    /// Asks the daemon to display a fresh one-time pairing code.
    ///
    /// The code is a five-minute secret: it leaves here only inside the returned
    /// frame, which the panel puts on screen. This method formats no frame into
    /// a message, so no error path of it can carry the code. No role is asked:
    /// the code-displaying device confirms every new pairing itself, and what
    /// the paired device becomes is decided later by whether it hosts a
    /// workspace.
    pub fn pairing_start(&self) -> Result<DaemonMessage, DaemonError> {
        self.require_pairing_dialect()?;
        let id = self.alloc_id();
        match self.roundtrip(ClientMessage::PairingStart { id })? {
            reply @ DaemonMessage::PairingCode { .. } => Ok(reply),
            DaemonMessage::Error(error) => Err(DaemonError::Handshake(error)),
            _ => pairing_reply_mismatch(),
        }
    }

    /// Refuse pairing against a daemon that predates the roleless requests.
    ///
    /// Every dialect before 32 requires a `role` field in `PairingStart` and
    /// `PairingComplete`; a roleless request is a frame that old decoder cannot
    /// construct, and the connection would die on it. The app and its daemon
    /// normally ship together, so the actionable sentence is to restart
    /// devboule so the matching daemon binary starts.
    fn require_pairing_dialect(&self) -> Result<(), DaemonError> {
        let daemon = self.inner.hello.protocol_version;
        if daemon >= devboule_protocol::PROTOCOL_VERSION {
            return Ok(());
        }
        Err(DaemonError::Handshake(WireError::new(
            ErrorCode::ProtocolVersionMismatch,
            format!(
                "the running daemon speaks protocol {daemon} and cannot take a roleless pairing; \
                 restart devboule so the matching daemon binary starts"
            ),
        )))
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
    ) -> Result<DaemonMessage, DaemonError> {
        self.require_pairing_dialect()?;
        let id = self.alloc_id();
        match self.roundtrip(ClientMessage::PairingComplete {
            id,
            address: address.to_string(),
            code,
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
    /// the table records it. Returns the row as it now stands.
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

    /// Switch one provider off or back on. Off hides it from the picker and
    /// refuses every spawn and probe; live sessions keep running. Rides the
    /// provider-switch capability, and the reply is the
    /// daemon's frame rather than `()`, like theirs.
    pub fn provider_set_enabled(
        &self,
        provider_id: &str,
        enabled: bool,
    ) -> Result<DaemonMessage, DaemonError> {
        self.require_agreed(devboule_protocol::caps::PROVIDER_SWITCHES)?;
        let id = self.alloc_id();
        match self.roundtrip(ClientMessage::ProviderSetEnabled {
            id,
            provider_id: provider_id.to_string(),
            enabled,
        })? {
            reply @ DaemonMessage::ProviderSetEnabledOk { .. } => Ok(reply),
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
        model: Option<&str>,
        refresh: bool,
    ) -> Result<DaemonMessage, DaemonError> {
        self.require_agreed(devboule_protocol::caps::PROVIDER_VOCABULARY)?;
        let id = self.alloc_id();
        match self.roundtrip(ClientMessage::ProviderVocabularyGet {
            id,
            provider: provider.to_string(),
            model: model.map(|model| model.to_string()),
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

    /// Install the handler for the daemon-pushed host state. A sidebar renders
    /// from this rather than from a reply, because a link comes up, drops and
    /// comes back while nobody is asking it anything.
    pub fn on_remote_host_status(&self, handler: RemoteHostStatusHandler) {
        *self
            .inner
            .remote_host_status_handler
            .lock()
            .unwrap_or_else(|err| err.into_inner()) = Some(handler);
    }

    /// Hold one link to a paired daemon peer for as long as this process wants
    /// that host's row. Several hosts may be held at once; a second watch of
    /// one host shares the link the first watch opened.
    pub fn remote_host_watch(&self, device_id: &str) -> Result<(), DaemonError> {
        self.require_agreed(devboule_protocol::caps::REMOTE_HOSTS)?;
        let id = self.alloc_id();
        match self.roundtrip(ClientMessage::RemoteHostWatch {
            id,
            device_id: device_id.to_string(),
        })? {
            DaemonMessage::Ok { .. } => Ok(()),
            DaemonMessage::Error(error) => Err(DaemonError::Handshake(error)),
            other => unexpected(other),
        }
    }

    /// Give back this process's lease on one host.
    pub fn remote_host_unwatch(&self, device_id: &str) -> Result<(), DaemonError> {
        self.require_agreed(devboule_protocol::caps::REMOTE_HOSTS)?;
        let id = self.alloc_id();
        match self.roundtrip(ClientMessage::RemoteHostUnwatch {
            id,
            device_id: device_id.to_string(),
        })? {
            DaemonMessage::Ok { .. } => Ok(()),
            DaemonMessage::Error(error) => Err(DaemonError::Handshake(error)),
            other => unexpected(other),
        }
    }

    /// Read one list from a paired daemon peer, over the link the watch holds.
    ///
    /// The body is the remote daemon's own rows: nothing here filters,
    /// reorders or fills it in, and a refusal is the remote's own error with
    /// its reason intact.
    pub fn remote_host_list(
        &self,
        device_id: &str,
        list: RemoteHostList,
    ) -> Result<RemoteHostListBody, DaemonError> {
        self.require_agreed(devboule_protocol::caps::REMOTE_HOSTS)?;
        let id = self.alloc_id();
        match self.roundtrip(ClientMessage::RemoteHostList {
            id,
            device_id: device_id.to_string(),
            list,
        })? {
            DaemonMessage::RemoteHostList { body, .. } => Ok(body),
            DaemonMessage::Error(error) => Err(DaemonError::Handshake(error)),
            other => unexpected(other),
        }
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

    /// `force` asks the daemon to measure again instead of reusing a fresh
    /// observation: a panel open may reuse, a deliberate Refresh must not.
    pub fn providers_auth_check(
        &self,
        force: bool,
    ) -> Result<(Vec<ProviderInfo>, u32), DaemonError> {
        self.require_agreed(devboule_protocol::caps::PROVIDER_AUTH_CHECK)?;
        let id = self.alloc_id();
        match self.roundtrip(ClientMessage::ProvidersAuthCheck { id, force })? {
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

/// Unix restart helpers: a graceful stop is read off the record, a killed
/// one off the process, since it writes no goodbye. Killing needs a fresh
/// peer check plus the executable.
#[cfg(unix)]
const RESTART_WAIT: Duration = Duration::from_secs(30);
#[cfg(unix)]
const RESTART_POLL: Duration = Duration::from_millis(100);

/// A live record for this instance means the old daemon is still up.
/// Anything else — absent, stale, stopped, another instance — counts as
/// gone for the caller's purpose.
#[cfg(unix)]
fn daemon_present(paths: &RuntimePaths, instance: &str) -> bool {
    match crate::DaemonState::read(&paths.lock_file) {
        crate::DaemonState::Live(record) => record.ready && record.instance_id == instance,
        _ => false,
    }
}

/// Whether the record stopped naming `instance` within `RESTART_WAIT`: the
/// graceful stop's own proof, its goodbye record.
#[cfg(unix)]
fn wait_while_present(paths: &RuntimePaths, instance: &str) -> bool {
    let deadline = Instant::now() + RESTART_WAIT;
    while daemon_present(paths, instance) {
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(RESTART_POLL);
    }
    true
}

/// Whether `pid` left within `RESTART_WAIT`. A killed daemon writes no
/// goodbye, so its record reads `Live` until `STALE_AFTER` — and even a
/// graceful goodbye is written before the lock drops. The process is what the
/// spawn needs gone, and the app's spawn waiter reaps it so this stops seeing
/// it.
#[cfg(unix)]
fn wait_while_alive(pid: u32) -> bool {
    let deadline = Instant::now() + RESTART_WAIT;
    while process_alive(pid) {
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(RESTART_POLL);
    }
    true
}

/// Whether the pid exists in any state, a zombie included: signal 0 asks the
/// kernel nothing else.
#[cfg(unix)]
fn process_alive(pid: u32) -> bool {
    // SAFETY: signal 0 delivers nothing; it only asks the kernel.
    unsafe { libc::kill(pid as libc::pid_t, 0) == 0 }
}

/// The replacement, recognized by a live ready record under a new id.
#[cfg(unix)]
fn wait_for_instance(paths: &RuntimePaths, old_instance: &str) -> Result<(), DaemonError> {
    let deadline = Instant::now() + RESTART_WAIT;
    loop {
        if let crate::DaemonState::Live(record) = crate::DaemonState::read(&paths.lock_file) {
            if record.ready && record.instance_id != old_instance {
                return Ok(());
            }
        }
        if Instant::now() >= deadline {
            return Err(DaemonError::timed_out("waiting for the restarted daemon"));
        }
        std::thread::sleep(RESTART_POLL);
    }
}

/// Last resort for an unreachable-but-present daemon: reconnect, confirm
/// the peer is still the expected one (same uid, same pid) and still the
/// expected executable, and only then signal it. Anything unverifiable
/// refuses instead of killing by a remembered number.
///
/// The Unix restart falls back to this when the daemon is unreachable but
/// still present; the Unix end-to-end test's cleanup guard shares it so a
/// leaked test daemon is stopped by the same rule.
#[cfg(unix)]
pub fn kill_verified_daemon(paths: &RuntimePaths, expected_pid: u32) -> Result<(), DaemonError> {
    // Held open for the whole guard, re-read and signal: the peer's identity
    // is taken from this connection, never from the remembered pid alone.
    let file = transport::connect(paths).map_err(|error| {
        DaemonError::Protocol(format!("cannot reach the daemon to verify it: {error}"))
    })?;
    let peer = transport::peer_identity(&file).map_err(DaemonError::from)?;
    if peer.user != transport::local_uid().to_string() {
        return Err(DaemonError::Protocol(
            "refusing to kill a daemon socket held by another user".to_string(),
        ));
    }
    if peer.pid != expected_pid {
        return Err(DaemonError::Protocol(format!(
            "daemon pid changed (expected {expected_pid}, holds {})",
            peer.pid
        )));
    }
    let actual = daemon_exe_of(peer.pid)?;
    let wanted = std::fs::canonicalize(resolve_daemon_binary()?)?;
    if actual != wanted {
        return Err(DaemonError::Protocol(format!(
            "daemon executable changed: {}",
            actual.display()
        )));
    }
    // While this end is connected the peer cannot have exited, so the pid it
    // reports now cannot be a recycled one; the signal uses this last read.
    let held = transport::peer_identity(&file).map_err(DaemonError::from)?;
    if held.user != peer.user || held.pid != peer.pid {
        return Err(DaemonError::Protocol(
            "the daemon's peer identity changed before the signal".to_string(),
        ));
    }
    // SAFETY: the pid was just re-read from the held connection and verified
    // against the expected user, pid and executable above; ESRCH (already
    // gone) already satisfies the caller, every other failure refuses.
    let killed = unsafe { libc::kill(held.pid as libc::pid_t, libc::SIGKILL) };
    if killed != 0 {
        let error = std::io::Error::last_os_error();
        if error.raw_os_error() != Some(libc::ESRCH) {
            return Err(DaemonError::from(error));
        }
    }
    Ok(())
}

/// What `/proc` (Linux) or the kernel (macOS) says this pid executes.
/// Anything unreadable refuses the kill that asked.
#[cfg(target_os = "macos")]
fn daemon_exe_of(pid: u32) -> io::Result<PathBuf> {
    use std::os::unix::ffi::OsStrExt;
    let mut buffer = vec![0u8; 1024];
    // SAFETY: proc_pidpath fills the live buffer up to its length.
    let length = unsafe {
        libc::proc_pidpath(
            pid as libc::pid_t,
            buffer.as_mut_ptr() as *mut libc::c_void,
            buffer.len() as u32,
        )
    };
    if length <= 0 {
        return Err(io::Error::last_os_error());
    }
    let bytes = &buffer[..length as usize];
    Ok(PathBuf::from(std::ffi::OsStr::from_bytes(bytes)))
}

#[cfg(target_os = "linux")]
fn daemon_exe_of(pid: u32) -> io::Result<PathBuf> {
    std::fs::read_link(format!("/proc/{pid}/exe"))
}

#[cfg(all(unix, not(any(target_os = "macos", target_os = "linux"))))]
fn daemon_exe_of(pid: u32) -> io::Result<PathBuf> {
    let _ = pid;
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "peer executable lookup is implemented for macOS and Linux only",
    ))
}

pub fn connect(paths: &RuntimePaths, hello: ClientHello) -> Result<DaemonClient, DaemonError> {
    let file = transport::connect(paths)?;
    handshake_with_runtime(file, hello, Some(paths.clone()))
}

/// [`connect`] for a caller that cannot wait out a busy pipe: it stops waiting
/// on one after about `budget`.
pub fn connect_within(
    paths: &RuntimePaths,
    hello: ClientHello,
    budget: Duration,
) -> Result<DaemonClient, DaemonError> {
    let file = transport::connect_within(paths, budget)?;
    handshake_with_runtime(file, hello, Some(paths.clone()))
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
        let child = spawn_daemon(binary, paths)?;
        reap_spawned_daemon(child);
        Ok(())
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
    handshake_with_runtime(file, hello, None)
}

fn handshake_with_runtime(
    file: File,
    hello: ClientHello,
    runtime: Option<RuntimePaths>,
) -> Result<DaemonClient, DaemonError> {
    #[cfg(windows)]
    let server_pid = crate::transport::server_process_id(&file).ok();
    #[cfg(unix)]
    let server_pid = Some(verify_server_peer(&file)?);
    #[cfg(not(any(windows, unix)))]
    let server_pid = None;
    let framed = Framed::new(file);
    framed.send(&ClientMessage::Hello(hello))?;
    let reply: DaemonMessage = framed.recv_timeout(HANDSHAKE_TIMEOUT)?;
    match reply {
        DaemonMessage::Hello(daemon_hello) => {
            let (browser_requests, browser_request_inbox) =
                mpsc::sync_channel(browser::REQUEST_QUEUE);
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
                remote_host_status_handler: Mutex::new(None),
                browser_requests: Mutex::new(Some(browser_requests)),
                browser_request_inbox: Mutex::new(Some(browser_request_inbox)),
                browser_rejects: Mutex::new(None),
                stop: AtomicBool::new(false),
                hello: daemon_hello,
                server_pid,
                runtime,
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

/// Kernel check on the server end before the hello goes out: the socket
/// must be held by this user, so a path planted by another account is
/// refused instead of talked to.
#[cfg(unix)]
fn verify_server_peer(file: &File) -> Result<u32, DaemonError> {
    let peer = crate::transport::peer_identity(file).map_err(DaemonError::from)?;
    if peer.user != crate::transport::local_uid().to_string() {
        return Err(DaemonError::Handshake(WireError::new(
            ErrorCode::Unauthorized,
            "refusing a daemon socket held by another user",
        )));
    }
    Ok(peer.pid)
}

pub fn test_owner(client: &str) -> Result<OwnerId, DaemonError> {
    #[cfg(windows)]
    {
        let user = crate::security::current_user_sid()?;
        OwnerId::new(user, client).map_err(DaemonError::Protocol)
    }
    #[cfg(not(any(windows, unix)))]
    {
        OwnerId::new("unix", client).map_err(DaemonError::Protocol)
    }
    #[cfg(unix)]
    {
        let user = crate::transport::local_uid().to_string();
        OwnerId::new(user, client).map_err(DaemonError::Protocol)
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
                // The host-status push answers no request either, and arrives
                // only for the hosts this connection watches.
                if let DaemonMessage::RemoteHostStatus {
                    device_id,
                    state,
                    last_failure,
                } = &message
                {
                    let handler = inner
                        .remote_host_status_handler
                        .lock()
                        .unwrap_or_else(|err| err.into_inner())
                        .clone();
                    if let Some(handler) = handler {
                        handler(RemoteHostStatus {
                            device_id: device_id.clone(),
                            state: *state,
                            last_failure: last_failure.clone(),
                        });
                    }
                    continue;
                }
                // A browser command for this process's host: queued for the
                // host's own thread and never run here, so this reader stays
                // free to receive the reply the host's answer waits on.
                if let DaemonMessage::BrowserExecuteRequest(request) = message {
                    browser::enqueue_request(&inner, request);
                    continue;
                }
                if let Some(id) = daemon_message_id(&message) {
                    if let DaemonMessage::SessionAttached {
                        subscription_id,
                        resume,
                        ..
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
                            reset,
                        }) = pending_subscription
                            .filter(|pending| pending.subscription_id == *subscription_id)
                        {
                            if let Some(on_reset) = reset {
                                if let Some(reset) = resume.as_ref().filter(|info| {
                                    matches!(info.resume, SessionResumeOutcome::Reset { .. })
                                }) {
                                    // On this thread, at this frame, so it is
                                    // strictly before the replay that follows
                                    // the reply: the view replaces its timeline
                                    // before a single row of it arrives.
                                    on_reset(reset.clone());
                                }
                            }
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
    inner
        .browser_requests
        .lock()
        .unwrap_or_else(|err| err.into_inner())
        .take();
    inner
        .browser_rejects
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
        | DaemonMessage::DelegationChanged { .. }
        // The host-status push is a broadcast for the same reason, and it is
        // handed to the watch handler before this match runs.
        | DaemonMessage::RemoteHostStatus { .. }
        // A command for the browser host answers no request either; the
        // reader queues it for the host before this match runs.
        | DaemonMessage::BrowserExecuteRequest(_) => None,
        DaemonMessage::Error(error) => error.id,
        // Every request-shaped reply carries its id. The device RPCs are
        // listed rather than swept into a wildcard: this match is exhaustive on
        // purpose, so a new reply variant is a compile error here until it is
        // given a decision. `DelegationState`/`DelegationSetOk` are
        // request-shaped and sit in this arm — putting one in the arm above
        // would silence the compiler and hang the caller forever.
        DaemonMessage::ProviderVocabulary { id, .. }
        | DaemonMessage::BrowserHostRegistered { id, .. }
        | DaemonMessage::Devices { id, .. }
        | DaemonMessage::PeerAgents { id, .. }
        | DaemonMessage::PairingCode { id, .. }
        | DaemonMessage::PairingPending { id, .. }
        | DaemonMessage::PairingDone { id, .. }
        | DaemonMessage::PeerUpdated { id, .. }
        | DaemonMessage::PairingDeclined { id, .. }
        | DaemonMessage::ToolPolicy { id, .. }
        | DaemonMessage::ToolPolicySetOk { id }
        | DaemonMessage::ProviderSetEnabledOk { id }
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
        | DaemonMessage::SessionTasks { id, .. }
        | DaemonMessage::Projects { id, .. }
        | DaemonMessage::Project { id, .. }
        | DaemonMessage::Workspaces { id, .. }
        | DaemonMessage::Workspace { id, .. }
        | DaemonMessage::WorkspaceGit { id, .. }
        | DaemonMessage::WorkspaceGitFile { id, .. }
        | DaemonMessage::WorkspaceGitLog { id, .. }
        | DaemonMessage::WorkspaceGitWrite { id, .. }
        | DaemonMessage::WorkspaceFiles { id, .. }
        | DaemonMessage::WorkspaceFileContent { id, .. }
        | DaemonMessage::WorkspaceOpenRoot { id, .. }
        | DaemonMessage::WorkspaceFileRenamed { id, .. }
        | DaemonMessage::WorkspaceFileDuplicated { id, .. }
        | DaemonMessage::WorkspaceFileDeleted { id, .. }
        | DaemonMessage::WorkspaceFilePreviewStaged { id, .. }
        | DaemonMessage::SessionAttached { id, .. }
        | DaemonMessage::JournalUsage { id, .. }
        | DaemonMessage::JournalRetention { id, .. }
        | DaemonMessage::Providers { id, .. }
        | DaemonMessage::ProviderUpdated { id, .. }
        | DaemonMessage::Ok { id }
        | DaemonMessage::QueueAccepted { id, .. }
        | DaemonMessage::SessionSend { id, .. }
        | DaemonMessage::AgentMessageReceipt { id, .. }
        | DaemonMessage::Resume { id, .. }
        | DaemonMessage::SessionDeposited { id, .. }
        | DaemonMessage::SessionUploadProgress { id, .. }
        | DaemonMessage::SessionAttachment { id, .. }
        | DaemonMessage::RemoteHostList { id, .. }
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

#[cfg(all(test, feature = "server"))]
#[path = "client_resume_tests.rs"]
mod resume_tests;

#[path = "client_browser.rs"]
mod browser;

#[cfg(all(test, feature = "server", windows))]
#[path = "client_browser_tests.rs"]
mod browser_tests;
