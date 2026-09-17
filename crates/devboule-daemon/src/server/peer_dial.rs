//! Outbound peer calls: dial a paired device, send one request, read one
//! reply.
//!
//! The mirror image of the accept path (`serve_noise_peer`). The responder
//! refuses a remote static key that no non-revoked `peers` row pins; the
//! dialer therefore never learns a key on the way out either: the key handed
//! to the handshake is the one the caller resolved from the row, so a machine
//! that answers at the right address with the wrong key fails the handshake
//! and nothing else is tried.

use std::net::{SocketAddr, TcpStream};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use devboule_protocol::{ClientHello, ClientMessage, DaemonMessage, OwnerId};

use super::ServerState;
use crate::framing::Framed;
use crate::peer_transport::{
    initiator_handshake, is_tailnet_or_test_loopback, split_session, HANDSHAKE_DEADLINE,
    PEER_NOISE_PATTERN, PEER_PROLOGUE,
};

/// How long the dialer waits for TCP itself. A tailnet round trip is
/// milliseconds; five seconds absorbs a DERP-relayed cold path without
/// letting a dead address park the caller.
pub(crate) const DIAL_CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// How long the dialer waits for the one reply. One small request, one small
/// reply; the bound exists so a responder that accepts and goes silent cannot
/// hold the caller past the handshake.
pub(crate) const DIAL_REPLY_TIMEOUT: Duration = Duration::from_secs(10);

/// How many calls this daemon may have in flight toward the tailnet at once.
///
/// Inbound, one source is capped at `MAX_REMOTE_CONNECTIONS_PER_SOURCE` (4);
/// the dialer is held to the same number toward the whole tailnet. Dials are
/// one-shot request/reply calls, so four in flight is already generous, and
/// the cap is what stops a failing peer — or a failing row — from consuming
/// unbounded threads and sockets on the way out.
pub(crate) const MAX_OUTBOUND_CALLS: usize = 4;

/// The outbound connection budget, held on the daemon's state so it is
/// per-daemon exactly like the accept path's budgets.
#[derive(Default)]
pub(crate) struct DialSlots {
    live: Mutex<usize>,
}

impl DialSlots {
    /// Take one of the outbound slots, or refuse the dial: a dial past the
    /// cap is answered `busy` before any socket is opened.
    pub(crate) fn admit(&self) -> Result<DialSlotGuard<'_>, DialStep> {
        let mut live = self.live.lock().unwrap_or_else(|error| error.into_inner());
        if *live >= MAX_OUTBOUND_CALLS {
            return Err(DialStep::Busy);
        }
        *live += 1;
        Ok(DialSlotGuard { slots: self })
    }
}

/// One held outbound slot, released on drop — whatever the call's outcome.
pub(crate) struct DialSlotGuard<'a> {
    slots: &'a DialSlots,
}

impl Drop for DialSlotGuard<'_> {
    fn drop(&mut self) {
        let mut live = self
            .slots
            .live
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        *live = live.saturating_sub(1);
    }
}

/// The step a dial failed at, so an error names where it died.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DialStep {
    /// No live `peers` row carries this device, or the journal could not be
    /// read — the same answer either way: fail closed.
    RowMissing,
    /// The row exists but was revoked. Revocation is read at dial time, so a
    /// revoke that landed after the caller decided to dial still stops it.
    Revoked,
    /// This device's own identity is not available.
    Identity,
    /// The outbound budget is spent; the dial is refused without dialling.
    Busy,
    /// The stored address was not an `ip:port` at all.
    Address,
    /// TCP would not connect.
    Connect,
    /// The Noise exchange failed — including a far end whose static key does
    /// not match the pinned one. `initiator_handshake` enforces that pin;
    /// handing the key to snow does not, because `remote_public_key` only
    /// seeds the expected key and plain `XX` overwrites the seed with
    /// whatever the responder presents.
    Handshake,
    /// The far end refused or did not answer the versioned hello.
    Hello,
    /// The request did not go out.
    Send,
    /// No reply came back in time.
    Reply,
}

