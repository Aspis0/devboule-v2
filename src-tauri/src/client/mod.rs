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
    connect_or_spawn, current_user_sid, daemon_file_name, DaemonClient, DaemonError, DaemonState,
    EventHandler, ExitReason, RuntimePaths, SessionStateHandler,
};
use devboule_protocol::{
    ClientHello, Cursor, DaemonStatusBody, ErrorCode, SessionEvent, SessionEventEnvelope,
    SessionState, SessionStateSnapshot, SubscriptionId, NOTHING_OWED_CURSOR,
};
use serde::Serialize;
use tauri::State;

use crate::backend::error::CommandError;

mod crash_loop;

use crash_loop::{CrashLoopBrake, HEALTHY_CONNECTED};

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

    /// Every subscription this bridge holds for one session, lowest id first.
    ///
    /// The subscription-less close path needs one of these as a token for the
    /// client's own per-subscription bookkeeping, and forgets all of them once
    /// the daemon confirmed the session is gone.
    fn subscriptions_for_session(&self, session_id: &str) -> Vec<SubscriptionId> {
        let mut ids = self
            .state
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .entries
            .iter()
            .filter(|(_, entry)| entry.session_id == session_id)
            .map(|(subscription_id, _)| *subscription_id)
            .collect::<Vec<_>>();
        ids.sort_unstable();
        ids
    }

    /// The newest bound subscription for one session, if any. Unbound
    /// entries — left by a deferred reattach that has not run yet — cannot
    /// serve the daemon's observer check, so they are not candidates no
    /// matter their age; newest wins among the bound, as the live view.
    fn bound_subscription_for_session(&self, session_id: &str) -> Option<SubscriptionId> {
        self.state
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .entries
            .iter()
            .filter(|(_, entry)| entry.session_id == session_id && entry.binding.is_some())
            .map(|(subscription_id, _)| *subscription_id)
            .max()
    }

    /// Drop every local attachment for one session. Call only after the daemon
    /// confirmed the session is gone: forgetting a live session's attachment
    /// silently stops delivering its events to the window that owns it.
    fn forget_session(&self, session_id: &str) {
        self.state
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .entries
            .retain(|_, entry| entry.session_id != session_id);
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
        // The roster wins: it is the daemon's own word on the current
        // generation, while an entry cursor can legitimately name an older
        // one mid-replay — history is restamped and never advances cursors.
        // Both paths through here attach without a mismatch retry, so a
        // stale generation would fail the stop instead of just restarting
        // the stream. When no roster names the session, fall back to the
        // newest bound entry, deterministically.
        state
            .roster
            .as_ref()
            .and_then(|roster| roster.get(session_id))
            .map(|snapshot| snapshot.state.generation())
            .or_else(|| {
                state
                    .entries
                    .iter()
                    .filter(|(_, entry)| entry.session_id == session_id && entry.binding.is_some())
                    .map(|(subscription_id, _)| *subscription_id)
                    .max()
                    .and_then(|newest| state.entries.get(&newest))
                    .and_then(|entry| entry.cursor)
                    .map(|cursor| cursor.generation)
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
            return Some(cursor_before_replacement(cursor));
        };
        let generation = snapshot.state.generation();
        if cursor.generation == generation {
            Some(cursor_before_replacement(cursor))
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
    let Some(seq) = envelope.transcript_seq else {
        return;
    };
    match entry.cursor {
        Some(cursor) if cursor.generation == envelope.generation => {
            // transcript_seq numbers the journal envelope, not each derived row.
            // claude_client.rs:1698-1712 can publish siblings under one seq, so it
            // cannot decide row identity. Always forward. Rebind backs off one
            // envelope so strict-after replay redelivers the boundary whole;
            // shown rows may repeat, but siblings cannot be lost.
            if seq > cursor.seq.saturating_add(1) {
                // TODO: Refetch the missing transcript range before accepting this gap.
            }
            entry.cursor = Some(Cursor {
                generation: cursor.generation,
                seq: cursor.seq.max(seq),
            });
        }
        _ => {
            // Cross-generation transcript replay shares the dispatch path
            // with live traffic, so an envelope from below the stored
            // generation must not drag the cursor backwards.
            let regress = entry
                .cursor
                .is_some_and(|cursor| cursor.generation > envelope.generation);
            if !regress {
                entry.cursor = Some(Cursor {
                    generation: envelope.generation,
                    seq,
                });
            }
        }
    }
}

fn cursor_before_replacement(cursor: Cursor) -> Cursor {
    Cursor {
        generation: cursor.generation,
        seq: cursor.seq.saturating_sub(1),
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
        subscription_id: Option<SubscriptionId>,
    ) -> Result<(), DaemonError> {
        self.inner.session_close(session_id, subscription_id)
    }

    pub(crate) fn session_stop(
        &self,
        session_id: &str,
        subscription_id: Option<SubscriptionId>,
    ) -> Result<(), DaemonError> {
        self.inner.session_stop(session_id, subscription_id)
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
        subscription_id: Option<SubscriptionId>,
    ) -> Result<(), DaemonError> {
        let _lifecycle = self
            .client_lifecycle
            .lock()
            .unwrap_or_else(|err| err.into_inner());
        let Some(subscription_id) = subscription_id else {
            // No subscription: close by session id alone. The daemon needs
            // nothing else — the wire frame carries only `session_id` and the
            // owner — so a session the frontend created and never attached can
            // still be destroyed.
            //
            // This build cannot call the client's subscription-less
            // `DaemonClient::session_close`: it sits behind the daemon's
            // `server` feature, which the GUI never enables. The client's own
            // per-subscription bookkeeping therefore needs a token. Name a
            // local attachment when one exists, and 0 otherwise: bridge ids
            // start at 1, so 0 can never name someone else's attachment and the
            // client's cleanup for it is a no-op.
            let token = self
                .attachments
                .subscriptions_for_session(session_id)
                .first()
                .copied()
                .unwrap_or(0);
            let client = self
                .client
                .lock()
                .unwrap_or_else(|err| err.into_inner())
                .clone()
                .ok_or(DaemonError::ConnectionLost)?;
            return match client.session_close_with_subscription(session_id, token) {
                Ok(()) => {
                    self.attachments.forget_session(session_id);
                    Ok(())
                }
                Err(DaemonError::ConnectionLost) => {
                    // Mirror the subscription-bearing path: a lost connection
                    // drops the local attachment too.
                    self.attachments.forget_session(session_id);
                    Err(DaemonError::ConnectionLost)
                }
                Err(error) => Err(error),
            };
        };
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

    /// Stop a session's process, keeping the session. The subscription check
    /// is `session_close`'s: a caller-held id must name this session. The
    /// wire differs — `SessionStop` carries a subscription the daemon
    /// validates as an observer, while `SessionClose` carries none — so a
    /// caller without one cannot skip the check the way close does. The
    /// bridge reuses a live attachment for the session when one exists; a
    /// swiped background tab has none, so it attaches briefly at the tail on
    /// a sink that discards, stops, and detaches again. Unlike close, nothing
    /// is forgotten here: the session outlives its process, and its
    /// attachments stay valid for the exit the stop emits.
    ///
    /// Only the resolve-and-clone holds the lifecycle lock. The daemon
    /// roundtrips (30 s timeout each) run outside it: holding it across
    /// three of them would stall recovery and every other session call
    /// behind up to 90 s on a wedged daemon. A client swapped mid-sequence
    /// fails honestly below instead of hanging.
    pub(crate) fn session_stop(
        &self,
        session_id: &str,
        subscription_id: Option<SubscriptionId>,
    ) -> Result<(), DaemonError> {
        enum Plan {
            Direct(Arc<DaemonClient>, SubscriptionId),
            Temporary(Arc<DaemonClient>),
        }
        let plan = {
            let _lifecycle = self
                .client_lifecycle
                .lock()
                .unwrap_or_else(|err| err.into_inner());
            match subscription_id {
                Some(subscription_id) => {
                    let attached_session = self
                        .attachments
                        .session_id_for(subscription_id)
                        .ok_or_else(|| {
                            DaemonError::Protocol(
                                "session attachment is not registered".to_string(),
                            )
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
                    // No attachment is dropped, not even on connection loss:
                    // the session survives a stop, so a view into it must
                    // survive a failed one too — the reconnect path reattaches
                    // it.
                    Plan::Direct(client, subscription_id)
                }
                None => {
                    let client = self
                        .client
                        .lock()
                        .unwrap_or_else(|err| err.into_inner())
                        .clone()
                        .ok_or(DaemonError::ConnectionLost)?;
                    match self.attachments.bound_subscription_for_session(session_id) {
                        Some(reuse) => Plan::Direct(client, reuse),
                        None => Plan::Temporary(client),
                    }
                }
            }
        };
        match plan {
            Plan::Direct(client, subscription_id) => {
                client.session_stop_with_subscription(session_id, subscription_id)
            }
            Plan::Temporary(client) => self.stop_via_temporary_attachment(&client, session_id),
        }
    }

    /// The no-attachment stop: attach briefly at the tail, stop, detach.
    /// Lock-free by construction — the caller resolved outside the lifecycle
    /// lock — so a client replacement mid-sequence lands as an honest
    /// connection error, never a stall.
    fn stop_via_temporary_attachment(
        &self,
        client: &DaemonClient,
        session_id: &str,
    ) -> Result<(), DaemonError> {
        let sink: AttachmentSink = Arc::new(|_| {});
        // The tail, not zero: a cursorless attach replays the whole journal
        // into this sink — for a live journaled agent that is the entire
        // transcript over the socket, thrown away frame by frame. `seq:
        // u64::MAX` asks for nothing after everything: the replay driver
        // short-circuits at cursor >= watermark, the transcript filters keep
        // only higher seqs, and `cursor_replay_ok` gates on generation alone,
        // so a process that changed identity mid-window fails loudly instead
        // of being killed unseen. The cursor is never persisted — conn-scoped
        // pull state, detached below either way. Without a known generation
        // there is nothing to gate on, so the attach stays cursorless and
        // pays the full replay; that is the honest fallback (see
        // `stop_tail_cursor`), not a second clever cursor.
        let temporary = self.attachments.insert(
            session_id,
            stop_tail_cursor(self.attachments.generation_for(session_id)),
            sink,
        );
        if let Err(error) = self.attachments.bind(client, temporary) {
            self.attachments.remove(temporary);
            // The roster removes unbound entries for terminal sessions without
            // this lock, so a push that landed between the insert and the
            // bind reads as a protocol error. If the roster now shows this
            // session terminal or gone, its process is already dead — the
            // stop's postcondition holds, and success is the true answer.
            if Self::stop_already_achieved(&self.attachments, session_id) {
                return Ok(());
            }
            return Err(error);
        }
        let stop = client.session_stop_with_subscription(session_id, temporary);
        // Best effort: the stop already happened or already failed, and a
        // detach failure must not rewrite that answer — least of all into a
        // success. The local entry is always dropped; a daemon-side leftover
        // observer dies with the connection and holds a sink that discards.
        let detach = client.session_detach_with_subscription(session_id, temporary);
        self.attachments.remove(temporary);
        stop?;
        let _ = detach;
        Ok(())
    }

    /// True when stopping is pointless because the process is already dead:
    /// the roster shows the session terminal, or does not show it at all.
    /// No roster yet means no evidence — that stays an error, never a guess.
    fn stop_already_achieved(attachments: &AttachmentRegistry, session_id: &str) -> bool {
        let state = attachments
            .state
            .lock()
            .unwrap_or_else(|err| err.into_inner());
        let Some(roster) = state.roster.as_ref() else {
            return false;
        };
        match roster.get(session_id) {
            None => true,
            Some(snapshot) => terminal_event(&snapshot.state).is_some(),
        }
    }
}

/// The cursor a stop's temporary attach carries: the tail when the
/// generation is known, nothing when it is not. The seq is the daemon's
/// nothing-owed sentinel, not a guess at the tail: history is owed to
/// readers regardless of position, so only the sentinel — which the owed-row
/// predicate recognises — says "send nothing". Unknown generation stays
/// cursorless: inventing one would fail loudly at best, and the full replay
/// it pays is honest.
fn stop_tail_cursor(generation: Option<u64>) -> Option<Cursor> {
    generation.map(|generation| Cursor {
        generation,
        seq: NOTHING_OWED_CURSOR,
    })
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

/// Whether the record says the daemon was *asked* to stop, which is a decision
/// of someone else's — the app does not overrule it.
///
/// `Idle` is not that decision. It is the daemon concluding that nobody was
/// using it, and it is dated a second after the connection this process just
/// lost: when the app is here asking, the daemon was wrong about being alone,
/// and the answer is to bring it back rather than to disappear.
fn record_declares_a_requested_exit(paths: &RuntimePaths) -> bool {
    matches!(
        DaemonState::read(&paths.lock_file),
        DaemonState::Stopped(_, ExitReason::Requested)
    )
}

/// Whether the record says the daemon left at all, whatever the reason: a
/// daemon that wrote a goodbye did not crash, so the connect it refused must
/// not be charged to the brake.
///
/// An `Idle` goodbye is a yes here and a no above, because the two callers ask
/// different questions — "was that departure a crash?" and "must the app stand
/// down?". Both read the record through `DaemonState::read`, which cannot
/// disturb the daemon: it is a file, not a connection.
fn record_declares_exit(paths: &RuntimePaths) -> bool {
    matches!(
        DaemonState::read(&paths.lock_file),
        DaemonState::Stopped(..)
    )
}

/// A failed connect, and whether the daemon it failed against had said it was
/// leaving.
///
/// The two are one event at the wire — a pipe that is not there — and they do
/// not want the same answer: a daemon that recorded why it stopped is not a
/// crash-loop symptom, so it must not feed the brake. The record is read here
/// at the failed connect; the supervisor's lost-connection arm asks the
/// narrower question — a stop someone *requested* — through
/// `daemon_declared_exit`. Reading cannot disturb the daemon: it is a file,
/// not a connection.
pub(super) struct ConnectFailure {
    message: String,
    declared_exit: bool,
}

impl ConnectFailure {
    fn fault(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            declared_exit: false,
        }
    }

    /// A declared exit as the record reports it, built without a file so the
    /// supervisor's own tests can script the sequence.
    #[cfg(test)]
    fn after_a_declared_exit(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            declared_exit: true,
        }
    }

    fn after(paths: &RuntimePaths, error: DaemonError) -> Self {
        Self {
            message: error.to_string(),
            declared_exit: record_declares_exit(paths),
        }
    }
}

/// The owner id is built before any connect is attempted, so its failure is a
/// fault by construction: no daemon was reached and none can have said why it
/// left.
impl From<String> for ConnectFailure {
    fn from(message: String) -> Self {
        Self::fault(message)
    }
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

fn run_supervisor_loop<C, Connect, Declared, Connected, Sleep, Now>(
    stop: &AtomicBool,
    mut connect: Connect,
    mut declared: Declared,
    mut connected: Connected,
    mut sleep: Sleep,
    now: Now,
) -> SupervisorLoopExit
where
    Connect: FnMut() -> Result<C, ConnectFailure>,
    Declared: FnMut() -> bool,
    Connected: FnMut(C) -> StatusLoopExit,
    Sleep: FnMut(Duration, Option<&str>) -> bool,
    Now: Fn() -> Instant,
{
    let mut brake = CrashLoopBrake::default();
    loop {
        if stop.load(Ordering::SeqCst) {
            return SupervisorLoopExit::Stopped;
        }
        match connect() {
            Ok(connection) => {
                // Timed from here, not from before `connect`: healthy means the
                // *connected phase* lasted, which is what the brake documents.
                // Starting the clock before the connect would count a slow
                // spawn as service and reset the brake on a daemon that died
                // the instant it finished handshaking.
                let connected_at = now();
                let outcome = connected(connection);
                let served = now().saturating_duration_since(connected_at);
                match outcome {
                    // A loss after a genuinely healthy connection is the
                    // normal handoff back to the reconnect path: reset the
                    // brake and retry as fast as ever. A loss soon after the
                    // spawn is a crash-loop symptom: the first few stay fast,
                    // then the brake delays, ceilinged. Only a deliberate
                    // stop terminates the supervisor itself.
                    StatusLoopExit::ConnectionLost => {
                        // A daemon that was *asked* to stop did not crash, and
                        // starting it again would overrule a decision the app
                        // did not make. Stop, as a deliberate stop does. An
                        // idle exit is not that decision — see
                        // `record_declares_a_requested_exit`.
                        if declared() {
                            return SupervisorLoopExit::Stopped;
                        }
                        if served >= HEALTHY_CONNECTED {
                            brake.reset();
                            continue;
                        }
                        brake.observe_fast_failure();
                        match brake.backoff_delay() {
                            None => continue,
                            Some(delay) => {
                                if !sleep(delay, None) {
                                    return SupervisorLoopExit::Stopped;
                                }
                            }
                        }
                    }
                    StatusLoopExit::Stopped => return SupervisorLoopExit::Stopped,
                }
            }
            Err(failure) => {
                // A refused connect is a fast failure too: the spawn died
                // before serving anyone. The flat period stands until the
                // brake's tolerance is used up. A daemon that left on purpose
                // is not evidence of a crash loop, so it is not counted: the
                // brake is for daemons that keep dying young, and this one
                // did what it was asked.
                if !failure.declared_exit {
                    brake.observe_fast_failure();
                }
                let delay = brake.backoff_delay().unwrap_or(PING_PERIOD);
                if !sleep(delay, Some(failure.message.as_str())) {
                    return SupervisorLoopExit::Stopped;
                }
            }
        }
    }
}

fn retry_status_message(delay: Duration, cause: Option<&str>) -> Option<String> {
    if delay <= PING_PERIOD {
        return None;
    }
    Some(match cause {
        Some(cause) => {
            // Preserve the cause verbatim; choose the separator and case from
            // its final punctuation instead of trimming characters from it.
            let (separator, retrying) = match cause.chars().last() {
                Some('.') | Some('!') | Some('?') => (" ", "Retrying"),
                Some(':') | Some(';') | Some(',') => (" ", "retrying"),
                _ => (". ", "Retrying"),
            };
            format!("{cause}{separator}{retrying} in {}s", delay.as_secs())
        }
        None => format!(
            "the daemon keeps stopping right after starting; retrying in {}s",
            delay.as_secs()
        ),
    })
}

/// The record question for a connection that was lost: a runtime folder that
/// cannot be resolved has no goodbye to report.
fn daemon_declared_exit() -> bool {
    RuntimePaths::from_env().is_ok_and(|paths| record_declares_a_requested_exit(&paths))
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
                        set_status(&inner.status, UiDaemonStatus::error(error.message.clone()));
                    }
                    Err(error)
                }
            }
        },
        daemon_declared_exit,
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
        // The delay names the brake's verdict: at or under the flat period
        // the loop is the reconnect path it always was; above it, the daemon
        // is crash-looping and the status says so instead of silently
        // spinning between "connecting" flashes.
        |delay, cause| {
            if let Some(message) = retry_status_message(delay, cause) {
                set_status(&inner.status, UiDaemonStatus::error(message));
            }
            sleep_interruptible(&stop, delay)
        },
        Instant::now,
    );
}

fn connect_once() -> Result<DaemonClient, ConnectFailure> {
    let paths =
        RuntimePaths::from_env().map_err(|error| ConnectFailure::fault(error.to_string()))?;
    let owner = {
        let user = current_user_sid().map_err(|error| ConnectFailure::fault(error.to_string()))?;
        let client = format!("app-{}", std::process::id());
        devboule_protocol::OwnerId::new(user, client)?
    };
    let hello = ClientHello::m3a(owner, "devboule-app");
    let binary = locate_daemon_binary()?;
    connect_or_spawn(&paths, hello, Some(&binary))
        .map_err(|error| ConnectFailure::after(&paths, error))
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
mod tests;
