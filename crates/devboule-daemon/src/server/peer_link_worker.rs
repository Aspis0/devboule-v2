//! One link's transport: dial, shake hands, answer the versioned hello, keep
//! the link alive, serve one read at a time, reconnect while a lease exists,
//! and close when none does.
//!
//! The worker is single-threaded on purpose. One thread owns the framed
//! transport, so there is no second reader to race the first and no writer that
//! can interleave a request mid-frame; a read is written and its reply read
//! back on the same thread that owns both. A caller waits on this worker
//! through a channel, never by touching the socket.
//!
//! The private key is borrowed for the handshake and dropped before the first
//! application byte: nothing here keeps a copy of it, and the link itself
//! carries no key material at all.

use std::sync::mpsc::{self, Receiver};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use devboule_protocol::{
    ClientHello, ClientMessage, DaemonHello, DaemonMessage, OwnerId, RemoteHostState,
    RemoteHostStatus,
};

use super::peer_dial::{connect_and_handshake, DialStep};
use super::peer_link::{sentence_for, state_for, LinkTuning};
use super::peer_link_read::serve_read;
use super::peer_link_state::{HostLink, LinkAnswer, LinkCommand};
use super::ServerState;
use crate::framing::Framed;
use crate::journal::PeerRecord;

/// Probe ids start above every id a read uses, so a pong can never be mistaken
/// for a read's reply, nor the reverse.
pub(crate) const PROBE_ID_BASE: u64 = 1 << 62;

/// Missed probes before a link is called offline and closed. Two, because one
/// lost pong is a tailnet fact and two are a host that has stopped answering.
const MAX_PROBE_MISSES: u32 = 2;

/// Start the one worker that owns this link's transport.
///
/// A link that already has a worker keeps it: two workers on one link would
/// each publish status for a transport the other replaced, and `serve` is what
/// settles which of them is the one.
pub(crate) fn spawn(state: &Arc<ServerState>, link: Arc<HostLink>, tuning: LinkTuning) {
    let (commands, queue) = mpsc::sync_channel::<LinkCommand>(1);
    let Ok(_token) = link.serve(commands) else {
        return;
    };
    let started = {
        let state = Arc::clone(state);
        let link = Arc::clone(&link);
        thread::Builder::new()
            .name(format!("daemon-peer-link-{}", link.device_id))
            .spawn(move || {
                run(state, link, tuning, queue);
            })
    };
    if started.is_err() {
        link.retire();
    }
}

fn run(
    state: Arc<ServerState>,
    link: Arc<HostLink>,
    tuning: LinkTuning,
    queue: Receiver<LinkCommand>,
) {
    let mut session: Option<LinkSession> = None;
    let mut empty_since: Option<Instant> = None;
    let mut backoff = tuning.backoff_min;
    let mut reads = 0u64;
    loop {
        if state.is_shutting_down() {
            link.retire();
            return;
        }
        if link.lease_count() == 0 {
            let since = *empty_since.get_or_insert_with(Instant::now);
            if since.elapsed() >= tuning.idle_grace {
                // The last lease is gone and the grace has run out: the link
                // closes and nothing retries it without a new watch.
                link.publish(status(
                    &link,
                    RemoteHostState::Offline,
                    Some(sentence_for(DialStep::Reply)),
                ));
                link.retire();
                return;
            }
            if session.is_none() && !link.wait_for_lease() {
                link.retire();
                return;
            }
        } else {
            empty_since = None;
        }
        if link.take_reconnect_request() {
            // This daemon's workspace presence changed: the transport is
            // replaced so the next hello states it. No `offline` edge — the
            // link is being redialed, not lost.
            if session.is_some() {
                link.bump_generation();
            }
            session = None;
        }
        if session.is_none() {
            // No `connecting` here. The watch that started this worker already
            // published it, and a retry must not publish it again: alternating
            // it with the failure below makes a host that stays down flicker
            // between connecting and offline on every backoff tick, and
            // `publish`'s dedupe reads each pair as two changes instead of the
            // one it is.
            match open(&state, &link) {
                Ok(opened) => {
                    backoff = tuning.backoff_min;
                    reads = 0;
                    session = Some(opened);
                    link.publish(status(&link, RemoteHostState::Online, None));
                }
                Err(step) => {
                    link.publish(status(&link, state_for(step), Some(sentence_for(step))));
                    refuse_reads_until(&queue, step, backoff_delay(&link.device_id, backoff));
                    backoff = (backoff * 2).min(tuning.backoff_max);
                    continue;
                }
            }
        }
        let Some(open) = session.as_mut() else {
            continue;
        };
        match queue.try_recv() {
            Ok(command) => {
                reads += 1;
                serve_read(&state, &link, open, command, reads, tuning.read_deadline);
                continue;
            }
            Err(mpsc::TryRecvError::Disconnected) => {
                link.retire();
                return;
            }
            Err(mpsc::TryRecvError::Empty) => {}
        }
        match keepalive(&state, &link, open, &tuning) {
            Keepalive::Alive => {}
            Keepalive::Lost => session = None,
        }
    }
}

