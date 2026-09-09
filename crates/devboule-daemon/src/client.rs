use std::collections::HashMap;
use std::fs::File;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use devboule_protocol::{
    AgentActivityState, ClientHello, ClientMessage, Cursor, DaemonHello, DaemonMessage,
    DaemonStatusBody, ErrorCode, JournalRetention, JournalUsage, OwnerId, PermissionOutcome,
    Persistence, Project, ProviderInfo, ResumeResult, RetentionPatch, Session, SessionEvent,
    SessionEventEnvelope, SessionKind, SessionStateSnapshot, SubscriptionId, WireError, Workspace,
    WorkspaceIsolation,
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

pub type EventHandler = Arc<dyn Fn(SessionEventEnvelope) + Send + Sync>;
pub type SessionStateHandler = Arc<dyn Fn(Vec<SessionStateSnapshot>) + Send + Sync>;

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
    default_subscriptions: Mutex<HashMap<String, SubscriptionId>>,
    session_state_subscription: Mutex<Option<SessionStateHandler>>,
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
            DaemonMessage::Error(error) if error.code == ErrorCode::Io => {
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
                &self.inner.framed.as_file(),
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
        self.session_create_with(workspace_id, kind, None, idempotency_key)
    }

    pub fn session_create_with(
        &self,
        workspace_id: Option<String>,
        kind: SessionKind,
        provider: Option<String>,
        idempotency_key: Option<String>,
    ) -> Result<Session, DaemonError> {
        let id = self.alloc_id();
        match self.roundtrip(ClientMessage::SessionCreate {
            id,
            workspace_id,
            kind,
            provider,
            idempotency_key,
        })? {
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

    pub fn session_claim(&self, session_id: &str) -> Result<(), DaemonError> {
        self.session_claim_with_subscription(session_id, self.control_subscription_id(session_id))
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

    pub fn session_close(&self, session_id: &str) -> Result<(), DaemonError> {
        let id = self.alloc_id();
        let result = self.roundtrip(ClientMessage::SessionClose {
            id,
            session_id: session_id.to_string(),
            idempotency_key: None,
        });
        self.unsubscribe_session(session_id);
        match result? {
            DaemonMessage::Ok { .. } => Ok(()),
            DaemonMessage::Error(error) => Err(DaemonError::Handshake(error)),
            other => unexpected(other),
        }
    }

    pub fn session_stop(&self, session_id: &str) -> Result<(), DaemonError> {
        self.session_stop_with_subscription(session_id, self.control_subscription_id(session_id))
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

    pub fn session_interrupt(&self, session_id: &str) -> Result<(), DaemonError> {
        self.session_interrupt_with_subscription(
            session_id,
            self.control_subscription_id(session_id),
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

    pub fn session_send(&self, session_id: &str, text: &str) -> Result<(), DaemonError> {
        self.session_send_with_subscription(
            session_id,
            self.control_subscription_id(session_id),
            text,
        )
    }

    pub fn session_send_with_subscription(
        &self,
        session_id: &str,
        subscription_id: SubscriptionId,
        text: &str,
    ) -> Result<(), DaemonError> {
        let id = self.alloc_id();
        match self.roundtrip(ClientMessage::SessionSend {
            id,
            session_id: session_id.to_string(),
            subscription_id,
            text: text.to_string(),
            idempotency_key: None,
        })? {
            DaemonMessage::Ok { .. } => Ok(()),
            DaemonMessage::Error(error) => Err(DaemonError::Handshake(error)),
            other => unexpected(other),
        }
    }

    pub fn session_resize(
        &self,
        session_id: &str,
        cols: u16,
        rows: u16,
    ) -> Result<(), DaemonError> {
        self.session_resize_with_subscription(
            session_id,
            self.control_subscription_id(session_id),
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

    pub fn session_permission_respond(
        &self,
        session_id: &str,
        request_id: &str,
        outcome: PermissionOutcome,
    ) -> Result<(), DaemonError> {
        self.session_permission_respond_with_subscription(
            session_id,
            self.control_subscription_id(session_id),
            request_id,
            outcome,
        )
    }

    pub fn session_permission_respond_with_subscription(
        &self,
        session_id: &str,
        subscription_id: SubscriptionId,
        request_id: &str,
        outcome: PermissionOutcome,
    ) -> Result<(), DaemonError> {
        let id = self.alloc_id();
        match self.roundtrip(ClientMessage::SessionPermissionRespond {
            id,
            session_id: session_id.to_string(),
            subscription_id,
            request_id: request_id.to_string(),
            outcome,
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
        match self.roundtrip(ClientMessage::SessionResume {
            id,
            persistence,
            idempotency_key,
        })? {
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
        let file = self.inner.framed.as_file();
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

    fn default_subscription(&self, session_id: &str) -> Option<SubscriptionId> {
        self.inner
            .default_subscriptions
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .get(session_id)
            .copied()
    }

    fn control_subscription_id(&self, session_id: &str) -> SubscriptionId {
        self.default_subscription(session_id).unwrap_or(0)
    }

    fn unsubscribe(&self, subscription_id: SubscriptionId) {
        let removed = self
            .inner
            .subscriptions
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .remove(&subscription_id);
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
                default_subscriptions: Mutex::new(HashMap::new()),
                session_state_subscription: Mutex::new(None),
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
                            // A reattach can follow a resume that replaced the
                            // runtime, so the old default token may no longer
                            // belong to this session generation.
                            inner
                                .default_subscriptions
                                .lock()
                                .unwrap_or_else(|err| err.into_inner())
                                .insert(session_id, *subscription_id);
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
            event: SessionEvent::Exit { code: None },
        });
    }
    for (session_id, handler) in pending_subscriptions {
        handler(SessionEventEnvelope {
            session_id,
            generation: 0,
            event: SessionEvent::Exit { code: None },
        });
    }
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
    let error = DaemonMessage::Error(WireError::new(ErrorCode::Io, error.to_string()));
    for tx in pending {
        let _ = tx.send(error.clone());
    }
}

fn daemon_message_id(message: &DaemonMessage) -> Option<u64> {
    match message {
        DaemonMessage::Hello(_)
        | DaemonMessage::Event(_)
        | DaemonMessage::SubscriptionEvent { .. } => None,
        DaemonMessage::Error(error) => error.id,
        DaemonMessage::Pong { id, .. }
        | DaemonMessage::Status { id, .. }
        | DaemonMessage::Diagnostics { id, .. }
        | DaemonMessage::Shutdown { id, .. }
        | DaemonMessage::Session { id, .. }
        | DaemonMessage::Sessions { id, .. }
        | DaemonMessage::Projects { id, .. }
        | DaemonMessage::Project { id, .. }
        | DaemonMessage::Workspaces { id, .. }
        | DaemonMessage::Workspace { id, .. }
        | DaemonMessage::SessionAttached { id, .. }
        | DaemonMessage::JournalUsage { id, .. }
        | DaemonMessage::JournalRetention { id, .. }
        | DaemonMessage::Providers { id, .. }
        | DaemonMessage::ProviderUpdated { id, .. }
        | DaemonMessage::Ok { id }
        | DaemonMessage::Resume { id, .. }
        | DaemonMessage::InvokeResult { id, .. } => Some(*id),
    }
}

fn unexpected<T>(message: DaemonMessage) -> Result<T, DaemonError> {
    Err(DaemonError::Protocol(format!(
        "unexpected daemon frame: {message:?}"
    )))
}

#[cfg(all(test, feature = "server"))]
mod tests {
    use super::{PROVIDER_UPDATE_RPC_TIMEOUT, RPC_TIMEOUT};
    use crate::framing::Framed;
    use crate::provider_update::UPDATE_TIMEOUT;
    #[cfg(windows)]
    use crate::transport::{Listener, NamedPipeListener};
    use devboule_protocol::{ClientMessage, DaemonHello, DaemonMessage, SessionEvent};
    use std::sync::atomic::AtomicBool;
    use std::sync::mpsc;
    use std::sync::Arc;
    use std::thread;
    use std::time::Duration;

    #[test]
    fn provider_update_deadline_has_install_headroom() {
        // Keep the RPC deadline above the runner timeout plus 30 seconds: reverting
        // provider_update to the normal 30-second RPC default would silently cut
        // off long installs. The complete wiring needs a fake pipe to test; these
        // constants protect the deadline relationship directly.
        assert!(PROVIDER_UPDATE_RPC_TIMEOUT > UPDATE_TIMEOUT + Duration::from_secs(30));
        assert_eq!(RPC_TIMEOUT, Duration::from_secs(30));
    }

    #[test]
    fn dead_connection_recovery_still_spawns_then_retries() {
        let paths = crate::paths::RuntimePaths::from_dir("fake-dead-daemon");
        let hello = devboule_protocol::ClientHello::m3a(
            super::test_owner("dead-recovery-test").expect("owner"),
            "dead-recovery-test",
        );
        let mut connects = 0;
        let mut spawns = 0;
        let result = super::connect_or_spawn_with(
            &paths,
            hello,
            std::path::Path::new("fake-daemon.exe"),
            |_, _| {
                connects += 1;
                if connects == 1 {
                    Err(crate::DaemonError::Io(std::io::Error::new(
                        std::io::ErrorKind::NotFound,
                        "dead daemon",
                    )))
                } else {
                    Ok(42u32)
                }
            },
            |_, _| {
                spawns += 1;
                Ok(())
            },
        )
        .expect("the next connection recovers");
        assert_eq!(result, 42);
        assert_eq!(connects, 2);
        assert_eq!(spawns, 1);
    }

    #[cfg(windows)]
    #[test]
    fn subscription_events_route_by_their_subscription_id() {
        let dir = std::env::temp_dir().join(format!(
            "devboule-client-routing-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        let paths = crate::paths::RuntimePaths::from_dir(&dir);
        let stop = Arc::new(AtomicBool::new(false));
        let mut listener = NamedPipeListener::bind(&paths, Arc::clone(&stop)).expect("bind");
        let (release_tx, release_rx) = mpsc::channel();
        let server = thread::spawn(move || {
            let file = listener.accept().expect("accept");
            let framed = Framed::new(file);
            let hello = framed.recv::<ClientMessage>().expect("client hello");
            assert!(matches!(hello, ClientMessage::Hello(_)));
            framed
                .send(&DaemonMessage::Hello(DaemonHello::plugin_backend(
                    "routing-test",
                    std::process::id(),
                )))
                .expect("hello reply");

            let first = framed
                .recv::<ClientMessage>()
                .expect("first attach request");
            let ClientMessage::SessionAttach {
                id: first_id,
                subscription_id: first_subscription,
                ..
            } = first
            else {
                panic!("expected first attach request");
            };
            framed
                .send(&DaemonMessage::SessionAttached {
                    id: first_id,
                    subscription_id: first_subscription,
                })
                .expect("first attach reply");
            framed
                .send(&DaemonMessage::SubscriptionEvent {
                    subscription_id: first_subscription,
                    envelope: devboule_protocol::SessionEventEnvelope {
                        session_id: "s.routing".to_string(),
                        generation: 1,
                        event: SessionEvent::AgentMessage {
                            message_id: None,
                            text: "a-1".to_string(),
                        },
                    },
                })
                .expect("first A event");

            let second = framed
                .recv::<ClientMessage>()
                .expect("second attach request");
            let ClientMessage::SessionAttach {
                id: second_id,
                subscription_id: second_subscription,
                ..
            } = second
            else {
                panic!("expected second attach request");
            };
            framed
                .send(&DaemonMessage::SubscriptionEvent {
                    subscription_id: first_subscription,
                    envelope: devboule_protocol::SessionEventEnvelope {
                        session_id: "s.routing".to_string(),
                        generation: 1,
                        event: SessionEvent::AgentMessage {
                            message_id: None,
                            text: "a-2".to_string(),
                        },
                    },
                })
                .expect("remaining A event");
            framed
                .send(&DaemonMessage::SessionAttached {
                    id: second_id,
                    subscription_id: second_subscription,
                })
                .expect("second attach reply");
            framed
                .send(&DaemonMessage::SubscriptionEvent {
                    subscription_id: second_subscription,
                    envelope: devboule_protocol::SessionEventEnvelope {
                        session_id: "s.routing".to_string(),
                        generation: 1,
                        event: SessionEvent::AgentMessage {
                            message_id: None,
                            text: "b-1".to_string(),
                        },
                    },
                })
                .expect("B event");
            let _ = release_rx.recv_timeout(Duration::from_secs(10));
        });

        let connection_deadline = std::time::Instant::now() + Duration::from_secs(10);
        let connection = loop {
            match crate::transport::connect(&paths) {
                Ok(connection) => break connection,
                Err(_) if std::time::Instant::now() < connection_deadline => {
                    thread::sleep(Duration::from_millis(10));
                }
                Err(error) => panic!("connect: {error}"),
            }
        };
        let client = super::handshake(
            connection,
            devboule_protocol::ClientHello::m3a(
                super::test_owner("client-routing-test").expect("owner"),
                "client-routing-test",
            ),
        )
        .expect("handshake");
        let (a_tx, a_rx) = mpsc::channel();
        client
            .session_attach(
                "s.routing",
                None,
                Arc::new(move |envelope| {
                    let _ = a_tx.send(envelope);
                }),
            )
            .expect("attach A");
        let (b_tx, b_rx) = mpsc::channel();
        client
            .session_attach(
                "s.routing",
                None,
                Arc::new(move |envelope| {
                    let _ = b_tx.send(envelope);
                }),
            )
            .expect("attach B");

        let a_events = [
            a_rx.recv_timeout(Duration::from_secs(10))
                .expect("first subscription event")
                .event,
            a_rx.recv_timeout(Duration::from_secs(10))
                .expect("second subscription event")
                .event,
        ];
        let b_events = [b_rx
            .recv_timeout(Duration::from_secs(10))
            .expect("other subscription event")
            .event];
        let text = |event: &SessionEvent| match event {
            SessionEvent::AgentMessage { text, .. } => text.clone(),
            other => format!("{other:?}"),
        };
        assert_eq!(
            a_events.iter().map(text).collect::<Vec<_>>(),
            vec!["a-1", "a-2"]
        );
        assert_eq!(
            b_events.iter().map(text).collect::<Vec<_>>(),
            vec!["b-1"],
            "the second subscription must not receive the first subscription's events"
        );

        let _ = release_tx.send(());
        drop(client);
        server.join().expect("server joins");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(windows)]
    #[test]
    fn session_detach_removes_only_its_subscription() {
        let dir = std::env::temp_dir().join(format!(
            "devboule-client-detach-pending-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        let paths = crate::paths::RuntimePaths::from_dir(&dir);
        let stop = Arc::new(AtomicBool::new(false));
        let mut listener = NamedPipeListener::bind(&paths, Arc::clone(&stop)).expect("bind");
        let (attach_seen_tx, attach_seen_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let server = thread::spawn(move || {
            let file = listener.accept().expect("accept");
            let framed = Framed::new(file);
            let hello = framed.recv::<ClientMessage>().expect("client hello");
            assert!(matches!(hello, ClientMessage::Hello(_)));
            framed
                .send(&DaemonMessage::Hello(DaemonHello::plugin_backend(
                    "detach-pending-test",
                    std::process::id(),
                )))
                .expect("hello reply");

            let attach = framed.recv::<ClientMessage>().expect("attach request");
            let ClientMessage::SessionAttach {
                id: attach_id,
                subscription_id,
                ..
            } = attach
            else {
                panic!("expected attach request");
            };
            attach_seen_tx.send(()).expect("attach seen");
            framed
                .send(&DaemonMessage::SessionAttached {
                    id: attach_id,
                    subscription_id,
                })
                .expect("attach reply");
            let detach = framed.recv::<ClientMessage>().expect("detach request");
            let detach_id = detach.request_id().expect("detach id");
            framed
                .send(&DaemonMessage::Ok { id: detach_id })
                .expect("detach reply");
            release_rx
                .recv_timeout(Duration::from_secs(10))
                .expect("detach completed");
            framed
                .send(&DaemonMessage::SubscriptionEvent {
                    subscription_id,
                    envelope: devboule_protocol::SessionEventEnvelope {
                        session_id: "s.detach.pending".to_string(),
                        generation: 1,
                        event: SessionEvent::AgentMessage {
                            message_id: None,
                            text: "resurrected".to_string(),
                        },
                    },
                })
                .expect("late event");
        });

        let connection_deadline = std::time::Instant::now() + Duration::from_secs(10);
        let connection = loop {
            match crate::transport::connect(&paths) {
                Ok(connection) => break connection,
                Err(_) if std::time::Instant::now() < connection_deadline => {
                    thread::sleep(Duration::from_millis(10));
                }
                Err(error) => panic!("connect: {error}"),
            }
        };
        let client = Arc::new(
            super::handshake(
                connection,
                devboule_protocol::ClientHello::m3a(
                    super::test_owner("client-detach-pending-test").expect("owner"),
                    "client-detach-pending-test",
                ),
            )
            .expect("handshake"),
        );
        let (event_tx, event_rx) = mpsc::channel();
        let attach_client = Arc::clone(&client);
        let attach_thread = thread::spawn(move || {
            attach_client.session_attach(
                "s.detach.pending",
                None,
                Arc::new(move |envelope| {
                    let _ = event_tx.send(envelope);
                }),
            )
        });
        attach_seen_rx
            .recv_timeout(Duration::from_secs(10))
            .expect("attach reached server");
        let subscription_id = attach_thread
            .join()
            .expect("attach joins")
            .expect("attach succeeds");
        client
            .session_detach_with_subscription("s.detach.pending", subscription_id)
            .expect("detach roundtrip");
        release_tx.send(()).expect("release server");
        assert!(event_rx.recv_timeout(Duration::from_millis(100)).is_err());

        drop(client);
        server.join().expect("server joins");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
