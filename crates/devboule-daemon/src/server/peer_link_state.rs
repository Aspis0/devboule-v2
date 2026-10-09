//! One paired device's link, as the manager and the worker both see it: the
//! leases that hold it, the worker queue that serves it, the generation that
//! fences a stale reply, and the state it publishes to its watchers.
//!
//! This is the link's own bookkeeping, with no admission policy and no
//! sentence vocabulary in it — those live in `peer_link.rs`, which is the side
//! that decides whether a link exists at all.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::SyncSender;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use devboule_protocol::{
    DaemonMessage, RemoteHostList, RemoteHostListBody, RemoteHostState, RemoteHostStatus, WireError,
};

use crate::session::ConnHandle;

/// What one read on a link came back with.
#[derive(Debug)]
pub(crate) enum LinkAnswer {
    /// The remote daemon's own body, carried through unchanged.
    Body(RemoteHostListBody),
    /// The far side accepted an attach or a detach.
    Accepted,
    /// The remote refused; its own error code and reason, intact.
    Refused(WireError),
    /// The link could not carry the read: the state the host row should show,
    /// plus the one sentence that goes with it.
    Failed(RemoteHostState, String),
}

/// A queued read on the link's worker.
pub(crate) enum LinkCommand {
    Read {
        /// The link generation the caller queued against. A read queued before a
        /// reconnect is refused rather than answered by the new transport, so a
        /// reply can never be attributed to the link that is no longer there.
        generation: u64,
        list: RemoteHostList,
        answer: SyncSender<LinkAnswer>,
    },
    /// Open one session's live stream on the far side. The connection is the
    /// local app connection the relayed events go back to.
    Attach {
        generation: u64,
        session_id: String,
        subscription_id: u64,
        conn: Arc<ConnHandle>,
        answer: SyncSender<LinkAnswer>,
    },
    /// Close one session's live stream.
    Detach {
        generation: u64,
        session_id: String,
        subscription_id: u64,
        answer: SyncSender<LinkAnswer>,
    },
    /// The row was revoked: drop the transport now, streams included.
    Revoke,
}

/// One live remote session stream: which local connection receives its events,
/// and which remote session they belong to.
struct Subscription {
    conn: Arc<ConnHandle>,
    session_id: String,
}

/// How many relayed events one local connection may hold before the oldest is
/// dropped. Bounded so a stopped reader cannot grow this daemon without limit,
/// and large enough that a transcript burst is not visibly thinned.
pub(crate) const MAX_RELAYED_EVENTS_PER_CONNECTION: usize = 256;

/// The largest workspace revision accepted as a link's baseline. A real
/// counter advances once per project/workspace mutation, so this is far beyond
/// any daemon's lifetime; anything above it is a poisoned value, not a state.
pub(crate) const REVISION_BASELINE_MAX: u64 = 1 << 40;

/// One paired device's link, shared by every connection watching it.
pub(crate) struct HostLink {
    pub(crate) device_id: String,
    /// The worker's queue, and the token that says which worker owns it. Taken
    /// by `serve` and left behind by the worker that installed it, so a queue
    /// can never outlive the thread that served it.
    worker: Mutex<Option<(u64, SyncSender<LinkCommand>)>>,
    next_worker: AtomicU64,
    /// One entry per watching connection. The worker reads this to publish
    /// status and to know when the last lease went.
    leases: Mutex<HashMap<u64, Arc<ConnHandle>>>,
    leases_changed: Condvar,
    /// Bumped by every fresh handshake. A reply is only valid for the
    /// generation it was sent on.
    generation: AtomicU64,
    /// Whether a read is on this link right now. The link serves one at a time,
    /// so a second is refused rather than queued behind a link that may never
    /// come back.
    inflight: AtomicBool,
    /// The live remote-session streams this link carries, by local
    /// subscription id. Cleared when a new transport replaces the old one: the
    /// app reattaches on the online edge, and a generation that is gone must
    /// never relay into the one that replaced it.
    subscriptions: Mutex<HashMap<u64, Subscription>>,
    /// Streams already told about a drop, so one overflow sends one gap
    /// marker; a re-attach (a fresh subscription) clears the mark.
    gapped: Mutex<std::collections::HashSet<u64>>,
    /// Peer detaches that could not be sent the moment the app asked (the
    /// link was serving a read). The worker drains them at its next idle
    /// turn, so the host stops streaming instead of being left running by a
    /// busy moment.
    deferred_detaches: Mutex<Vec<(String, u64)>>,
    /// The last status handed to the watchers, so an unchanged poll is not a
    /// flood: one change, one push. Compared before the coalescer, so an
    /// identical state is dropped rather than parked as a trailing duplicate.
    published: Mutex<Option<RemoteHostStatus>>,
    /// One status delivery per window, latest wins with a trailing delivery:
    /// a host that flaps cannot wake every watcher at flapping rate, and the
    /// final state is still the one delivered last.
    status_coalescer: Mutex<super::remote_status::StatusCoalescer<RemoteHostStatus>>,
    /// The newest workspace revision this host pushed, if any. Reloads are
    /// the app's call, and this is the number its status push carries.
    remote_revision: Mutex<Option<u64>>,
    /// Set when this daemon's own hosting state changed and the outbound
    /// hello must be spoken again. One-shot: the worker takes it and redials.
    reconnect_requested: AtomicBool,
    idle_grace: Duration,
}

