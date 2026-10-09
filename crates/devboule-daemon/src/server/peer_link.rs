//! Held links to paired daemon peers: admission, the leases that hold them,
//! the budgets they share, and the vocabulary of states the app is told about.
//!
//! A watch past the link cap is refused `busy` here, before anything is
//! opened. Nothing in this file opens a socket or speaks a frame: the link's
//! own bookkeeping is `peer_link_state.rs` and its transport is
//! `peer_link_worker.rs`. This is the side that decides *whether* a link exists
//! and *what the app is told about it*.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use devboule_protocol::{RemoteHostList, RemoteHostState, RemoteHostStatus};

use super::peer_dial::DialStep;
use super::peer_link_state::{HostLink, LinkAnswer};
use super::ServerState;
use crate::session::ConnHandle;

#[cfg(test)]
#[path = "peer_link_harness.rs"]
mod harness;
#[cfg(test)]
#[path = "peer_link_down_tests.rs"]
mod peer_link_down_tests;
#[cfg(test)]
#[path = "peer_link_lifecycle_tests.rs"]
mod peer_link_lifecycle_tests;
#[cfg(test)]
#[path = "peer_link_policy_tests.rs"]
mod peer_link_policy_tests;
#[cfg(test)]
#[path = "peer_link_status_tests.rs"]
mod peer_link_status_tests;
#[cfg(test)]
#[path = "peer_link_test_support.rs"]
mod peer_link_test_support;

/// Held links this daemon may keep at once. The same upper bound the one-shot
/// dialer uses for calls in flight (`peer_dial::MAX_OUTBOUND_CALLS`): four
/// hosts are already more sidebar rows than a person has open, and the cap is
/// what stops N rows from opening N sockets and N threads on the way out. The
/// two budgets are separate on purpose — a held link is not a call, and four
/// idle links plus four in-flight one-shot calls are both within design.
pub(crate) const MAX_HELD_LINKS: usize = 4;

/// Reads queued across every link at once. A link serves one read at a time,
/// so this is the number of callers that can be waiting on a worker, bounded
/// so a sidebar that re-renders cannot grow the queue without limit. Anything
/// past it is refused `busy` with nothing sent.
pub(crate) const MAX_PENDING_READS: usize = 8;

/// How long a link with no leases left stays up, so opening and closing a host
/// row in quick succession does not re-handshake every time.
pub(crate) const LINK_IDLE_GRACE: Duration = Duration::from_secs(30);

/// How long one read waits for its answer before the link is called offline.
/// Generous next to a tailnet round trip and short next to the keepalive
/// interval, so a silent host is noticed by the probe rather than by a caller.
pub(crate) const READ_DEADLINE: Duration = Duration::from_secs(10);

/// One link's pacing. Production is [`LinkTuning::default`]; a test sets the
/// fields down so keepalive, backoff and grace can be observed in
/// milliseconds instead of minutes.
#[derive(Clone)]
pub(crate) struct LinkTuning {
    /// How often the worker looks at the transport between events.
    pub(crate) poll: Duration,
    pub(crate) keepalive_every: Duration,
    pub(crate) pong_timeout: Duration,
    /// How long one read waits for its answer.
    pub(crate) read_deadline: Duration,
    pub(crate) backoff_min: Duration,
    pub(crate) backoff_max: Duration,
    pub(crate) idle_grace: Duration,
    /// At most one status delivery per host per window, latest wins with a
    /// trailing delivery. Production is one second; a test turns it down (or
    /// off) so a sequence is observable without waiting.
    pub(crate) status_window: Duration,
}

impl Default for LinkTuning {
    fn default() -> Self {
        Self {
            poll: Duration::from_millis(100),
            keepalive_every: Duration::from_secs(15),
            pong_timeout: Duration::from_secs(10),
            read_deadline: READ_DEADLINE,
            backoff_min: Duration::from_secs(1),
            backoff_max: Duration::from_secs(60),
            idle_grace: LINK_IDLE_GRACE,
            status_window: Duration::from_secs(1),
        }
    }
}

struct Links {
    links: HashMap<String, Arc<HostLink>>,
    pending_reads: usize,
}

/// Every held link this daemon owns, and the budget they share.
pub(crate) struct PeerLinks {
    inner: Mutex<Links>,
    /// How the links are paced. Production is [`LinkTuning::default`]; a test
    /// sets the fields down so keepalive, backoff and grace can be observed in
    /// milliseconds instead of minutes.
    tuning: LinkTuning,
}

impl Default for PeerLinks {
    fn default() -> Self {
        Self::new()
    }
}

impl PeerLinks {
    pub(crate) fn new() -> Self {
        Self::with_tuning(LinkTuning::default())
    }

