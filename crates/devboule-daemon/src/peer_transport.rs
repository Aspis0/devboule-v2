//! The peer transport: Noise over TCP, the accept loop, and its brakes.
//!
//! One listener per tailnet address, and every byte on it inside a Noise
//! session. The transport is a trait because a relay will be one later
//! (`DESIGN-remote-agents.md` §2 D2b); only the tailnet implementation exists
//! in this slice.
//!
//! # Wire framing on the peer transport
//!
//! Every unit — each Noise handshake message, each transport message, and each
//! pairing message — is `[u16 big-endian length][bytes]`. During the handshake
//! the bytes are the Noise handshake message; in transport mode they are the
//! AEAD ciphertext and the length prefix is in the clear (Noise puts it outside
//! the ciphertext). The receiver reads the length first and then exactly that
//! many bytes, so a short read is an error rather than a silent truncation.
//!
//! # Order of business for an accepted socket
//!
//! 1. caps (total, per source, per source per minute);
//! 2. `pre_noise_filter` **before any read**: a paired address goes to Noise, a
//!    pairing candidate only while a code is active, everything else is closed
//!    without a read;
//! 3. a pairing candidate is peeked for `DBP1` under a 2 s timeout;
//! 4. Noise `XX` responder under one 10 s wall-clock deadline, then the remote
//!    static must match a pinned, non-revoked `peers` row;
//! 5. `whois` must agree with the binding recorded at pairing;
//! 6. `handle_client` with the remote identity.
//!
//! Steps 1–3 are cheap pre-filters. Step 4 is the authentication: a source that
//! gets past 1–3 but fails 4 still consumed a handshake slot, which is why the
//! slot budgets are separate from the connection cap.

use std::collections::{HashMap, VecDeque};
use std::io::{self, Read, Write};
use std::net::{IpAddr, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::journal::{AuditRecord, PeerRecord};
use crate::peer_policy::{ConnPeer, PeerRole, TransportBinding};
use crate::server::{handle_client, ClientKind, ServerState};

/// The steady-state handshake. Separate from the pairing pattern by name *and*
/// by prologue, so the two transcripts cannot be confused (muse M6).
pub const PEER_PROLOGUE: &[u8] = b"devboule-peer-v1";
pub const PAIR_PROLOGUE: &[u8] = b"devboule-pair-v1";
pub const PEER_NOISE_PATTERN: &str = "Noise_XX_25519_ChaChaPoly_BLAKE2s";
pub const PAIR_NOISE_PATTERN: &str = "Noise_XXpsk3_25519_ChaChaPoly_BLAKE2s";
/// The pairing PSK's position in the pattern. `XXpsk3` puts it at the third
/// token, which is what `.psk(3, …)` names; the tests prove the parser agrees.
pub const PAIR_PSK_LOCATION: u8 = 3;
/// `DBP1` marks a pairing candidate on a connection that is not (yet) a peer.
pub const PAIRING_MAGIC: [u8; 4] = *b"DBP1";

/// Noise's own ceiling for one message (the length prefix is 16 bits).
pub const MAX_NOISE_MESSAGE: usize = 65535;
/// One ChaChaPoly tag, one plaintext flags byte.
pub const NOISE_TAG_LEN: usize = 16;
/// The largest payload one transport message can carry.
pub const NOISE_CHUNK: usize = MAX_NOISE_MESSAGE - NOISE_TAG_LEN - 1;
/// `bit0` of the plaintext flags byte: this message is the last chunk of the
/// frame. Bits 1–7 are reserved and must be zero; a non-zero reserved bit
/// closes the stream, which is what makes adding padding later a
/// plaintext-only change instead of a framing change.
const FLAG_LAST_CHUNK: u8 = 0x01;
const FLAG_RESERVED: u8 = 0xfe;

/// One wall-clock budget for the whole `XX` handshake, not per operation: three
/// messages at a per-operation timeout would allow roughly three times this.
pub const HANDSHAKE_DEADLINE: Duration = Duration::from_secs(10);
/// A pairing candidate may wait this long for its four magic bytes.
pub const PAIRING_PEEK_TIMEOUT: Duration = Duration::from_secs(2);
/// The accept loop's housekeeping period (pairing code expiry, parked
/// confirmations). Also bounds how long an idle accept waits.
pub const HOUSEKEEPING_TICK: Duration = Duration::from_secs(1);

pub const DEFAULT_PEER_PORT: u16 = 47831;
pub const PEER_PORT_ENV: &str = "DEVBOULE_PEER_PORT";

pub const MAX_REMOTE_CONNECTIONS: usize = 32;
pub const MAX_REMOTE_CONNECTIONS_PER_SOURCE: usize = 4;
pub const MAX_ACCEPTS_PER_SOURCE_PER_MINUTE: usize = 10;
pub const MAX_NOISE_HANDSHAKES_IN_FLIGHT: usize = 6;
/// Pairing has its own budget: an unpaired source must never be able to spend
/// a paired peer's handshake slot.
pub const MAX_PAIRING_HANDSHAKES_IN_FLIGHT: usize = 2;

/// Per-connection request rate limit for remote peers.
pub const RATE_SUSTAINED_PER_SEC: f64 = 20.0;
pub const RATE_BURST: f64 = 50.0;

/// The idle budget for one read on a remote connection, used when a caller does
/// not supply its own deadline.
///
/// A paired peer holds its connection open between requests, so this must not
/// be short; but it must exist, because a half-open peer (a machine that was
/// powered off without closing its sockets) otherwise holds one of
/// `MAX_REMOTE_CONNECTIONS` slots indefinitely. When it expires the read fails
/// and the connection is torn down, which is the same path a broken pipe takes.
pub const STREAM_IDLE_TIMEOUT: Duration = Duration::from_secs(300);

/// How long the accept loop waits for its per-connection threads on shutdown.
/// A parked pairing can hold a socket for up to `CONFIRM_WINDOW`; the daemon
/// must not wait that long to exit.
pub const TEARDOWN_BUDGET: Duration = Duration::from_secs(2);

#[derive(Debug)]
pub enum PeerError {
    Io(String),
    Noise(String),
    Protocol(String),
    Rejected(RejectReason),
    Binding(BindingError),
}

impl std::fmt::Display for PeerError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(message) => write!(formatter, "peer transport io: {message}"),
            Self::Noise(message) => write!(formatter, "noise: {message}"),
            Self::Protocol(message) => write!(formatter, "peer transport protocol: {message}"),
            Self::Rejected(reason) => write!(formatter, "connection rejected: {reason}"),
            Self::Binding(error) => write!(formatter, "binding check failed: {error}"),
        }
    }
}