/// The one live transport plus the bookkeeping that decides whether it is
/// still alive. Its fields are the worker's and the read's; nothing else reads
/// them.
pub(crate) struct LinkSession {
    pub(crate) framed: Framed,
    /// The remote's own hello, kept for the frame-capability check: a daemon
    /// that predates a list must be refused before the request can fail its
    /// reader.
    pub(crate) hello: DaemonHello,
    next_probe: Instant,
    pub(crate) outstanding_since: Option<Instant>,
    pub(crate) misses: u32,
}

enum Keepalive {
    Alive,
    Lost,
}

/// Probe, count a miss, and drop the link when two probes go unanswered.
///
/// A frame that arrives here is not a read's: reads are served with their own
/// wait, and anything else the remote pushes in idle is ignored rather than
/// counted against a caller.
fn keepalive(
    state: &Arc<ServerState>,
    link: &HostLink,
    session: &mut LinkSession,
    tuning: &LinkTuning,
) -> Keepalive {
    if let Ok(message) = session.framed.recv_timeout(tuning.poll) {
        if let DaemonMessage::Pong { id, .. } = &message {
            if *id >= PROBE_ID_BASE {
                session.outstanding_since = None;
                session.misses = 0;
                return Keepalive::Alive;
            }
        }
        record_workspace_change(link, session.hello.protocol_version, &message);
    }
    if let Some(sent) = session.outstanding_since {
        if sent.elapsed() >= tuning.pong_timeout {
            session.misses += 1;
            session.outstanding_since = None;
            session.next_probe = Instant::now() + tuning.keepalive_every;
        }
    }
    if session.misses >= MAX_PROBE_MISSES {
        // The link closes here, once: reconnecting is the loop's own decision
        // and it starts from `Connecting`, so the app sees one offline edge.
        link.publish(status(
            link,
            RemoteHostState::Offline,
            Some(sentence_for(DialStep::Reply)),
        ));
        return Keepalive::Lost;
    }
    if Instant::now() >= session.next_probe && session.outstanding_since.is_none() {
        // The row is re-read on the way to every probe, so a revoke that lands
        // while the link is up closes it on the next tick instead of waiting
        // for the transport to fail. One journal read per link per probe
        // period is the cost of saying "revoked" the moment it is true.
        if let Err(step) = peer_row(state, &link.device_id) {
            link.publish(status(link, state_for(step), Some(sentence_for(step))));
            return Keepalive::Lost;
        }
        if session
            .framed
            .send(&ClientMessage::Ping {
                id: PROBE_ID_BASE + u64::from(session.misses),
            })
            .is_ok()
        {
            session.outstanding_since = Some(Instant::now());
            session.next_probe = Instant::now() + tuning.keepalive_every;
        } else {
            link.publish(status(
                link,
                RemoteHostState::Offline,
                Some(sentence_for(DialStep::Send)),
            ));
            return Keepalive::Lost;
        }
    }
    Keepalive::Alive
}

/// Record one pushed workspace revision and tell the watchers, which is what
/// makes the sidebar reload that host's snapshots. A frame naming another
/// device is dropped: the link's own device id is the only identity this side
/// trusts, and a peer cannot make this daemon act for a third one.
pub(crate) fn record_workspace_change(
    link: &HostLink,
    far_protocol_version: u32,
    message: &DaemonMessage,
) {
    let DaemonMessage::HostWorkspaceChanged {
        device_id,
        revision,
    } = message
    else {
        return;
    };
    // The frame exists only from the roleless dialect on; a peer that
    // negotiated less cannot have sent it legitimately.
    if far_protocol_version < 32 {
        return;
    }
    if device_id != &link.device_id {
        return;
    }
    // A refused number is not news: it moves nothing and is not re-published.
    if !link.set_remote_revision(*revision) {
        return;
    }
    link.publish(status(link, RemoteHostState::Online, None));
}

