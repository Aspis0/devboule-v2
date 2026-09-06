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
    Persistence, ProviderInfo, ResumeResult, RetentionPatch, Session, SessionEvent,
    SessionEventEnvelope, SessionKind, SessionStateSnapshot, WireError,
};

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
    session_id: String,
    handler: EventHandler,
}

struct ClientInner {
    framed: Framed,
    next_id: AtomicU64,
    pending: Mutex<HashMap<u64, mpsc::Sender<DaemonMessage>>>,
    pending_subscriptions: Mutex<HashMap<u64, PendingSubscription>>,
    subscriptions: Mutex<HashMap<String, EventHandler>>,
    session_state_subscription: Mutex<Option<SessionStateHandler>>,
    stop: AtomicBool,
    hello: DaemonHello,
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
    ) -> Result<(), DaemonError> {
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
                    session_id: session_id.to_string(),
                    handler,
                },
            );
        }
        let result = self.roundtrip(ClientMessage::SessionAttach {
            id,
            session_id: session_id.to_string(),
            from_cursor,
        });
        match result {
            Ok(DaemonMessage::Ok { .. }) => Ok(()),
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
        let id = self.alloc_id();
        let result = self.roundtrip(ClientMessage::SessionDetach {
            id,
            session_id: session_id.to_string(),
        });
        self.unsubscribe(session_id);
        self.remove_pending_subscriptions(session_id);
        match result? {
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
        self.unsubscribe(session_id);
        match result? {
            DaemonMessage::Ok { .. } => Ok(()),
            DaemonMessage::Error(error) => Err(DaemonError::Handshake(error)),
            other => unexpected(other),
        }
    }

    pub fn session_stop(&self, session_id: &str) -> Result<(), DaemonError> {
        let id = self.alloc_id();
        match self.roundtrip(ClientMessage::SessionStop {
            id,
            session_id: session_id.to_string(),
        })? {
            DaemonMessage::Ok { .. } => Ok(()),
            DaemonMessage::Error(error) => Err(DaemonError::Handshake(error)),
            other => unexpected(other),
        }
    }

    pub fn session_interrupt(&self, session_id: &str) -> Result<(), DaemonError> {
        let id = self.alloc_id();
        match self.roundtrip(ClientMessage::SessionInterrupt {
            id,
            session_id: session_id.to_string(),
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
        let id = self.alloc_id();
        match self.roundtrip(ClientMessage::SessionSend {
            id,
            session_id: session_id.to_string(),
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
        let id = self.alloc_id();
        match self.roundtrip(ClientMessage::SessionResize {
            id,
            session_id: session_id.to_string(),
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
        let id = self.alloc_id();
        match self.roundtrip(ClientMessage::SessionPermissionRespond {
            id,
            session_id: session_id.to_string(),
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
            Err(RecvTimeoutError::Disconnected) => Err(DaemonError::Protocol(
                "daemon connection was lost".to_string(),
            )),
        }
    }

    /// Write a frame without reading a reply. A client that stops reading
    /// uses this so we can prove other connections still make progress.
    pub fn write_frame(&self, message: &ClientMessage) -> Result<(), DaemonError> {
        if self.inner.stop.load(Ordering::SeqCst) {
            return Err(DaemonError::Protocol(
                "daemon connection was lost".to_string(),
            ));
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

    fn unsubscribe(&self, session_id: &str) {
        self.inner
            .subscriptions
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .remove(session_id);
    }

    fn remove_pending_subscription(&self, request_id: u64) {
        self.inner
            .pending_subscriptions
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .remove(&request_id);
    }

    fn remove_pending_subscriptions(&self, session_id: &str) {
        self.inner
            .pending_subscriptions
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .retain(|_, pending| pending.session_id != session_id);
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
    let mut spawned = false;
    for attempt in 0..SPAWN_ATTEMPTS {
        match connect(paths, hello.clone()) {
            Ok(client) => return Ok(client),
            Err(error) => {
                if attempt + 1 == SPAWN_ATTEMPTS {
                    return Err(error);
                }
            }
        }
        if !spawned {
            match spawn_daemon(&binary, paths) {
                Ok(child) => {
                    drop(child);
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
    let framed = Framed::new(file);
    framed.send(&ClientMessage::Hello(hello))?;
    let reply: DaemonMessage = framed.recv_timeout(HANDSHAKE_TIMEOUT)?;
    match reply {
        DaemonMessage::Hello(daemon_hello) => {
            let inner = Arc::new(ClientInner {
                framed,
                next_id: AtomicU64::new(1),
                pending: Mutex::new(HashMap::new()),
                pending_subscriptions: Mutex::new(HashMap::new()),
                subscriptions: Mutex::new(HashMap::new()),
                session_state_subscription: Mutex::new(None),
                stop: AtomicBool::new(false),
                hello: daemon_hello,
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
            fail_connection(&inner, "daemon connection was closed");
            return;
        }
        match inner
            .framed
            .recv_timeout::<DaemonMessage>(Duration::from_millis(100))
        {
            Ok(DaemonMessage::Event(envelope)) => match envelope.event {
                SessionEvent::SessionsSnapshot { sessions } => {
                    let handler = inner
                        .session_state_subscription
                        .lock()
                        .unwrap_or_else(|err| err.into_inner())
                        .clone();
                    if let Some(handler) = handler {
                        handler(sessions);
                    }
                }
                event => {
                    let handler = inner
                        .subscriptions
                        .lock()
                        .unwrap_or_else(|err| err.into_inner())
                        .get(&envelope.session_id)
                        .cloned();
                    if let Some(handler) = handler {
                        handler(SessionEventEnvelope {
                            session_id: envelope.session_id,
                            generation: envelope.generation,
                            event,
                        });
                    }
                }
            },
            Ok(message) => {
                if let Some(id) = daemon_message_id(&message) {
                    if matches!(&message, DaemonMessage::Ok { .. }) {
                        // The pipe is FIFO for replies and events. Keep a new
                        // handler pending until its Ok has been consumed, so
                        // frames before that boundary remain with the prior
                        // attachment (or are dropped if it is gone).
                        let pending_subscription = inner
                            .pending_subscriptions
                            .lock()
                            .unwrap_or_else(|err| err.into_inner())
                            .remove(&id);
                        if let Some(PendingSubscription {
                            session_id,
                            handler,
                        }) = pending_subscription
                        {
                            inner
                                .subscriptions
                                .lock()
                                .unwrap_or_else(|err| err.into_inner())
                                .insert(session_id, handler);
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
                fail_connection(&inner, "daemon connection was lost");
                return;
            }
        }
    }
}

fn fail_connection(inner: &ClientInner, message: &str) {
    inner.stop.store(true, Ordering::SeqCst);
    inner
        .session_state_subscription
        .lock()
        .unwrap_or_else(|err| err.into_inner())
        .take();
    let mut subscriptions: Vec<(String, EventHandler)> = inner
        .subscriptions
        .lock()
        .unwrap_or_else(|err| err.into_inner())
        .drain()
        .collect();
    subscriptions.extend(
        inner
            .pending_subscriptions
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .drain()
            .map(|(_, pending)| (pending.session_id, pending.handler)),
    );
    for (session_id, handler) in subscriptions {
        handler(SessionEventEnvelope {
            session_id,
            generation: 0,
            event: SessionEvent::Exit { code: None },
        });
    }
    let pending: Vec<mpsc::Sender<DaemonMessage>> = inner
        .pending
        .lock()
        .unwrap_or_else(|err| err.into_inner())
        .drain()
        .map(|(_, tx)| tx)
        .collect();
    let error = DaemonMessage::Error(WireError::new(ErrorCode::Io, message));
    for tx in pending {
        let _ = tx.send(error.clone());
    }
}

fn daemon_message_id(message: &DaemonMessage) -> Option<u64> {
    match message {
        DaemonMessage::Hello(_) | DaemonMessage::Event(_) => None,
        DaemonMessage::Error(error) => error.id,
        DaemonMessage::Pong { id, .. }
        | DaemonMessage::Status { id, .. }
        | DaemonMessage::Shutdown { id, .. }
        | DaemonMessage::Session { id, .. }
        | DaemonMessage::Sessions { id, .. }
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

    #[cfg(windows)]
    #[test]
    fn attach_reply_promotes_the_pending_handler_at_the_fifo_boundary() {
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
            let first_id = first.request_id().expect("first attach id");
            framed
                .send(&DaemonMessage::Ok { id: first_id })
                .expect("first attach reply");
            framed
                .send(&DaemonMessage::Event(
                    devboule_protocol::SessionEventEnvelope {
                        session_id: "s.routing".to_string(),
                        generation: 1,
                        event: SessionEvent::AgentMessage {
                            message_id: None,
                            text: "a-1".to_string(),
                        },
                    },
                ))
                .expect("first A event");

            let second = framed
                .recv::<ClientMessage>()
                .expect("second attach request");
            let second_id = second.request_id().expect("second attach id");
            framed
                .send(&DaemonMessage::Event(
                    devboule_protocol::SessionEventEnvelope {
                        session_id: "s.routing".to_string(),
                        generation: 1,
                        event: SessionEvent::AgentMessage {
                            message_id: None,
                            text: "a-2".to_string(),
                        },
                    },
                ))
                .expect("remaining A event");
            framed
                .send(&DaemonMessage::Ok { id: second_id })
                .expect("second attach reply");
            framed
                .send(&DaemonMessage::Event(
                    devboule_protocol::SessionEventEnvelope {
                        session_id: "s.routing".to_string(),
                        generation: 1,
                        event: SessionEvent::AgentMessage {
                            message_id: None,
                            text: "b-1".to_string(),
                        },
                    },
                ))
                .expect("B event");
            let _ = release_rx.recv_timeout(Duration::from_secs(2));
        });

        let connection = (0..100)
            .find_map(|_| match crate::transport::connect(&paths) {
                Ok(connection) => Some(connection),
                Err(_) => {
                    thread::yield_now();
                    None
                }
            })
            .expect("connect");
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

        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        let mut a_events = Vec::new();
        let mut b_events = Vec::new();
        while std::time::Instant::now() < deadline && a_events.len() + b_events.len() < 3 {
            a_events.extend(a_rx.try_iter().map(|envelope| envelope.event));
            b_events.extend(b_rx.try_iter().map(|envelope| envelope.event));
            thread::sleep(Duration::from_millis(5));
        }
        a_events.extend(a_rx.try_iter().map(|envelope| envelope.event));
        b_events.extend(b_rx.try_iter().map(|envelope| envelope.event));
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
            "the promoted handler must not receive A's FIFO-prefix events"
        );

        let _ = release_tx.send(());
        drop(client);
        server.join().expect("server joins");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(windows)]
    #[test]
    fn session_detach_drops_a_pending_attach_handler() {
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
            let attach_id = attach.request_id().expect("attach id");
            attach_seen_tx.send(()).expect("attach seen");
            let detach = framed.recv::<ClientMessage>().expect("detach request");
            let detach_id = detach.request_id().expect("detach id");
            framed
                .send(&DaemonMessage::Ok { id: detach_id })
                .expect("detach reply");
            release_rx.recv().expect("release late attach");
            framed
                .send(&DaemonMessage::Ok { id: attach_id })
                .expect("late attach reply");
            framed
                .send(&DaemonMessage::Event(
                    devboule_protocol::SessionEventEnvelope {
                        session_id: "s.detach.pending".to_string(),
                        generation: 1,
                        event: SessionEvent::AgentMessage {
                            message_id: None,
                            text: "resurrected".to_string(),
                        },
                    },
                ))
                .expect("late event");
        });

        let connection = (0..100)
            .find_map(|_| match crate::transport::connect(&paths) {
                Ok(connection) => Some(connection),
                Err(_) => {
                    thread::yield_now();
                    None
                }
            })
            .expect("connect");
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
            .recv_timeout(Duration::from_secs(2))
            .expect("attach reached server");
        client
            .session_detach("s.detach.pending")
            .expect("detach roundtrip");
        release_tx.send(()).expect("release server");
        assert!(attach_thread.join().expect("attach joins").is_ok());
        assert!(event_rx.recv_timeout(Duration::from_millis(100)).is_err());

        drop(client);
        server.join().expect("server joins");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