impl std::error::Error for PeerError {}

impl From<io::Error> for PeerError {
    fn from(error: io::Error) -> Self {
        Self::Io(error.to_string())
    }
}

impl From<snow::Error> for PeerError {
    fn from(error: snow::Error) -> Self {
        Self::Noise(error.to_string())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RejectReason {
    /// The source is not inside a tailnet range.
    NotATailnetAddress,
    /// Not an address of a live `peers` row and no pairing is active.
    UnknownSource,
    /// A `peers` row exists for the address but has been revoked.
    Revoked,
    /// A cap or an in-flight budget is exhausted.
    Busy,
}

impl std::fmt::Display for RejectReason {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl RejectReason {
    /// The name an audit row carries, so a rejection is traceable without
    /// storing prose.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NotATailnetAddress => "not_a_tailnet_address",
            Self::UnknownSource => "unknown_source",
            Self::Revoked => "revoked",
            Self::Busy => "busy",
        }
    }
}

#[derive(Debug)]
pub enum BindingError {
    WhoisFailed(String),
    NotATailnetAddress,
    Mismatch,
}

impl std::fmt::Display for BindingError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::WhoisFailed(message) => write!(formatter, "whois failed: {message}"),
            Self::NotATailnetAddress => write!(formatter, "source is not a tailnet address"),
            Self::Mismatch => {
                write!(
                    formatter,
                    "whois stable id does not match the pinned binding"
                )
            }
        }
    }
}

impl std::error::Error for BindingError {}

// ---------------------------------------------------------------------------
// Length-framed messages (handshake and transport share this)
// ---------------------------------------------------------------------------

fn apply_deadline(stream: &TcpStream, deadline: Instant) -> io::Result<()> {
    let remaining = deadline.saturating_duration_since(Instant::now());
    if remaining.is_zero() {
        return Err(io::Error::new(
            io::ErrorKind::TimedOut,
            "peer handshake deadline elapsed",
        ));
    }
    stream.set_read_timeout(Some(remaining))?;
    stream.set_write_timeout(Some(remaining))?;
    Ok(())
}

/// The socket deadline is refreshed before every operation, so the budget is
/// wall-clock rather than per syscall. `Write` is implemented for
/// `&TcpStream`; the binding is `mut` because that is the receiver it needs.
pub fn write_framed(mut stream: &TcpStream, bytes: &[u8], deadline: Instant) -> io::Result<()> {
    if bytes.len() > MAX_NOISE_MESSAGE {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "peer message of {} bytes exceeds the Noise ceiling",
                bytes.len()
            ),
        ));
    }
    apply_deadline(stream, deadline)?;
    let length = (bytes.len() as u16).to_be_bytes();
    stream.write_all(&length)?;
    apply_deadline(stream, deadline)?;
    stream.write_all(bytes)
}

/// Read one `[u16 length][bytes]` unit into `buffer`, returning its length.
/// The length prefix is in the clear, so it is read before any decryption.
pub fn read_framed(
    mut stream: &TcpStream,
    buffer: &mut [u8],
    deadline: Instant,
) -> io::Result<usize> {
    apply_deadline(stream, deadline)?;
    let mut length = [0u8; 2];
    stream.read_exact(&mut length)?;
    let length = u16::from_be_bytes(length) as usize;
    if length > buffer.len() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "peer message of {length} bytes does not fit the {} byte buffer",
                buffer.len()
            ),
        ));
    }
    apply_deadline(stream, deadline)?;
    stream.read_exact(&mut buffer[..length])?;
    Ok(length)
}

// ---------------------------------------------------------------------------
// Noise session halves
// ---------------------------------------------------------------------------

/// The receiving half of a Noise session. Owns its own `TcpStream`, so a
/// blocking read holds no lock the writer needs.
pub struct NoiseReader {
    stream: TcpStream,
    state: Arc<Mutex<snow::TransportState>>,
    pending: Vec<u8>,
    offset: usize,
}

impl NoiseReader {
    pub fn new(stream: TcpStream, state: Arc<Mutex<snow::TransportState>>) -> Self {
        Self {
            stream,
            state,
            pending: Vec::new(),
            offset: 0,
        }
    }

    /// Plaintext bytes for the caller's frame buffer. `0` means end of stream.
    ///
    /// One call reads as many Noise messages as it takes to produce a byte or
    /// to see a real close: an empty message (a frame boundary or, later,
    /// padding) must not read as end-of-stream. The ciphertext is read
    /// **without** the state lock (it is just bytes), and the lock is taken
    /// only to decrypt, so a slow socket never blocks the sender.
    pub fn read_plaintext(
        &mut self,
        out: &mut [u8],
        deadline: Option<Instant>,
    ) -> io::Result<usize> {
        if out.is_empty() {
            return Ok(0);
        }
        let deadline = deadline.unwrap_or_else(|| Instant::now() + STREAM_IDLE_TIMEOUT);
        loop {
            if self.offset < self.pending.len() {
                let available = &self.pending[self.offset..];
                let take = available.len().min(out.len());
                out[..take].copy_from_slice(&available[..take]);
                self.offset += take;
                return Ok(take);
            }
            let mut ciphertext = [0u8; MAX_NOISE_MESSAGE];
            let length = read_framed(&self.stream, &mut ciphertext, deadline)?;
            if length < NOISE_TAG_LEN {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "noise message is shorter than its authentication tag",
                ));
            }
            let mut plaintext = vec![0u8; length];
            let read = {
                let mut state = self
                    .state
                    .lock()
                    .map_err(|_| io::Error::other("noise state lock poisoned"))?;
                state
                    .read_message(&ciphertext[..length], &mut plaintext)
                    .map_err(|error| {
                        io::Error::new(
                            io::ErrorKind::InvalidData,
                            format!("noise authentication failed: {error}"),
                        )
                    })?
            };
            plaintext.truncate(read);
            let flags = plaintext.first().copied().ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    "noise message carries no flags byte",
                )
            })?;
            if flags & FLAG_RESERVED != 0 {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "noise message reserved a flags bit that must be zero",
                ));
            }
            self.pending = plaintext[1..].to_vec();
            self.offset = 0;
        }
    }
}

/// The sending half of a Noise session. Owns its own `TcpStream`.
pub struct NoiseWriter {
    stream: TcpStream,
    state: Arc<Mutex<snow::TransportState>>,
}

