//! Daemon client hosted by the Tauri process. Sessions live in the daemon;
//! This process forwards RPCs and binds daemon subscriptions to Tauri
//! Channels. A failed daemon must not hang a terminal: attached
//! Channels receive `exit` with a null code.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use devboule_daemon::{
    connect_or_spawn, current_user_sid, daemon_file_name, DaemonClient, DaemonError, EventHandler,
    RuntimePaths, SessionStateHandler,
};
use devboule_protocol::{
    ClientHello, Cursor, DaemonStatusBody, ErrorCode, SessionEvent, SessionEventEnvelope,
    SessionState, SessionStateSnapshot, SubscriptionId,
};
use serde::Serialize;
use tauri::State;

use crate::backend::error::CommandError;

const PING_PERIOD: Duration = Duration::from_secs(2);
const JOIN_BUDGET: Duration = Duration::from_millis(1500);
const ROSTER_SNAPSHOT_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct UiDaemonStatus {
    pub state: String,
    pub pid: Option<u32>,
    pub instance_id: Option<String>,
    pub protocol_version: Option<u32>,
    pub clients: Option<u32>,
    pub capabilities: Vec<String>,
    pub message: Option<String>,
}

impl UiDaemonStatus {
    fn connecting() -> Self {
        Self {
            state: "connecting".to_string(),
            pid: None,
            instance_id: None,
            protocol_version: None,
            clients: None,
            capabilities: Vec::new(),
            message: None,
        }
    }

    fn disconnected(message: impl Into<String>) -> Self {
        Self {
            state: "disconnected".to_string(),
            pid: None,
            instance_id: None,
            protocol_version: None,
            clients: None,
            capabilities: Vec::new(),
            message: Some(message.into()),
        }
    }

    fn error(message: impl Into<String>) -> Self {
        Self {
            state: "error".to_string(),
            pid: None,
            instance_id: None,
            protocol_version: None,
            clients: None,
            capabilities: Vec::new(),
            message: Some(message.into()),
        }
    }
}

trait SessionWatchClient {
    fn sessions_watch(&self, handler: SessionStateHandler) -> Result<(), DaemonError>;
    fn sessions_unwatch(&self) -> Result<(), DaemonError>;
}

impl SessionWatchClient for DaemonClient {
    fn sessions_watch(&self, handler: SessionStateHandler) -> Result<(), DaemonError> {
        DaemonClient::sessions_watch(self, handler)
    }

    fn sessions_unwatch(&self) -> Result<(), DaemonError> {
        DaemonClient::sessions_unwatch(self)
    }
}

/// Desired roster subscription owned by the bridge rather than one daemon
/// connection. Rebinding it after a client swap keeps the frontend's single
/// watch alive across daemon recovery; there is no journal state to restore.
#[derive(Default)]
struct RosterSubscription {
    handler: Mutex<Option<SessionStateHandler>>,
    snapshot_epoch: Mutex<u64>,
    snapshot_ready: Condvar,
    binding_gate: Mutex<()>,
    active_binding: Mutex<Option<u64>>,
    next_binding: Mutex<u64>,
}

impl RosterSubscription {
    fn next_binding(&self) -> u64 {
        let mut next = self
            .next_binding
            .lock()
            .unwrap_or_else(|err| err.into_inner());
        *next = next.saturating_add(1);
        *next
    }

    fn begin_rebind(&self) -> Option<u64> {
        let _gate = self
            .binding_gate
            .lock()
            .unwrap_or_else(|err| err.into_inner());
        if self
            .handler
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .is_none()
        {
            return None;
        }
        *self
            .active_binding
            .lock()
            .unwrap_or_else(|err| err.into_inner()) = Some(self.next_binding());
        let mut epoch = self
            .snapshot_epoch
            .lock()
            .unwrap_or_else(|err| err.into_inner());
        *epoch = epoch.saturating_add(1);
        Some(*epoch)
    }

    fn accept_snapshot(
        &self,
        binding: u64,
        handler: &SessionStateHandler,
        snapshots: Vec<SessionStateSnapshot>,
    ) {
        let _gate = self
            .binding_gate
            .lock()
            .unwrap_or_else(|err| err.into_inner());
        if self
            .active_binding
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .as_ref()
            != Some(&binding)
        {
            return;
        }
        handler(snapshots);
        let mut epoch = self
            .snapshot_epoch
            .lock()
            .unwrap_or_else(|err| err.into_inner());
        *epoch = epoch.saturating_add(1);
        self.snapshot_ready.notify_all();
    }

    fn guarded_handler(
        self: &Arc<Self>,
        binding: u64,
        handler: SessionStateHandler,
    ) -> SessionStateHandler {
        let subscription = Arc::clone(self);
        Arc::new(move |snapshots| {
            subscription.accept_snapshot(binding, &handler, snapshots);
        })
    }

    fn wait_for_snapshot(&self, requested_epoch: u64) -> bool {
        let deadline = Instant::now() + ROSTER_SNAPSHOT_TIMEOUT;
        let mut epoch = self
            .snapshot_epoch
            .lock()
            .unwrap_or_else(|err| err.into_inner());
        while *epoch <= requested_epoch {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return false;
            }
            let (next, result) = self
                .snapshot_ready
                .wait_timeout(epoch, remaining)
                .unwrap_or_else(|err| err.into_inner());
            epoch = next;
            if result.timed_out() {
                return *epoch > requested_epoch;
            }
        }
        true
    }

    fn watch<C: SessionWatchClient>(
        self: &Arc<Self>,
        client: Option<&C>,
        handler: SessionStateHandler,
    ) -> Result<(), DaemonError> {
        let binding = {
            let _gate = self
                .binding_gate
                .lock()
                .unwrap_or_else(|err| err.into_inner());
            *self.handler.lock().unwrap_or_else(|err| err.into_inner()) =
                Some(Arc::clone(&handler));
            let binding = self.next_binding();
            *self
                .active_binding
                .lock()
                .unwrap_or_else(|err| err.into_inner()) = Some(binding);
            binding
        };
        if let Some(client) = client {
            client.sessions_watch(self.guarded_handler(binding, handler))?;
        }
        Ok(())
    }

    fn unwatch<C: SessionWatchClient>(&self, client: Option<&C>) -> Result<(), DaemonError> {
        {
            let _gate = self
                .binding_gate
                .lock()
                .unwrap_or_else(|err| err.into_inner());
            *self
                .active_binding
                .lock()
                .unwrap_or_else(|err| err.into_inner()) = None;
            self.handler
                .lock()
                .unwrap_or_else(|err| err.into_inner())
                .take();
        }
        if let Some(client) = client {
            client.sessions_unwatch()?;
        }
        Ok(())
    }

    fn rebind<C: SessionWatchClient>(self: &Arc<Self>, client: &C) -> Result<(), DaemonError> {
        let (binding, handler) = {
            let _gate = self
                .binding_gate
                .lock()
                .unwrap_or_else(|err| err.into_inner());
            let handler = self
                .handler
                .lock()
                .unwrap_or_else(|err| err.into_inner())
                .clone();
            let Some(handler) = handler else {
                return Ok(());
            };
            let binding = self.next_binding();
            *self
                .active_binding
                .lock()
                .unwrap_or_else(|err| err.into_inner()) = Some(binding);
            (binding, handler)
        };
        client.sessions_watch(self.guarded_handler(binding, handler))?;
        Ok(())
    }
}

pub(crate) type AttachmentSink = Arc<dyn Fn(SessionEvent) + Send + Sync>;

trait SessionAttachmentClient {
    fn session_attach(
        &self,
        subscription_id: SubscriptionId,
        session_id: &str,
        from_cursor: Option<Cursor>,
        handler: EventHandler,
    ) -> Result<SubscriptionId, DaemonError>;
}

impl SessionAttachmentClient for DaemonClient {
    fn session_attach(
        &self,
        subscription_id: SubscriptionId,
        session_id: &str,
        from_cursor: Option<Cursor>,
        handler: EventHandler,
    ) -> Result<SubscriptionId, DaemonError> {
        DaemonClient::session_attach_with_subscription(
            self,
            subscription_id,
            session_id,
            from_cursor,
            handler,
        )
    }
}

struct AttachmentEntry {
    session_id: String,
    sink: AttachmentSink,
    cursor: Option<Cursor>,
    binding: Option<u64>,
}