impl HostLink {
    pub(crate) fn new(
        device_id: String,
        idle_grace: Duration,
        status_window: Duration,
    ) -> Arc<Self> {
        Arc::new(Self {
            device_id,
            worker: Mutex::new(None),
            next_worker: AtomicU64::new(0),
            leases: Mutex::new(HashMap::new()),
            leases_changed: Condvar::new(),
            generation: AtomicU64::new(0),
            inflight: AtomicBool::new(false),
            subscriptions: Mutex::new(HashMap::new()),
            gapped: Mutex::new(std::collections::HashSet::new()),
            deferred_detaches: Mutex::new(Vec::new()),
            published: Mutex::new(None),
            status_coalescer: Mutex::new(super::remote_status::StatusCoalescer::new(status_window)),
            remote_revision: Mutex::new(None),
            reconnect_requested: AtomicBool::new(false),
            idle_grace,
        })
    }

    /// Record a workspace revision the host pushed over this link, if it is a
    /// plausible continuation.
    ///
    /// The first number of a link generation is the baseline; every later one
    /// must advance by exactly one, because a host bumps the counter once per
    /// project/workspace mutation and pushes each bump on the same ordered
    /// link. That refuses a replay, a jump over an unseen value, an extreme
    /// `u64` sent once to poison every later legitimate revision, and a host
    /// that restarted its counter — a reconnect clears the baseline, so the
    /// next link re-baselines instead. A refused number leaves the baseline
    /// untouched and is never re-published.
    ///
    /// Returns whether the number was accepted.
    pub(crate) fn set_remote_revision(&self, revision: u64) -> bool {
        let mut held = self
            .remote_revision
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let accepted = match *held {
            None => (1..=REVISION_BASELINE_MAX).contains(&revision),
            Some(current) => revision == current.saturating_add(1),
        };
        if accepted {
            *held = Some(revision);
        }
        accepted
    }

    /// Ask the worker to drop this transport and dial again, so the hello it
    /// sends states a presence that just changed.
    pub(crate) fn request_reconnect(&self) {
        self.reconnect_requested.store(true, Ordering::SeqCst);
    }

    /// Take the pending reconnect request, if any. One shot by design: the
    /// worker that takes it performs exactly one redial for it.
    pub(crate) fn take_reconnect_request(&self) -> bool {
        self.reconnect_requested.swap(false, Ordering::SeqCst)
    }

    /// Drop the baseline. Called when a new transport replaces the old one: a
    /// host that restarted resets its counter, and its next push is a fresh
    /// baseline rather than a value this link has already surpassed.
    pub(crate) fn clear_remote_revision(&self) {
        *self
            .remote_revision
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = None;
    }

    /// The newest workspace revision pushed by this host, if one arrived.
    pub(crate) fn remote_revision(&self) -> Option<u64> {
        *self
            .remote_revision
            .lock()
            .unwrap_or_else(|error| error.into_inner())
    }

    pub(crate) fn generation(&self) -> u64 {
        self.generation.load(Ordering::SeqCst)
    }