impl NoiseWriter {
    pub fn new(stream: TcpStream, state: Arc<Mutex<snow::TransportState>>) -> Self {
        Self { stream, state }
    }

    /// Write one NDJSON frame: the bytes, then `\n`, chunked across Noise
    /// messages. The last chunk of the frame carries `FLAG_LAST_CHUNK`.
    pub fn write_frame(&mut self, frame: &[u8], deadline: Option<Instant>) -> io::Result<()> {
        let mut payload = Vec::with_capacity(frame.len() + 1);
        payload.extend_from_slice(frame);
        payload.push(b'\n');
        self.write_all(&payload, deadline)
    }

    /// Chunk `bytes` into Noise messages. An empty input still sends one empty
    /// message so the far side sees a frame boundary and not a stall.
    pub fn write_all(&mut self, bytes: &[u8], deadline: Option<Instant>) -> io::Result<()> {
        let deadline = deadline.unwrap_or_else(|| Instant::now() + STREAM_IDLE_TIMEOUT);
        let chunks = bytes.len().div_ceil(NOISE_CHUNK).max(1);
        for index in 0..chunks {
            let start = index * NOISE_CHUNK;
            let end = (start + NOISE_CHUNK).min(bytes.len());
            let last = index + 1 == chunks;
            let mut plaintext = Vec::with_capacity(1 + (end - start));
            plaintext.push(if last { FLAG_LAST_CHUNK } else { 0 });
            plaintext.extend_from_slice(&bytes[start..end]);

            let mut message = vec![0u8; plaintext.len() + NOISE_TAG_LEN];
            let written = {
                let mut state = self
                    .state
                    .lock()
                    .map_err(|_| io::Error::other("noise state lock poisoned"))?;
                state
                    .write_message(&plaintext, &mut message)
                    .map_err(|error| {
                        io::Error::new(
                            io::ErrorKind::InvalidData,
                            format!("noise write failed: {error}"),
                        )
                    })?
            };
            write_framed(&self.stream, &message[..written], deadline)?;
        }
        Ok(())
    }
}

/// Complete a Noise `XX` handshake as the responder.
///
/// The handshake messages use the same length framing as the transport, so the
/// whole peer link is one wire shape. `.prologue` is mandatory: it is what
/// separates the pairing transcript from the steady-state one beyond the
/// pattern name.
pub fn responder_handshake(
    stream: &TcpStream,
    deadline: Instant,
    static_private: &[u8],
    prologue: &[u8],
    psk: Option<&[u8; 32]>,
    pattern: &str,
) -> Result<snow::TransportState, PeerError> {
    let params = pattern
        .parse::<snow::params::NoiseParams>()
        .map_err(|error| PeerError::Noise(error.to_string()))?;
    let builder = snow::Builder::new(params).local_private_key(static_private)?;
    let builder = builder.prologue(prologue)?;
    let builder = match psk {
        Some(key) => builder.psk(PAIR_PSK_LOCATION, key)?,
        None => builder,
    };
    let mut handshake = builder.build_responder()?;
    run_handshake(stream, deadline, &mut handshake)?;
    handshake.into_transport_mode().map_err(PeerError::from)
}

/// Complete a Noise handshake as the initiator: the pairing exchange, and any
/// dial to a device already paired with.
///
/// `remote_static` is the key the caller demands the far end prove, and this
/// function ENFORCES it. Handing it to snow does not: `remote_public_key` only
/// seeds the expected key, and plain `XX` overwrites that seed with whatever
/// the responder actually presents — so a dial pinned to key A completes
/// happily against key B. Measured 2026-09-17, the first time any caller
/// passed `Some`. The check is here rather than at the call site because a
/// pin every caller has to remember is a pin the next caller will forget.
///
/// `None` means the caller has no key yet and authenticates by other means:
/// pairing derives a PSK from the spoken code, and the PSK is what fails when
/// the code is wrong.
pub fn initiator_handshake(
    stream: &TcpStream,
    deadline: Instant,
    static_private: &[u8],
    remote_static: Option<&[u8]>,
    prologue: &[u8],
    psk: Option<&[u8; 32]>,
    pattern: &str,
) -> Result<snow::TransportState, PeerError> {
    let params = pattern
        .parse::<snow::params::NoiseParams>()
        .map_err(|error| PeerError::Noise(error.to_string()))?;
    let builder = snow::Builder::new(params).local_private_key(static_private)?;
    let builder = match remote_static {
        Some(key) => builder.remote_public_key(key)?,
        None => builder,
    };
    let builder = builder.prologue(prologue)?;
    let builder = match psk {
        Some(key) => builder.psk(PAIR_PSK_LOCATION, key)?,
        None => builder,
    };
    let mut handshake = builder.build_initiator()?;
    run_handshake(stream, deadline, &mut handshake)?;
    if let Some(expected) = remote_static {
        // After the exchange, not before: only now does snow know what the far
        // end presented. No fallback — a pinned dial has nothing to fall back to.
        if handshake.get_remote_static() != Some(expected) {
            return Err(PeerError::Noise(
                "the far end's static key does not match the pinned key".to_string(),
            ));
        }
    }
    handshake.into_transport_mode().map_err(PeerError::from)
}

fn run_handshake(
    stream: &TcpStream,
    deadline: Instant,
    handshake: &mut snow::HandshakeState,
) -> Result<(), PeerError> {
    let mut message = [0u8; MAX_NOISE_MESSAGE];
    let mut plaintext = [0u8; MAX_NOISE_MESSAGE];
    while !handshake.is_handshake_finished() {
        if handshake.is_my_turn() {
            let written = handshake.write_message(&[], &mut message)?;
            write_framed(stream, &message[..written], deadline)?;
        } else {
            let length = read_framed(stream, &mut message, deadline)?;
            handshake.read_message(&message[..length], &mut plaintext)?;
        }
    }
    Ok(())
}

/// Split one completed session into a reader and a writer. Each side gets its
/// own `TcpStream` so neither direction can block the other.
pub fn split_session(
    stream: &TcpStream,
    state: snow::TransportState,
) -> Result<(NoiseReader, NoiseWriter, TcpStream), PeerError> {
    let reader_stream = stream.try_clone()?;
    let writer_stream = stream.try_clone()?;
    // A third handle, held outside both halves' mutexes, purely so a cancelling
    // thread can shut the socket down without waiting for the reader.
    let closer = stream.try_clone()?;
    let shared = Arc::new(Mutex::new(state));
    Ok((
        NoiseReader::new(reader_stream, Arc::clone(&shared)),
        NoiseWriter::new(writer_stream, shared),
        closer,
    ))
}

