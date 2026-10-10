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
use super::peer_link_state::{HostLink, LinkAnswer, LinkCommand};
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

/// How long one remote create waits for the host's answer. A create runs
/// the provider's whole handshake inline on the host — the local create
/// budget is 210 s for the same reason — so the link's ten-second read
/// budget would refuse every agent create while the host is still starting
/// it. When this lapses the outcome is unknown, and only an explicit retry
/// with the same idempotency key may ask again.
pub(crate) const CREATE_DEADLINE: Duration = Duration::from_secs(120);

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
    /// How long one remote create waits for the host's answer. Production
    /// is two minutes against the provider handshake; a test turns it down
    /// with the rest of the pacing.
    pub(crate) create_deadline: Duration,
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
            create_deadline: CREATE_DEADLINE,
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
        if link.create_inflight() {
            self.finish_read();
            return LinkAnswer::Failed(RemoteHostState::Busy, create_busy_sentence());
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
            return Self::queue_lost(&link);
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
        link.mark_revoked();
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
        if link.create_inflight() {
            self.finish_read();
            return LinkAnswer::Failed(RemoteHostState::Busy, create_busy_sentence());
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
            return Self::queue_lost(&link);
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

    /// Run one operate call over the host's link, waiting on its worker.
    ///
    /// The same budgets a read meets: the cross-link pending cap, the link's
    /// published failure, and the one-call-at-a-time slot. Called from a
    /// worker thread of the *local* connection, never from that connection's
    /// reader. `wait` is how long the caller holds for the worker's answer:
    /// the read budget for most calls, the create budget for a create.
    fn operate(
        &self,
        device_id: &str,
        wait: Duration,
        is_create: bool,
        build: impl FnOnce(u64, std::sync::mpsc::SyncSender<LinkAnswer>) -> LinkCommand,
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
        if is_create {
            // A create holds the create lane, not the read lane: it queues
            // behind a short read already running, and a second create is
            // refused rather than stacked behind a minutes-long handshake.
            let Some(_create) = link.try_create_permit() else {
                self.finish_read();
                return LinkAnswer::Failed(RemoteHostState::Busy, create_busy_sentence());
            };
            if !link.queue_operate(build(link.generation(), answer_tx)) {
                self.finish_read();
                return Self::queue_lost(&link);
            }
            let answer = answer_rx.recv_timeout(wait).unwrap_or_else(|_| {
                LinkAnswer::Failed(RemoteHostState::Offline, offline_sentence().to_string())
            });
            self.finish_read();
            return answer;
        }
        // Anything short fails fast while a create runs instead of queueing
        // behind the provider handshake and timing out as `offline`.
        if link.create_inflight() {
            self.finish_read();
            return LinkAnswer::Failed(RemoteHostState::Busy, create_busy_sentence());
        }
        let Some(_permit) = link.try_read_permit() else {
            self.finish_read();
            return LinkAnswer::Failed(
                RemoteHostState::Busy,
                busy_sentence(1, "read in flight on this link"),
            );
        };
        if !link.queue_operate(build(link.generation(), answer_tx)) {
            self.finish_read();
            return Self::queue_lost(&link);
        }
        let answer = answer_rx.recv_timeout(wait).unwrap_or_else(|_| {
            LinkAnswer::Failed(RemoteHostState::Offline, offline_sentence().to_string())
        });
        self.finish_read();
        answer
    }

    /// The one-slot worker queue refused a call the permits admitted: a
    /// serving worker means another call is ahead in the queue (busy, with
    /// a sentence that says to wait), while a retired worker means nobody
    /// is holding the link at all (offline). The race is microseconds wide
    /// — the worker takes a queued call within one loop turn — but the
    /// loser must still read busy, never a dead host.
    pub(crate) fn queue_lost(link: &HostLink) -> LinkAnswer {
        if link.is_serving() {
            LinkAnswer::Failed(RemoteHostState::Busy, queue_busy_sentence())
        } else {
            LinkAnswer::Failed(RemoteHostState::Offline, offline_sentence().to_string())
        }
    }

    /// How long one operate caller holds for the worker's answer: the read
    /// budget, except a create, which runs the host's provider handshake
    /// and waits on the create budget with a margin for the queue.
    fn operate_wait(&self, create: bool) -> Duration {
        if create {
            self.tuning.create_deadline + Duration::from_secs(30)
        } else {
            Duration::from_secs(30)
        }
    }

    /// Create one session on the host. The idempotency key travels with the
    /// peer's own `SessionCreate`; the worker never retries itself.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn operate_create(
        &self,
        device_id: &str,
        workspace_id: Option<String>,
        kind: devboule_protocol::SessionKind,
        provider: Option<String>,
        mode: Option<String>,
        display_name: Option<String>,
        idempotency_key: Option<String>,
        cols: Option<u16>,
        rows: Option<u16>,
    ) -> LinkAnswer {
        let wait = self.operate_wait(true);
        self.operate(device_id, wait, true, |generation, answer| {
            LinkCommand::Create {
                generation,
                workspace_id,
                kind,
                provider,
                mode,
                display_name,
                idempotency_key,
                cols,
                rows,
                answer,
            }
        })
    }

    /// Send text into one session on the host.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn operate_send(
        &self,
        device_id: &str,
        session_id: String,
        subscription_id: u64,
        text: String,
        attachments: Vec<devboule_protocol::PromptAttachment>,
        active_turn_behavior: Option<devboule_protocol::ActiveTurnBehavior>,
        idempotency_key: Option<String>,
        attachment_references: Vec<devboule_protocol::AttachmentReference>,
    ) -> LinkAnswer {
        let wait = self.operate_wait(false);
        self.operate(device_id, wait, false, |generation, answer| {
            LinkCommand::Send {
                generation,
                session_id,
                subscription_id,
                text,
                attachments,
                active_turn_behavior,
                idempotency_key,
                attachment_references,
                answer,
            }
        })
    }

    /// Resize one terminal on the host.
    pub(crate) fn operate_resize(
        &self,
        device_id: &str,
        session_id: String,
        subscription_id: u64,
        cols: u16,
        rows: u16,
    ) -> LinkAnswer {
        let wait = self.operate_wait(false);
        self.operate(device_id, wait, false, |generation, answer| {
            LinkCommand::Resize {
                generation,
                session_id,
                subscription_id,
                cols,
                rows,
                answer,
            }
        })
    }

    /// Claim one terminal's resize right on the host.
    pub(crate) fn operate_claim(
        &self,
        device_id: &str,
        session_id: String,
        subscription_id: u64,
    ) -> LinkAnswer {
        let wait = self.operate_wait(false);
        self.operate(device_id, wait, false, |generation, answer| {
            LinkCommand::Claim {
                generation,
                session_id,
                subscription_id,
                answer,
            }
        })
    }

    /// Interrupt one session on the host.
    pub(crate) fn operate_interrupt(
        &self,
        device_id: &str,
        session_id: String,
        subscription_id: u64,
    ) -> LinkAnswer {
        let wait = self.operate_wait(false);
        self.operate(device_id, wait, false, |generation, answer| {
            LinkCommand::Interrupt {
                generation,
                session_id,
                subscription_id,
                answer,
            }
        })
    }

    /// Answer one permission card on the host.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn operate_permission_respond(
        &self,
        device_id: &str,
        session_id: String,
        subscription_id: u64,
        request_id: String,
        outcome: devboule_protocol::PermissionOutcome,
        option_id: Option<String>,
        answer_text: Option<String>,
        idempotency_key: Option<String>,
    ) -> LinkAnswer {
        let wait = self.operate_wait(false);
        self.operate(device_id, wait, false, |generation, answer| {
            LinkCommand::PermissionRespond {
                generation,
                session_id,
                subscription_id,
                request_id,
                outcome,
                option_id,
                answer_text,
                idempotency_key,
                answer,
            }
        })
    }

    /// Close one session on the host.
    pub(crate) fn operate_close(
        &self,
        device_id: &str,
        session_id: String,
        idempotency_key: Option<String>,
    ) -> LinkAnswer {
        let wait = self.operate_wait(false);
        self.operate(device_id, wait, false, |generation, answer| {
            LinkCommand::Close {
                generation,
                session_id,
                idempotency_key,
                answer,
            }
        })
    }

    /// Stop one session's process on the host, keeping the session.
    pub(crate) fn operate_stop(
        &self,
        device_id: &str,
        session_id: String,
        subscription_id: u64,
    ) -> LinkAnswer {
        let wait = self.operate_wait(false);
        self.operate(device_id, wait, false, |generation, answer| {
            LinkCommand::Stop {
                generation,
                session_id,
                subscription_id,
                answer,
            }
        })
    }

    /// Read the host's provider catalog.
    pub(crate) fn operate_providers(&self, device_id: &str) -> LinkAnswer {
        let wait = self.operate_wait(false);
        self.operate(device_id, wait, false, |generation, answer| {
            LinkCommand::Providers { generation, answer }
        })
    }

    /// Switch the model of one session on the host.
    pub(crate) fn operate_set_model(
        &self,
        device_id: &str,
        session_id: String,
        model_id: Option<String>,
        effort: Option<String>,
    ) -> LinkAnswer {
        let wait = self.operate_wait(false);
        self.operate(device_id, wait, false, |generation, answer| {
            LinkCommand::SetModel {
                generation,
                session_id,
                model_id,
                effort,
                answer,
            }
        })
    }

    /// Switch the mode of one session on the host.
    pub(crate) fn operate_set_mode(
        &self,
        device_id: &str,
        session_id: String,
        mode_id: String,
    ) -> LinkAnswer {
        let wait = self.operate_wait(false);
        self.operate(device_id, wait, false, |generation, answer| {
            LinkCommand::SetMode {
                generation,
                session_id,
                mode_id,
                answer,
            }
        })
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

/// The refusal a roster read, attach or operate call gets while a remote
/// create is running on the same link. A create runs the host's provider
/// handshake inline — minutes, not milliseconds — so anything short fails
/// fast with the one repair (wait for the start to land) instead of
/// queueing behind it and timing out as `offline`.
pub(crate) fn create_busy_sentence() -> String {
    "This daemon is still starting a session on this host.".to_string()
}

/// The refusal a call gets when it loses the worker's one-slot queue to
/// another call: wait and ask again, the host is up.
pub(crate) fn queue_busy_sentence() -> String {
    "Another call is already queued on this host.".to_string()
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