#[derive(Default)]
struct AttachmentRegistryState {
    entries: HashMap<SubscriptionId, AttachmentEntry>,
    roster: Option<HashMap<String, SessionStateSnapshot>>,
    next_subscription_id: SubscriptionId,
    next_binding: u64,
}

/// Desired per-session subscriptions owned by the bridge rather than by one
/// daemon connection. A binding token makes an old DaemonClient callback
/// harmless after replacement: the old client can still deliver its synthetic
/// connection-loss exit while its reader thread is being dropped.
#[derive(Default)]
struct AttachmentRegistry {
    state: Mutex<AttachmentRegistryState>,
}

impl AttachmentRegistry {
    fn insert(
        &self,
        session_id: &str,
        cursor: Option<Cursor>,
        sink: AttachmentSink,
    ) -> SubscriptionId {
        let mut state = self.state.lock().unwrap_or_else(|err| err.into_inner());
        state.next_subscription_id = state.next_subscription_id.saturating_add(1);
        let subscription_id = state.next_subscription_id;
        state.entries.insert(
            subscription_id,
            AttachmentEntry {
                session_id: session_id.to_string(),
                sink,
                cursor,
                binding: None,
            },
        );
        subscription_id
    }

    fn remove(&self, subscription_id: SubscriptionId) {
        self.state
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .entries
            .remove(&subscription_id);
    }

    fn session_id_for(&self, subscription_id: SubscriptionId) -> Option<String> {
        self.state
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .entries
            .get(&subscription_id)
            .map(|entry| entry.session_id.clone())
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.state
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .entries
            .len()
    }

    #[cfg(test)]
    fn is_bound(&self, session_id: &str) -> bool {
        self.state
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .entries
            .values()
            .any(|entry| entry.session_id == session_id && entry.binding.is_some())
    }

    fn generation_for(&self, session_id: &str) -> Option<u64> {
        let state = self.state.lock().unwrap_or_else(|err| err.into_inner());
        state
            .entries
            .values()
            .find(|entry| entry.session_id == session_id)
            .and_then(|entry| entry.cursor)
            .map(|cursor| cursor.generation)
            .or_else(|| {
                state
                    .roster
                    .as_ref()
                    .and_then(|roster| roster.get(session_id))
                    .map(|snapshot| snapshot.state.generation())
            })
    }

    fn forget_generation(&self, session_id: &str) {
        if let Some(roster) = self
            .state
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .roster
            .as_mut()
        {
            roster.remove(session_id);
        }
    }

    fn begin_replacement(&self) {
        let mut state = self.state.lock().unwrap_or_else(|err| err.into_inner());
        state.roster = None;
        for entry in state.entries.values_mut() {
            entry.binding = None;
        }
    }

    fn observe_roster(&self, snapshots: &[SessionStateSnapshot]) {
        let mut terminal = Vec::new();
        let mut state = self.state.lock().unwrap_or_else(|err| err.into_inner());
        state.roster = Some(
            snapshots
                .iter()
                .map(|snapshot| (snapshot.id.clone(), snapshot.clone()))
                .collect(),
        );
        for snapshot in snapshots {
            let Some(event) = terminal_event(&snapshot.state) else {
                continue;
            };
            // A normal live attachment receives its authoritative exit from
            // the session stream. During replacement all bindings are cleared,
            // so this branch is specifically the new daemon's roster answer.
            let ids = state
                .entries
                .iter()
                .filter(|(_, entry)| entry.session_id == snapshot.id && entry.binding.is_none())
                .map(|(id, _)| *id)
                .collect::<Vec<_>>();
            for id in ids {
                if let Some(entry) = state.entries.remove(&id) {
                    terminal.push((entry.sink, event.clone()));
                }
            }
        }
        drop(state);
        for (sink, event) in terminal {
            sink(event);
        }
    }

    fn bind<C: SessionAttachmentClient>(
        self: &Arc<Self>,
        client: &C,
        subscription_id: SubscriptionId,
    ) -> Result<(), DaemonError> {
        let cursor = self
            .state
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .entries
            .get(&subscription_id)
            .and_then(|entry| entry.cursor);
        self.bind_with_cursor(client, subscription_id, cursor)
    }

    fn bind_with_cursor<C: SessionAttachmentClient>(
        self: &Arc<Self>,
        client: &C,
        subscription_id: SubscriptionId,
        cursor: Option<Cursor>,
    ) -> Result<(), DaemonError> {
        let (binding, session_id, handler) = {
            let mut state = self.state.lock().unwrap_or_else(|err| err.into_inner());
            let binding = next_binding(&mut state);
            let Some(entry) = state.entries.get_mut(&subscription_id) else {
                return Err(DaemonError::Protocol(
                    "session attachment is no longer registered".to_string(),
                ));
            };
            entry.binding = Some(binding);
            let session_id = entry.session_id.clone();
            let handler = self.handler(subscription_id, session_id.clone(), binding);
            (binding, session_id, handler)
        };
        let result = client.session_attach(subscription_id, &session_id, cursor, handler);
        match result {
            Ok(confirmed) if confirmed == subscription_id => Ok(()),
            Ok(_) => {
                self.clear_binding(subscription_id, binding);
                Err(DaemonError::Protocol(
                    "daemon returned a different subscription id".to_string(),
                ))
            }
            Err(error) => {
                self.clear_binding(subscription_id, binding);
                Err(error)
            }
        }
    }

    fn handler(
        self: &Arc<Self>,
        subscription_id: SubscriptionId,
        session_id: String,
        binding: u64,
    ) -> EventHandler {
        let registry = Arc::clone(self);
        Arc::new(move |envelope| registry.dispatch(subscription_id, &session_id, binding, envelope))
    }

    fn clear_binding(&self, subscription_id: SubscriptionId, binding: u64) {
        let mut state = self.state.lock().unwrap_or_else(|err| err.into_inner());
        if state
            .entries
            .get(&subscription_id)
            .and_then(|entry| entry.binding)
            == Some(binding)
        {
            if let Some(entry) = state.entries.get_mut(&subscription_id) {
                entry.binding = None;
            }
        }
    }

    fn dispatch(
        &self,
        subscription_id: SubscriptionId,
        session_id: &str,
        binding: u64,
        envelope: SessionEventEnvelope,
    ) {
        // DaemonClient uses generation 0 only for the synthetic Exit emitted
        // when a connection fails. A real daemon Exit always belongs to the
        // session generation and must still reach the tab.
        if envelope.generation == 0 && matches!(envelope.event, SessionEvent::Exit { code: None }) {
            return;
        }
        let (sink, event) = {
            let mut state = self.state.lock().unwrap_or_else(|err| err.into_inner());
            let (sink, event, remove) = {
                let Some(entry) = state.entries.get_mut(&subscription_id) else {
                    return;
                };
                if entry.binding != Some(binding) || entry.session_id != session_id {
                    return;
                }
                advance_cursor(entry, &envelope);
                let sink = Arc::clone(&entry.sink);
                let event = envelope.event;
                let remove = matches!(
                    &event,
                    SessionEvent::Exit { .. } | SessionEvent::Recovered { .. }
                );
                (sink, event, remove)
            };
            if remove {
                state.entries.remove(&subscription_id);
            }
            (sink, event)
        };
        sink(event);
    }

    fn reattach_cursor(&self, subscription_id: SubscriptionId) -> Option<Cursor> {
        let state = self.state.lock().unwrap_or_else(|err| err.into_inner());
        let entry = state.entries.get(&subscription_id)?;
        let session_id = entry.session_id.as_str();
        let cursor = entry.cursor;
        let cursor = cursor?;
        let Some(snapshot) = state
            .roster
            .as_ref()
            .and_then(|roster| roster.get(session_id))
        else {
            return Some(cursor);
        };
        let generation = snapshot.state.generation();
        if cursor.generation == generation {
            Some(cursor)
        } else {
            // Sequence numbers restart for a new process generation. Never
            // send the old generation's seq to the daemon; a zero cursor for
            // the new generation is the only honest replay position.
            Some(Cursor { generation, seq: 0 })
        }
    }