// ---------------------------------------------------------------------------
// The peers snapshot the accept loop consults
// ---------------------------------------------------------------------------

/// The non-revoked rows of `peers`, loaded once per accepted connection.
///
/// Addresses and pinned keys both come from here: the address decides which
/// path a connection takes *before any byte is read*, and the pinned key
/// decides whether the Noise handshake authenticated anybody.
#[derive(Clone, Debug, Default)]
pub struct PeerTable {
    rows: Vec<PeerRecord>,
}

impl PeerTable {
    pub fn load(state: &ServerState) -> Result<Self, String> {
        Ok(Self {
            rows: state.peers()?,
        })
    }

    #[cfg(test)]
    pub fn from_rows(rows: Vec<PeerRecord>) -> Self {
        Self { rows }
    }

    #[cfg(test)]
    pub fn rows(&self) -> &[PeerRecord] {
        &self.rows
    }

    /// The **live** row that owns `address`, if any. Numeric comparison, so
    /// `100.64.0.1` and `100.64.0.10` cannot match each other.
    pub fn by_address(&self, address: &IpAddr) -> Option<&PeerRecord> {
        self.rows
            .iter()
            .filter(|row| !row.is_revoked())
            .find(|row| row.owns_address(address))
    }

    /// Whether `address` belongs to a peer that was revoked. Kept distinct from
    /// `UnknownSource` because the two mean different things in the trail: a
    /// revoked row is a device this daemon used to trust, an unknown one is a
    /// stranger.
    pub fn revoked_address(&self, address: &IpAddr) -> bool {
        self.rows
            .iter()
            .filter(|row| row.is_revoked())
            .any(|row| row.owns_address(address))
    }

    /// The **live** row whose pinned public key is `key`. A revoked peer's key
    /// must not authenticate, even though its row is still on disk.
    pub fn by_static_key(&self, key: &[u8]) -> Option<&PeerRecord> {
        self.rows
            .iter()
            .filter(|row| !row.is_revoked())
            .find(|row| row.public_key.as_slice() == key)
    }
}

/// A transport that accepts loopback and pins a fixed binding, so S5, S6 and
/// the accept-loop tests can be exercised with no Tailscale. Test-only: the
/// production transport is [`Tailnet`].
#[cfg(test)]
pub struct TestTransport {
    pub stable_id: String,
    pub node_name: String,
}

#[cfg(test)]
impl Default for TestTransport {
    fn default() -> Self {
        Self {
            stable_id: "nstable".to_string(),
            node_name: "host.tailnet.ts.net.".to_string(),
        }
    }
}

#[cfg(test)]
impl PeerTransport for TestTransport {
    fn listen(
        &self,
        _paths: &crate::paths::RuntimePaths,
        stop: Arc<AtomicBool>,
    ) -> io::Result<PeerListener> {
        Tailnet::bind_peer_listener(&["127.0.0.1".parse().expect("ip")], 0, stop)
    }

    fn pre_noise_filter(&self, _peer: &SocketAddr, _peers: &PeerTable) -> Result<(), RejectReason> {
        Ok(())
    }

    fn binding(&self, _peer: &SocketAddr) -> Result<TransportBinding, BindingError> {
        Ok(TransportBinding::tailnet(
            self.stable_id.clone(),
            self.node_name.clone(),
            "user@example.com".to_string(),
        ))
    }
}

/// Whether `address` is a tailnet address, or loopback in this crate's own unit
/// tests.
///
/// The single place the test-only loopback allowance lives. Two callers depend on
/// it: the pairing-target check in `pairing.rs` (the in-process responder listens
/// on `127.0.0.1`) and the pairing-candidate check in the accept path. Nothing in
/// a production build admits loopback: `Tailnet` binds only tailnet addresses and
/// the address a pairing initiator dials comes from `SelfInfo.addresses`.
pub fn is_tailnet_or_test_loopback(address: &IpAddr) -> bool {
    if is_tailnet_address(address) {
        return true;
    }
    #[cfg(test)]
    if address.is_loopback() {
        return true;
    }
    false
}

// ---------------------------------------------------------------------------
// The transport trait
// ---------------------------------------------------------------------------

/// One bound peer listener. Not the pipe `Listener`: that trait's associated
/// stream is a `File`, and forcing a `TcpStream` through it would either lie
/// about the type or push the pipe path onto a trait object.
pub struct PeerListener {
    listeners: Vec<TcpListener>,
    addrs: Vec<SocketAddr>,
    stop: Arc<AtomicBool>,
}

impl PeerListener {
    pub fn addrs(&self) -> &[SocketAddr] {
        &self.addrs
    }

    /// Non-blocking accept across every bound address. `WouldBlock` means
    /// nothing is pending on any of them.
    ///
    /// The accepted stream is returned **blocking**, whatever the listener's
    /// mode is.
    pub fn accept(&mut self) -> io::Result<(TcpStream, SocketAddr)> {
        if self.stop.load(Ordering::SeqCst) {
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "peer listener is shutting down",
            ));
        }
        for listener in &self.listeners {
            match listener.accept() {
                Ok((stream, addr)) => {
                    // Windows: an accepted socket **inherits** the listening
                    // socket's non-blocking mode, so with the polling accept
                    // above every downstream read and write would fail with
                    // `WSAEWOULDBLOCK` instead of blocking: the Noise
                    // handshake, the pairing exchange and the whole connection
                    // loop. Setting it back here is the one place that can be
                    // correct for all of them, and it is also why the
                    // non-blocking listener is not observable outside this
                    // method.
                    stream.set_nonblocking(false)?;
                    return Ok((stream, addr));
                }
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => continue,
                Err(error) => return Err(error),
            }
        }
        Err(io::Error::new(io::ErrorKind::WouldBlock, "no pending peer"))
    }

    pub fn shutdown(&self) {
        self.stop.store(true, Ordering::SeqCst);
        // Closing the socket is what unblocks a parked accept.
        for listener in &self.listeners {
            let _ = listener.set_nonblocking(true);
        }
    }

    /// Whether [`PeerListener::shutdown`] has been called. The accept loop
    /// polls this as well as the daemon's own stop flag, so a caller that only
    /// has the listener (a test, or a future shutdown path) can still stop it.
    pub fn is_stopped(&self) -> bool {
        self.stop.load(Ordering::SeqCst)
    }

    /// The stop flag itself, for a caller that wants to raise it directly.
    /// Test-only: `run_windows` raises the daemon's own flag, which the accept
    /// loop polls alongside this one.
    #[cfg(test)]
    pub fn stop_handle(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.stop)
    }
}