    pub(crate) fn with_tuning(tuning: LinkTuning) -> Self {
        Self {
            inner: Mutex::new(Links {
                links: HashMap::new(),
                pending_reads: 0,
            }),
            tuning,
        }
    }

    /// Take a lease on one host, opening or joining its link.
    ///
    /// `Ok(())` means the lease is held; it does not mean the host is
    /// reachable. A host with no paired row, a revoked row or no local key gets
    /// its lease and then its own state, because the sidebar needs the row to
    /// say why it is empty. `Err(())` is the one refusal: the link cap, which
    /// is answered before a socket or a thread exists.
    ///
    /// Joining a link that is already up moves nothing. `connecting` is a fact
    /// about a worker that is starting, so it is published when one starts and
    /// never by a lease that found the link running; the watcher that joined is
    /// handed the state the link is in now instead.
    pub(crate) fn watch(
        &self,
        state: &Arc<ServerState>,
        conn: Arc<ConnHandle>,
        device_id: &str,
    ) -> Result<(), ()> {
        let link = {
            let mut inner = self.inner.lock().unwrap_or_else(|error| error.into_inner());
            match inner.links.get(device_id) {
                Some(link) => Arc::clone(link),
                None => {
                    if inner.links.len() >= MAX_HELD_LINKS {
                        return Err(());
                    }
                    let link = HostLink::new(
                        device_id.to_string(),
                        self.tuning.idle_grace,
                        self.tuning.status_window,
                    );
                    inner.links.insert(device_id.to_string(), Arc::clone(&link));
                    link
                }
            }
        };
        link.lease(Arc::clone(&conn));
        if link.is_serving() {
            link.replay_status(&conn);
        } else {
            link.publish(RemoteHostStatus {
                device_id: device_id.to_string(),
                state: RemoteHostState::Connecting,
                last_failure: None,
                revision: None,
            });
            super::peer_link_worker::spawn(state, link, self.tuning.clone());
        }
        Ok(())
    }

    /// Whether an outbound link to this device is up now. The Devices list
    /// reads online as the union of this and the inbound connection registry:
    /// a host watching a peer holds an outbound link and no inbound one.
    pub(crate) fn is_online(&self, device_id: &str) -> bool {
        let link = {
            let inner = self.inner.lock().unwrap_or_else(|error| error.into_inner());
            inner.links.get(device_id).cloned()
        };
        link.is_some_and(|link| link.published_state() == Some(RemoteHostState::Online))
    }

    /// Test-only: publish a state for a link whether or not a worker serves it,
    /// so the online union can be read without a clock or a socket.
    #[cfg(test)]
    pub(crate) fn publish_for_test(&self, device_id: &str, state: RemoteHostState) {
        let link = {
            let mut inner = self.inner.lock().unwrap_or_else(|error| error.into_inner());
            Arc::clone(inner.links.entry(device_id.to_string()).or_insert_with(|| {
                HostLink::new(
                    device_id.to_string(),
                    self.tuning.idle_grace,
                    self.tuning.status_window,
                )
            }))
        };
        link.publish(RemoteHostStatus {
            device_id: device_id.to_string(),
            state,
            last_failure: None,
            revision: None,
        });
    }

    /// Ask every held link to drop its transport and dial again. Called when
    /// this daemon's own workspace-host presence changes: the hello it sends
    /// states the new presence, and the far side re-resolves the scope from
    /// the record it keeps.
    pub(crate) fn request_reconnect_all(&self) {
        let links: Vec<Arc<HostLink>> = {
            let inner = self.inner.lock().unwrap_or_else(|error| error.into_inner());
            inner.links.values().cloned().collect()
        };
        for link in links {
            link.request_reconnect();
        }
    }

    /// Give back one connection's lease. A host nobody watches any more keeps
    /// its link until the grace runs out, so the next watch reuses it.
    pub(crate) fn unwatch(&self, conn_id: u64, device_id: &str) {
        let link = {
            let inner = self.inner.lock().unwrap_or_else(|error| error.into_inner());
            match inner.links.get(device_id) {
                Some(link) => Arc::clone(link),
                None => return,
            }
        };
        link.release(conn_id);
        self.collect_expired();
    }

    /// Give back every lease one connection held. A dropped window takes its
    /// hosts with it; the links linger only for the grace.
    pub(crate) fn release_connection(&self, conn_id: u64) {
        let links: Vec<Arc<HostLink>> = {
            let inner = self.inner.lock().unwrap_or_else(|error| error.into_inner());
            inner.links.values().cloned().collect()
        };
        for link in &links {
            // A dropped window takes its remote streams with it, wherever it
            // holds the host: the lease may be shared with another window, so
            // the subscription pass is not conditional on `leased_by`.
            link.remove_subscriptions_for(conn_id);
        }
        let held: Vec<Arc<HostLink>> = links
            .into_iter()
            .filter(|link| link.leased_by(conn_id))
            .collect();
        for link in held {
            link.release(conn_id);
        }
        self.collect_expired();
    }