    fn terminal_for_replacement(
        &self,
        subscription_id: SubscriptionId,
    ) -> Option<(AttachmentSink, SessionEvent)> {
        let mut state = self.state.lock().unwrap_or_else(|err| err.into_inner());
        let session_id = state.entries.get(&subscription_id)?.session_id.clone();
        let snapshot = state
            .roster
            .as_ref()
            .and_then(|roster| roster.get(&session_id))?;
        let event = terminal_event(&snapshot.state)?;
        let entry = state.entries.remove(&subscription_id)?;
        Some((entry.sink, event))
    }

    fn attach_one<C: SessionAttachmentClient>(
        self: &Arc<Self>,
        client: &C,
        subscription_id: SubscriptionId,
    ) -> Result<(), DaemonError> {
        let cursor = self.reattach_cursor(subscription_id);
        let result = self.bind_with_cursor(client, subscription_id, cursor);
        match result {
            Err(error) if is_generation_mismatch(&error) && cursor.is_some() => {
                // Without a fresh roster generation, the daemon is the
                // authority. Retry exactly once from the new stream's
                // beginning; this is still one active RPC at a time.
                self.bind_with_cursor(client, subscription_id, None)
            }
            other => other,
        }
    }

    fn retry_one<C: SessionAttachmentClient>(
        self: &Arc<Self>,
        client: &C,
        subscription_id: SubscriptionId,
    ) -> Result<(), DaemonError> {
        let should_retry = {
            let state = self.state.lock().unwrap_or_else(|err| err.into_inner());
            let Some(entry) = state.entries.get(&subscription_id) else {
                return Err(DaemonError::Protocol(
                    "session attachment is not registered".to_string(),
                ));
            };
            entry.binding.is_none()
        };
        if !should_retry {
            return Ok(());
        }
        if let Some((sink, event)) = self.terminal_for_replacement(subscription_id) {
            sink(event);
            return Ok(());
        }
        let result = self.attach_one(client, subscription_id);
        if let Err(error) = &result {
            self.emit_reattach_error(subscription_id, error);
        }
        result
    }

    fn reattach_all<C: SessionAttachmentClient>(
        self: &Arc<Self>,
        client: &C,
    ) -> Vec<(String, DaemonError)> {
        // This is intentionally a single sequential worker: the daemon gets
        // at most one re-attach RPC at a time even when many tabs were open.
        let ids = self
            .state
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .entries
            .keys()
            .cloned()
            .collect::<Vec<_>>();
        let mut failures = Vec::new();
        for subscription_id in ids {
            let session_id = self
                .session_id_for(subscription_id)
                .unwrap_or_else(|| "unknown".to_string());
            if let Err(error) = self.retry_one(client, subscription_id) {
                failures.push((session_id, error));
            }
        }
        failures
    }

    fn emit_reattach_error(&self, subscription_id: SubscriptionId, error: &DaemonError) {
        let sink = self
            .state
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .entries
            .get(&subscription_id)
            .map(|entry| Arc::clone(&entry.sink));
        if let Some(sink) = sink {
            sink(SessionEvent::AgentError {
                message: format!("Could not reattach the agent session: {error}"),
            });
        }
    }
}

fn next_binding(state: &mut AttachmentRegistryState) -> u64 {
    state.next_binding = state.next_binding.saturating_add(1);
    state.next_binding
}

fn advance_cursor(entry: &mut AttachmentEntry, envelope: &SessionEventEnvelope) {
    let seq = match &envelope.event {
        SessionEvent::Output { seq, .. }
        | SessionEvent::AgentReported { seq, .. }
        | SessionEvent::Snapshot { as_of_seq: seq, .. } => Some(*seq),
        _ => None,
    };
    match entry.cursor {
        Some(cursor) if cursor.generation == envelope.generation => {
            if let Some(seq) = seq {
                entry.cursor = Some(Cursor {
                    generation: cursor.generation,
                    seq: cursor.seq.max(seq),
                });
            }
        }
        _ => {
            entry.cursor = Some(Cursor {
                generation: envelope.generation,
                seq: seq.unwrap_or(0),
            });
        }
    }
}

fn terminal_event(state: &SessionState) -> Option<SessionEvent> {
    match state {
        SessionState::Ended { code, .. } => Some(SessionEvent::Exit { code: *code }),
        SessionState::Recovered { integrity, .. } => Some(SessionEvent::Recovered {
            integrity: *integrity,
        }),
        SessionState::Live { .. } | SessionState::Silent { .. } => None,
    }
}

fn is_generation_mismatch(error: &DaemonError) -> bool {
    matches!(
        error,
        DaemonError::Handshake(error) if error.code == ErrorCode::SessionGenerationMismatch
    )
}

pub(crate) struct BridgeInner {
    status: Mutex<UiDaemonStatus>,
    client: Mutex<Option<Arc<DaemonClient>>>,
    client_lifecycle: Mutex<()>,
    roster_subscription: Arc<RosterSubscription>,
    attachments: Arc<AttachmentRegistry>,
}

pub struct DaemonBridge {
    inner: Arc<BridgeInner>,
    stop: Arc<AtomicBool>,
    thread: Mutex<Option<JoinHandle<()>>>,
}

impl DaemonBridge {
    pub fn start() -> Self {
        let inner = Arc::new(BridgeInner {
            status: Mutex::new(UiDaemonStatus::connecting()),
            client: Mutex::new(None),
            client_lifecycle: Mutex::new(()),
            roster_subscription: Arc::new(RosterSubscription::default()),
            attachments: Arc::new(AttachmentRegistry::default()),
        });
        let stop = Arc::new(AtomicBool::new(false));
        let thread_inner = Arc::clone(&inner);
        let thread_stop = Arc::clone(&stop);
        let thread = std::thread::Builder::new()
            .name("daemon-client".into())
            .spawn(move || supervisor(thread_inner, thread_stop))
            .ok();
        Self {
            inner,
            stop,
            thread: Mutex::new(thread),
        }
    }

    pub fn snapshot(&self) -> UiDaemonStatus {
        self.inner
            .status
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .clone()
    }

    pub fn client(&self) -> Result<Arc<DaemonClient>, String> {
        self.inner.client()
    }

    pub fn sessions_watch(&self, handler: SessionStateHandler) -> Result<(), DaemonError> {
        self.inner.sessions_watch(handler)
    }

    pub fn sessions_unwatch(&self) -> Result<(), DaemonError> {
        self.inner.sessions_unwatch()
    }

    pub(crate) fn session_attach(
        &self,
        session_id: &str,
        from_seq: Option<u64>,
        sink: AttachmentSink,
    ) -> Result<SubscriptionId, DaemonError> {
        self.inner.session_attach(session_id, from_seq, sink)
    }

    pub(crate) fn ensure_subscription_attached(
        &self,
        subscription_id: SubscriptionId,
    ) -> Result<(), DaemonError> {
        self.inner.ensure_subscription_attached(subscription_id)
    }

    pub(crate) fn session_detach(
        &self,
        subscription_id: SubscriptionId,
    ) -> Result<(), DaemonError> {
        self.inner.session_detach(subscription_id)
    }

    pub(crate) fn session_claim(&self, subscription_id: SubscriptionId) -> Result<(), DaemonError> {
        self.inner.session_claim(subscription_id)
    }

    pub(crate) fn session_close(
        &self,
        session_id: &str,
        subscription_id: SubscriptionId,
    ) -> Result<(), DaemonError> {
        self.inner.session_close(session_id, subscription_id)
    }

    pub fn forget_generation(&self, session_id: &str) {
        self.inner.forget_generation(session_id);
    }

    /// Deliberate shutdown so M3c can flush the journal on the daemon side.
    pub fn shutdown(&self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(client) = self.inner.take_client() {
            let _ = client.shutdown();
        }
        let handle = self
            .thread
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .take();
        if let Some(handle) = handle {
            let deadline = Instant::now() + JOIN_BUDGET;
            while !handle.is_finished() && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(20));
            }
            if handle.is_finished() {
                let _ = handle.join();
            }
        }
    }
}

impl BridgeInner {
    fn client(&self) -> Result<Arc<DaemonClient>, String> {
        self.client
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .clone()
            .ok_or_else(|| "The daemon connection was lost.".to_string())
    }