impl Drop for PeerListener {
    fn drop(&mut self) {
        self.shutdown();
    }
}

pub trait PeerTransport: Send + Sync {
    /// Bind the listener. `Err` means no listener at all (Tailscale absent, no
    /// tailnet address), and the caller reports that as the remote state.
    fn listen(
        &self,
        paths: &crate::paths::RuntimePaths,
        stop: Arc<AtomicBool>,
    ) -> io::Result<PeerListener>;

    /// Step 2, before any read from the socket.
    fn pre_noise_filter(&self, peer: &SocketAddr, peers: &PeerTable) -> Result<(), RejectReason>;

    /// Step 5, after the static key authenticated the peer.
    fn binding(&self, peer: &SocketAddr) -> Result<TransportBinding, BindingError>;
}

/// The tailnet transport: addresses from Tailscale's own `status`, binding from
/// `whois`.
pub struct Tailnet;

impl Tailnet {
    /// Bind every tailnet address at `port`. Unspecified addresses are
    /// refused: the design forbids listening on `0.0.0.0`/`::`.
    pub fn bind_peer_listener(
        addresses: &[IpAddr],
        port: u16,
        stop: Arc<AtomicBool>,
    ) -> io::Result<PeerListener> {
        let mut listeners = Vec::new();
        let mut addrs = Vec::new();
        for address in addresses {
            if address.is_unspecified() {
                continue;
            }
            let socket = SocketAddr::new(*address, port);
            match TcpListener::bind(socket) {
                Ok(listener) => {
                    listener.set_nonblocking(true)?;
                    // The **bound** address, not the requested one: with port 0
                    // the kernel picks a port, and reporting the request would
                    // advertise `ip:0` to the panel and to every test.
                    let local = listener.local_addr()?;
                    listeners.push(listener);
                    addrs.push(local);
                }
                Err(error) => {
                    // One unreachable address must not remove the others; a
                    // single tailnet address is the normal case anyway.
                    if listeners.is_empty() {
                        return Err(error);
                    }
                }
            }
        }
        if listeners.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::AddrNotAvailable,
                "no tailnet address could be bound",
            ));
        }
        Ok(PeerListener {
            listeners,
            addrs,
            stop,
        })
    }
}

impl PeerTransport for Tailnet {
    fn listen(
        &self,
        paths: &crate::paths::RuntimePaths,
        stop: Arc<AtomicBool>,
    ) -> io::Result<PeerListener> {
        let client = crate::tailscale_localapi::LocalApiClient::new();
        // `_fresh`, not the cached answer: this runs once at start-up and must
        // see whether Tailscale is up *now*. A cached `Absent` from an earlier
        // probe (say, `status` asked a moment ago) would otherwise keep the
        // listener down for the rest of the cache TTL on a machine whose
        // Tailscale is running.
        let node = client
            .self_node_fresh()
            .map_err(|error| io::Error::other(error.to_string()))?;
        let port = peer_port();
        let listener = Self::bind_peer_listener(&node.addresses, port, stop)?;
        let _ = paths;
        Ok(listener)
    }

    fn pre_noise_filter(&self, peer: &SocketAddr, peers: &PeerTable) -> Result<(), RejectReason> {
        // Order matters for the trail: a revoked address is a device this
        // daemon used to trust, an out-of-range address is not a tailnet source
        // at all, and only then is anything else a stranger.
        if peers.by_address(&peer.ip()).is_some() {
            return Ok(());
        }
        if peers.revoked_address(&peer.ip()) {
            return Err(RejectReason::Revoked);
        }
        if !is_tailnet_address(&peer.ip()) {
            return Err(RejectReason::NotATailnetAddress);
        }
        Err(RejectReason::UnknownSource)
    }

    fn binding(&self, peer: &SocketAddr) -> Result<TransportBinding, BindingError> {
        if !is_tailnet_address(&peer.ip()) {
            return Err(BindingError::NotATailnetAddress);
        }
        let client = crate::tailscale_localapi::LocalApiClient::new();
        let whois = client
            .whois(*peer)
            .map_err(|error| BindingError::WhoisFailed(error.to_string()))?;
        if whois.stable_id.is_empty() {
            return Err(BindingError::Mismatch);
        }
        Ok(TransportBinding::tailnet(
            whois.stable_id,
            whois.node_name,
            whois.login_name,
        ))
    }
}

/// The text form of a peer address. `SocketAddr` brackets IPv6 by
/// construction; hand-composing `format!("{ip}:{port}")` does not, and
/// `fd7a:115c:a1e0::1:47831` is not an address at all — or worse, parses as a
/// different one where an `IpAddr` is tried first.
pub fn compose_peer_address(ip: IpAddr, port: u16) -> String {
    SocketAddr::new(ip, port).to_string()
}

/// Whether `address` is inside a tailnet range: Tailscale allocates from
/// `100.64.0.0/10` for IPv4 and `fd7a:115c:a1e0::/48` for IPv6.
///
/// This is a pre-filter, not authentication: the source address is trivially
/// spoofable on a shared segment, and the pinned Noise static key is what
/// actually authenticates. It exists so a non-tailnet source is refused before
/// any byte is read (design §7 condition 1).
pub fn is_tailnet_address(address: &IpAddr) -> bool {
    match address {
        IpAddr::V4(v4) => {
            let octets = v4.octets();
            octets[0] == 100 && (64..128).contains(&octets[1])
        }
        IpAddr::V6(v6) => {
            let octets = v6.octets();
            octets[..6] == [0xfd, 0x7a, 0x11, 0x5c, 0xa1, 0xe0]
        }
    }
}

// A relay transport is designed (`DESIGN-remote-agents.md` §2 D2b: raw TCP on
// the tailnet now, a WebSocket through Cloudflare later) and is **not built**.
// It is deliberately not present here as a stub type: a struct that nothing
// constructs and every method of which returns `Unsupported` is dead code that
// `-D warnings` cannot see through once it carries an allow, and the extension
// point that matters is this trait, which `Tailnet` implements and which a
// future `Relay` would implement the same way. The design's normative content
// for the relay — the binding becomes the relay account and the pinned Noise
// static keys remain the only authentication (§8 R10) — belongs with the slice
// that writes it.

pub fn peer_port() -> u16 {
    peer_port_from(std::env::var(PEER_PORT_ENV).ok().as_deref())
}