/// The peer's row, read now, with the dialable facts already judged.
///
/// Every check on this path goes through here rather than through a row read
/// earlier: a revoke that lands while the link is up stops the next read and
/// the next probe, not only the next reconnect.
pub(crate) fn peer_row(state: &Arc<ServerState>, device_id: &str) -> Result<PeerRecord, DialStep> {
    let row = state
        .peer_get(device_id)
        .map_err(|_| DialStep::RowMissing)?
        .ok_or(DialStep::RowMissing)?;
    if row.is_revoked() {
        return Err(DialStep::Revoked);
    }
    // Every paired, non-revoked device is eligible for an authenticated
    // connection (design decision, section 1): the link is how a client
    // device that later creates its first workspace is discovered, so the
    // recorded hosting fact cannot gate the dial without making that
    // discovery impossible. The presence word on the authenticated hello,
    // recorded one-way, decides the scope and which rows a sidebar draws;
    // `legacy_dialable` only matters for the dialect the hello speaks.
    let address: std::net::SocketAddr = row.address.parse().map_err(|_| DialStep::Address)?;
    if address.port() == 0 {
        // A row with port `0` is the record of a device that never advertised
        // a listener (a phone, or a pairing that predates the port field).
        // Connecting anyway would reach whatever owns an ephemeral port now.
        return Err(DialStep::NoListenPort);
    }
    Ok(row)
}

/// Dial one host and complete the hello, re-reading its `peers` row first.
///
/// The row is read here, not carried in from the watch: a revoke that landed
/// while the link was down is in the table now, and it must close the link
/// rather than be retried around.
fn open(state: &Arc<ServerState>, link: &HostLink) -> Result<LinkSession, DialStep> {
    let row = peer_row(state, &link.device_id)?;
    let identity = state
        .device_identity()
        .as_ref()
        .map_err(|_| DialStep::Identity)?;
    let hello = ClientHello::peer(
        OwnerId::new(
            format!("peer_{}", identity.device_id),
            crate::peer_policy::PEER_OWNER_TAG,
        )
        .map_err(|_| DialStep::Identity)?,
        "devboule-daemon",
        state.has_hosted_workspace(),
    );
    // The key is borrowed here and gone before the first application byte.
    // The handshake's own step is the link's step, so the error travels as it
    // came rather than through a second spelling of the same dial.
    let framed = connect_and_handshake(identity.private_key(), &row.public_key, &row.address)
        .map_err(|error| error.dial_step())?;
    framed
        .send(&ClientMessage::Hello(hello))
        .map_err(|_| DialStep::Send)?;
    let answer: DaemonMessage = framed
        .recv_timeout(crate::peer_transport::HANDSHAKE_DEADLINE)
        .map_err(|_| DialStep::Hello)?;
    let DaemonMessage::Hello(remote) = answer else {
        return Err(DialStep::Hello);
    };
    // The far end's hello is what the frame-capability check reads, so it is
    // kept; nothing else about the dial survives into the link.
    link.bump_generation();
    // A new transport re-baselines the host's counter: a host that restarted
    // resets its revision, and the old value must not outrank the new link's.
    link.clear_remote_revision();
    Ok(LinkSession {
        framed,
        hello: remote,
        next_probe: Instant::now(),
        outstanding_since: None,
        misses: 0,
    })
}

fn status(
    link: &HostLink,
    state: RemoteHostState,
    last_failure: Option<String>,
) -> RemoteHostStatus {
    RemoteHostStatus {
        device_id: link.device_id.clone(),
        state,
        last_failure,
        revision: link.remote_revision(),
    }
}

/// Wait out one backoff, answering every read that is queued before it ends
/// with the step that stopped the dial: a caller on a host that cannot be
/// reached learns the reason now, whenever in the backoff it asked.
pub(super) fn refuse_reads_until(queue: &Receiver<LinkCommand>, step: DialStep, delay: Duration) {
    let wake = Instant::now() + delay;
    loop {
        let left = wake.saturating_duration_since(Instant::now());
        match queue.recv_timeout(left) {
            Ok(LinkCommand::Read { answer, .. }) => {
                let _ = answer.send(LinkAnswer::Failed(state_for(step), sentence_for(step)));
            }
            Err(mpsc::RecvTimeoutError::Timeout) => return,
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                thread::sleep(left);
                return;
            }
        }
    }
}

/// Reconnect backoff plus jitter, derived from the host's own name so two
/// hosts that failed together do not retry on the same tick.
fn backoff_delay(device_id: &str, base: Duration) -> Duration {
    let seed = device_id.bytes().fold(2_166_136_261u64, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(16_777_619)
    });
    let base_ms = base.as_millis() as u64;
    let spread = base_ms / 2;
    if spread == 0 {
        return base;
    }
    Duration::from_millis(base_ms + seed % spread)
}