    /// Drop the links whose worker has retired and whose grace has run out.
    fn collect_expired(&self) {
        let mut inner = self.inner.lock().unwrap_or_else(|error| error.into_inner());
        inner
            .links
            .retain(|_, link| link.lease_count() > 0 || link.is_serving());
    }

    /// Open one session's live stream on the host, waiting on its worker.
    ///
    /// The connection is the local app connection; relayed events go back to
    /// it through the normal writer, so nothing a local handler owns is held
    /// while the peer is talked to.
    pub(crate) fn attach(
        &self,
        device_id: &str,
        session_id: &str,
        subscription_id: u64,
        conn: Arc<ConnHandle>,
    ) -> LinkAnswer {
        let link = {
            let mut inner = self.inner.lock().unwrap_or_else(|error| error.into_inner());
            if inner.pending_reads >= MAX_PENDING_READS {
                return LinkAnswer::Failed(
                    RemoteHostState::Busy,
                    busy_sentence(MAX_PENDING_READS, "reads"),
                );
            }
            inner.pending_reads += 1;
            inner.links.get(device_id).cloned()
        };
        let (answer_tx, answer_rx) = std::sync::mpsc::sync_channel(1);
        let Some(link) = link else {
            self.finish_read();
            return LinkAnswer::Failed(RemoteHostState::Offline, offline_sentence().to_string());
        };
        if let Some(failure) = link.failure() {
            self.finish_read();
            return failure;
        }
        let Some(_permit) = link.try_read_permit() else {
            self.finish_read();
            return LinkAnswer::Failed(
                RemoteHostState::Busy,
                busy_sentence(1, "read in flight on this link"),
            );
        };
        if !link.queue_attach(
            link.generation(),
            session_id.to_string(),
            subscription_id,
            conn,
            answer_tx,
        ) {
            self.finish_read();
            return LinkAnswer::Failed(RemoteHostState::Offline, offline_sentence().to_string());
        }
        let answer = answer_rx
            .recv_timeout(Duration::from_secs(30))
            .unwrap_or_else(|_| {
                LinkAnswer::Failed(RemoteHostState::Offline, offline_sentence().to_string())
            });
        self.finish_read();
        answer
    }

    /// Close a revoked host's link now: its streams stop with it, and the
    /// watchers are told the pairing is gone. Called next to
    /// `revoke_peer_connections`, so an inbound close and an outbound close
    /// happen at the same moment rather than one probe apart.
    pub(crate) fn revoke(&self, device_id: &str) {
        let link = {
            let inner = self.inner.lock().unwrap_or_else(|error| error.into_inner());
            inner.links.get(device_id).cloned()
        };
        let Some(link) = link else {
            return;
        };
        link.clear_subscriptions();
        link.publish(RemoteHostStatus {
            device_id: device_id.to_string(),
            state: RemoteHostState::NeedsPairing,
            last_failure: Some(needs_pairing_sentence().to_string()),
            revision: None,
        });
        if !link.queue_revoke() {
            link.retire();
        }
    }

    /// Close one session's live stream.
    ///
    /// The local close is unconditional and first: whatever the link can do,
    /// no event is relayed after this call. The peer is asked to stop right
    /// away when the link is free, and the request is parked for the worker's
    /// next idle turn when it is not — a busy moment must not leave the host
    /// streaming. A link this daemon no longer has is still an `Accepted`.
    pub(crate) fn detach(
        &self,
        device_id: &str,
        session_id: &str,
        subscription_id: u64,
    ) -> LinkAnswer {
        let link = {
            let inner = self.inner.lock().unwrap_or_else(|error| error.into_inner());
            inner.links.get(device_id).cloned()
        };
        let Some(link) = link else {
            return LinkAnswer::Accepted;
        };
        link.remove_subscription(subscription_id);
        let Some(_permit) = link.try_read_permit() else {
            link.defer_detach(session_id.to_string(), subscription_id);
            return LinkAnswer::Accepted;
        };
        let (answer_tx, answer_rx) = std::sync::mpsc::sync_channel(1);
        if !link.queue_detach(
            link.generation(),
            session_id.to_string(),
            subscription_id,
            answer_tx,
        ) {
            link.defer_detach(session_id.to_string(), subscription_id);
            return LinkAnswer::Accepted;
        }
        answer_rx
            .recv_timeout(Duration::from_secs(30))
            .unwrap_or(LinkAnswer::Accepted)
    }