/// The parsing rule for `DEVBOULE_PEER_PORT`, split out so it can be tested
/// **without mutating the process environment**.
///
/// The test used to `set_var`/`remove_var` a process-global, which is a race
/// against every other test that reads it: `peer_port()` is on the listener's
/// path, so any future test that starts a listener would have seen a port it did
/// not ask for. A pure function removes the race instead of serialising around
/// it.
///
/// A missing value, a value that is not a number, and `0` all mean "use the
/// default": zero is not a port a listener can bind meaningfully, and silently
/// binding an arbitrary ephemeral port would make the address the panel shows
/// unreproducible.
fn peer_port_from(value: Option<&str>) -> u16 {
    value
        .and_then(|value| value.trim().parse::<u16>().ok())
        .filter(|port| *port != 0)
        .unwrap_or(DEFAULT_PEER_PORT)
}

// ---------------------------------------------------------------------------
// Caps and budgets
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HandshakeKind {
    Noise,
    Pairing,
}

#[derive(Default)]
struct SourceState {
    live: usize,
    accepted: VecDeque<Instant>,
}

#[derive(Default)]
struct CapsState {
    total: usize,
    per_source: HashMap<IpAddr, SourceState>,
    noise_in_flight: usize,
    pairing_in_flight: usize,
}

/// The accept loop's counters. One mutex: the operations are a handful of
/// integer comparisons, and the accept path is nowhere near the hot path.
#[derive(Default)]
pub struct AcceptCaps {
    inner: Mutex<CapsState>,
    admitted: AtomicUsize,
}

impl AcceptCaps {
    /// Step 1: total cap, per-source cap, per-source churn cap. Returns a guard
    /// that releases the per-source slot on drop.
    pub fn admit_source(
        self: &Arc<Self>,
        ip: IpAddr,
        now: Instant,
    ) -> Result<AcceptGuard, RejectReason> {
        let mut state = self.inner.lock().unwrap_or_else(|error| error.into_inner());
        if state.total >= MAX_REMOTE_CONNECTIONS {
            return Err(RejectReason::Busy);
        }
        let source = state.per_source.entry(ip).or_default();
        if source.live >= MAX_REMOTE_CONNECTIONS_PER_SOURCE {
            return Err(RejectReason::Busy);
        }
        while let Some(oldest) = source.accepted.front() {
            if now.saturating_duration_since(*oldest) > Duration::from_secs(60) {
                source.accepted.pop_front();
            } else {
                break;
            }
        }
        if source.accepted.len() >= MAX_ACCEPTS_PER_SOURCE_PER_MINUTE {
            return Err(RejectReason::Busy);
        }
        source.live += 1;
        source.accepted.push_back(now);
        state.total += 1;
        self.admitted.fetch_add(1, Ordering::Relaxed);
        Ok(AcceptGuard {
            caps: Arc::clone(self),
            ip,
        })
    }

    /// Steps 3 and 4: the two in-flight budgets, counted separately so an
    /// unpaired source can never spend a paired peer's slot.
    pub fn admit_handshake(
        self: &Arc<Self>,
        kind: HandshakeKind,
    ) -> Result<HandshakeGuard, RejectReason> {
        let mut state = self.inner.lock().unwrap_or_else(|error| error.into_inner());
        match kind {
            HandshakeKind::Noise => {
                if state.noise_in_flight >= MAX_NOISE_HANDSHAKES_IN_FLIGHT {
                    return Err(RejectReason::Busy);
                }
                state.noise_in_flight += 1;
            }
            HandshakeKind::Pairing => {
                if state.pairing_in_flight >= MAX_PAIRING_HANDSHAKES_IN_FLIGHT {
                    return Err(RejectReason::Busy);
                }
                state.pairing_in_flight += 1;
            }
        }
        Ok(HandshakeGuard {
            caps: Arc::clone(self),
            kind,
        })
    }

    fn release_source(&self, ip: IpAddr) {
        let mut state = self.inner.lock().unwrap_or_else(|error| error.into_inner());
        state.total = state.total.saturating_sub(1);
        if let Some(source) = state.per_source.get_mut(&ip) {
            source.live = source.live.saturating_sub(1);
            if source.live == 0 && source.accepted.is_empty() {
                state.per_source.remove(&ip);
            }
        }
    }

    fn release_handshake(&self, kind: HandshakeKind) {
        let mut state = self.inner.lock().unwrap_or_else(|error| error.into_inner());
        match kind {
            HandshakeKind::Noise => state.noise_in_flight = state.noise_in_flight.saturating_sub(1),
            HandshakeKind::Pairing => {
                state.pairing_in_flight = state.pairing_in_flight.saturating_sub(1)
            }
        }
    }
}

pub struct AcceptGuard {
    caps: Arc<AcceptCaps>,
    ip: IpAddr,
}

impl Drop for AcceptGuard {
    fn drop(&mut self) {
        self.caps.release_source(self.ip);
    }
}

pub struct HandshakeGuard {
    caps: Arc<AcceptCaps>,
    kind: HandshakeKind,
}

impl Drop for HandshakeGuard {
    fn drop(&mut self) {
        self.caps.release_handshake(self.kind);
    }
}

/// Token bucket for one remote connection's requests (muse M1). `Local`
/// connections are never limited.
pub struct TokenBucket {
    tokens: f64,
    last: Instant,
}

impl TokenBucket {
    pub fn new(now: Instant) -> Self {
        Self {
            tokens: RATE_BURST,
            last: now,
        }
    }

    pub fn take(&mut self, now: Instant) -> bool {
        let elapsed = now.saturating_duration_since(self.last).as_secs_f64();
        self.last = now;
        self.tokens = (self.tokens + elapsed * RATE_SUSTAINED_PER_SEC).min(RATE_BURST);
        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            true
        } else {
            false
        }
    }
}

// ---------------------------------------------------------------------------
// Pairing seam
// ---------------------------------------------------------------------------

/// What the accept loop needs from pairing (S6). Kept behind a trait so the
/// transport compiles and is tested with no pairing at all, and so S6 adds the
/// state machine without changing this file's accept order.
pub trait PairingHook: Send + Sync {
    /// Whether a code is active. This is the whole of step 2's second branch:
    /// with no active code a non-peer source is closed before any read.
    fn is_active(&self) -> bool;

    /// Expire codes and park deadlines. Called once per accept-loop tick.
    fn housekeeping(&self, now: Instant);

