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
}

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
    /// The last status handed to the watchers, so an unchanged poll is not a
    /// flood: one change, one push.
    published: Mutex<Option<RemoteHostStatus>>,
    idle_grace: Duration,
}

impl HostLink {
    pub(crate) fn new(device_id: String, idle_grace: Duration) -> Arc<Self> {
        Arc::new(Self {
            device_id,
            worker: Mutex::new(None),
            next_worker: AtomicU64::new(0),
            leases: Mutex::new(HashMap::new()),
            leases_changed: Condvar::new(),
            generation: AtomicU64::new(0),
            inflight: AtomicBool::new(false),
            published: Mutex::new(None),
            idle_grace,
        })
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