impl DialStep {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::RowMissing => "row_missing",
            Self::Revoked => "revoked",
            Self::Identity => "identity",
            Self::Busy => "busy",
            Self::Address => "address",
            Self::Connect => "connect",
            Self::Handshake => "handshake",
            Self::Hello => "hello",
            Self::Send => "send",
            Self::Reply => "reply",
        }
    }
}

/// Why a dial failed, with the step it failed at.
#[derive(Debug)]
pub struct DialError {
    step: DialStep,
    message: String,
}

impl DialError {
    fn at(step: DialStep, message: impl Into<String>) -> Self {
        Self {
            step,
            message: message.into(),
        }
    }

    /// The failed step, as the name an error line or a test can match on.
    pub fn step(&self) -> &'static str {
        self.step.as_str()
    }
}

impl std::fmt::Display for DialError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "peer dial failed at {}: {}",
            self.step.as_str(),
            self.message
        )
    }
}

impl std::error::Error for DialError {}

/// Dial a paired device, complete the same versioned hello every client of a
/// daemon speaks, send one `ClientMessage`, read the `DaemonMessage` that
/// comes back, and close.
///
/// `remote_static` is the key the caller's `peers` row pins — never a key
/// learned from the network. The prologue, pattern and absent PSK must stay
/// exactly what `serve_noise_peer` answers: the steady-state link is plain
/// Noise `XX`, and the pinned key is the whole of the authentication. The
/// hello's owner is not an authorization — the responder replaces it with the
/// identity the Noise handshake authenticated — so `hello` should say who the
/// dialer is (`peer_{device_id}`), and nothing rides on it.
pub fn dial_peer(
    static_private: &[u8; 32],
    remote_static: &[u8],
    address: &str,
    hello: &ClientHello,
    request: &ClientMessage,
) -> Result<DaemonMessage, DialError> {
    let framed = connect_and_handshake(static_private, remote_static, address)?;
    exchange(&framed, hello, request)
}

/// Everything up to and including the authentication, and nothing after it.
///
/// Split out so a caller can decide between the handshake and the first
/// application byte — the last moment at which refusing still discloses
/// nothing and causes nothing. `call_peer` re-reads the revocation there.
fn connect_and_handshake(
    static_private: &[u8; 32],
    remote_static: &[u8],
    address: &str,
) -> Result<Framed, DialError> {
    let address: SocketAddr = address.parse().map_err(|error| {
        DialError::at(
            DialStep::Address,
            format!("{address:?} is not an ip:port: {error}"),
        )
    })?;
    // The same footing the pairing initiator enforces: a stored row is data,
    // and data can be wrong or tampered with, so nothing outside the tailnet
    // is ever dialled — before any socket is opened.
    if !is_tailnet_or_test_loopback(&address.ip()) {
        return Err(DialError::at(
            DialStep::Address,
            "the address is not a tailnet address (100.64.0.0/10 or fd7a:115c:a1e0::/48)",
        ));
    }
    let stream = TcpStream::connect_timeout(&address, DIAL_CONNECT_TIMEOUT)
        .map_err(|error| DialError::at(DialStep::Connect, error.to_string()))?;
    let session = initiator_handshake(
        &stream,
        Instant::now() + HANDSHAKE_DEADLINE,
        static_private,
        Some(remote_static),
        PEER_PROLOGUE,
        None,
        PEER_NOISE_PATTERN,
    )
    // A far end presenting any other static key fails inside the handshake:
    // the pin lives there, so no caller can forget it. Nothing is retried.
    .map_err(|error| DialError::at(DialStep::Handshake, error.to_string()))?;
    let (reader, writer, closer) = split_session(&stream, session)
        .map_err(|error| DialError::at(DialStep::Handshake, error.to_string()))?;
    Ok(Framed::from_stream(reader, writer, closer))
}