    /// Step 3's pairing path. Runs on its own thread; the socket is owned here.
    ///
    /// `in_flight` is the pairing handshake slot the accept loop took for this
    /// connection. The implementation drops it as soon as the exchange stops
    /// being a handshake — which for a parked `Client` pairing is *before* it
    /// waits for the local confirmation, so that two parked pairings cannot
    /// hold the budget for a minute and turn a third candidate into a dropped
    /// connection instead of the ready answer the panel can show.
    fn handle(
        &self,
        transport: &dyn PeerTransport,
        stream: TcpStream,
        peer_addr: SocketAddr,
        state: &Arc<ServerState>,
        in_flight: HandshakeGuard,
    );
}

/// The hook used by this module's accept-loop test, which does not pair. The
/// listener path still works with it: with no active code, a source that is not
/// a peer is closed before any read.
#[cfg(test)]
pub struct PairingDisabled;

#[cfg(test)]
impl PairingHook for PairingDisabled {
    fn is_active(&self) -> bool {
        false
    }

    fn housekeeping(&self, _now: Instant) {}

    fn handle(
        &self,
        _transport: &dyn PeerTransport,
        stream: TcpStream,
        _peer_addr: SocketAddr,
        _state: &Arc<ServerState>,
        _in_flight: HandshakeGuard,
    ) {
        let _ = stream.shutdown(std::net::Shutdown::Both);
    }
}

// ---------------------------------------------------------------------------
// The accept loop
// ---------------------------------------------------------------------------

/// The `daemon-peer-accept` thread. Polls the listener non-blocking and uses
/// the idle tick for pairing housekeeping, so one loop serves both.
///
/// Runs `handle_client` for an authenticated peer on its own thread, exactly
/// as the pipe accept loop does: one connection never blocks the next.
pub fn accept_peers(
    mut listener: PeerListener,
    transport: Arc<dyn PeerTransport>,
    state: Arc<ServerState>,
    pairing: Arc<dyn PairingHook>,
) {
    let caps = Arc::new(AcceptCaps::default());
    let mut threads: Vec<std::thread::JoinHandle<()>> = Vec::new();
    loop {
        // Two stop conditions, both cheap: the daemon's own flag (the normal
        // shutdown path) and the listener's (a caller that only holds the
        // listener). Without the second, a test that stops only the listener
        // would spin here forever.
        if state.stop_flag().load(Ordering::SeqCst) || listener.is_stopped() {
            break;
        }
        pairing.housekeeping(Instant::now());
        let accepted = listener.accept();
        let (stream, peer_addr) = match accepted {
            Ok(accepted) => accepted,
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                // Finished per-connection threads are reaped here, so a daemon
                // under connection churn cannot accumulate join handles for its
                // whole uptime.
                threads.retain(|handle| !handle.is_finished());
                std::thread::sleep(HOUSEKEEPING_TICK);
                continue;
            }
            Err(_) if state.stop_flag().load(Ordering::SeqCst) || listener.is_stopped() => break,
            Err(_) => {
                std::thread::sleep(HOUSEKEEPING_TICK);
                continue;
            }
        };

        // Step 1. Still on the accept thread: these caps are what keeps a
        // connect flood from consuming threads at all, so they must run before
        // anything is spawned.
        let guard = match caps.admit_source(peer_addr.ip(), Instant::now()) {
            Ok(guard) => guard,
            Err(reason) => {
                let _ = stream.shutdown(std::net::Shutdown::Both);
                let _ = reason;
                continue;
            }
        };

        // Everything from here on runs on the connection's own thread: the
        // peer-table read, the pre-Noise filter, the `DBP1` peek, and the
        // decision about which handshake budget to spend. None of it can block
        // the accept loop, which is the point (M1): a candidate that connects
        // and never writes used to park this thread inside `peek_magic` for the
        // whole peek budget, delaying every other accept behind it.
        let transport_for_task = Arc::clone(&transport);
        let state_for_task = Arc::clone(&state);
        let pairing_for_task = Arc::clone(&pairing);
        let caps_for_task = Arc::clone(&caps);
        if let Ok(handle) = std::thread::Builder::new()
            .name("daemon-peer-connection".into())
            .spawn(move || {
                let _guard = guard;
                dispatch_peer_connection(
                    transport_for_task.as_ref(),
                    &caps_for_task,
                    pairing_for_task.as_ref(),
                    stream,
                    peer_addr,
                    &state_for_task,
                );
            })
        {
            threads.push(handle);
        }
        // A failed spawn drops the guard with the closure, so the per-source
        // slot is released either way.
        continue;
    }
    // Bounded teardown. A parked pairing can wait up to `CONFIRM_WINDOW` for a
    // local answer, so joining every connection thread without a bound would
    // make daemon shutdown wait a minute on an idle pairing screen. Threads
    // still running when the budget expires are left to finish on their own;
    // they hold no state the process needs to release.
    let deadline = Instant::now() + TEARDOWN_BUDGET;
    for handle in threads {
        while !handle.is_finished() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        if handle.is_finished() {
            let _ = handle.join();
        }
    }
}