    fn sessions_watch(self: &Arc<Self>, handler: SessionStateHandler) -> Result<(), DaemonError> {
        // Serialize desired-subscription changes with client replacement. The
        // lifecycle guard is always acquired before the client mutex, and no
        // path acquires them in the reverse order.
        let _lifecycle = self
            .client_lifecycle
            .lock()
            .unwrap_or_else(|err| err.into_inner());
        let client = self
            .client
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .clone();
        let bridge = Arc::clone(self);
        let handler = Arc::new(move |snapshots: Vec<SessionStateSnapshot>| {
            bridge.observe_roster(&snapshots);
            handler(snapshots);
        });
        self.roster_subscription.watch(client.as_deref(), handler)
    }

    fn sessions_unwatch(&self) -> Result<(), DaemonError> {
        let _lifecycle = self
            .client_lifecycle
            .lock()
            .unwrap_or_else(|err| err.into_inner());
        let client = self
            .client
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .clone();
        self.roster_subscription.unwatch(client.as_deref())
    }

    fn replace_client(&self, client: Arc<DaemonClient>) -> Result<(), DaemonError> {
        let _lifecycle = self
            .client_lifecycle
            .lock()
            .unwrap_or_else(|err| err.into_inner());
        self.attachments.begin_replacement();
        let roster_epoch = self.roster_subscription.begin_rebind();
        *self.client.lock().unwrap_or_else(|err| err.into_inner()) = Some(Arc::clone(&client));
        if let Err(error) = self.roster_subscription.rebind(client.as_ref()) {
            let mut current = self.client.lock().unwrap_or_else(|err| err.into_inner());
            if current
                .as_ref()
                .is_some_and(|active| Arc::ptr_eq(active, &client))
            {
                *current = None;
            }
            return Err(error);
        }
        if let Some(roster_epoch) = roster_epoch {
            if !self.roster_subscription.wait_for_snapshot(roster_epoch) {
                // Keep the successfully connected client. A slow journal
                // read must not create another reconnect cycle and strand all
                // tabs; attachment retries use the daemon as the generation
                // authority when the roster catches up later.
                eprintln!("daemon session roster is slow; reattaching from stored cursors");
            }
        }
        let failures = self.attachments.reattach_all(client.as_ref());
        for (session_id, error) in failures {
            eprintln!("session {session_id} reattach deferred until the next user action: {error}");
        }
        Ok(())
    }

    fn clear_client(&self, expected: &Arc<DaemonClient>) {
        let _lifecycle = self
            .client_lifecycle
            .lock()
            .unwrap_or_else(|err| err.into_inner());
        let mut current = self.client.lock().unwrap_or_else(|err| err.into_inner());
        if current
            .as_ref()
            .is_some_and(|active| Arc::ptr_eq(active, expected))
        {
            *current = None;
            self.attachments.begin_replacement();
        }
    }

    fn take_client(&self) -> Option<Arc<DaemonClient>> {
        let _lifecycle = self
            .client_lifecycle
            .lock()
            .unwrap_or_else(|err| err.into_inner());
        self.client
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .take()
    }

    fn observe_roster(&self, snapshots: &[SessionStateSnapshot]) {
        self.attachments.observe_roster(snapshots);
    }

    pub(crate) fn ensure_subscription_attached(
        &self,
        subscription_id: SubscriptionId,
    ) -> Result<(), DaemonError> {
        let _lifecycle = self
            .client_lifecycle
            .lock()
            .unwrap_or_else(|err| err.into_inner());
        if self.attachments.session_id_for(subscription_id).is_none() {
            return Err(DaemonError::Protocol(
                "session attachment is not registered".to_string(),
            ));
        }
        let client = self
            .client
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .clone()
            .ok_or(DaemonError::ConnectionLost)?;
        self.attachments.retry_one(client.as_ref(), subscription_id)
    }

    pub(crate) fn session_claim(&self, subscription_id: SubscriptionId) -> Result<(), DaemonError> {
        let _lifecycle = self
            .client_lifecycle
            .lock()
            .unwrap_or_else(|err| err.into_inner());
        let session_id = self
            .attachments
            .session_id_for(subscription_id)
            .ok_or_else(|| {
                DaemonError::Protocol("session attachment is not registered".to_string())
            })?;
        let client = self
            .client
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .clone()
            .ok_or(DaemonError::ConnectionLost)?;
        self.attachments
            .retry_one(client.as_ref(), subscription_id)?;
        client.session_claim_with_subscription(&session_id, subscription_id)
    }

    pub(crate) fn session_attach(
        &self,
        session_id: &str,
        from_seq: Option<u64>,
        sink: AttachmentSink,
    ) -> Result<SubscriptionId, DaemonError> {
        let _lifecycle = self
            .client_lifecycle
            .lock()
            .unwrap_or_else(|err| err.into_inner());
        let client = self
            .client
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .clone()
            .ok_or(DaemonError::ConnectionLost)?;
        let cursor = from_seq.map(|seq| Cursor {
            generation: self.attachments.generation_for(session_id).unwrap_or(1),
            seq,
        });
        let subscription_id = self.attachments.insert(session_id, cursor, sink);
        if let Err(error) = self.attachments.bind(client.as_ref(), subscription_id) {
            self.attachments.remove(subscription_id);
            return Err(error);
        }
        Ok(subscription_id)
    }

    pub(crate) fn session_detach(
        &self,
        subscription_id: SubscriptionId,
    ) -> Result<(), DaemonError> {
        let _lifecycle = self
            .client_lifecycle
            .lock()
            .unwrap_or_else(|err| err.into_inner());
        let session_id = self
            .attachments
            .session_id_for(subscription_id)
            .ok_or_else(|| {
                DaemonError::Protocol("session attachment is not registered".to_string())
            })?;
        let client = self
            .client
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .clone()
            .ok_or_else(|| {
                self.attachments.remove(subscription_id);
                DaemonError::ConnectionLost
            })?;
        match client.session_detach_with_subscription(&session_id, subscription_id) {
            Ok(()) => {
                self.attachments.remove(subscription_id);
                Ok(())
            }
            Err(DaemonError::ConnectionLost) => {
                self.attachments.remove(subscription_id);
                Err(DaemonError::ConnectionLost)
            }
            Err(error) => Err(error),
        }
    }

    pub(crate) fn session_close(
        &self,
        session_id: &str,
        subscription_id: SubscriptionId,
    ) -> Result<(), DaemonError> {
        let _lifecycle = self
            .client_lifecycle
            .lock()
            .unwrap_or_else(|err| err.into_inner());
        let attached_session = self
            .attachments
            .session_id_for(subscription_id)
            .ok_or_else(|| {
                DaemonError::Protocol("session attachment is not registered".to_string())
            })?;
        if attached_session != session_id {
            return Err(DaemonError::Protocol(
                "session subscription does not belong to this session".to_string(),
            ));
        }
        let client = self
            .client
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .clone()
            .ok_or(DaemonError::ConnectionLost)?;
        match client.session_close_with_subscription(session_id, subscription_id) {
            Ok(()) => {
                self.attachments.remove(subscription_id);
                Ok(())
            }
            Err(DaemonError::ConnectionLost) => {
                self.attachments.remove(subscription_id);
                Err(DaemonError::ConnectionLost)
            }
            Err(error) => Err(error),
        }
    }

    pub fn forget_generation(&self, session_id: &str) {
        self.attachments.forget_generation(session_id);
    }
}

#[tauri::command]
pub fn daemon_status(bridge: State<'_, DaemonBridge>) -> UiDaemonStatus {
    bridge.snapshot()
}

#[tauri::command]
pub fn daemon_restart(bridge: State<'_, DaemonBridge>) -> Result<(), CommandError> {
    let client = bridge
        .client()
        .map_err(|message| CommandError::new(ErrorCode::Io, message))?;
    Ok(client.restart_daemon()?)
}

const STATUS_FAILURE_THRESHOLD: u32 = 3;

#[derive(Clone, Debug, PartialEq, Eq)]
struct StatusSignal {
    state: &'static str,
    message: Option<String>,
}

#[derive(Default)]
struct StatusFailureTracker {
    consecutive_failures: u32,
    silent_since: Option<Instant>,
}

impl StatusFailureTracker {
    fn record_failure(
        &mut self,
        attempt_started: Instant,
        observed_at: Instant,
        error: &str,
    ) -> StatusSignal {
        let silent_since = *self.silent_since.get_or_insert(attempt_started);
        self.consecutive_failures = self.consecutive_failures.saturating_add(1);
        if self.consecutive_failures >= STATUS_FAILURE_THRESHOLD {
            return self.unresponsive_status(observed_at, silent_since);
        }
        StatusSignal {
            state: "error",
            message: Some(error.to_string()),
        }
    }