/// The application half: the versioned hello every client speaks, then one
/// request and its reply. Both sends carry the dial's own deadline — without
/// one they fall back to the stream's 300 s idle bound, and a far end that
/// completes the handshake and then stops reading would park the caller (and
/// its outbound slot) for five minutes.
fn exchange(
    framed: &Framed,
    hello: &ClientHello,
    request: &ClientMessage,
) -> Result<DaemonMessage, DialError> {
    framed
        .send_until(
            &ClientMessage::Hello(hello.clone()),
            Instant::now() + DIAL_REPLY_TIMEOUT,
        )
        .map_err(|error| DialError::at(DialStep::Send, error.to_string()))?;
    match framed.recv_timeout::<DaemonMessage>(DIAL_REPLY_TIMEOUT) {
        Ok(DaemonMessage::Hello(_)) => {}
        Ok(DaemonMessage::Error(error)) => {
            return Err(DialError::at(DialStep::Hello, error.message))
        }
        Ok(other) => {
            return Err(DialError::at(
                DialStep::Hello,
                format!("expected hello, got {other:?}"),
            ))
        }
        Err(error) => return Err(DialError::at(DialStep::Hello, error.to_string())),
    }
    framed
        .send_until(request, Instant::now() + DIAL_REPLY_TIMEOUT)
        .map_err(|error| DialError::at(DialStep::Send, error.to_string()))?;
    framed
        .recv_timeout::<DaemonMessage>(DIAL_REPLY_TIMEOUT)
        .map_err(|error| DialError::at(DialStep::Reply, error.to_string()))
}

/// The state-aware dial: resolve the paired device's row at dial time, then
/// make the call.
///
/// This is the entry a production caller uses. The row is read from the
/// journal here — not from a snapshot taken earlier — so whatever it says
/// when the dial happens is what the dial obeys. The outbound budget is
/// spent first, so a dial past the cap is refused before anything is
/// resolved or opened.
pub fn call_peer(
    state: &Arc<ServerState>,
    device_id: &str,
    request: ClientMessage,
) -> Result<DaemonMessage, DialError> {
    // The slot is the dial's first act: a dial past the cap is refused before
    // it resolves a row or opens a socket, and the slot rides the whole call —
    // connect, handshake, request, reply — so the budget counts what is in
    // flight on the wire, not what merely started.
    let _slot = state
        .outbound_dials
        .admit()
        .map_err(|step| DialError::at(step, "the outbound budget is spent"))?;
    let row = state
        .peer_get(device_id)
        .map_err(|error| DialError::at(DialStep::RowMissing, error))?
        .ok_or_else(|| DialError::at(DialStep::RowMissing, "no peers row carries this device"))?;
    // A revoked peer is not dialable, full stop. The row was read above, at
    // dial time, so this is the table's answer now, not a cache's answer from
    // an earlier moment.
    if row.is_revoked() {
        return Err(DialError::at(
            DialStep::Revoked,
            "the peer's row was revoked",
        ));
    }
    let identity = state
        .device_identity()
        .as_ref()
        .map_err(|error| DialError::at(DialStep::Identity, error.to_string()))?;
    let hello = ClientHello::m3a(
        OwnerId::new(format!("peer_{}", identity.device_id), "daemon")
            .map_err(|error| DialError::at(DialStep::Identity, error))?,
        "devboule-daemon",
    );
    let framed = connect_and_handshake(identity.private_key(), &row.public_key, &row.address)?;
    // Ask again, now. Connecting and shaking hands can take fifteen seconds,
    // and a revoke that lands inside them would not be in the row read above.
    // The inbound side closes live connections on revoke; outbound keeps no
    // registry, so this is where an outbound call hears it. It is also the
    // last honest moment: after the hello the request has been disclosed and
    // acted upon, and a refusal afterwards would be theatre.
    match state.peer_get(device_id) {
        Ok(Some(fresh)) if !fresh.is_revoked() => {}
        Ok(_) => {
            return Err(DialError::at(
                DialStep::Revoked,
                "the peer was revoked while this dial was connecting",
            ))
        }
        Err(error) => return Err(DialError::at(DialStep::RowMissing, error)),
    }
    exchange(&framed, &hello, &request)
}

#[cfg(test)]
#[path = "peer_dial_tests.rs"]
mod tests;