/// Steps 2–6 for one accepted connection, on its own thread.
///
/// The order is the design's (§7 condition 1, §8 R4) and is deliberately
/// unchanged from when it ran inline; only the thread it runs on has moved:
///
/// 2. the peer table decides whether this source is a peer or a pairing
///    candidate, **before any read**;
/// 3. only a candidate is peeked for `DBP1`;
/// 4. the matching in-flight handshake budget is spent;
/// 5. an authenticated peer is served, or the pairing exchange is run.
///
/// Anything that is neither a peer nor a candidate is closed here, with no
/// read, no wait and no audit row.
fn dispatch_peer_connection(
    transport: &dyn PeerTransport,
    caps: &Arc<AcceptCaps>,
    pairing: &dyn PairingHook,
    stream: TcpStream,
    peer_addr: SocketAddr,
    state: &Arc<ServerState>,
) {
    // Step 2. The table is cached in the daemon (M1), so this is a mutex read
    // rather than a journal round trip per accepted socket.
    let peers = match state.peer_table() {
        Ok(peers) => peers,
        Err(_) => {
            let _ = stream.shutdown(std::net::Shutdown::Both);
            return;
        }
    };

    if transport.pre_noise_filter(&peer_addr, &peers).is_ok() {
        let guard_handshake = match caps.admit_handshake(HandshakeKind::Noise) {
            Ok(slot) => slot,
            Err(_) => {
                let _ = stream.shutdown(std::net::Shutdown::Both);
                return;
            }
        };
        if let Err(error) = serve_noise_peer(transport, stream, peer_addr, &peers, state) {
            // No identity in the line: it is attacker-adjacent input on this
            // path.
            eprintln!("daemon peer connection ended: {error}");
        }
        drop(guard_handshake);
        return;
    }

    if pairing.is_active() {
        // Step 2b: a pairing candidate must be a tailnet source too (C11).
        //
        // Without this, an off-tailnet source was *peeked* while a code was
        // active — up to four bytes read and, if it said `DBP1`, a full
        // SPAKE2+Noise exchange — which is exactly what design §7 condition 1
        // rules out ("refused before any byte is read"). The gate is here, at
        // the head of the branch, so the peek below can never be reached from an
        // address that the design says must be refused unread.
        if !is_tailnet_or_test_loopback(&peer_addr.ip()) {
            let _ = stream.shutdown(std::net::Shutdown::Both);
            return;
        }
        // Step 3: only a pairing candidate is ever peeked.
        let is_candidate = matches!(
            peek_magic(&stream, Instant::now() + PAIRING_PEEK_TIMEOUT),
            Ok(magic) if magic == PAIRING_MAGIC
        );
        if !is_candidate {
            let _ = stream.shutdown(std::net::Shutdown::Both);
            return;
        }
        // The pairing budget is its own: an unpaired source must never be able
        // to spend a paired peer's Noise slot, and vice versa. It is taken here,
        // after the peek proved this really is a candidate, so a random byte
        // cannot occupy a slot either.
        let guard_handshake = match caps.admit_handshake(HandshakeKind::Pairing) {
            Ok(slot) => slot,
            Err(_) => {
                let _ = stream.shutdown(std::net::Shutdown::Both);
                return;
            }
        };
        pairing.handle(transport, stream, peer_addr, state, guard_handshake);
        return;
    }

    // Neither paired nor a candidate: close at once, no read, no wait, and no
    // audit row. An unpaired source must not be able to write one.
    let _ = stream.shutdown(std::net::Shutdown::Both);
}

/// `peek` the four magic bytes without consuming them, under `deadline`.
///
/// The deadline is armed here rather than by the caller so this can never be
/// reached with an unbounded socket: a source that connects and then says
/// nothing must not park the accept loop.
fn peek_magic(stream: &TcpStream, deadline: Instant) -> io::Result<[u8; 4]> {
    let mut magic = [0u8; 4];
    let mut filled = 0;
    while filled < magic.len() {
        // Re-armed every iteration, so a source that dribbles one byte at a
        // time still cannot hold this past `deadline`.
        apply_deadline(stream, deadline)?;
        match stream.peek(&mut magic[filled..]) {
            Ok(0) => {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "peer closed before the pairing magic",
                ))
            }
            Ok(read) => filled += read,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        }
    }
    Ok(magic)
}

/// Steps 4–6 for one connection whose source address belongs to a peer.
fn serve_noise_peer(
    transport: &dyn PeerTransport,
    stream: TcpStream,
    peer_addr: SocketAddr,
    peers: &PeerTable,
    state: &Arc<ServerState>,
) -> Result<(), PeerError> {
    let identity = match state.device_identity() {
        Ok(identity) => Arc::clone(identity),
        Err(error) => return Err(PeerError::Protocol(error.to_string())),
    };

    // Step 4: one wall-clock budget for the whole XX exchange.
    let deadline = Instant::now() + HANDSHAKE_DEADLINE;
    let session = responder_handshake(
        &stream,
        deadline,
        identity.private_key(),
        PEER_PROLOGUE,
        None,
        PEER_NOISE_PATTERN,
    )?;
    let remote_static = session
        .get_remote_static()
        .ok_or_else(|| PeerError::Protocol("noise session has no remote static key".to_string()))?
        .to_vec();
    // This is the authentication. A source that gets here without a pinned key
    // is closed, and it never wrote an audit row to get there.
    let row = peers
        .by_static_key(&remote_static)
        .ok_or(PeerError::Rejected(RejectReason::UnknownSource))?
        .clone();

    // Step 5: the network binding must agree with what pairing recorded.
    let binding = transport.binding(&peer_addr).map_err(PeerError::Binding)?;
    if binding.kind != row.binding_kind
        || row.binding_stable_id.as_deref() != Some(binding.stable_id.as_str())
    {
        let role = PeerRole::parse(&row.role).unwrap_or(PeerRole::Daemon);
        state.audit(AuditRecord {
            device_id: row.device_id.clone(),
            role: role.as_str().to_string(),
            claimed_origin: None,
            action: "PeerConnect".to_string(),
            session_id: None,
            outcome: "binding_mismatch".to_string(),
        });
        return Err(PeerError::Binding(BindingError::Mismatch));
    }

    let role = PeerRole::parse(&row.role)
        .ok_or_else(|| PeerError::Protocol(format!("peer role {:?} is not known", row.role)))?;
    let conn_peer = ConnPeer::Remote {
        device_id: row.device_id.clone(),
        role,
        paired_by_user: row.paired_by_user.clone(),
        binding: binding.clone(),
    };
    let (reader, writer, closer) = split_session(&stream, session)?;
    // The same admission the pipe accept loop takes, and for the same reason:
    // a connected peer must keep the daemon up, or the idle exit the app arms
    // when it detaches hangs up on a paired device a second later. Taken here,
    // after the handshake and the binding check — counting before the Noise
    // exchange would let a connect flood park the daemon. The slot is held for
    // the whole connection, panic included.
    let Some((_slot, quit_intent)) = state.admit_client(ClientKind::Peer) else {
        // Shutting down: `handle_client` would answer `ShuttingDown` and return,
        // so there is nothing to serve and no slot to hold.
        return Ok(());
    };
    // Step 6. The remote identity is decided above; `handle_client` must never
    // ask for a pipe handle on this path.
    handle_client(
        crate::framing::Framed::from_stream(reader, writer, closer),
        Arc::clone(state),
        Some(conn_peer),
        quit_intent,
    )
    .map_err(|error| PeerError::Io(error.to_string()))
}

/// Whether this framing helper is the one the reader expects. Kept out of the
/// reader's hot path; the value is asserted by the round-trip tests.
#[cfg(test)]
fn reference_patterns() -> (&'static str, &'static str) {
    (PEER_NOISE_PATTERN, PAIR_NOISE_PATTERN)
}

#[cfg(test)]
#[path = "peer_transport_tests.rs"]
mod tests;