    fn record_success(&mut self) {
        self.consecutive_failures = 0;
        self.silent_since = None;
    }

    fn connection_status(&self, now: Instant) -> Option<StatusSignal> {
        (self.consecutive_failures >= STATUS_FAILURE_THRESHOLD)
            .then(|| self.unresponsive_status(now, self.silent_since.unwrap_or(now)))
    }

    fn unresponsive_status(&self, now: Instant, silent_since: Instant) -> StatusSignal {
        let silent_seconds = now.saturating_duration_since(silent_since).as_secs();
        StatusSignal {
            state: "unresponsive",
            message: Some(format!(
                "The daemon has not answered status checks for at least {silent_seconds} seconds ({count} consecutive failures).",
                count = self.consecutive_failures,
            )),
        }
    }
}

trait StatusSource {
    fn status(&self) -> Result<DaemonStatusBody, DaemonError>;
}

impl StatusSource for DaemonClient {
    fn status(&self) -> Result<DaemonStatusBody, DaemonError> {
        DaemonClient::status(self)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StatusLoopExit {
    ConnectionLost,
    Stopped,
}

enum StatusUpdate {
    Connected(DaemonStatusBody),
    Failure(StatusSignal),
}

fn status_error_is_connection_lost(error: &DaemonError) -> bool {
    match error {
        DaemonError::TimedOut(_) => false,
        DaemonError::Io(error) => error.kind() != std::io::ErrorKind::TimedOut,
        DaemonError::ConnectionLost => true,
        DaemonError::Protocol(_)
        | DaemonError::AlreadyRunning
        | DaemonError::UnsupportedPlatform
        | DaemonError::Handshake(_) => false,
    }
}

fn run_status_loop<S, Sleep, Publish>(
    source: &S,
    status_tracker: &mut StatusFailureTracker,
    stop: &AtomicBool,
    mut sleep: Sleep,
    mut publish: Publish,
) -> StatusLoopExit
where
    S: StatusSource,
    Sleep: FnMut() -> bool,
    Publish: FnMut(StatusUpdate),
{
    loop {
        if stop.load(Ordering::SeqCst) {
            return StatusLoopExit::Stopped;
        }
        let status_attempt_started = Instant::now();
        match source.status() {
            Ok(body) => {
                status_tracker.record_success();
                publish(StatusUpdate::Connected(body));
            }
            Err(error) => {
                let signal = status_tracker.record_failure(
                    status_attempt_started,
                    Instant::now(),
                    &error.to_string(),
                );
                publish(StatusUpdate::Failure(signal));
                if status_error_is_connection_lost(&error) {
                    return StatusLoopExit::ConnectionLost;
                }
            }
        }
        if !sleep() {
            return StatusLoopExit::Stopped;
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SupervisorLoopExit {
    Stopped,
}

fn run_supervisor_loop<C, Connect, Connected, Sleep>(
    stop: &AtomicBool,
    mut connect: Connect,
    mut connected: Connected,
    mut sleep: Sleep,
) -> SupervisorLoopExit
where
    Connect: FnMut() -> Result<C, String>,
    Connected: FnMut(C) -> StatusLoopExit,
    Sleep: FnMut() -> bool,
{
    loop {
        if stop.load(Ordering::SeqCst) {
            return SupervisorLoopExit::Stopped;
        }
        match connect() {
            Ok(connection) => match connected(connection) {
                // A lost connection is the normal handoff back to the
                // reconnect path. Only a deliberate stop terminates the
                // supervisor itself.
                StatusLoopExit::ConnectionLost => continue,
                StatusLoopExit::Stopped => return SupervisorLoopExit::Stopped,
            },
            Err(_) => {
                if !sleep() {
                    return SupervisorLoopExit::Stopped;
                }
            }
        }
    }
}

fn supervisor(inner: Arc<BridgeInner>, stop: Arc<AtomicBool>) {
    // This tracker deliberately lives outside the connection loop. A
    // successful handshake is not a successful status check, so reconnecting
    // to the same hung daemon must not erase the evidence of silence.
    let status_tracker = std::cell::RefCell::new(StatusFailureTracker::default());
    let _ = run_supervisor_loop(
        &stop,
        || {
            set_status(&inner.status, UiDaemonStatus::connecting());
            match connect_once() {
                Ok(client) => {
                    let client = Arc::new(client);
                    let hello = client.hello().clone();
                    inner
                        .replace_client(Arc::clone(&client))
                        .map_err(|error| error.to_string())?;
                    let connection_signal =
                        status_tracker.borrow().connection_status(Instant::now());
                    set_status(
                        &inner.status,
                        UiDaemonStatus {
                            state: connection_signal.as_ref().map_or_else(
                                || "connected".to_string(),
                                |signal| signal.state.into(),
                            ),
                            pid: Some(hello.pid),
                            instance_id: Some(hello.instance_id.clone()),
                            protocol_version: Some(hello.protocol_version),
                            clients: None,
                            capabilities: hello
                                .capabilities
                                .iter()
                                .map(|capability| capability.as_str().to_string())
                                .collect(),
                            message: connection_signal.and_then(|signal| signal.message),
                        },
                    );
                    Ok((client, hello))
                }
                Err(error) => {
                    if let Some(signal) = status_tracker.borrow().connection_status(Instant::now())
                    {
                        set_status(
                            &inner.status,
                            UiDaemonStatus {
                                state: signal.state.to_string(),
                                pid: None,
                                instance_id: None,
                                protocol_version: None,
                                clients: None,
                                capabilities: Vec::new(),
                                message: signal.message,
                            },
                        );
                    } else {
                        set_status(&inner.status, UiDaemonStatus::error(error.clone()));
                    }
                    Err(error)
                }
            }
        },
        |(client, hello)| match run_status_loop(
            client.as_ref(),
            &mut status_tracker.borrow_mut(),
            &stop,
            || sleep_interruptible(&stop, PING_PERIOD),
            |update| match update {
                StatusUpdate::Connected(body) => set_status(
                    &inner.status,
                    UiDaemonStatus {
                        state: "connected".to_string(),
                        pid: Some(body.pid),
                        instance_id: Some(body.instance_id),
                        protocol_version: Some(body.protocol_version),
                        clients: Some(body.clients),
                        capabilities: hello
                            .capabilities
                            .iter()
                            .map(|capability| capability.as_str().to_string())
                            .collect(),
                        message: body.journal_error,
                    },
                ),
                StatusUpdate::Failure(signal) => set_status(
                    &inner.status,
                    UiDaemonStatus {
                        state: signal.state.to_string(),
                        pid: None,
                        instance_id: None,
                        protocol_version: None,
                        clients: None,
                        capabilities: Vec::new(),
                        message: signal.message,
                    },
                ),
            },
        ) {
            StatusLoopExit::ConnectionLost => {
                inner.clear_client(&client);
                StatusLoopExit::ConnectionLost
            }
            StatusLoopExit::Stopped => {
                let _ = client.shutdown();
                inner.clear_client(&client);
                set_status(
                    &inner.status,
                    UiDaemonStatus::disconnected("daemon stopped"),
                );
                StatusLoopExit::Stopped
            }
        },
        || sleep_interruptible(&stop, PING_PERIOD),
    );
}

fn connect_once() -> Result<DaemonClient, String> {
    let paths = RuntimePaths::from_env().map_err(|error| error.to_string())?;
    let owner = {
        let user = current_user_sid().map_err(|error| error.to_string())?;
        let client = format!("app-{}", std::process::id());
        devboule_protocol::OwnerId::new(user, client)?
    };
    let hello = ClientHello::m3a(owner, "devboule-app");
    let binary = locate_daemon_binary()?;
    connect_or_spawn(&paths, hello, Some(&binary)).map_err(|error| error.to_string())
}

fn locate_daemon_binary() -> Result<PathBuf, String> {
    if let Some(path) = std::env::var_os("DEVBOULE_DAEMON") {
        return Ok(PathBuf::from(path));
    }
    let exe = std::env::current_exe().map_err(|error| error.to_string())?;
    let sibling = exe.with_file_name(daemon_file_name());
    if sibling.is_file() {
        return Ok(sibling);
    }
    let fallback = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("target")
        .join(if cfg!(debug_assertions) {
            "debug"
        } else {
            "release"
        })
        .join(daemon_file_name());
    if fallback.is_file() {
        return Ok(fallback);
    }
    eprintln!(
        "daemon binary not found next to {} or at {}",
        exe.display(),
        fallback.display()
    );
    Err(
        "Devboule daemon not found. Set DEVBOULE_DAEMON or install devboule-daemon.exe beside the app."
            .to_string(),
    )
}

fn set_status(status: &Mutex<UiDaemonStatus>, next: UiDaemonStatus) {
    *status.lock().unwrap_or_else(|err| err.into_inner()) = next;
}

fn sleep_interruptible(stop: &AtomicBool, total: Duration) -> bool {
    let deadline = Instant::now() + total;
    while Instant::now() < deadline {
        if stop.load(Ordering::SeqCst) {
            return false;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    !stop.load(Ordering::SeqCst)
}

#[cfg(test)]
mod tests {
    use super::*;
    use devboule_daemon::{DaemonError, EventHandler, SessionStateHandler};
    use devboule_protocol::DaemonStatusBody;
    use devboule_protocol::{SessionKind, SessionState, SessionStateSnapshot};
    use std::collections::HashSet;
    use std::sync::atomic::AtomicUsize;

    #[derive(Default)]
    struct FakeAttachmentClient {
        calls: Mutex<Vec<(String, Option<devboule_protocol::Cursor>)>>,
        handlers: Mutex<HashMap<String, Vec<EventHandler>>>,
        failures: Mutex<HashSet<String>>,
        in_flight: AtomicUsize,
        max_in_flight: AtomicUsize,
    }

    impl FakeAttachmentClient {
        fn emit(&self, session_id: &str, envelope: devboule_protocol::SessionEventEnvelope) {
            let handlers = self
                .handlers
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .get(session_id)
                .cloned()
                .unwrap_or_default();
            for handler in handlers {
                handler(envelope.clone());
            }
        }

        fn emit_stale(&self, session_id: &str, envelope: devboule_protocol::SessionEventEnvelope) {
            if let Some(handler) = self
                .handlers
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .get(session_id)
                .and_then(|handlers| handlers.first().cloned())
            {
                handler(envelope);
            }
        }

        fn fail_for(&self, session_id: &str) {
            self.failures
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .insert(session_id.to_string());
        }

        fn allow_for(&self, session_id: &str) {
            self.failures
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .remove(session_id);
        }
    }

    impl SessionAttachmentClient for FakeAttachmentClient {
        fn session_attach(
            &self,
            subscription_id: SubscriptionId,
            session_id: &str,
            from_cursor: Option<devboule_protocol::Cursor>,
            handler: EventHandler,
        ) -> Result<SubscriptionId, DaemonError> {
            self.calls
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .push((session_id.to_string(), from_cursor));
            let active = self.in_flight.fetch_add(1, Ordering::SeqCst) + 1;
            self.max_in_flight.fetch_max(active, Ordering::SeqCst);
            let failed = self
                .failures
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .contains(session_id);
            self.in_flight.fetch_sub(1, Ordering::SeqCst);
            if failed {
                return Err(DaemonError::Protocol("fake attach failed".to_string()));
            }
            self.handlers
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .entry(session_id.to_string())
                .or_default()
                .push(handler);
            Ok(subscription_id)
        }
    }

    #[test]
    fn attached_session_reattaches_with_the_cursor_received_before_replacement() {
        let registry = Arc::new(AttachmentRegistry::default());
        let received = Arc::new(Mutex::new(Vec::new()));
        let received_by_handler = Arc::clone(&received);
        let sink: AttachmentSink = Arc::new(move |event| {
            received_by_handler
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .push(event);
        });
        let old_client = FakeAttachmentClient::default();
        let new_client = FakeAttachmentClient::default();

        let subscription_id = registry.insert("session-1", None, sink);
        registry
            .bind(&old_client, subscription_id)
            .expect("initial attach");
        old_client.emit(
            "session-1",
            devboule_protocol::SessionEventEnvelope {
                session_id: "session-1".to_string(),
                generation: 4,
                event: devboule_protocol::SessionEvent::Output {
                    seq: 17,
                    data: "before replacement".to_string(),
                },
            },
        );

        registry.begin_replacement();
        old_client.emit_stale(
            "session-1",
            devboule_protocol::SessionEventEnvelope {
                session_id: "session-1".to_string(),
                generation: 0,
                event: devboule_protocol::SessionEvent::Exit { code: None },
            },
        );
        assert_eq!(
            received
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .len(),
            1,
            "connection-loss exit from the old client must not reach the tab"
        );
        registry.reattach_all(&new_client);

        assert_eq!(
            new_client
                .calls
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .as_slice(),
            &[(
                "session-1".to_string(),
                Some(devboule_protocol::Cursor {
                    generation: 4,
                    seq: 17,
                }),
            )]
        );
        new_client.emit(
            "session-1",
            devboule_protocol::SessionEventEnvelope {
                session_id: "session-1".to_string(),
                generation: 4,
                event: devboule_protocol::SessionEvent::Output {
                    seq: 18,
                    data: "after replacement".to_string(),
                },
            },
        );
        old_client.emit_stale(
            "session-1",
            devboule_protocol::SessionEventEnvelope {
                session_id: "session-1".to_string(),
                generation: 4,
                event: devboule_protocol::SessionEvent::Output {
                    seq: 99,
                    data: "late old-client event".to_string(),
                },
            },
        );
        assert!(received
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .iter()
            .any(|event| matches!(
                event,
                devboule_protocol::SessionEvent::Output { seq: 18, .. }
            )));
        assert!(!received
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .iter()
            .any(|event| matches!(
                event,
                devboule_protocol::SessionEvent::Output { seq: 99, .. }
            )));
    }

    #[test]
    fn attachment_registry_keeps_same_session_subscriptions_independent() {
        let registry = Arc::new(AttachmentRegistry::default());
        let first_events = Arc::new(Mutex::new(Vec::<SessionEvent>::new()));
        let second_events = Arc::new(Mutex::new(Vec::<SessionEvent>::new()));
        let first_sink_events = Arc::clone(&first_events);
        let second_sink_events = Arc::clone(&second_events);
        let first = registry.insert(
            "shared",
            None,
            Arc::new(move |event| {
                first_sink_events
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .push(event);
            }),
        );
        let second = registry.insert(
            "shared",
            None,
            Arc::new(move |event| {
                second_sink_events
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .push(event);
            }),
        );
        let client = FakeAttachmentClient::default();
        registry.bind(&client, first).expect("first attach");
        registry.bind(&client, second).expect("second attach");

        let event = devboule_protocol::SessionEventEnvelope {
            session_id: "shared".to_string(),
            generation: 1,
            event: SessionEvent::AgentMessage {
                message_id: None,
                text: "shared event".to_string(),
            },
        };
        client.emit("shared", event.clone());
        assert_eq!(first_events.lock().unwrap().len(), 1);
        assert_eq!(second_events.lock().unwrap().len(), 1);

        registry.remove(first);
        assert_eq!(registry.len(), 1);
        assert_eq!(registry.session_id_for(second).as_deref(), Some("shared"));
        client.emit("shared", event);
        assert_eq!(first_events.lock().unwrap().len(), 1);
        assert_eq!(second_events.lock().unwrap().len(), 2);
    }

    #[test]
    fn generation_bump_discards_the_old_sequence_but_keeps_the_session_binding() {
        let registry = Arc::new(AttachmentRegistry::default());
        let sink: AttachmentSink = Arc::new(|_| {});
        let old_client = FakeAttachmentClient::default();
        let new_client = FakeAttachmentClient::default();
        let subscription_id = registry.insert("session-2", None, sink);
        registry
            .bind(&old_client, subscription_id)
            .expect("initial attach");
        old_client.emit(
            "session-2",
            devboule_protocol::SessionEventEnvelope {
                session_id: "session-2".to_string(),
                generation: 4,
                event: devboule_protocol::SessionEvent::Output {
                    seq: 17,
                    data: "old generation".to_string(),
                },
            },
        );
        registry.begin_replacement();
        registry.observe_roster(&[SessionStateSnapshot {
            id: "session-2".to_string(),
            workspace_id: None,
            kind: SessionKind::Acp,
            title: "agent".to_string(),
            state: SessionState::Live { generation: 5 },
            elapsed_ms: None,
            attention: None,
        }]);
        registry.reattach_all(&new_client);

        assert_eq!(
            new_client
                .calls
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .first()
                .and_then(|(_, cursor)| *cursor),
            Some(devboule_protocol::Cursor {
                generation: 5,
                seq: 0,
            })
        );
        assert!(registry.is_bound("session-2"));
    }

    #[test]
    fn ended_while_disconnected_is_delivered_as_ended_without_an_attach() {
        let registry = Arc::new(AttachmentRegistry::default());
        let received = Arc::new(Mutex::new(Vec::new()));
        let received_by_handler = Arc::clone(&received);
        let sink: AttachmentSink = Arc::new(move |event| {
            received_by_handler
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .push(event);
        });
        let old_client = FakeAttachmentClient::default();
        let new_client = FakeAttachmentClient::default();
        let subscription_id = registry.insert("ended", None, sink);
        registry
            .bind(&old_client, subscription_id)
            .expect("initial attach");
        registry.begin_replacement();
        registry.observe_roster(&[SessionStateSnapshot {
            id: "ended".to_string(),
            workspace_id: None,
            kind: SessionKind::Terminal,
            title: "ended".to_string(),
            state: SessionState::Ended {
                generation: 4,
                code: Some(23),
                integrity: devboule_protocol::TranscriptIntegrity::Complete,
            },
            elapsed_ms: None,
            attention: None,
        }]);
        registry.reattach_all(&new_client);

        assert!(new_client
            .calls
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .is_empty());
        assert!(received
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .iter()
            .any(|event| matches!(
                event,
                devboule_protocol::SessionEvent::Exit { code: Some(23) }
            )));
        assert_eq!(registry.len(), 0);
    }

    #[test]
    fn a_failed_reattach_does_not_abort_the_remaining_tabs() {
        let registry = Arc::new(AttachmentRegistry::default());
        let received = Arc::new(Mutex::new(Vec::new()));
        let received_by_handler = Arc::clone(&received);
        let sink: AttachmentSink = Arc::new(move |event| {
            received_by_handler
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .push(event);
        });
        let second_sink = Arc::clone(&sink);
        let client = FakeAttachmentClient::default();
        client.fail_for("bad");
        registry.insert("bad", None, Arc::clone(&sink));
        registry.insert("good", None, second_sink);
        registry.begin_replacement();
        let failures = registry.reattach_all(&client);

        assert_eq!(failures.len(), 1);
        assert!(!registry.is_bound("bad"));
        assert!(registry.is_bound("good"));
        client.emit(
            "good",
            devboule_protocol::SessionEventEnvelope {
                session_id: "good".to_string(),
                generation: 1,
                event: devboule_protocol::SessionEvent::AgentMessage {
                    message_id: None,
                    text: "still live".to_string(),
                },
            },
        );
        assert!(received
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .iter()
            .any(|event| matches!(event, devboule_protocol::SessionEvent::AgentMessage { text, .. } if text == "still live")));
    }

    #[test]
    fn a_failed_reattach_can_be_retried_for_a_later_user_action() {
        let registry = Arc::new(AttachmentRegistry::default());
        let received = Arc::new(Mutex::new(Vec::new()));
        let received_by_handler = Arc::clone(&received);
        let sink: AttachmentSink = Arc::new(move |event| {
            received_by_handler
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .push(event);
        });
        let client = FakeAttachmentClient::default();
        client.fail_for("retry");
        let subscription_id = registry.insert("retry", None, sink);
        registry.begin_replacement();
        assert_eq!(registry.reattach_all(&client).len(), 1);
        assert!(!registry.is_bound("retry"));

        client.allow_for("retry");
        registry
            .retry_one(&client, subscription_id)
            .expect("the later action retries the attachment");
        assert!(registry.is_bound("retry"));
        client.emit(
            "retry",
            devboule_protocol::SessionEventEnvelope {
                session_id: "retry".to_string(),
                generation: 1,
                event: devboule_protocol::SessionEvent::AgentMessage {
                    message_id: None,
                    text: "recovered".to_string(),
                },
            },
        );
        assert!(received
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .iter()
            .any(|event| matches!(event, devboule_protocol::SessionEvent::AgentMessage { text, .. } if text == "recovered")));
    }

    #[test]
    fn reattach_worker_allows_only_one_attach_in_flight() {
        let registry = Arc::new(AttachmentRegistry::default());
        let client = FakeAttachmentClient::default();
        for index in 0..8 {
            registry.insert(&format!("session-{index}"), None, Arc::new(|_| {}));
        }
        registry.begin_replacement();
        registry.reattach_all(&client);

        assert_eq!(client.max_in_flight.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn closed_session_events_remove_the_registry_entry() {
        let registry = Arc::new(AttachmentRegistry::default());
        let client = FakeAttachmentClient::default();
        let subscription_id = registry.insert("closed", None, Arc::new(|_| {}));
        registry.bind(&client, subscription_id).expect("attach");
        client.emit(
            "closed",
            devboule_protocol::SessionEventEnvelope {
                session_id: "closed".to_string(),
                generation: 1,
                event: devboule_protocol::SessionEvent::Exit { code: Some(0) },
            },
        );

        assert_eq!(registry.len(), 0);
    }

    #[test]
    fn removing_one_session_subscription_keeps_the_other() {
        let registry = Arc::new(AttachmentRegistry::default());
        let first = registry.insert("shared", None, Arc::new(|_| {}));
        let second = registry.insert("shared", None, Arc::new(|_| {}));

        registry.remove(first);

        assert_eq!(registry.len(), 1);
        assert_eq!(registry.session_id_for(second).as_deref(), Some("shared"));
        assert_eq!(registry.session_id_for(first), None);
    }

    #[test]
    fn retrying_an_unknown_subscription_is_rejected() {
        let registry = Arc::new(AttachmentRegistry::default());
        let client = FakeAttachmentClient::default();

        let error = registry
            .retry_one(&client, 41)
            .expect_err("unknown subscriptions must not pass command validation");
        match error {
            DaemonError::Protocol(message) => {
                assert_eq!(message, "session attachment is not registered")
            }
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[derive(Default)]
    struct FakeRosterClient {
        handler: Mutex<Option<SessionStateHandler>>,
    }

    impl FakeRosterClient {
        fn emit(&self, snapshot: SessionStateSnapshot) {
            if let Some(handler) = self
                .handler
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .as_ref()
            {
                handler(vec![snapshot]);
            }
        }
    }

    impl SessionWatchClient for FakeRosterClient {
        fn sessions_watch(&self, handler: SessionStateHandler) -> Result<(), DaemonError> {
            *self
                .handler
                .lock()
                .unwrap_or_else(|error| error.into_inner()) = Some(handler);
            Ok(())
        }

        fn sessions_unwatch(&self) -> Result<(), DaemonError> {
            self.handler
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .take();
            Ok(())
        }
    }

    fn roster_snapshot(id: &str) -> SessionStateSnapshot {
        SessionStateSnapshot {
            id: id.to_string(),
            workspace_id: None,
            kind: SessionKind::Terminal,
            title: "new daemon".to_string(),
            state: SessionState::Live { generation: 1 },
            elapsed_ms: Some(1),
            attention: None,
        }
    }

    #[test]
    fn roster_update_reaches_a_subscription_after_client_replacement() {
        let subscription = Arc::new(RosterSubscription::default());
        let received = Arc::new(Mutex::new(Vec::new()));
        let received_by_handler = Arc::clone(&received);
        let handler: SessionStateHandler = Arc::new(move |snapshots| {
            received_by_handler
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .extend(snapshots);
        });
        let old_client = FakeRosterClient::default();
        let new_client = FakeRosterClient::default();

        subscription
            .watch(Some(&old_client), handler)
            .expect("watch old client");
        subscription
            .rebind(&new_client)
            .expect("rebind replacement client");
        new_client.emit(roster_snapshot("new-daemon-session"));

        assert_eq!(
            received
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .iter()
                .map(|snapshot| snapshot.id.as_str())
                .collect::<Vec<_>>(),
            vec!["new-daemon-session"]
        );
    }

    #[test]
    fn roster_subscription_registered_while_disconnected_binds_later() {
        let subscription = Arc::new(RosterSubscription::default());
        let received = Arc::new(Mutex::new(Vec::new()));
        let received_by_handler = Arc::clone(&received);
        let handler: SessionStateHandler = Arc::new(move |snapshots| {
            received_by_handler
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .extend(snapshots);
        });
        let new_client = FakeRosterClient::default();

        subscription
            .watch::<FakeRosterClient>(None, handler)
            .expect("save watch");
        subscription.rebind(&new_client).expect("bind new client");
        new_client.emit(roster_snapshot("connected-later"));

        assert_eq!(
            received
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .iter()
                .map(|snapshot| snapshot.id.as_str())
                .collect::<Vec<_>>(),
            vec!["connected-later"]
        );
    }

    #[test]
    fn roster_unwatch_stays_unsubscribed_across_client_replacement() {
        let subscription = Arc::new(RosterSubscription::default());
        let received = Arc::new(Mutex::new(Vec::new()));
        let received_by_handler = Arc::clone(&received);
        let handler: SessionStateHandler = Arc::new(move |snapshots| {
            received_by_handler
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .extend(snapshots);
        });
        let old_client = FakeRosterClient::default();
        let new_client = FakeRosterClient::default();

        subscription
            .watch(Some(&old_client), handler)
            .expect("watch old client");
        subscription
            .unwatch(Some(&old_client))
            .expect("unwatch old client");
        subscription
            .rebind(&new_client)
            .expect("rebind replacement client");
        new_client.emit(roster_snapshot("must-not-arrive"));

        assert!(received
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .is_empty());
    }

    #[test]
    fn an_old_roster_client_cannot_satisfy_the_new_client_snapshot_barrier() {
        let subscription = Arc::new(RosterSubscription::default());
        let received = Arc::new(Mutex::new(Vec::new()));
        let received_by_handler = Arc::clone(&received);
        let handler: SessionStateHandler = Arc::new(move |snapshots| {
            received_by_handler
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .extend(snapshots);
        });
        let old_client = FakeRosterClient::default();
        let new_client = FakeRosterClient::default();
        subscription
            .watch(Some(&old_client), handler)
            .expect("watch old client");
        let requested_epoch = subscription
            .begin_rebind()
            .expect("a desired roster watch exists");
        subscription.rebind(&new_client).expect("rebind new client");

        old_client.emit(roster_snapshot("old-daemon"));
        assert!(received
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .is_empty());
        assert_eq!(
            *subscription
                .snapshot_epoch
                .lock()
                .unwrap_or_else(|error| error.into_inner()),
            requested_epoch,
            "the old callback must not advance the snapshot barrier"
        );

        new_client.emit(roster_snapshot("new-daemon"));
        assert!(subscription.wait_for_snapshot(requested_epoch));
        assert_eq!(
            received
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .iter()
                .map(|snapshot| snapshot.id.as_str())
                .collect::<Vec<_>>(),
            vec!["new-daemon"]
        );
    }

    struct TimeoutStatusSource {
        calls: AtomicUsize,
    }

    impl StatusSource for TimeoutStatusSource {
        fn status(&self) -> Result<DaemonStatusBody, devboule_daemon::DaemonError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Err(devboule_daemon::DaemonError::timed_out("fake status"))
        }
    }

    struct LostStatusSource {
        calls: AtomicUsize,
    }

    impl StatusSource for LostStatusSource {
        fn status(&self) -> Result<DaemonStatusBody, devboule_daemon::DaemonError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Err(devboule_daemon::DaemonError::ConnectionLost)
        }
    }

    #[test]
    fn supervisor_reconnects_after_a_connected_loop_reports_connection_loss() {
        let stop = AtomicBool::new(false);
        let mut connect_attempts = 0;
        let mut connected_runs = 0;
        let outcome = run_supervisor_loop(
            &stop,
            || {
                connect_attempts += 1;
                Ok(connect_attempts)
            },
            |_| {
                connected_runs += 1;
                if connected_runs == 1 {
                    StatusLoopExit::ConnectionLost
                } else {
                    StatusLoopExit::Stopped
                }
            },
            || true,
        );

        assert_eq!(outcome, SupervisorLoopExit::Stopped);
        assert_eq!(connect_attempts, 2);
        assert_eq!(connected_runs, 2);
    }

    #[test]
    fn connected_timeout_source_reaches_unresponsive_without_reconnect() {
        let source = TimeoutStatusSource {
            calls: AtomicUsize::new(0),
        };
        let stop = AtomicBool::new(false);
        let mut tracker = StatusFailureTracker::default();
        let mut updates = Vec::new();
        let mut sleeps = 0;
        let outcome = run_status_loop(
            &source,
            &mut tracker,
            &stop,
            || {
                sleeps += 1;
                sleeps < 3
            },
            |update| updates.push(update),
        );

        assert_eq!(outcome, StatusLoopExit::Stopped);
        assert_eq!(source.calls.load(Ordering::SeqCst), 3);
        assert!(updates.iter().any(|update| matches!(
            update,
            StatusUpdate::Failure(signal) if signal.state == "unresponsive"
        )));
    }

    #[test]
    fn typed_connection_loss_exits_the_status_loop_for_reconnect() {
        let source = LostStatusSource {
            calls: AtomicUsize::new(0),
        };
        let stop = AtomicBool::new(false);
        let mut tracker = StatusFailureTracker::default();
        let mut updates = Vec::new();
        let outcome = run_status_loop(
            &source,
            &mut tracker,
            &stop,
            || true,
            |update| updates.push(update),
        );

        assert_eq!(outcome, StatusLoopExit::ConnectionLost);
        assert_eq!(source.calls.load(Ordering::SeqCst), 1);
        assert!(matches!(updates.as_slice(), [StatusUpdate::Failure(_)]));
    }

    #[test]
    fn two_status_failures_do_not_raise_unresponsive_but_three_do() {
        let first_attempt = Instant::now();
        let mut tracker = StatusFailureTracker::default();
        let first = tracker.record_failure(
            first_attempt,
            first_attempt + Duration::from_secs(30),
            "timed out",
        );
        assert_eq!(first.state, "error");
        let second = tracker.record_failure(
            first_attempt + Duration::from_secs(30),
            first_attempt + Duration::from_secs(60),
            "timed out",
        );
        assert_eq!(second.state, "error");
        let third = tracker.record_failure(
            first_attempt + Duration::from_secs(60),
            first_attempt + Duration::from_secs(90),
            "timed out",
        );
        assert_eq!(third.state, "unresponsive");
        assert!(third
            .message
            .as_deref()
            .is_some_and(|message| message.contains("90 seconds")));
    }

    #[test]
    fn a_success_resets_the_failure_count() {
        let first_attempt = Instant::now();
        let mut tracker = StatusFailureTracker::default();
        tracker.record_failure(
            first_attempt,
            first_attempt + Duration::from_secs(30),
            "timed out",
        );
        tracker.record_success();
        let first_after_success = tracker.record_failure(
            first_attempt + Duration::from_secs(60),
            first_attempt + Duration::from_secs(90),
            "timed out",
        );
        let second_after_success = tracker.record_failure(
            first_attempt + Duration::from_secs(90),
            first_attempt + Duration::from_secs(120),
            "timed out",
        );
        assert_eq!(first_after_success.state, "error");
        assert_eq!(second_after_success.state, "error");
    }

    #[test]
    fn reconnect_does_not_reset_failures_when_status_keeps_failing() {
        let first_attempt = Instant::now();
        let mut tracker = StatusFailureTracker::default();
        tracker.record_failure(
            first_attempt,
            first_attempt + Duration::from_secs(30),
            "timed out",
        );
        assert!(tracker
            .connection_status(first_attempt + Duration::from_secs(30))
            .is_none());
        tracker.record_failure(
            first_attempt + Duration::from_secs(30),
            first_attempt + Duration::from_secs(60),
            "timed out",
        );
        let status = tracker.record_failure(
            first_attempt + Duration::from_secs(60),
            first_attempt + Duration::from_secs(90),
            "timed out",
        );
        assert_eq!(status.state, "unresponsive");
    }

    #[test]
    fn daemon_restart_has_the_frozen_tauri_signature() {
        let _: fn(State<'_, DaemonBridge>) -> Result<(), crate::backend::error::CommandError> =
            daemon_restart;
    }
}
