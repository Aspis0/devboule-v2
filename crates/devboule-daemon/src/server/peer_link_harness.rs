//! The fixture a held-link test starts from: a state with one paired host row,
//! a watching connection, a manager paced in milliseconds, and the responder
//! that host is reachable at.
//!
//! The manager is the test's own, not `state.peer_links`: production pacing is
//! fifteen-second probes and a thirty-second grace, and no test should wait
//! thirty seconds to see a link close.

use std::sync::mpsc::Receiver;
use std::sync::Arc;
use std::time::Duration;

use devboule_protocol::{ClientMessage, RemoteHostState};

use super::peer_link_test_support::{
    current_capabilities, host_row, pinned_keypair, spawn, Responder,
};
use super::{LinkTuning, PeerLinks};

/// The pacing every harness but the keepalive test's runs at.
///
/// The probes are paced in milliseconds, so the tests still exercise the
/// keepalive path, but the pong budget is longer than any test lives and
/// closing a link takes two unanswered probes: a harness's link is closed by
/// the test that breaks it and by nothing else. A budget tight enough for a
/// loaded core to miss manufactures the very failure the keepalive test is
/// here to cause, and did — a 1.5 s budget closed links whose remote never
/// stopped answering.
///
/// The read budget is five seconds. A loopback read answers in microseconds,
/// so this is a hundred times the round trip: here to catch a stall, not to
/// survive one.
fn pacing() -> LinkTuning {
    LinkTuning {
        status_window: Duration::ZERO,
        poll: Duration::from_millis(10),
        keepalive_every: Duration::from_millis(400),
        pong_timeout: Duration::from_secs(30),
        read_deadline: Duration::from_secs(5),
        backoff_min: Duration::from_millis(20),
        backoff_max: Duration::from_millis(60),
        idle_grace: Duration::from_millis(60),
    }
}

pub(crate) struct Harness {
    pub(crate) state: Arc<crate::server::ServerState>,
    pub(crate) links: PeerLinks,
    pub(crate) conn: Arc<crate::session::ConnHandle>,
    pub(crate) responder: Responder,
    pub(crate) requests: Receiver<ClientMessage>,
}

impl Harness {
    /// The paced fixture; [`Self::start`] is this with a current daemon's
    /// capabilities.
    pub(crate) fn start(instance: &str) -> Self {
        Self::with_capabilities(instance, current_capabilities())
    }

    /// The fixture for the tests whose subject is what the link going down
    /// looks like: the same link with the pong budget paced in milliseconds,
    /// so a missed pong and the edge that follows it both happen inside a
    /// test's life.
    pub(crate) fn with_fast_probes(instance: &str) -> Self {
        let mut tuning = pacing();
        tuning.pong_timeout = Duration::from_millis(1500);
        Self::paced(instance, current_capabilities(), tuning)
    }

    /// The fixture for a host that cannot be dialled: the row points at a port
    /// nothing listens on, and every retry waits `backoff`, so a test lives
    /// inside one backoff sleep.
    pub(crate) fn host_down(instance: &str, backoff: Duration) -> Self {
        let mut tuning = pacing();
        tuning.backoff_min = backoff;
        tuning.backoff_max = backoff;
        let harness = Self::paced(instance, current_capabilities(), tuning);
        let dead = std::net::TcpListener::bind("127.0.0.1:0")
            .and_then(|listener| listener.local_addr())
            .expect("a free port to leave unlistened");
        harness
            .state
            .peer_upsert(host_row(dead.to_string(), &pinned_keypair().public))
            .expect("repoint the row");
        harness
    }

    pub(crate) fn with_capabilities(
        instance: &str,
        capabilities: Vec<devboule_protocol::Capability>,
    ) -> Self {
        Self::paced(instance, capabilities, pacing())
    }

    fn paced(
        instance: &str,
        capabilities: Vec<devboule_protocol::Capability>,
        tuning: LinkTuning,
    ) -> Self {
        let keypair = pinned_keypair();
        let (responder, requests) = spawn(
            keypair.private.clone().try_into().expect("32 bytes"),
            capabilities,
        );
        let state = crate::server::ServerState::new(instance.to_string());
        state
            .peer_upsert(host_row(responder.address.to_string(), &keypair.public))
            .expect("upsert the row");
        Self {
            links: PeerLinks::with_tuning(tuning),
            conn: crate::session::ConnHandle::new(state.alloc_conn()),
            state,
            responder,
            requests,
        }
    }

    pub(crate) fn watch(&self) {
        self.links
            .watch(&self.state, Arc::clone(&self.conn), "b")
            .expect("the first watch is inside the link cap");
    }

    /// Every state change the link has published to this connection so far.
    pub(crate) fn statuses(&self) -> Vec<(RemoteHostState, Option<String>)> {
        self.statuses_for(&self.conn)
    }

    /// Every state change published to one connection. The router tests read
    /// another window's, because that is the connection whose lease was taken.
    pub(crate) fn statuses_for(
        &self,
        conn: &crate::session::ConnHandle,
    ) -> Vec<(RemoteHostState, Option<String>)> {
        conn.outbound
            .pull_replies()
            .into_iter()
            .filter_map(|reply| match reply {
                devboule_protocol::DaemonMessage::RemoteHostStatus {
                    state,
                    last_failure,
                    ..
                } => Some((state, last_failure)),
                _ => None,
            })
            .collect()
    }

    /// The pushed workspace revisions this connection has been told about,
    /// with the state each status carried. Kept beside `statuses_for` so the
    /// existing tuple stays for the state-only tests.
    pub(crate) fn statuses_with_revision(&self) -> Vec<(RemoteHostState, Option<u64>)> {
        self.conn
            .outbound
            .pull_replies()
            .into_iter()
            .filter_map(|reply| match reply {
                devboule_protocol::DaemonMessage::RemoteHostStatus {
                    state, revision, ..
                } => Some((state, revision)),
                _ => None,
            })
            .collect()
    }

    /// Block until the link reports itself online to `conn`.
    ///
    /// A read queued before the handshake finished is refused `offline`: it was
    /// queued against the generation the new transport replaced. So a caller
    /// waits for the state it already asked for, rather than reading a link it
    /// cannot see yet.
    pub(crate) fn wait_online(&self, conn: &crate::session::ConnHandle) {
        super::peer_link_test_support::eventually("the link comes up", || {
            self.statuses_for(conn)
                .iter()
                .any(|(state, _)| *state == RemoteHostState::Online)
        });
    }

    /// Count the offline edges this link has published, keeping what was already
    /// counted so a drain between polls does not lose them.
    pub(crate) fn count_offline(&self, seen: &mut usize) {
        *seen += self
            .statuses()
            .iter()
            .filter(|(state, _)| *state == RemoteHostState::Offline)
            .count();
    }

    /// A second window on the same daemon, for the lease tests.
    pub(crate) fn other_conn(&self) -> Arc<crate::session::ConnHandle> {
        crate::session::ConnHandle::new(self.state.alloc_conn())
    }
}