    /// Mark a fresh handshake: every reply queued against the previous
    /// generation is now stale, and reads queued again get the new one.
    pub(crate) fn bump_generation(&self) {
        self.generation.fetch_add(1, Ordering::SeqCst);
    }

    /// Record one lease for a connection, replacing any it already held on this
    /// host. Several windows watching one host therefore take one link.
    pub(crate) fn lease(&self, conn: Arc<ConnHandle>) {
        let mut leases = self
            .leases
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        leases.insert(conn.id, conn);
        self.leases_changed.notify_all();
    }

    /// Give back one connection's lease on this host.
    pub(crate) fn release(&self, conn_id: u64) {
        let mut leases = self
            .leases
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        leases.remove(&conn_id);
        self.leases_changed.notify_all();
    }

    pub(crate) fn lease_count(&self) -> usize {
        self.leases
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .len()
    }

    /// Whether this connection holds a lease on this host.
    pub(crate) fn leased_by(&self, conn_id: u64) -> bool {
        self.leases
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .contains_key(&conn_id)
    }

    /// Take the one read slot on this link, or `None` when it is taken.
    ///
    /// The permit lives in the caller's frame and releases on drop, so the
    /// slot comes back whether the worker answered or the caller's own
    /// deadline ran out. A slot that had to be handed back by hand would leak
    /// on the timeout and wedge the link for good.
    pub(crate) fn try_read_permit(&self) -> Option<ReadPermit<'_>> {
        if self.inflight.swap(true, Ordering::SeqCst) {
            return None;
        }
        Some(ReadPermit { link: self })
    }

    /// Block until a lease exists or the last one has been gone for the grace.
    /// Returns `false` when the link has nothing left to serve.
    pub(crate) fn wait_for_lease(&self) -> bool {
        let mut leases = self
            .leases
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if !leases.is_empty() {
            return true;
        }
        let since = Instant::now();
        while leases.is_empty() {
            let left = self
                .idle_grace
                .checked_sub(since.elapsed())
                .unwrap_or(Duration::ZERO);
            let (guard, timeout) = self
                .leases_changed
                .wait_timeout(leases, left)
                .unwrap_or_else(|error| error.into_inner());
            leases = guard;
            if !leases.is_empty() {
                return true;
            }
            if timeout.timed_out() {
                return false;
            }
        }
        true
    }

    /// Install this worker's command queue, or refuse when one is already
    /// installed. Two workers on one link would each publish status for a
    /// transport the other replaced, so exactly one is ever served.
    pub(crate) fn serve(&self, commands: SyncSender<LinkCommand>) -> Result<u64, ()> {
        let mut slot = self
            .worker
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if slot.is_some() {
            return Err(());
        }
        let token = self.next_worker.fetch_add(1, Ordering::SeqCst) + 1;
        *slot = Some((token, commands));
        Ok(token)
    }

    /// Whether a worker is still serving this link's queue.
    pub(crate) fn is_serving(&self) -> bool {
        self.worker
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .is_some()
    }

    /// Queue one read for this link's worker, against the generation the caller
    /// read. `false` means nobody is holding the link any more.
    pub(crate) fn queue_read(
        &self,
        generation: u64,
        list: RemoteHostList,
        answer: SyncSender<LinkAnswer>,
    ) -> bool {
        let slot = self
            .worker
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        match slot.as_ref() {
            Some((_, commands)) => commands
                .try_send(LinkCommand::Read {
                    generation,
                    list,
                    answer,
                })
                .is_ok(),
            None => false,
        }
    }

    /// Queue one attach for this link's worker, against the generation the
    /// caller read. `false` means nobody is holding the link any more.
    pub(crate) fn queue_attach(
        &self,
        generation: u64,
        session_id: String,
        subscription_id: u64,
        conn: Arc<ConnHandle>,
        answer: SyncSender<LinkAnswer>,
    ) -> bool {
        let slot = self
            .worker
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        match slot.as_ref() {
            Some((_, commands)) => commands
                .try_send(LinkCommand::Attach {
                    generation,
                    session_id,
                    subscription_id,
                    conn,
                    answer,
                })
                .is_ok(),
            None => false,
        }
    }

    /// Queue one detach for this link's worker.
    pub(crate) fn queue_detach(
        &self,
        generation: u64,
        session_id: String,
        subscription_id: u64,
        answer: SyncSender<LinkAnswer>,
    ) -> bool {
        let slot = self
            .worker
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        match slot.as_ref() {
            Some((_, commands)) => commands
                .try_send(LinkCommand::Detach {
                    generation,
                    session_id,
                    subscription_id,
                    answer,
                })
                .is_ok(),
            None => false,
        }
    }

    /// Queue the immediate close a revoke asks for. The worker drops the
    /// transport (and every stream on it) on its next turn; `false` means no
    /// worker is there to take it.
    pub(crate) fn queue_revoke(&self) -> bool {
        let slot = self
            .worker
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        match slot.as_ref() {
            Some((_, commands)) => commands.try_send(LinkCommand::Revoke).is_ok(),
            None => false,
        }
    }

    /// Register a live stream before its request leaves, so an event that
    /// overtakes the attach reply is already addressed to a connection.
    pub(crate) fn register_subscription(
        &self,
        subscription_id: u64,
        conn: Arc<ConnHandle>,
        session_id: String,
    ) {
        self.gapped
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .remove(&subscription_id);
        self.subscriptions
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .insert(subscription_id, Subscription { conn, session_id });
    }

    /// Drop one live stream. `false` means it was not registered (a detach
    /// after a reconnect, or a second detach).
    pub(crate) fn remove_subscription(&self, subscription_id: u64) -> bool {
        self.gapped
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .remove(&subscription_id);
        self.subscriptions
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .remove(&subscription_id)
            .is_some()
    }

    /// Relay one event to the local connection that opened the stream. A
    /// subscription that is gone (detached, or fenced by a reconnect) drops
    /// the event: the app reloads the transcript snapshot on reattach.
    pub(crate) fn forward_event(
        &self,
        subscription_id: u64,
        envelope: &devboule_protocol::SessionEventEnvelope,
    ) -> bool {
        let conn = {
            let subscriptions = self
                .subscriptions
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            match subscriptions.get(&subscription_id) {
                Some(subscription) => Arc::clone(&subscription.conn),
                None => return false,
            }
        };
        let session_id = {
            let subscriptions = self
                .subscriptions
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            match subscriptions.get(&subscription_id) {
                Some(subscription) => subscription.session_id.clone(),
                None => return false,
            }
        };
        let dropped = conn.outbound.enqueue_relayed_event(
            DaemonMessage::RemoteHostEvent {
                device_id: self.device_id.clone(),
                session_id: session_id.clone(),
                subscription_id,
                envelope: envelope.clone(),
            },
            MAX_RELAYED_EVENTS_PER_CONNECTION,
        );
        if dropped && self.mark_gapped(subscription_id) {
            conn.outbound.enqueue_relayed_event(
                DaemonMessage::RemoteHostGap {
                    device_id: self.device_id.clone(),
                    session_id,
                    subscription_id,
                },
                MAX_RELAYED_EVENTS_PER_CONNECTION,
            );
        }
        true
    }

    /// Whether this overflow is the first for the stream; `true` means the
    /// caller should send the one gap marker.
    fn mark_gapped(&self, subscription_id: u64) -> bool {
        self.gapped
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .insert(subscription_id)
    }

    /// Drop every live stream. Called when a new transport replaces the old
    /// one (and when the link retires): the generation that carried them is
    /// gone, and the app reattaches on the next online edge.
    pub(crate) fn clear_subscriptions(&self) {
        self.subscriptions
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clear();
        self.deferred_detaches
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clear();
    }

    /// Drop every live stream one local connection opened, parking a peer
    /// detach for each so the host stops streaming too. Called when that
    /// connection goes away: a dead app must not keep a remote session
    /// attached, and its bounded queue must not keep filling.
    pub(crate) fn remove_subscriptions_for(&self, conn_id: u64) {
        let dropped: Vec<(u64, String)> = {
            let mut subscriptions = self
                .subscriptions
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            let dropped: Vec<(u64, String)> = subscriptions
                .iter()
                .filter(|(_, subscription)| subscription.conn.id == conn_id)
                .map(|(id, subscription)| (*id, subscription.session_id.clone()))
                .collect();
            for (id, _) in &dropped {
                subscriptions.remove(id);
            }
            dropped
        };
        for (subscription_id, session_id) in dropped {
            self.defer_detach(session_id, subscription_id);
        }
    }

    /// Park a peer detach for the worker's next idle turn. The local
    /// subscription is already gone by the time this runs, so no event is
    /// relayed in the meantime.
    pub(crate) fn defer_detach(&self, session_id: String, subscription_id: u64) {
        self.deferred_detaches
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .push((session_id, subscription_id));
    }

    /// Take every parked detach.
    pub(crate) fn take_deferred_detaches(&self) -> Vec<(String, u64)> {
        std::mem::take(
            &mut *self
                .deferred_detaches
                .lock()
                .unwrap_or_else(|error| error.into_inner()),
        )
    }

    /// Leave the queue behind, if it is still the one this worker installed.
    pub(crate) fn retire(&self) {
        let mut slot = self
            .worker
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        *slot = None;
    }

    /// Publish one state to the watchers, once. The same state twice is
    /// dropped here rather than on the connection side, so no caller can
    /// flood the app by re-reading a host's row.
    pub(crate) fn publish(&self, status: RemoteHostStatus) {
        {
            let mut published = self
                .published
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            if published.as_ref() == Some(&status) {
                return;
            }
            *published = Some(status.clone());
        }
        let due = {
            let mut coalescer = self
                .status_coalescer
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            coalescer.offer(status, Instant::now())
        };
        if let Some(status) = due {
            self.deliver_status(status);
        }
    }

    /// Deliver the trailing status once its window has passed. Called on the
    /// worker's poll, so a parked final state is always sent even when no
    /// further change arrives to flush it.
    pub(crate) fn flush_status(&self, now: Instant) {
        let due = {
            let mut coalescer = self
                .status_coalescer
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            coalescer.tick(now)
        };
        if let Some(status) = due {
            self.deliver_status(status);
        }
    }

    fn deliver_status(&self, status: RemoteHostStatus) {
        let leases = self
            .leases
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .values()
            .cloned()
            .collect::<Vec<_>>();
        for conn in leases {
            conn.outbound.enqueue_reply(status_message(&status));
        }
    }

    /// The state this link last published, if any. The Devices panel reads it
    /// to report a peer online over an outbound link, where no inbound
    /// connection exists.
    pub(crate) fn published_state(&self) -> Option<RemoteHostState> {
        self.published
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .as_ref()
            .map(|status| status.state)
    }

    /// The failure this link last published, when that is the state it is in.
    ///
    /// A link that has failed is not serving reads however soon its worker
    /// dials again, so a read asks here first and is answered with the failure
    /// instead of queueing behind the retry. `Connecting` is not a failure: a
    /// read during the first handshake waits for it.
    pub(crate) fn failure(&self) -> Option<LinkAnswer> {
        let published = self
            .published
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let status = published.as_ref()?;
        match status.state {
            RemoteHostState::Online | RemoteHostState::Connecting => None,
            state => Some(LinkAnswer::Failed(state, status.last_failure.clone()?)),
        }
    }

    /// Hand one watcher the state the link is in now, and move nothing.
    ///
    /// A lease that joins a link already up missed every state published before
    /// it arrived, so without this it would wait for the next transition that
    /// may never come. Nothing is replayed while the link has published
    /// nothing: the worker's own first state still reaches every lease.
    pub(crate) fn replay_status(&self, conn: &Arc<ConnHandle>) {
        let current = self
            .published
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone();
        if let Some(status) = current {
            conn.outbound.enqueue_reply(status_message(&status));
        }
    }
}

/// The one shape a host's state takes on the wire, for a publish and for a
/// replay alike.
fn status_message(status: &RemoteHostStatus) -> DaemonMessage {
    DaemonMessage::RemoteHostStatus {
        device_id: status.device_id.clone(),
        state: status.state,
        last_failure: status.last_failure.clone(),
        revision: status.revision,
    }
}

/// The one read a link serves at a time, released when the caller's read call
/// returns.
pub(crate) struct ReadPermit<'a> {
    link: &'a HostLink,
}

impl Drop for ReadPermit<'_> {
    fn drop(&mut self) {
        self.link.inflight.store(false, Ordering::SeqCst);
    }
}