    /// Read one list over the host's link, waiting on its worker.
    ///
    /// Called from a worker thread of the *local* connection, never from that
    /// connection's reader: the reply comes back through the normal writer, so
    /// nothing a local handler owns is held while a peer is talked to.
    pub(crate) fn read(&self, device_id: &str, list: RemoteHostList) -> LinkAnswer {
        let link = {
            let mut inner = self.inner.lock().unwrap_or_else(|error| error.into_inner());
            if inner.pending_reads >= MAX_PENDING_READS {
                return LinkAnswer::Failed(
                    RemoteHostState::Busy,
                    busy_sentence(MAX_PENDING_READS, "reads"),
                );
            }
            inner.pending_reads += 1;
            inner.links.get(device_id).cloned()
        };
        let (answer_tx, answer_rx) = std::sync::mpsc::sync_channel(1);
        // No link, or a link whose worker has retired, is the same fact to the
        // caller: nobody is holding a connection to that host.
        let Some(link) = link else {
            self.finish_read();
            return LinkAnswer::Failed(RemoteHostState::Offline, offline_sentence().to_string());
        };
        if let Some(failure) = link.failure() {
            self.finish_read();
            return failure;
        }
        let Some(_permit) = link.try_read_permit() else {
            self.finish_read();
            return LinkAnswer::Failed(
                RemoteHostState::Busy,
                busy_sentence(1, "read in flight on this link"),
            );
        };
        if !link.queue_read(link.generation(), list, answer_tx) {
            self.finish_read();
            return LinkAnswer::Failed(RemoteHostState::Offline, offline_sentence().to_string());
        }
        // A queued read is answered by the worker: when it dials (a refusal
        // or a read), and while it waits out a backoff. The cases that wait
        // longer are a dial in progress, whose steps sum to about twenty-five
        // seconds, and a link with no lease left, which retires at its grace.
        // Thirty seconds covers both and a worker that never picks it up.
        let answer = answer_rx
            .recv_timeout(Duration::from_secs(30))
            .unwrap_or_else(|_| {
                LinkAnswer::Failed(RemoteHostState::Offline, offline_sentence().to_string())
            });
        self.finish_read();
        answer
    }

    fn finish_read(&self) {
        let mut inner = self.inner.lock().unwrap_or_else(|error| error.into_inner());
        inner.pending_reads = inner.pending_reads.saturating_sub(1);
    }
}

// The three refusals the app can see, and the one sentence each carries.
// Written here rather than formatted from a `DialStep` so no address, key or
// tailnet name can reach the UI by accident: these strings are the whole of
// what a host's `last_failure` can say.

pub(crate) fn needs_pairing_sentence() -> &'static str {
    "This device is no longer paired with this daemon."
}

pub(crate) fn identity_missing_sentence() -> &'static str {
    "This daemon has no device identity of its own, so it cannot reach any host."
}

pub(crate) fn unsupported_sentence() -> &'static str {
    "The host runs an older Devboule that cannot serve this list."
}

pub(crate) fn offline_sentence() -> &'static str {
    "The host stopped answering."
}

pub(crate) fn busy_sentence(limit: usize, unit: &str) -> String {
    format!("This daemon is already using all of its {limit} {unit} allowances.")
}

/// The host row a failed dial belongs to, and the sentence that goes with it.
///
/// `Dialect`-level facts (`RowMissing`, `Revoked`, `Identity`, `Unsupported`)
/// keep their own state; every transport-level step is a plain `offline`,
/// because a TCP failure and a silent host are the same fact to the user and
/// the same repair.
pub(crate) fn state_for(step: DialStep) -> RemoteHostState {
    match step {
        DialStep::RowMissing | DialStep::Revoked | DialStep::NoListenPort | DialStep::Address => {
            RemoteHostState::NeedsPairing
        }
        DialStep::Identity => RemoteHostState::IdentityMissing,
        DialStep::Unsupported => RemoteHostState::Unsupported,
        DialStep::Busy => RemoteHostState::Busy,
        DialStep::Connect
        | DialStep::Handshake
        | DialStep::Hello
        | DialStep::Send
        | DialStep::Reply => RemoteHostState::Offline,
    }
}

/// The one sentence for a failed dial step.
pub(crate) fn sentence_for(step: DialStep) -> String {
    match state_for(step) {
        RemoteHostState::NeedsPairing => needs_pairing_sentence().to_string(),
        RemoteHostState::IdentityMissing => identity_missing_sentence().to_string(),
        RemoteHostState::Unsupported => unsupported_sentence().to_string(),
        RemoteHostState::Busy => busy_sentence(super::peer_dial::MAX_OUTBOUND_CALLS, "calls"),
        RemoteHostState::Connecting | RemoteHostState::Online | RemoteHostState::Offline => {
            offline_sentence().to_string()
        }
    }
}
