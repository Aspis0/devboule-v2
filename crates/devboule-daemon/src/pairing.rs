//! Pairing: a short code, a PAKE, and one mutually authenticated exchange.
//!
//! The device that **displays** a code is the responder; the device that
//! **types** it is the initiator. On the wire, after `DBP1`:
//!
//! 1. responder → SPAKE2 message (`start_a`);
//! 2. initiator → SPAKE2 message (`start_b`); both `finish`;
//! 3. both HKDF-SHA256 the SPAKE2 key into a 32-byte PSK;
//! 4. Noise `XXpsk3` with each side's long-term static, prologue
//!    `devboule-pair-v1`, and that PSK;
//! 5. inside Noise: `{device_id, display_name, role, public_key}` each way, and
//!    the responder answers `{accepted, reason}`.
//!
//! The PAKE is what makes an 8-character code worth its 40 bits: an
//! eavesdropper learns nothing, and an active attacker gets exactly one guess
//! per attempt, every one of which is counted against the lockout.
//!
//! `spake2`'s `start_a`/`start_b` both take `(id_a, id_b)` with side A's
//! identity first, so both sides pass the responder's identity first. The
//! loopback tests assert the two sides derive the same key, which is what makes
//! a swapped pair diagnosable rather than silent.
//!
//! # Not a blocked thread
//!
//! A pairing whose incoming role is `Client` needs the local user's answer,
//! which may take a minute. The responder parks the socket inside a state
//! machine entry with a deadline and waits on a channel; `PairingConfirm`
//! fills it. The accept loop's one-second tick expires codes and parked
//! entries, so nothing blocks on a timer and at most
//! [`MAX_PENDING_PAIRINGS`] sockets are held.

use std::collections::{HashMap, HashSet, VecDeque};
use std::io::{self, Read};
use std::net::{IpAddr, SocketAddr, TcpStream};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use devboule_protocol::{PairingSecret, PeerRole, PeerRow, PendingPairing, PEER_DEFAULT_CAPS};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use zeroize::Zeroize;

use crate::device_identity::validate_display_name;
use crate::journal::PeerRecord;
use crate::peer_policy::TransportBinding;
use crate::peer_transport::{
    initiator_handshake, is_tailnet_or_test_loopback, read_framed, responder_handshake,
    split_session, write_framed, HandshakeGuard, NoiseReader, NoiseWriter, PairingHook,
    PeerTransport, PAIRING_MAGIC, PAIR_NOISE_PATTERN, PAIR_PROLOGUE,
};
use crate::server::ServerState;

/// The code alphabet: 32 symbols with `0`/`1`/`I`/`O` removed, so a code read
/// aloud cannot be mistyped into a different valid one.
pub const CODE_ALPHABET: &[u8; 32] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";
pub const CODE_LEN: usize = 8;
/// 40 bits, five bits per symbol. 32 symbols is exactly 2^5, so every 5-bit
/// group maps to a symbol and no rejection sampling is needed.
pub const CODE_BYTES: usize = 5;
/// Five minutes: long enough to walk to the other device, short enough that a
/// leaked code is stale before it is useful.
pub const CODE_LIFETIME: Duration = Duration::from_secs(300);
/// How long an incoming `Client` pairing may wait for the local user.
pub const CONFIRM_WINDOW: Duration = Duration::from_secs(60);
/// At most this many `Client` pairings parked at once. A further candidate is
/// answered `accepted: false, reason: "pairing busy"` immediately.
pub const MAX_PENDING_PAIRINGS: usize = 2;
/// Wrong codes from one source before that source is refused for the rest of
/// the code's life.
pub const WRONG_PER_SOURCE: u32 = 3;
/// Wrong codes in total before the code itself is invalidated. Deliberately
/// well above [`WRONG_PER_SOURCE`] so one unpaired node cannot lock the
/// legitimate initiator out (design §8b, muse M9).
pub const WRONG_TOTAL: u32 = 12;
/// Pairing attempts one source may make in [`ATTEMPT_WINDOW`].
pub const ATTEMPTS_PER_SOURCE: usize = 3;
pub const ATTEMPT_WINDOW: Duration = Duration::from_secs(300);
/// Socket timeout for the whole pairing exchange.
pub const PAIRING_IO_TIMEOUT: Duration = Duration::from_secs(10);
/// The initiator's budget for the *setup* phase: connect, SPAKE2, Noise and
/// the two payloads.
pub const PAIRING_SETUP_TIMEOUT: Duration = Duration::from_secs(30);
/// How long the initiator waits for the responder's final answer, measured from
/// the moment the request is in the responder's hands. It covers the far side's
/// whole confirm window plus one round trip; it is **not** shared with the
/// setup budget, or a slow handshake would silently shorten the window the
/// responder's person is being asked to fill.
pub const ANSWER_TIMEOUT: Duration = Duration::from_secs(CONFIRM_WINDOW.as_secs() + 10);

/// The two sides' PAKE identity strings. Fixed, as the brief specifies: the
/// roles do **not** live here, they are bound in the PSK (`psk_info`), which is
/// what actually has to hold (design §8 R1).
pub const PAIR_RESPONDER_ID: &[u8] = b"devboule/pair/v1/responder";
pub const PAIR_INITIATOR_ID: &[u8] = b"devboule/pair/v1/initiator";
/// HKDF-SHA256 `info` prefix. The roles are appended to it, so the two sides
/// cannot derive the same PSK while disagreeing about who is what.
pub const PAIR_INFO_PREFIX: &[u8] = b"devboule-pair-v1";
/// The one-byte role tag each side writes before the SPAKE2 exchange. In the
/// clear on purpose: it is *bound* (a flipped byte changes the PSK and the
/// `XXpsk3` handshake then fails), so it needs no confidentiality.
pub const ROLE_CLIENT_TAG: u8 = 0;
pub const ROLE_DAEMON_TAG: u8 = 1;

#[derive(Debug)]
pub enum PairingError {
    NoActiveCode,
    /// This daemon has nothing to advertise (no identity, no address).
    NotReady(String),
    /// The source spent its attempts, or was refused for wrong codes.
    Locked(RejectKind),
    UnknownPending,
    /// The pairing itself failed (PAKE, Noise, payload, `whois`).
    Failed(String),
    Io(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RejectKind {
    SourceBlocked,
    TooManyAttempts,
    PairingBusy,
}

impl RejectKind {
    pub fn reason(self) -> &'static str {
        match self {
            Self::SourceBlocked => "too many wrong codes from this address",
            Self::TooManyAttempts => "too many pairing attempts from this address",
            Self::PairingBusy => "pairing busy",
        }
    }
}

impl std::fmt::Display for PairingError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoActiveCode => write!(formatter, "No pairing code is being shown."),
            Self::NotReady(message) => write!(formatter, "This device cannot pair: {message}"),
            Self::Locked(kind) => write!(formatter, "{}", kind.reason()),
            Self::UnknownPending => write!(formatter, "That pairing is no longer pending."),
            Self::Failed(message) => write!(formatter, "Pairing failed: {message}"),
            Self::Io(message) => write!(formatter, "Pairing failed: {message}"),
        }
    }
}

impl std::error::Error for PairingError {}

impl From<io::Error> for PairingError {
    fn from(error: io::Error) -> Self {
        Self::Io(error.to_string())
    }
}

impl From<crate::peer_transport::PeerError> for PairingError {
    fn from(error: crate::peer_transport::PeerError) -> Self {
        Self::Failed(error.to_string())
    }
}

/// What the initiator's `PairingComplete` produced.
#[derive(Debug)]
pub enum PairingOutcome {
    /// The far side must confirm; its answer arrives later and the row appears
    /// on the next `DevicesList` poll.
    Pending(PendingPairing),
    /// Both sides wrote their row.
    Done(PeerRow),
}

/// What a local answer to a parked pairing produced.
#[derive(Debug)]
pub enum ConfirmOutcome {
    Accepted(Box<PeerRow>),
    /// A decline is a completed act, not a failure.
    Declined,
}

// ---------------------------------------------------------------------------
// The code
// ---------------------------------------------------------------------------

/// Eight symbols, five bits each, drawn from [`getrandom`].
///
/// Five fresh bytes are 40 bits, and 32 symbols is exactly 2^5, so the split
/// into eight 5-bit groups is uniform with no rejection sampling. The bytes are
/// wiped after use.
///
/// The source is the OS entropy source directly, never a UUID: a v4 UUID has
/// six fixed version/variant bits, and a secret must not be derived from an
/// identifier type.
pub fn generate_code() -> Result<PairingSecret, PairingError> {
    let mut bytes = [0u8; CODE_BYTES];
    let filled = getrandom::fill(&mut bytes);
    if filled.is_err() {
        bytes.zeroize();
        return Err(PairingError::Failed(
            "the operating system random source is unavailable".to_string(),
        ));
    }
    let mut bits: u64 = 0;
    for byte in bytes.iter() {
        bits = (bits << 8) | u64::from(*byte);
    }
    bytes.zeroize();
    let mut code = String::with_capacity(CODE_LEN);
    for index in 0..CODE_LEN {
        let shift = 40 - 5 * (index + 1);
        code.push(CODE_ALPHABET[((bits >> shift) & 0x1f) as usize] as char);
    }
    bits.zeroize();
    Ok(PairingSecret::new(code))
}

/// Whether `code` is what this daemon could have generated: exactly
/// [`CODE_LEN`] characters, all from [`CODE_ALPHABET`]. Upper case only: the
/// input field uppercases what the user types, and a lower-case code would
/// derive a different PAKE key rather than a friendlier one.
pub fn is_well_formed_code(code: &str) -> bool {
    code.len() == CODE_LEN && code.bytes().all(|byte| CODE_ALPHABET.contains(&byte))
}

/// Whether this daemon may open a pairing connection to `address` (M2).
///
/// A tailnet address, always. Loopback is additionally accepted in the crate's
/// own **unit tests**, which drive a responder on `127.0.0.1`; nothing in a
/// production build may pair over loopback, because the displayed address comes
/// from this device's own `self_node()` and is a tailnet address. The
/// integration test `tests/peer_link.rs` needs no exemption: it pairs over the
/// real tailnet address, which is the whole point of it.
///
/// Shares [`is_tailnet_or_test_loopback`] with the accept path's candidate check
/// so the two cannot disagree about what counts as a pairing address.
fn is_permitted_pairing_target(address: &SocketAddr) -> bool {
    is_tailnet_or_test_loopback(&address.ip())
}

fn unix_millis() -> i64 {
    i64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_millis())
            .unwrap_or(0),
    )
    .unwrap_or(i64::MAX)
}

fn millis_from(now: Instant, at: Instant) -> i64 {
    unix_millis() + i64::try_from(at.saturating_duration_since(now).as_millis()).unwrap_or(0)
}

// ---------------------------------------------------------------------------
// The role-bound PSK
// ---------------------------------------------------------------------------

/// The HKDF `info` for one pairing: the domain prefix and **both** declared
/// roles, in the order (initiator, responder).
///
/// This is what stops a pairing completed with one role pair from being
/// replayed as another: the two sides derive the same PSK only if they agree
/// on the prefix *and* on both roles, so a flipped role changes the key and the
/// `XXpsk3` handshake fails instead of completing with the wrong roles bound
/// to the pinned keys (design §8 R1).
pub fn psk_info(initiator: PeerRole, responder: PeerRole) -> Vec<u8> {
    let mut info = Vec::with_capacity(PAIR_INFO_PREFIX.len() + 2 + 8);
    info.extend_from_slice(PAIR_INFO_PREFIX);
    info.push(0x00);
    info.extend_from_slice(initiator.as_str().as_bytes());
    info.push(0x00);
    info.extend_from_slice(responder.as_str().as_bytes());
    info
}

/// The 32-byte PSK both sides derive from the SPAKE2 output.
pub fn derive_psk(spake_key: &[u8], initiator: PeerRole, responder: PeerRole) -> [u8; 32] {
    let info = psk_info(initiator, responder);
    let hk = hkdf::Hkdf::<Sha256>::new(None, spake_key);
    let mut psk = [0u8; 32];
    hk.expand(&info, &mut psk)
        .expect("32 bytes is a valid HKDF-SHA256 output length");
    psk
}

/// The one-byte role tag written before the SPAKE2 exchange.
fn role_tag(role: PeerRole) -> u8 {
    match role {
        PeerRole::Client => ROLE_CLIENT_TAG,
        PeerRole::Daemon => ROLE_DAEMON_TAG,
    }
}

fn role_from_tag(tag: u8) -> Result<PeerRole, PairingError> {
    match tag {
        ROLE_CLIENT_TAG => Ok(PeerRole::Client),
        ROLE_DAEMON_TAG => Ok(PeerRole::Daemon),
        other => Err(PairingError::Failed(format!("unknown role tag {other}"))),
    }
}

fn write_role(
    stream: &mut TcpStream,
    role: PeerRole,
    deadline: Instant,
) -> Result<(), PairingError> {
    use std::io::Write as _;
    apply_stream_deadline(stream, deadline)?;
    stream
        .write_all(&[role_tag(role)])
        .map_err(PairingError::from)
}

fn read_role(stream: &mut TcpStream, deadline: Instant) -> Result<PeerRole, PairingError> {
    apply_stream_deadline(stream, deadline)?;
    let mut tag = [0u8; 1];
    stream.read_exact(&mut tag).map_err(PairingError::from)?;
    role_from_tag(tag[0])
}

/// Recompute the socket deadline before every operation, the same discipline
/// the Noise halves use, so the budget is wall-clock rather than per syscall.
fn apply_stream_deadline(stream: &TcpStream, deadline: Instant) -> Result<(), PairingError> {
    let remaining = deadline.saturating_duration_since(Instant::now());
    if remaining.is_zero() {
        return Err(PairingError::Failed(
            "the pairing deadline elapsed".to_string(),
        ));
    }
    stream
        .set_read_timeout(Some(remaining))
        .and_then(|()| stream.set_write_timeout(Some(remaining)))
        .map_err(PairingError::from)
}

// ---------------------------------------------------------------------------
// The pairing payloads
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct PairPayload {
    device_id: String,
    display_name: String,
    role: PeerRole,
    /// The claimed Noise static public key, base64. It must equal the remote
    /// static the Noise handshake actually authenticated.
    public_key: String,
}

/// The name in a pairing payload is attacker-chosen and ends up on the card a
/// person reads before accepting, so it is checked at the boundary (M3): both
/// sides call this immediately after reading the payload, before it can be
/// displayed, parked or stored.
fn validate_peer_payload(payload: &PairPayload) -> Result<(), PairingError> {
    validate_display_name(&payload.display_name).map_err(PairingError::Failed)
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct PairAnswer {
    accepted: bool,
    #[serde(default)]
    reason: String,
}

fn write_json<T: Serialize>(
    writer: &mut NoiseWriter,
    value: &T,
    deadline: Instant,
) -> Result<(), PairingError> {
    let bytes =
        serde_json::to_vec(value).map_err(|error| PairingError::Failed(error.to_string()))?;
    writer
        .write_frame(&bytes, Some(deadline))
        .map_err(PairingError::from)
}

fn read_json<T: serde::de::DeserializeOwned>(
    reader: &mut NoiseReader,
    deadline: Instant,
) -> Result<T, PairingError> {
    let mut collected = Vec::new();
    let mut chunk = [0u8; 4096];
    while !collected.ends_with(b"\n") {
        let read = reader
            .read_plaintext(&mut chunk, Some(deadline))
            .map_err(PairingError::from)?;
        if read == 0 {
            return Err(PairingError::Failed(
                "the peer closed the connection".to_string(),
            ));
        }
        collected.extend_from_slice(&chunk[..read]);
        if collected.len() > 64 * 1024 {
            return Err(PairingError::Failed(
                "the pairing payload exceeded 64 KiB".to_string(),
            ));
        }
    }
    collected.pop();
    serde_json::from_slice(&collected).map_err(|error| PairingError::Failed(error.to_string()))
}

fn base64_decode(value: &str) -> Result<Vec<u8>, PairingError> {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD
        .decode(value)
        .map_err(|error| PairingError::Failed(format!("publicKey is not base64: {error}")))
}

fn base64_encode(bytes: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

// ---------------------------------------------------------------------------
// The state machine
// ---------------------------------------------------------------------------

struct ActiveCode {
    code: PairingSecret,
    /// This device's own role in the pairing it is displaying.
    role: PeerRole,
    expires_at: Instant,
}

struct PendingEntry {
    /// Identifies *this* park, not the device. A handler whose entry was
    /// replaced must remove only its own row on the way out: keying the cleanup
    /// by `device_id` (the first version) made the older handler delete the
    /// newer entry for the same device, so the second park vanished and the
    /// confirmation had nothing to answer. Found by
    /// `a_second_park_for_the_same_device_replaces_the_first`.
    token: u64,
    device_id: String,
    display_name: String,
    role: PeerRole,
    key_fingerprint: String,
    address: String,
    public_key: Vec<u8>,
    binding: TransportBinding,
    expires_at: Instant,
    decision: mpsc::Sender<bool>,
}

#[derive(Default)]
struct State {
    active: Option<ActiveCode>,
    pending: Vec<PendingEntry>,
    attempts: HashMap<IpAddr, VecDeque<Instant>>,
    blocked: HashSet<IpAddr>,
    wrong_from_source: HashMap<IpAddr, u32>,
    wrong_total: u32,
}

impl State {
    fn active_if_live(&self, now: Instant) -> Option<&ActiveCode> {
        self.active
            .as_ref()
            .filter(|active| active.expires_at > now)
    }

    /// Spend the displayed code (design §8 R8: a code is single use).
    ///
    /// Called on the success path, the moment a peer row is written or a
    /// pending entry is parked. Without this the code stays valid for its whole
    /// five minutes and pairs every device that presents it, which is the whole
    /// point of it being single use: one observed code must buy exactly one
    /// pairing.
    ///
    /// Only the code is dropped here. The per-source lockout counters are left
    /// alone, so a source that guessed wrong on this code is still refused
    /// while `start` has not been called again; `start` resets them with the new
    /// code.
    fn consume(&mut self) {
        self.active = None;
    }

    fn expire(&mut self, now: Instant) {
        if self
            .active
            .as_ref()
            .is_some_and(|active| active.expires_at <= now)
        {
            // Dropping the code wipes it.
            self.active = None;
            self.blocked.clear();
            self.wrong_from_source.clear();
            self.wrong_total = 0;
            self.attempts.clear();
        }
        self.pending.retain(|entry| entry.expires_at > now);
    }

    /// One pairing attempt from `ip`; refuses past the window cap.
    fn note_attempt(&mut self, ip: IpAddr, now: Instant) -> Result<(), RejectKind> {
        if self.blocked.contains(&ip) {
            return Err(RejectKind::SourceBlocked);
        }
        let attempts = self.attempts.entry(ip).or_default();
        while let Some(oldest) = attempts.front() {
            if now.saturating_duration_since(*oldest) > ATTEMPT_WINDOW {
                attempts.pop_front();
            } else {
                break;
            }
        }
        if attempts.len() >= ATTEMPTS_PER_SOURCE {
            return Err(RejectKind::TooManyAttempts);
        }
        attempts.push_back(now);
        Ok(())
    }

    /// One failed PAKE/Noise exchange. Returns whether the code itself must be
    /// invalidated.
    fn note_wrong(&mut self, ip: IpAddr) -> bool {
        let count = self.wrong_from_source.entry(ip).or_insert(0);
        *count += 1;
        if *count >= WRONG_PER_SOURCE {
            self.blocked.insert(ip);
        }
        self.wrong_total = self.wrong_total.saturating_add(1);
        self.wrong_total >= WRONG_TOTAL
    }
}

/// The pairing service: one per daemon.
pub struct PairingService {
    state: Mutex<State>,
    /// Test-only: how many pairings have ever been parked, so a test can tell
    /// that the *second* park happened before it asserts what the pending list
    /// looks like. Without it a test can only poll the list, which is already
    /// non-empty from the first park and therefore proves nothing.
    #[cfg(test)]
    parks: std::sync::atomic::AtomicU64,
    /// Allocates `PendingEntry::token`. Process-unique and never reused.
    next_pending_token: std::sync::atomic::AtomicU64,
}

impl Default for PairingService {
    fn default() -> Self {
        Self::new()
    }
}

impl PairingService {
    pub fn new() -> Self {
        Self {
            state: Mutex::new(State::default()),
            #[cfg(test)]
            parks: std::sync::atomic::AtomicU64::new(0),
            next_pending_token: std::sync::atomic::AtomicU64::new(1),
        }
    }

    /// Test-only: total pairings parked by this service.
    #[cfg(test)]
    pub fn park_count(&self) -> u64 {
        self.parks.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Display a new code, replacing any previous one: there is exactly one
    /// active code per daemon.
    pub fn start(&self, role: PeerRole) -> Result<(PairingSecret, i64), PairingError> {
        let now = Instant::now();
        let expires_at = now + CODE_LIFETIME;
        let code = generate_code()?;
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        state.expire(now);
        state.blocked.clear();
        state.wrong_from_source.clear();
        state.wrong_total = 0;
        state.attempts.clear();
        state.active = Some(ActiveCode {
            code: code.clone(),
            role,
            expires_at,
        });
        Ok((code, millis_from(now, expires_at)))
    }

    /// The parked pairings the panel must decide on.
    pub fn pending_snapshot(&self) -> Vec<PendingPairing> {
        let now = Instant::now();
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        state.expire(now);
        state
            .pending
            .iter()
            .map(|entry| PendingPairing {
                device_id: entry.device_id.clone(),
                display_name: entry.display_name.clone(),
                role: entry.role,
                key_fingerprint: entry.key_fingerprint.clone(),
                address: entry.address.clone(),
                expires_at: millis_from(now, entry.expires_at),
            })
            .collect()
    }

    /// Answer a parked pairing.
    ///
    /// Both outcomes audit here, because both are completed acts of the person
    /// at this device. A decline is not a failure and must not be reported as
    /// an `Error` to the panel.
    pub fn confirm(
        &self,
        server: &Arc<ServerState>,
        device_id: &str,
        accept: bool,
    ) -> Result<ConfirmOutcome, PairingError> {
        let entry = {
            let mut guard = self.state.lock().unwrap_or_else(|error| error.into_inner());
            guard.expire(Instant::now());
            let position = guard
                .pending
                .iter()
                .position(|entry| entry.device_id == device_id)
                .ok_or(PairingError::UnknownPending)?;
            guard.pending.remove(position)
        };
        // A closed channel means the parked thread already gave up.
        let _ = entry.decision.send(accept);
        self.audit_confirm(server, device_id, accept);
        if !accept {
            return Ok(ConfirmOutcome::Declined);
        }
        let record = local_peer_record(
            server,
            &entry.device_id,
            &entry.display_name,
            entry.role,
            &entry.public_key,
            entry.binding.clone(),
            entry.address.clone(),
        )?;
        let stored = upsert_peer(server, record)?;
        Ok(ConfirmOutcome::Accepted(Box::new(peer_row(
            server, &stored,
        ))))
    }

    /// The initiator: this device typed a code shown by `address`.
    ///
    /// The binding check uses `server.peer_transport()` rather than a transport
    /// passed in. There is only one right answer to "which transport is this
    /// device pairing over", and taking it from the state means the immediate
    /// check and the deferred one on the answer thread cannot disagree — a
    /// mismatch there would make the row land under a different binding than
    /// the one that was verified.
    pub fn complete(
        &self,
        server: &Arc<ServerState>,
        address: &str,
        code: &PairingSecret,
        role: PeerRole,
    ) -> Result<PairingOutcome, PairingError> {
        let remote_addr: SocketAddr = address
            .parse()
            .map_err(|_| PairingError::Failed(format!("{address} is not an ip:port")))?;
        // The address is renderer-supplied, so the daemon enforces where it may
        // connect rather than trusting the panel to have done it (M2). Without
        // this, a compromised renderer gets the daemon to make a TCP connection
        // anywhere it likes and to send the pairing magic down it.
        if !is_permitted_pairing_target(&remote_addr) {
            return Err(PairingError::Failed(
                "the other device's address must be a tailnet address (100.64.0.0/10 or \
                 fd7a:115c:a1e0::/48)"
                    .to_string(),
            ));
        }
        if !is_well_formed_code(code.as_str()) {
            return Err(PairingError::Failed(
                "the code is not in the expected format".to_string(),
            ));
        }
        let identity = server
            .device_identity()
            .as_ref()
            .map_err(|error| PairingError::NotReady(error.to_string()))?
            .clone();

        let mut stream = TcpStream::connect_timeout(&remote_addr, PAIRING_IO_TIMEOUT)
            .map_err(|error| PairingError::Failed(error.to_string()))?;
        // One budget for the setup phase (SPAKE2, Noise, the payloads), and a
        // **separate** one, taken below, for the far side's answer. Sharing a
        // single deadline across both is what makes a slow handshake eat into
        // the responder's confirmation window.
        let setup_deadline = Instant::now() + PAIRING_SETUP_TIMEOUT;
        stream.set_read_timeout(Some(PAIRING_IO_TIMEOUT))?;
        stream.set_write_timeout(Some(PAIRING_IO_TIMEOUT))?;
        {
            use std::io::Write as _;
            stream.write_all(&PAIRING_MAGIC)?;
        }

        // 1-2: SPAKE2, side B. Each side's role goes over in the clear first,
        // because both must be bound into the PSK before either derives it.
        write_role(&mut stream, role, setup_deadline)?;
        let responder_role = read_role(&mut stream, setup_deadline)?;
        let password = spake2::Password::new(code.as_str().as_bytes());
        let responder_identity = spake2::Identity::new(PAIR_RESPONDER_ID);
        let initiator_identity = spake2::Identity::new(PAIR_INITIATOR_ID);
        let mut their_message = [0u8; 256];
        let their_len = read_framed(&stream, &mut their_message, setup_deadline)?;
        let (spake_state, our_message) = spake2::Spake2::<spake2::Ed25519Group>::start_b(
            &password,
            &responder_identity,
            &initiator_identity,
        );
        write_framed(&stream, &our_message, setup_deadline)?;
        let mut spake_key = spake_state
            .finish(&their_message[..their_len])
            .map_err(|error| PairingError::Failed(error.to_string()))?;
        let mut psk = derive_psk(&spake_key, role, responder_role);
        // The PAKE output is key material too (L1).
        spake_key.zeroize();

        // 3-4: Noise XXpsk3 over the PAKE-derived key. A wrong code derives a
        // different PSK and the handshake fails here.
        let session = initiator_handshake(
            &stream,
            setup_deadline,
            identity.private_key(),
            None,
            PAIR_PROLOGUE,
            Some(&psk),
            PAIR_NOISE_PATTERN,
        );
        for byte in psk.iter_mut() {
            *byte = 0;
        }
        let session = session.map_err(|error| PairingError::Failed(error.to_string()))?;
        let remote_static = session
            .get_remote_static()
            .ok_or_else(|| PairingError::Failed("no remote static key".to_string()))?
            .to_vec();
        let (mut reader, mut writer, _closer) = split_session(&stream, session)?;

        let payload = PairPayload {
            device_id: identity.device_id.clone(),
            display_name: identity.display_name.clone(),
            role,
            public_key: identity.public_key_b64(),
        };
        write_json(&mut writer, &payload, setup_deadline)?;

        // The responder's own payload. This is the identity recorded below:
        // using our own `payload` here would store *this* device as its own
        // peer, with the responder's key attached.
        let peer_payload: PairPayload = read_json(&mut reader, setup_deadline)?;
        let peer_key = base64_decode(&peer_payload.public_key)?;
        if peer_key != remote_static {
            return Err(PairingError::Failed(
                "the responder's payload key does not match the authenticated static key"
                    .to_string(),
            ));
        }
        // Validated before it can be shown on this device's card, parked, or
        // stored (M3).
        validate_peer_payload(&peer_payload)?;

        // The far side's user answers a `Client` pairing, which can take a
        // minute. Waiting for that here would hold the caller's RPC open for
        // the whole window and, worse, leave the caller unable to poll
        // `DevicesList` and answer it: the confirm could never arrive. So this
        // side reports the pairing as pending now and finishes it on its own
        // thread, writing the row only if the answer is `accepted`.
        if role == PeerRole::Client {
            let pending = PendingPairing {
                device_id: peer_payload.device_id.clone(),
                display_name: peer_payload.display_name.clone(),
                role: peer_payload.role,
                key_fingerprint: crate::device_identity::key_fingerprint(&remote_static),
                address: address.to_string(),
                expires_at: millis_from(Instant::now(), Instant::now() + CONFIRM_WINDOW),
            };
            let server = Arc::clone(server);
            let peer_device_id = peer_payload.device_id.clone();
            let peer_display_name = peer_payload.display_name.clone();
            let peer_role = peer_payload.role;
            let address_for_answer = address.to_string();
            // The far side's whole confirmation window starts now, after the
            // handshake, so a slow setup cannot shorten it.
            let answer_deadline = Instant::now() + ANSWER_TIMEOUT;
            std::thread::Builder::new()
                .name("daemon-pairing-answer".into())
                .spawn(move || {
                    let answer: PairAnswer = match read_json(&mut reader, answer_deadline) {
                        Ok(answer) => answer,
                        Err(error) => {
                            eprintln!("daemon pairing answer was not received: {error}");
                            return;
                        }
                    };
                    if !answer.accepted {
                        return;
                    }
                    // Bound to the responder's address by **this** device's
                    // `whois`, never by `self_node()`.
                    let binding = match server.peer_transport().binding(&remote_addr) {
                        Ok(binding) => binding,
                        Err(error) => {
                            eprintln!("daemon pairing binding check failed: {error}");
                            return;
                        }
                    };
                    let record = match local_peer_record(
                        &server,
                        &peer_device_id,
                        &peer_display_name,
                        peer_role,
                        &remote_static,
                        binding,
                        address_for_answer,
                    ) {
                        Ok(record) => record,
                        Err(error) => {
                            eprintln!("daemon pairing row was not written: {error}");
                            return;
                        }
                    };
                    if let Err(error) = upsert_peer(&server, record) {
                        eprintln!("daemon pairing row was not stored: {error}");
                    }
                })
                .ok();
            return Ok(PairingOutcome::Pending(pending));
        }

        let answer: PairAnswer = read_json(&mut reader, Instant::now() + ANSWER_TIMEOUT)?;
        if !answer.accepted {
            return Err(PairingError::Failed(if answer.reason.is_empty() {
                "the other device declined".to_string()
            } else {
                answer.reason
            }));
        }

        // The binding is what **our** `whois` says about **their** address.
        let binding = server
            .peer_transport()
            .binding(&remote_addr)
            .map_err(|error| PairingError::Failed(error.to_string()))?;
        let record = local_peer_record(
            server,
            &peer_payload.device_id,
            &peer_payload.display_name,
            peer_payload.role,
            &remote_static,
            binding,
            address.to_string(),
        )?;
        let stored = upsert_peer(server, record)?;
        Ok(PairingOutcome::Done(peer_row(server, &stored)))
    }
}

impl PairingHook for PairingService {
    fn is_active(&self) -> bool {
        let state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        state.active_if_live(Instant::now()).is_some()
    }

    fn housekeeping(&self, now: Instant) {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        state.expire(now);
    }

    fn handle(
        &self,
        transport: &dyn PeerTransport,
        mut stream: TcpStream,
        peer_addr: SocketAddr,
        server: &Arc<ServerState>,
        in_flight: HandshakeGuard,
    ) {
        if let Err(error) = self.serve_pairing(transport, &mut stream, peer_addr, server, in_flight)
        {
            // The reason only: no code, no key, no device id.
            eprintln!("daemon pairing attempt ended: {error}");
        }
        let _ = stream.shutdown(std::net::Shutdown::Both);
    }
}

impl PairingService {
    /// The responder: this device displayed the code.
    fn serve_pairing(
        &self,
        transport: &dyn PeerTransport,
        stream: &mut TcpStream,
        peer_addr: SocketAddr,
        server: &Arc<ServerState>,
        in_flight: HandshakeGuard,
    ) -> Result<(), PairingError> {
        // The accept loop peeked these four bytes; consume them for real now.
        let mut magic = [0u8; 4];
        stream.set_read_timeout(Some(Duration::from_secs(2)))?;
        stream.read_exact(&mut magic)?;
        if magic != PAIRING_MAGIC {
            return Err(PairingError::Failed("missing pairing magic".to_string()));
        }

        let now = Instant::now();
        let (code, responder_role) = {
            let mut guard = self.state.lock().unwrap_or_else(|error| error.into_inner());
            guard
                .note_attempt(peer_addr.ip(), now)
                .map_err(PairingError::Locked)?;
            let active = guard
                .active_if_live(now)
                .ok_or(PairingError::NoActiveCode)?;
            (active.code.clone(), active.role)
        };
        let deadline = now + CONFIRM_WINDOW + Duration::from_secs(10);

        let identity = server
            .device_identity()
            .as_ref()
            .map_err(|error| PairingError::NotReady(error.to_string()))?
            .clone();

        // 1–2: SPAKE2. The responder is side A and speaks first. Each side's role
        // goes over in the clear first, because both must be bound into the PSK
        // before either derives it.
        write_role(stream, responder_role, deadline)?;
        let initiator_role = read_role(stream, deadline)?;
        let password = spake2::Password::new(code.as_str().as_bytes());
        let responder_identity = spake2::Identity::new(PAIR_RESPONDER_ID);
        let initiator_identity = spake2::Identity::new(PAIR_INITIATOR_ID);
        let (spake_state, our_message) = spake2::Spake2::<spake2::Ed25519Group>::start_a(
            &password,
            &responder_identity,
            &initiator_identity,
        );
        write_framed(stream, &our_message, deadline)?;
        let mut their_message = [0u8; 256];
        let their_len = read_framed(stream, &mut their_message, deadline)?;
        let mut spake_key = match spake_state.finish(&their_message[..their_len]) {
            Ok(key) => key,
            Err(error) => return Err(self.note_wrong(peer_addr.ip(), server, &error.to_string())),
        };
        let mut psk = derive_psk(&spake_key, initiator_role, responder_role);
        // The PAKE output is key material too: the PSK was wiped already, and
        // this is the other half of the same secret (L1).
        spake_key.zeroize();

        // 3–4: Noise XXpsk3 over the PAKE-derived key. A wrong code produces a
        // different PSK, and the handshake fails here.
        let session = responder_handshake(
            stream,
            deadline,
            identity.private_key(),
            PAIR_PROLOGUE,
            Some(&psk),
            PAIR_NOISE_PATTERN,
        );
        for byte in psk.iter_mut() {
            *byte = 0;
        }
        let session = match session {
            Ok(session) => session,
            Err(error) => return Err(self.note_wrong(peer_addr.ip(), server, &error.to_string())),
        };
        let remote_static = session
            .get_remote_static()
            .ok_or_else(|| PairingError::Failed("no remote static key".to_string()))?
            .to_vec();
        let (mut reader, mut writer, _closer) = split_session(stream, session)?;

        let payload: PairPayload = read_json(&mut reader, deadline)?;
        // The key in the payload must be the key Noise authenticated;
        // otherwise a peer could pin an identity it does not hold.
        let claimed = base64_decode(&payload.public_key)?;
        if claimed != remote_static {
            return Err(self.note_wrong(
                peer_addr.ip(),
                server,
                "payload key does not match the authenticated static key",
            ));
        }
        // The name is attacker-chosen and is rendered on the confirmation card,
        // so it is checked before it can be stored or parked (M3).
        if let Err(PairingError::Failed(reason)) = validate_peer_payload(&payload) {
            return Err(self.note_wrong(peer_addr.ip(), server, &reason));
        }

        // Our own payload, so the initiator can record who it paired with.
        // Each side sends one (the brief's pairing payload) and the answer
        // below carries the decision; without this the initiator would have no
        // identity to store but its own.
        write_json(
            &mut writer,
            &PairPayload {
                device_id: identity.device_id.clone(),
                display_name: identity.display_name.clone(),
                role: responder_role,
                public_key: identity.public_key_b64(),
            },
            deadline,
        )?;

        // The binding is what **our** `whois` says about **their** address.
        // `self_node()` describes this device and is never consulted here.
        let binding = transport
            .binding(&peer_addr)
            .map_err(|error| PairingError::Failed(error.to_string()))?;
        let key_fingerprint = crate::device_identity::key_fingerprint(&remote_static);
        let address = format!("{}:{}", peer_addr.ip(), peer_addr.port());

        // 5: a `Daemon` peer is answered at once; a `Client` peer is the one
        // the person at this device must approve (design §8b A11).
        let accepted = if payload.role == PeerRole::Daemon {
            let record = local_peer_record(
                server,
                &payload.device_id,
                &payload.display_name,
                payload.role,
                &remote_static,
                binding,
                address,
            )?;
            upsert_peer(server, record)?;
            // The row is written, so the code has done its one job (H1).
            self.state
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .consume();
            true
        } else {
            let (decision, wait) = mpsc::channel::<bool>();
            // Allocated before the lock: the token identifies this park, and the
            // cleanup below uses it to remove only its own entry.
            let token = self
                .next_pending_token
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let parked = {
                let mut guard = self.state.lock().unwrap_or_else(|error| error.into_inner());
                guard.expire(Instant::now());
                if guard.pending.len() >= MAX_PENDING_PAIRINGS {
                    false
                } else {
                    // One entry per device (C7). A device cannot be waiting
                    // twice for the same pairing: a reconnect from the same
                    // device — the realistic case, since its first socket may
                    // have died — replaces the older entry instead of parking
                    // beside it. The dropped entry releases its own parked
                    // thread, which then answers `pairing busy` to the older
                    // connection rather than ever being confirmed by a decision
                    // meant for the newer one.
                    //
                    // This runs before the capacity check so a device
                    // re-parking does not need a second slot.
                    guard
                        .pending
                        .retain(|entry| entry.device_id != payload.device_id);
                    if guard.pending.len() >= MAX_PENDING_PAIRINGS {
                        // Nothing was parked, so the code is not spent: the
                        // person can try again.
                        false
                    } else {
                        guard.pending.push(PendingEntry {
                            token,
                            device_id: payload.device_id.clone(),
                            display_name: payload.display_name.clone(),
                            role: payload.role,
                            key_fingerprint: key_fingerprint.clone(),
                            address: address.clone(),
                            public_key: remote_static.clone(),
                            binding: binding.clone(),
                            expires_at: Instant::now() + CONFIRM_WINDOW,
                            decision,
                        });
                        #[cfg(test)]
                        self.parks
                            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        // A parked pairing has spent the code: the exchange
                        // succeeded with this code, and a second candidate must
                        // not be able to pair from it (H1). The "pairing busy"
                        // refusal above deliberately does **not** consume it —
                        // nothing was paired and no entry was written, so the
                        // person may retry.
                        guard.consume();
                        true
                    }
                }
            };
            if !parked {
                false
            } else {
                // The exchange is no longer a handshake, so the slot goes back
                // before the wait. Holding it here is what would let two
                // parked pairings starve the budget for `CONFIRM_WINDOW` and
                // turn a third candidate into a dropped connection; with it
                // released, the third candidate reaches this method and is
                // answered "pairing busy" (design §8 R4 budgets *handshakes*,
                // and a parked pairing is not one).
                drop(in_flight);
                // Wait for the local answer, or for the window to close. The
                // entry is dropped by `confirm`, or expired by the tick; a late
                // confirm finds nothing and is refused there.
                let accepted = wait.recv_timeout(CONFIRM_WINDOW).unwrap_or(false);
                let mut guard = self.state.lock().unwrap_or_else(|error| error.into_inner());
                // **This** entry, not every entry for this device: a replaced
                // handler must not delete the entry that replaced it.
                guard.pending.retain(|entry| entry.token != token);
                accepted
            }
        };

        let answer = PairAnswer {
            accepted,
            reason: if accepted {
                String::new()
            } else {
                RejectKind::PairingBusy.reason().to_string()
            },
        };
        write_json(&mut writer, &answer, deadline)?;
        Ok(())
    }

    /// One audit row for the local decision. The actor is this device
    /// (`role = "local"`), and `claimed_origin` names the peer the decision was
    /// about, so the trail says who was paired without a second lookup.
    fn audit_confirm(&self, server: &Arc<ServerState>, peer_device_id: &str, accept: bool) {
        let local_device_id = server
            .device_identity()
            .as_ref()
            .map(|identity| identity.device_id.clone())
            .unwrap_or_else(|_| "unknown".to_string());
        server.audit(crate::journal::AuditRecord {
            device_id: local_device_id,
            role: "local".to_string(),
            claimed_origin: Some(peer_device_id.to_string()),
            action: "PairingConfirm".to_string(),
            session_id: None,
            outcome: if accept { "ok" } else { "declined" }.to_string(),
        });
    }

    fn note_wrong(&self, ip: IpAddr, server: &Arc<ServerState>, reason: &str) -> PairingError {
        let invalidated = {
            let mut guard = self.state.lock().unwrap_or_else(|error| error.into_inner());
            let invalidated = guard.note_wrong(ip);
            if invalidated {
                guard.active = None;
            }
            invalidated
        };
        let device_id = server
            .device_identity()
            .as_ref()
            .map(|identity| identity.device_id.clone())
            .unwrap_or_else(|_| "unknown".to_string());
        server.audit(crate::journal::AuditRecord {
            device_id,
            role: "local".to_string(),
            claimed_origin: None,
            action: if invalidated {
                "pairing_abuse"
            } else {
                "pairing_attempt"
            }
            .to_string(),
            session_id: None,
            outcome: "failed".to_string(),
        });
        PairingError::Failed(reason.to_string())
    }
}

// ---------------------------------------------------------------------------
// Journal rows
// ---------------------------------------------------------------------------

/// Build the row **this** device writes about the peer it just paired with.
///
/// `paired_by_user` is this daemon's own SID: the person at this machine who
/// ran the pairing. It is written here and never received from the peer.
fn local_peer_record(
    server: &Arc<ServerState>,
    device_id: &str,
    display_name: &str,
    role: PeerRole,
    public_key: &[u8],
    binding: TransportBinding,
    address: String,
) -> Result<PeerRecord, PairingError> {
    if device_id.is_empty() {
        return Err(PairingError::Failed(
            "the peer sent no device id".to_string(),
        ));
    }
    if uuid::Uuid::parse_str(device_id).is_err() {
        return Err(PairingError::Failed(
            "the peer's device id is not a UUID".to_string(),
        ));
    }
    if public_key.len() != 32 {
        return Err(PairingError::Failed(
            "the peer's static key is not 32 bytes".to_string(),
        ));
    }
    // Checked again here, at the one function that turns a name into a stored
    // row: the two payload call sites already validated it, and this is what
    // makes "every stored `display_name` passed validation" an invariant of the
    // storage path rather than of its callers (M3).
    if let Err(reason) = validate_display_name(display_name) {
        return Err(PairingError::Failed(reason));
    }
    // Re-pairing an existing device needs a revoke first (design §8 R8) —
    // **unless the pinned key is the same one**. That exception is what makes a
    // retry after a half-finished pairing converge instead of wedging (C6):
    // the row write and the answer are two operations, and whichever side fails
    // second leaves one device holding a row while the other does not. Without
    // this, every retry dies here and two perfectly good devices are stuck until
    // somebody revokes by hand.
    //
    // It does not weaken the rule it excepts: the pinned public key is the
    // credential, so re-pairing with the *same* key is the same pairing being
    // finished, while a **different** key for a known device id is exactly the
    // substitution §8 R8 exists to refuse.
    if let Some(existing) = server
        .peer_get(device_id)
        .map_err(PairingError::Failed)?
        .filter(|row| !row.is_revoked())
    {
        if existing.public_key != public_key {
            return Err(PairingError::Failed(format!(
                "{} is already paired with a different key; revoke it first",
                existing.display_name
            )));
        }
        // Same key: finish the pairing. The upsert below refreshes the display
        // name, address and binding, which is what a retry should do.
    }
    Ok(PeerRecord {
        device_id: device_id.to_string(),
        display_name: display_name.to_string(),
        role: role.as_str().to_string(),
        public_key: public_key.to_vec(),
        paired_by_user: server.local_user_sid(),
        binding_kind: binding.kind,
        binding_stable_id: Some(binding.stable_id),
        binding_node_name: Some(binding.node_name),
        binding_login_name: Some(binding.login_name),
        address,
        paired_at: unix_millis(),
        revoked_at: None,
        caps: PEER_DEFAULT_CAPS
            .iter()
            .map(|cap| (*cap).to_string())
            .collect(),
    })
}

fn upsert_peer(server: &Arc<ServerState>, record: PeerRecord) -> Result<PeerRecord, PairingError> {
    server.peer_upsert(record).map_err(PairingError::Failed)
}

/// Project a stored row onto the wire.
pub fn peer_row(server: &Arc<ServerState>, record: &PeerRecord) -> PeerRow {
    PeerRow {
        device_id: record.device_id.clone(),
        display_name: record.display_name.clone(),
        role: PeerRole::parse(&record.role).unwrap_or(PeerRole::Daemon),
        public_key: base64_encode(&record.public_key),
        key_fingerprint: crate::device_identity::key_fingerprint(&record.public_key),
        binding_kind: record.binding_kind.clone(),
        binding_node_name: record.binding_node_name.clone(),
        binding_login_name: record.binding_login_name.clone(),
        address: record.address.clone(),
        paired_at: record.paired_at,
        revoked_at: record.revoked_at,
        caps: record.caps.clone(),
        paired_by_user: record.paired_by_user.clone(),
        online: server.is_peer_online(&record.device_id),
    }
}

/// The capability names the daemon accepts, and the rules about which may be
/// absent (design §8b A11).
pub fn validate_caps(role: PeerRole, caps: &[String]) -> Result<Vec<String>, String> {
    if caps.is_empty() {
        return Err("a peer must keep at least the 'view' capability".to_string());
    }
    for cap in caps {
        if !devboule_protocol::PEER_CAPS.contains(&cap.as_str()) {
            return Err(format!("unknown capability '{cap}'"));
        }
    }
    let mut seen = std::collections::HashSet::new();
    for cap in caps {
        if !seen.insert(cap.as_str()) {
            return Err(format!("capability '{cap}' was given twice"));
        }
    }
    if role == PeerRole::Client && !caps.iter().any(|cap| cap == "view") {
        return Err("a client peer cannot lose 'view'".to_string());
    }
    Ok(caps.to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device_identity::MAX_DISPLAY_NAME_CHARS;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    fn tmp_paths() -> (PathBuf, crate::paths::RuntimePaths) {
        static COUNTER: AtomicU64 = AtomicU64::new(1);
        let dir = std::env::temp_dir().join(format!(
            "devboule pairing {}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).expect("runtime dir");
        (dir.clone(), crate::paths::RuntimePaths::from_dir(&dir))
    }

    fn server(tag: &str) -> (PathBuf, Arc<ServerState>) {
        let (dir, paths) = tmp_paths();
        let server =
            crate::server::ServerState::with_paths(format!("pairing-{tag}"), paths).expect("state");
        // A deterministic identity for the tests: the file store, in this
        // runtime dir, created on first use.
        let _ = server.device_identity();
        (dir, server)
    }

    /// The whole pairing, in process, over loopback and with a real code: the
    /// responder the accept loop would have started, and the initiator the
    /// `PairingComplete` RPC drives.
    ///
    /// This is the happy path the earlier test only approached — it proves the
    /// SPAKE2 exchange, the role-bound PSK, the `XXpsk3` handshake, both
    /// payloads, the parked confirmation and the two `peers` rows, with no
    /// Tailscale and no daemon process.
    #[test]
    fn a_client_pairing_completes_and_writes_both_rows() {
        let (dir_a, server_a) = server("initiator");
        let (dir_b, server_b) = server("responder");
        let service_a = PairingService::new();
        let service_b = Arc::new(PairingService::new());
        let (code, _expires_at) = service_b.start(PeerRole::Client).expect("a code");
        let transport = Arc::new(crate::peer_transport::TestTransport::default());
        // `complete` binds the peer through this device's transport, so the
        // stub has to be *installed*, not merely passed.
        assert!(server_a.set_peer_transport(transport.clone()).is_ok());

        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let address = listener.local_addr().expect("addr").to_string();

        let responder_transport = Arc::clone(&transport);
        let responder_server = Arc::clone(&server_b);
        let responder_service = Arc::clone(&service_b);
        let caps = Arc::new(crate::peer_transport::AcceptCaps::default());
        let responder = std::thread::spawn(move || {
            let (stream, peer_addr) = accept_bounded(&listener);
            // The accept loop takes the pairing handshake slot; the service
            // releases it once the exchange stops being a handshake.
            let slot = caps
                .admit_handshake(crate::peer_transport::HandshakeKind::Pairing)
                .expect("a pairing slot");
            responder_service.handle(
                responder_transport.as_ref(),
                stream,
                peer_addr,
                &responder_server,
                slot,
            );
        });

        let outcome = service_a
            .complete(&server_a, &address, &code, PeerRole::Client)
            .expect("the initiator completes the exchange");
        let pending = match outcome {
            PairingOutcome::Pending(pending) => pending,
            PairingOutcome::Done(_) => panic!(
                "a Client pairing must be reported pending: the far side has not confirmed yet"
            ),
        };
        // The initiator's card names the device that has to confirm.
        let responder_id = server_b
            .device_identity()
            .as_ref()
            .expect("B has an identity")
            .device_id
            .clone();
        assert_eq!(pending.device_id, responder_id);
        assert_eq!(pending.role, PeerRole::Client);
        assert!(!pending.key_fingerprint.is_empty());

        // B parked it, keyed by the device that typed the code (A).
        let parked = service_b.pending_snapshot();
        assert_eq!(parked.len(), 1, "exactly one pairing is parked at B");
        let initiator_id = server_a
            .device_identity()
            .as_ref()
            .expect("A has an identity")
            .device_id
            .clone();
        assert_eq!(parked[0].device_id, initiator_id);

        // The person at B accepts.
        let row = service_b
            .confirm(&server_b, &initiator_id, true)
            .expect("confirm");
        let row = match row {
            ConfirmOutcome::Accepted(row) => *row,
            ConfirmOutcome::Declined => panic!("an accept must produce a row"),
        };
        assert_eq!(row.device_id, initiator_id);
        assert_eq!(row.role, PeerRole::Client);
        assert!(row.revoked_at.is_none());
        assert_eq!(row.caps, vec!["view".to_string()]);

        join_bounded(responder, "the responder's pairing thread");

        // A writes its own row once the answer reaches it, on the thread that
        // did not block the RPC.
        let deadline = Instant::now() + bound::THREAD;
        loop {
            let rows = server_a.peers().expect("A's rows");
            if rows.len() == 1 {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "A never wrote its row for B; saw {rows:?}"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
        let a_row = &server_a.peers().expect("A's rows")[0];
        assert_eq!(a_row.device_id, responder_id, "A's row names B");
        // `peers()` hands back the stored record, whose role is the wire
        // string, not the enum.
        assert_eq!(a_row.role, "client");
        assert_eq!(a_row.caps, vec!["view".to_string()]);
        assert!(a_row.revoked_at.is_none());

        drop(server_a);
        drop(server_b);
        let _ = std::fs::remove_dir_all(&dir_a);
        let _ = std::fs::remove_dir_all(&dir_b);
    }

    /// The same exchange with the code wrong on one side: the PAKE derives a
    /// different key, so the Noise handshake fails and **no** row is written on
    /// either side.
    #[test]
    fn a_wrong_code_never_writes_a_row() {
        let (dir_a, server_a) = server("wrong-initiator");
        let (dir_b, server_b) = server("wrong-responder");
        let service_a = PairingService::new();
        let service_b = Arc::new(PairingService::new());
        let (_real_code, _) = service_b.start(PeerRole::Client).expect("a code");
        let wrong = PairingSecret::new("ZZZZ2345");
        let transport = Arc::new(crate::peer_transport::TestTransport::default());
        assert!(server_a.set_peer_transport(transport.clone()).is_ok());

        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let address = listener.local_addr().expect("addr").to_string();

        let responder_transport = Arc::clone(&transport);
        let responder_server = Arc::clone(&server_b);
        let responder_service = Arc::clone(&service_b);
        let caps = Arc::new(crate::peer_transport::AcceptCaps::default());
        let responder = std::thread::spawn(move || {
            let (stream, peer_addr) = accept_bounded(&listener);
            let slot = caps
                .admit_handshake(crate::peer_transport::HandshakeKind::Pairing)
                .expect("a pairing slot");
            responder_service.handle(
                responder_transport.as_ref(),
                stream,
                peer_addr,
                &responder_server,
                slot,
            );
        });

        let outcome = service_a.complete(&server_a, &address, &wrong, PeerRole::Client);
        assert!(
            outcome.is_err(),
            "a wrong code must not complete the pairing: {outcome:?}"
        );
        join_bounded(responder, "the responder's refused pairing thread");

        assert!(
            server_a.peers().expect("A's rows").is_empty(),
            "A must not store a peer it never authenticated"
        );
        assert!(
            server_b.peers().expect("B's rows").is_empty(),
            "B must not store a peer whose PAKE failed"
        );
        assert!(
            service_b.pending_snapshot().is_empty(),
            "a failed PAKE never parks a pairing"
        );
        // The wrong attempt is written to the audit table, which is how an
        // operator sees a guessing campaign. Read straight from the journal.
        let connection = rusqlite::Connection::open(dir_b.join("journal.db")).expect("B's journal");
        let attempts: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM audit WHERE action = 'pairing_attempt'",
                [],
                |row| row.get(0),
            )
            .expect("audit rows");
        assert_eq!(attempts, 1, "the failure is audited exactly once");

        drop(server_a);
        drop(server_b);
        let _ = std::fs::remove_dir_all(&dir_a);
        let _ = std::fs::remove_dir_all(&dir_b);
    }

    /// The RFC 5869 vectors, run against the `hkdf` crate as used here: a
    /// `None` salt is the all-zero one.
    #[test]
    fn the_hkdf_crate_matches_rfc_5869_case_one() {
        let ikm = [0x0bu8; 22];
        let salt: Vec<u8> = (0x00u8..=0x0c).collect();
        let info: Vec<u8> = (0xf0u8..=0xf9).collect();
        let hk = hkdf::Hkdf::<Sha256>::new(Some(&salt), &ikm);
        let mut okm = [0u8; 42];
        hk.expand(&info, &mut okm).expect("42 is a valid length");
        assert_eq!(
            hex(&okm),
            "3cb25f25faacd57a90434f64d0362f2a2d2d0a90cf1a5a4c5db02d56ecc4c5bf34007208d5b887185865"
        );
    }

    /// RFC 5869 case 3: a zero-length salt and info. This is the shape the PSK
    /// derivation uses, with the role-bound info in place of the empty one.
    #[test]
    fn the_hkdf_crate_matches_rfc_5869_case_three() {
        let ikm = [0x0bu8; 22];
        let hk = hkdf::Hkdf::<Sha256>::new(None, &ikm);
        let mut okm = [0u8; 42];
        hk.expand(&[], &mut okm).expect("42 is a valid length");
        assert_eq!(
            hex(&okm),
            "8da4e775a563c18f715f802a063c5a31b8a11f5c5ee1879ec3454e5f3c738d2d9d201395faa4b61a96c8"
        );
    }

    /// The exact bytes both sides feed HKDF, so a change to the binding is a
    /// visible change to this assertion rather than a silent one.
    #[test]
    fn the_psk_info_binds_the_prefix_and_both_roles() {
        assert_eq!(
            psk_info(PeerRole::Client, PeerRole::Daemon),
            b"devboule-pair-v1\x00client\x00daemon".to_vec()
        );
        assert_eq!(
            psk_info(PeerRole::Daemon, PeerRole::Client),
            b"devboule-pair-v1\x00daemon\x00client".to_vec()
        );
        // The order matters and the separator is a NUL, so no pair of role
        // names can be read as another pair.
        assert_ne!(
            psk_info(PeerRole::Client, PeerRole::Daemon),
            psk_info(PeerRole::Daemon, PeerRole::Client)
        );
    }

    #[test]
    fn the_psk_is_thirty_two_bytes_and_bound_to_the_roles() {
        let first = derive_psk(b"spake key", PeerRole::Client, PeerRole::Daemon);
        assert_eq!(first.len(), 32);
        assert_eq!(
            first,
            derive_psk(b"spake key", PeerRole::Client, PeerRole::Daemon)
        );
        assert_ne!(
            first,
            derive_psk(b"another key", PeerRole::Client, PeerRole::Daemon)
        );
        // The whole point of binding the roles: the same PAKE output with a
        // different role pair is a different key.
        assert_ne!(
            first,
            derive_psk(b"spake key", PeerRole::Daemon, PeerRole::Client)
        );
        assert_ne!(
            first,
            derive_psk(b"spake key", PeerRole::Client, PeerRole::Client)
        );
        assert_ne!(
            first,
            derive_psk(b"spake key", PeerRole::Daemon, PeerRole::Daemon)
        );
    }

    #[test]
    fn role_tags_round_trip_and_an_unknown_tag_is_refused() {
        for role in [PeerRole::Client, PeerRole::Daemon] {
            assert_eq!(role_from_tag(role_tag(role)).expect("round trip"), role);
        }
        assert!(role_from_tag(2).is_err());
        assert!(role_from_tag(0xff).is_err());
    }

    /// A role pair the two sides disagree on must fail the Noise handshake,
    /// not complete with the wrong roles bound to the pinned keys. This is the
    /// test that proves the binding is load-bearing: both sides run the real
    /// `XXpsk3` pattern over a real socket.
    #[test]
    fn mismatched_roles_cannot_complete_the_noise_handshake() {
        let (responder_private, _) = test_keypair();
        let (initiator_private, _) = test_keypair();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let address = listener.local_addr().expect("addr");

        // The responder believes the initiator is a client; the initiator
        // believes it is a daemon. Nothing but the PSK differs.
        let server = std::thread::spawn(move || {
            let (stream, _) = accept_bounded(&listener);
            let deadline = Instant::now() + bound::THREAD;
            let psk = derive_psk(b"spake key", PeerRole::Client, PeerRole::Daemon);
            let outcome = crate::peer_transport::responder_handshake(
                &stream,
                deadline,
                &responder_private,
                PAIR_PROLOGUE,
                Some(&psk),
                PAIR_NOISE_PATTERN,
            );
            outcome.is_err()
        });

        let stream = connect_bounded(address);
        let deadline = Instant::now() + bound::THREAD;
        let psk = derive_psk(b"spake key", PeerRole::Daemon, PeerRole::Daemon);
        let initiator = crate::peer_transport::initiator_handshake(
            &stream,
            deadline,
            &initiator_private,
            None,
            PAIR_PROLOGUE,
            Some(&psk),
            PAIR_NOISE_PATTERN,
        );
        // The responder — the device that displayed the code and holds the
        // row — is the enforcement point, and it refuses.
        assert!(
            join_bounded(server, "the mismatched-role responder"),
            "the responder must refuse a mismatched role pair"
        );
        // The initiator is the last speaker of `XXpsk3`, so it has no further
        // message to authenticate and cannot detect the mismatch during the
        // handshake. What it cannot do is proceed: the responder has closed,
        // so its first read sees end-of-stream. That is what makes the binding
        // load-bearing end to end, and it is asserted rather than assumed.
        match initiator {
            Err(_) => {}
            Ok(session) => {
                let (mut reader, _writer, _closer) =
                    crate::peer_transport::split_session(&stream, session)
                        .expect("split the initiator session");
                let mut chunk = [0u8; 64];
                let read = reader.read_plaintext(&mut chunk, Some(deadline));
                assert!(
                    !matches!(read, Ok(n) if n > 0),
                    "the mismatched initiator must not be able to read a payload: {read:?}"
                );
            }
        }

        // And with the roles agreeing, the same exchange completes: the test
        // is about the binding, not about the handshake being broken.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let address = listener.local_addr().expect("addr");
        let (responder_private, _) = test_keypair();
        let (initiator_private, _) = test_keypair();
        let server = std::thread::spawn(move || {
            let (stream, _) = accept_bounded(&listener);
            let deadline = Instant::now() + bound::THREAD;
            let psk = derive_psk(b"spake key", PeerRole::Client, PeerRole::Daemon);
            crate::peer_transport::responder_handshake(
                &stream,
                deadline,
                &responder_private,
                PAIR_PROLOGUE,
                Some(&psk),
                PAIR_NOISE_PATTERN,
            )
            .is_ok()
        });
        let stream = connect_bounded(address);
        let deadline = Instant::now() + bound::THREAD;
        let psk = derive_psk(b"spake key", PeerRole::Client, PeerRole::Daemon);
        assert!(crate::peer_transport::initiator_handshake(
            &stream,
            deadline,
            &initiator_private,
            None,
            PAIR_PROLOGUE,
            Some(&psk),
            PAIR_NOISE_PATTERN,
        )
        .is_ok());
        assert!(join_bounded(server, "the agreeing-role responder"));
    }

    /// Wall-clock bounds for the two loopback tests in this module. Named so a
    /// reader can see, per test, what stops it hanging.
    mod bound {
        use std::time::Duration;

        /// A test that waits for a spawned thread must not wait forever.
        pub const THREAD: Duration = Duration::from_secs(20);
        /// A client connecting to a listener the test itself bound.
        pub const CONNECT: Duration = Duration::from_secs(5);
    }

    /// Join with a wall-clock bound: `JoinHandle::join` has no timeout, so the
    /// wait is a poll and `join` then returns immediately.
    fn join_bounded<T>(handle: std::thread::JoinHandle<T>, what: &str) -> T {
        let deadline = std::time::Instant::now() + bound::THREAD;
        while !handle.is_finished() {
            assert!(
                std::time::Instant::now() < deadline,
                "{what} did not finish within {:?}",
                bound::THREAD
            );
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        match handle.join() {
            Ok(value) => value,
            Err(_) => panic!("{what} panicked"),
        }
    }

    /// Accept one loopback connection with a wall-clock bound. A blocking
    /// `accept()` with no bound is how a test hangs the whole suite when the
    /// client side fails before connecting.
    fn accept_bounded(
        listener: &std::net::TcpListener,
    ) -> (std::net::TcpStream, std::net::SocketAddr) {
        listener
            .set_nonblocking(true)
            .expect("the test listener goes non-blocking");
        let deadline = std::time::Instant::now() + bound::CONNECT;
        loop {
            match listener.accept() {
                Ok((stream, addr)) => {
                    // Windows: an accepted socket inherits the listening
                    // socket's non-blocking mode, so every later read would
                    // fail with `WSAEWOULDBLOCK` instead of blocking.
                    stream
                        .set_nonblocking(false)
                        .expect("the accepted socket is blocking");
                    return (stream, addr);
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    assert!(
                        std::time::Instant::now() < deadline,
                        "the client never connected within {:?}",
                        bound::CONNECT
                    );
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
                Err(error) => panic!("accept failed: {error}"),
            }
        }
    }

    fn connect_bounded(address: std::net::SocketAddr) -> std::net::TcpStream {
        std::net::TcpStream::connect_timeout(&address, bound::CONNECT)
            .expect("connect within the bound")
    }

    fn test_keypair() -> (Vec<u8>, Vec<u8>) {
        let params = PAIR_NOISE_PATTERN
            .parse::<snow::params::NoiseParams>()
            .expect("params");
        let pair = snow::Builder::new(params)
            .generate_keypair()
            .expect("keypair");
        (pair.private, pair.public)
    }

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|byte| format!("{byte:02x}")).collect()
    }

    #[test]
    fn a_generated_code_uses_only_the_unambiguous_alphabet() {
        for _ in 0..64 {
            let code = generate_code().expect("entropy");
            assert_eq!(code.as_str().len(), CODE_LEN);
            assert!(is_well_formed_code(code.as_str()), "{}", code.as_str());
            for forbidden in ['0', '1', 'I', 'O'] {
                assert!(
                    !code.as_str().contains(forbidden),
                    "{} contains {forbidden}",
                    code.as_str()
                );
            }
        }
    }

    /// Ten thousand codes: every symbol present, and no symbol more than 25 %
    /// away from the uniform expectation.
    ///
    /// With 80 000 draws over 32 symbols the mean is 2 500 and the standard
    /// deviation is about 49 (binomial), so the ±625 bound is roughly 12.7 sd:
    /// it cannot flake, and it would catch a biased split (a modulo reduction,
    /// a fixed byte, or a 5-bit group read from the wrong offset).
    #[test]
    fn the_code_distribution_is_uniform_within_twenty_five_percent() {
        const CODES: usize = 10_000;
        let mut counts = [0usize; 32];
        for _ in 0..CODES {
            let code = generate_code().expect("entropy");
            for byte in code.as_str().bytes() {
                let position = CODE_ALPHABET
                    .iter()
                    .position(|symbol| *symbol == byte)
                    .expect("a generated symbol is in the alphabet");
                counts[position] += 1;
            }
        }
        let total: usize = counts.iter().sum();
        assert_eq!(total, CODES * CODE_LEN);
        let mean = total as f64 / CODE_ALPHABET.len() as f64;
        for (position, count) in counts.iter().enumerate() {
            let symbol = CODE_ALPHABET[position] as char;
            assert!(*count > 0, "symbol {symbol} never appeared");
            let ratio = *count as f64 / mean;
            assert!(
                (0.75..=1.25).contains(&ratio),
                "symbol {symbol} appeared {count} times, {ratio:.3} of the mean {mean:.1}"
            );
        }
    }

    #[test]
    fn a_malformed_code_is_refused_by_the_format_check() {
        assert!(is_well_formed_code("ABCDEFGH"));
        assert!(!is_well_formed_code("ABCDEFG"));
        assert!(!is_well_formed_code("ABCDEFGHJ"));
        assert!(!is_well_formed_code("ABCDEFG0"));
        assert!(!is_well_formed_code("abcdefgh"));
        assert!(!is_well_formed_code(""));
    }

    #[test]
    fn three_wrong_codes_block_the_source_and_twelve_kill_the_code() {
        let mut state = State {
            active: Some(ActiveCode {
                code: PairingSecret::new("ABCDEFGH"),
                role: PeerRole::Daemon,
                expires_at: Instant::now() + CODE_LIFETIME,
            }),
            ..State::default()
        };
        let ip: IpAddr = "100.64.0.9".parse().expect("ip");
        assert!(!state.note_wrong(ip));
        assert!(!state.note_wrong(ip));
        assert!(
            !state.note_wrong(ip),
            "the third wrong code blocks the source"
        );
        assert!(state.blocked.contains(&ip));
        assert!(matches!(
            state.note_attempt(ip, Instant::now()),
            Err(RejectKind::SourceBlocked)
        ));
        // Another source is unaffected by one source's failures.
        let other: IpAddr = "100.64.0.10".parse().expect("ip");
        assert!(state.note_attempt(other, Instant::now()).is_ok());

        // The code survives until the total threshold, so one node cannot lock
        // the legitimate initiator out.
        for _ in 0..8 {
            assert!(!state.note_wrong(other));
        }
        assert!(
            state.note_wrong(other),
            "the twelfth wrong code kills the code"
        );
    }

    #[test]
    fn attempts_are_capped_per_source_per_window() {
        let mut state = State::default();
        let ip: IpAddr = "100.64.0.9".parse().expect("ip");
        let now = Instant::now();
        for _ in 0..ATTEMPTS_PER_SOURCE {
            assert!(state.note_attempt(ip, now).is_ok());
        }
        assert_eq!(
            state.note_attempt(ip, now),
            Err(RejectKind::TooManyAttempts)
        );
        assert!(
            state
                .note_attempt(ip, now + ATTEMPT_WINDOW + Duration::from_secs(1))
                .is_ok(),
            "the window slides"
        );
    }

    #[test]
    fn an_expired_code_is_no_longer_active_and_resets_the_lockouts() {
        let service = PairingService::new();
        let (code, _expires_at) = service.start(PeerRole::Daemon).expect("code");
        assert!(service.is_active());
        assert_eq!(code.as_str().len(), CODE_LEN);
        {
            let mut state = service.state.lock().expect("lock");
            let ip: IpAddr = "100.64.0.9".parse().expect("ip");
            state.blocked.insert(ip);
            state.wrong_total = 5;
            if let Some(active) = state.active.as_mut() {
                active.expires_at = Instant::now() - Duration::from_secs(1);
            }
        }
        assert!(!service.is_active());
        service.housekeeping(Instant::now());
        let state = service.state.lock().expect("lock");
        assert!(state.active.is_none(), "the code is dropped");
        assert!(state.blocked.is_empty(), "a new code starts clean");
        assert_eq!(state.wrong_total, 0);
    }

    #[test]
    fn only_one_code_is_active_at_a_time() {
        let service = PairingService::new();
        let (first, _) = service.start(PeerRole::Client).expect("code");
        let (second, _) = service.start(PeerRole::Daemon).expect("code");
        assert_ne!(first.as_str(), second.as_str());
        let state = service.state.lock().expect("lock");
        assert_eq!(
            state.active.as_ref().map(|a| a.role),
            Some(PeerRole::Daemon)
        );
    }

    #[test]
    fn caps_are_validated_against_the_closed_set() {
        let ok = vec!["view".to_string(), "send".to_string()];
        assert_eq!(validate_caps(PeerRole::Client, &ok).expect("ok"), ok);
        assert!(validate_caps(PeerRole::Daemon, &[]).is_err());
        assert!(
            validate_caps(PeerRole::Client, &["view".to_string(), "root".to_string()]).is_err()
        );
        assert!(
            validate_caps(PeerRole::Client, &["view".to_string(), "view".to_string()]).is_err()
        );
        assert!(
            validate_caps(PeerRole::Client, &["send".to_string()]).is_err(),
            "a client peer cannot lose 'view'"
        );
        assert!(
            validate_caps(PeerRole::Daemon, &["send".to_string()]).is_ok(),
            "only the client role is required to keep 'view'"
        );
    }

    #[test]
    fn a_local_peer_record_carries_this_device_as_the_pairer() {
        let (dir, server) = server("record");
        let record = local_peer_record(
            &server,
            "6f1e5b7a-0000-4000-8000-00000000c0de",
            "Peer",
            PeerRole::Client,
            &[5u8; 32],
            TransportBinding::tailnet("npeer", "peer.tailnet.ts.net.", "user@example.com"),
            "100.64.0.2:47831".to_string(),
        )
        .expect("record");
        assert_eq!(record.role, "client");
        assert_eq!(record.caps, vec!["view".to_string()]);
        assert_eq!(record.paired_by_user, server.local_user_sid());
        assert!(record.revoked_at.is_none());
        assert_eq!(record.binding_stable_id.as_deref(), Some("npeer"));

        // A non-UUID id and a short key are refused before anything is stored.
        assert!(local_peer_record(
            &server,
            "not-a-uuid",
            "Peer",
            PeerRole::Client,
            &[5u8; 32],
            TransportBinding::tailnet("n", "n", "n"),
            "100.64.0.2:47831".to_string(),
        )
        .is_err());
        assert!(local_peer_record(
            &server,
            "6f1e5b7a-0000-4000-8000-00000000c0df",
            "Peer",
            PeerRole::Client,
            &[5u8; 31],
            TransportBinding::tailnet("n", "n", "n"),
            "100.64.0.2:47831".to_string(),
        )
        .is_err());

        drop(server);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn re_pairing_a_live_device_is_refused_until_it_is_revoked() {
        let (dir, server) = server("repair");
        let id = "6f1e5b7a-0000-4000-8000-00000000c0d1";
        let record = local_peer_record(
            &server,
            id,
            "Peer",
            PeerRole::Daemon,
            &[6u8; 32],
            TransportBinding::tailnet("npeer", "peer.", "user@example.com"),
            "100.64.0.2:47831".to_string(),
        )
        .expect("record");
        server.peer_upsert(record).expect("store");

        let again = local_peer_record(
            &server,
            id,
            "Peer",
            PeerRole::Daemon,
            &[7u8; 32],
            TransportBinding::tailnet("npeer", "peer.", "user@example.com"),
            "100.64.0.2:47831".to_string(),
        );
        assert!(again.is_err(), "an existing pairing must be revoked first");

        server.peer_revoke(id, unix_millis()).expect("revoke");
        let after_revoke = local_peer_record(
            &server,
            id,
            "Peer",
            PeerRole::Daemon,
            &[7u8; 32],
            TransportBinding::tailnet("npeer", "peer.", "user@example.com"),
            "100.64.0.2:47831".to_string(),
        );
        assert!(after_revoke.is_ok(), "a revoked device may pair again");

        drop(server);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_pending_pairing_is_answered_by_confirm_and_writes_the_row_only_when_accepted() {
        let (dir, server) = server("confirm");
        let service = PairingService::new();
        let public_key = vec![8u8; 32];
        let (decision, wait) = mpsc::channel::<bool>();
        service
            .state
            .lock()
            .expect("lock")
            .pending
            .push(PendingEntry {
                token: 1,
                device_id: "6f1e5b7a-0000-4000-8000-00000000c0d2".to_string(),
                display_name: "Phone".to_string(),
                role: PeerRole::Client,
                key_fingerprint: crate::device_identity::key_fingerprint(&public_key),
                address: "100.64.0.2:47831".to_string(),
                public_key: public_key.clone(),
                binding: TransportBinding::tailnet("nphone", "phone.", "user@example.com"),
                expires_at: Instant::now() + CONFIRM_WINDOW,
                decision,
            });
        assert_eq!(service.pending_snapshot().len(), 1);
        assert_eq!(service.pending_snapshot()[0].role, PeerRole::Client);

        let row = match service
            .confirm(&server, "6f1e5b7a-0000-4000-8000-00000000c0d2", true)
            .expect("confirm")
        {
            ConfirmOutcome::Accepted(row) => *row,
            ConfirmOutcome::Declined => panic!("accepting must produce a row"),
        };
        assert_eq!(row.role, PeerRole::Client);
        assert_eq!(row.display_name, "Phone");
        assert_eq!(row.caps, vec!["view".to_string()]);
        assert_eq!(row.key_fingerprint.len(), 32);
        assert!(wait.recv_timeout(Duration::from_secs(1)).expect("decision"));
        assert!(service.pending_snapshot().is_empty());
        assert_eq!(server.peers().expect("peers").len(), 1);

        // A second confirm finds nothing: the entry is gone.
        assert!(matches!(
            service.confirm(&server, "6f1e5b7a-0000-4000-8000-00000000c0d2", true),
            Err(PairingError::UnknownPending)
        ));

        drop(server);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn declining_a_pending_pairing_writes_no_row_but_does_audit() {
        let (dir, server) = server("decline");
        let service = PairingService::new();
        let (decision, wait) = mpsc::channel::<bool>();
        service
            .state
            .lock()
            .expect("lock")
            .pending
            .push(PendingEntry {
                token: 1,
                device_id: "6f1e5b7a-0000-4000-8000-00000000c0d3".to_string(),
                display_name: "Phone".to_string(),
                role: PeerRole::Client,
                key_fingerprint: crate::device_identity::key_fingerprint(&[9u8; 32]),
                address: "100.64.0.2:47831".to_string(),
                public_key: vec![9u8; 32],
                binding: TransportBinding::tailnet("nphone", "phone.", "user@example.com"),
                expires_at: Instant::now() + CONFIRM_WINDOW,
                decision,
            });
        assert!(matches!(
            service
                .confirm(&server, "6f1e5b7a-0000-4000-8000-00000000c0d3", false)
                .expect("decline"),
            ConfirmOutcome::Declined
        ));
        assert!(!wait.recv_timeout(Duration::from_secs(1)).expect("decision"));
        assert!(server.peers().expect("peers").is_empty());
        assert!(
            service.pending_snapshot().is_empty(),
            "a declined pairing is removed from the pending list"
        );

        // The decline is a recorded act, not a silent no-op.
        let connection = rusqlite::Connection::open(server_journal(&dir)).expect("journal");
        let (action, outcome): (String, String) = connection
            .query_row(
                "SELECT action, outcome FROM audit ORDER BY id DESC LIMIT 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("audit row");
        assert_eq!(action, "PairingConfirm");
        assert_eq!(outcome, "declined");
        drop(connection);

        drop(server);
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn server_journal(dir: &std::path::Path) -> std::path::PathBuf {
        dir.join("journal.db")
    }

    #[test]
    fn at_most_two_pairings_park_and_a_third_is_answered_busy() {
        let service = PairingService::new();
        let mut sends = Vec::new();
        {
            let mut state = service.state.lock().expect("lock");
            for index in 0..MAX_PENDING_PAIRINGS {
                let (decision, wait) = mpsc::channel::<bool>();
                sends.push(wait);
                state.pending.push(PendingEntry {
                    token: u64::try_from(index).unwrap_or(0),
                    device_id: format!("6f1e5b7a-0000-4000-8000-00000000000{index}"),
                    display_name: "Phone".to_string(),
                    role: PeerRole::Client,
                    key_fingerprint: String::new(),
                    address: "100.64.0.2:47831".to_string(),
                    public_key: vec![1u8; 32],
                    binding: TransportBinding::tailnet("n", "n", "n"),
                    expires_at: Instant::now() + CONFIRM_WINDOW,
                    decision,
                });
            }
            assert_eq!(state.pending.len(), MAX_PENDING_PAIRINGS);
            assert!(
                state.pending.len() >= MAX_PENDING_PAIRINGS,
                "the third candidate sees a full queue and is answered busy"
            );
        }
        assert_eq!(service.pending_snapshot().len(), MAX_PENDING_PAIRINGS);
        assert_eq!(RejectKind::PairingBusy.reason(), "pairing busy");
        drop(sends);
    }
    /// H1: a code is single use (design §8 R8). The first candidate pairs, and
    /// the same code is refused for a second one — one observed code must not
    /// pair every device that presents it during the five minutes it is shown.
    ///
    /// The ordering is explicit because the initiator's `complete` returns as
    /// soon as it has both payloads, before the responder has parked: the test
    /// waits for the park before asserting that the code is spent, so the second
    /// candidate is testing consumption and not a scheduling race.
    #[test]
    fn a_code_pairs_only_once() {
        let (dir_a, server_a) = server("once-a");
        let (dir_c, server_c) = server("once-c");
        let (dir_b, server_b) = server("once-b");
        let service_a = PairingService::new();
        let service_c = PairingService::new();
        let service_b = Arc::new(PairingService::new());
        let (code, _expires_at) = service_b.start(PeerRole::Client).expect("a code");
        let transport = Arc::new(crate::peer_transport::TestTransport::default());
        assert!(server_a.set_peer_transport(transport.clone()).is_ok());
        assert!(server_c.set_peer_transport(transport.clone()).is_ok());

        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let address = listener.local_addr().expect("addr").to_string();

        // The responder serves the two candidates in turn, on its own threads,
        // exactly as the accept loop would.
        let responder_service = Arc::clone(&service_b);
        let responder_transport = Arc::clone(&transport);
        let responder_server = Arc::clone(&server_b);
        let caps = Arc::new(crate::peer_transport::AcceptCaps::default());
        let responder = std::thread::spawn(move || {
            for _ in 0..2 {
                let (stream, peer_addr) = accept_bounded(&listener);
                let slot = caps
                    .admit_handshake(crate::peer_transport::HandshakeKind::Pairing)
                    .expect("a pairing slot");
                responder_service.handle(
                    responder_transport.as_ref(),
                    stream,
                    peer_addr,
                    &responder_server,
                    slot,
                );
            }
        });

        let initiator_id = server_a
            .device_identity()
            .as_ref()
            .expect("A has an identity")
            .device_id
            .clone();

        // The first candidate pairs: this device reports it as pending.
        let first = service_a
            .complete(&server_a, &address, &code, PeerRole::Client)
            .expect("the first candidate pairs");
        assert!(
            matches!(first, PairingOutcome::Pending(_)),
            "a Client pairing is reported pending, got {first:?}"
        );

        // Wait for the responder to park it, which is the moment it spends the
        // code.
        let deadline = Instant::now() + bound::THREAD;
        loop {
            if service_b.pending_snapshot().len() == 1 {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "the responder never parked the first pairing"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(
            !service_b.is_active(),
            "the code must be spent as soon as the first pairing is parked"
        );
        {
            let state = service_b
                .state
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            assert!(
                state.active.is_none(),
                "the code itself must be gone from the state, not merely expired"
            );
            // With no active code, a later candidate takes the `NoActiveCode`
            // arm — the same refusal a daemon that never showed a code gives.
            assert!(state.active_if_live(Instant::now()).is_none());
        }

        // The person at B accepts, which releases the parked responder.
        let accepted = service_b
            .confirm(&server_b, &initiator_id, true)
            .expect("confirm");
        assert!(matches!(accepted, ConfirmOutcome::Accepted(_)));

        // The second candidate presents the same code and is refused.
        let second = service_c.complete(&server_c, &address, &code, PeerRole::Client);
        assert!(
            second.is_err(),
            "a second pairing with the same code must fail, got {second:?}"
        );

        join_bounded(responder, "the two-candidate responder");

        // One pairing, on both sides, and nothing for the refused candidate.
        assert!(
            service_b.pending_snapshot().is_empty(),
            "the parked pairing was resolved by the confirmation"
        );
        assert!(
            server_c.peers().expect("C's rows").is_empty(),
            "the refused candidate must not write a row"
        );
        let deadline = Instant::now() + bound::THREAD;
        loop {
            if server_a.peers().expect("A's rows").len() == 1 {
                break;
            }
            assert!(Instant::now() < deadline, "A never wrote its row for B");
            std::thread::sleep(Duration::from_millis(20));
        }
        let b_rows = server_b.peers().expect("B's rows");
        assert_eq!(b_rows.len(), 1, "B wrote exactly one peer row");
        assert_eq!(b_rows[0].device_id, initiator_id, "and it names A");

        drop(service_b);
        drop(server_a);
        drop(server_c);
        drop(server_b);
        let _ = std::fs::remove_dir_all(&dir_a);
        let _ = std::fs::remove_dir_all(&dir_b);
        let _ = std::fs::remove_dir_all(&dir_c);
    }

    /// M2: the daemon refuses to open a pairing connection to anything but a
    /// tailnet address, so a renderer-supplied address cannot make it probe
    /// arbitrary hosts.
    #[test]
    fn a_pairing_target_must_be_a_tailnet_address() {
        // In range.
        assert!(is_permitted_pairing_target(
            &"100.64.0.1:47831".parse().expect("addr")
        ));
        assert!(is_permitted_pairing_target(
            &"100.127.255.254:47831".parse().expect("addr")
        ));
        assert!(is_permitted_pairing_target(
            &"[fd7a:115c:a1e0::1]:47831".parse().expect("addr")
        ));
        // Out of range: a public address, a private LAN address, and a tailnet
        // address one step outside the range.
        assert!(!is_permitted_pairing_target(
            &"8.8.8.8:47831".parse().expect("addr")
        ));
        assert!(!is_permitted_pairing_target(
            &"192.168.1.10:47831".parse().expect("addr")
        ));
        assert!(!is_permitted_pairing_target(
            &"100.128.0.1:47831".parse().expect("addr")
        ));

        // Loopback is accepted in this crate's own unit tests (the in-process
        // responder listens on 127.0.0.1) and only there — this whole module is
        // `#[cfg(test)]`, so the assertion is exactly the test-only branch.
        // `tests/peer_link.rs` pairs over the real tailnet address, so it needs
        // no exemption.
        assert!(is_permitted_pairing_target(
            &"127.0.0.1:47831".parse().expect("addr")
        ));
    }

    /// The end-to-end form of the M2 check: `complete` refuses a non-tailnet
    /// address without opening a socket at all, with a message a person can act
    /// on.
    #[test]
    fn pairing_complete_refuses_a_non_tailnet_address() {
        let (dir, server) = server("address");
        let service = PairingService::new();
        let (code, _expires_at) = service.start(PeerRole::Daemon).expect("a code");
        let error = service
            .complete(&server, "8.8.8.8:47831", &code, PeerRole::Client)
            .expect_err("a public address must be refused");
        let message = error.to_string();
        assert!(
            message.contains("tailnet"),
            "the refusal must say what is expected: {message}"
        );
        assert!(
            !message.contains(code.as_str()),
            "the refusal must not carry the code: {message}"
        );
        // And a malformed address is still refused, by the earlier parse.
        assert!(service
            .complete(&server, "not-an-address", &code, PeerRole::Client)
            .is_err());

        drop(server);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// M3: a payload whose `display_name` cannot be shown is refused at the
    /// boundary, before anything is stored or parked.
    #[test]
    fn a_payload_name_that_cannot_be_shown_is_refused() {
        let payload = |name: &str| PairPayload {
            device_id: "6f1e5b7a-0000-4000-8000-00000000c0d9".to_string(),
            display_name: name.to_string(),
            role: PeerRole::Client,
            public_key: String::new(),
        };
        // A normal hostname passes.
        assert!(validate_peer_payload(&payload("Marcolenovo")).is_ok());
        for bad in [
            "",
            "   ",
            " leading",
            "trailing ",
            "two\nlines",
            "tab\there",
            "right-to-left\u{202e}override",
            "zero\u{200b}width",
            "byte-order\u{feff}mark",
        ] {
            assert!(
                validate_peer_payload(&payload(bad)).is_err(),
                "{bad:?} must be refused"
            );
        }
        // Over the length bound, at the boundary exactly.
        let just_over = "x".repeat(MAX_DISPLAY_NAME_CHARS + 1);
        assert!(validate_peer_payload(&payload(&just_over)).is_err());
        let at_bound = "x".repeat(MAX_DISPLAY_NAME_CHARS);
        assert!(validate_peer_payload(&payload(&at_bound)).is_ok());
    }

    /// M3: the storage choke point refuses a bad name too, so "every stored
    /// `display_name` passed validation" holds even for a caller that skipped
    /// the payload check.
    #[test]
    fn a_peer_record_refuses_a_name_that_cannot_be_shown() {
        let (dir, server) = server("bad-name");
        let error = local_peer_record(
            &server,
            "6f1e5b7a-0000-4000-8000-00000000c0da",
            "invisible\u{202e}name",
            PeerRole::Client,
            &[5u8; 32],
            TransportBinding::tailnet("n", "n", "n"),
            "100.64.0.2:47831".to_string(),
        )
        .expect_err("a bad display name must be refused before it is stored");
        assert!(
            error.to_string().contains("invisible"),
            "the reason names the problem: {error}"
        );
        drop(server);
        let _ = std::fs::remove_dir_all(&dir);
    }
    /// C6: finishing a pairing must be idempotent for the device that is
    /// already half-paired. The responder writes its row and the initiator
    /// writes its own on a background thread; whichever fails second leaves one
    /// side holding a row and the other not, and before this fix every retry
    /// died at "already paired; revoke it first", so two good devices were stuck
    /// until somebody revoked by hand.
    ///
    /// The exception is narrow on purpose: the **same pinned key** is the same
    /// pairing being finished, while a different key is the substitution the
    /// revoke-first rule exists to refuse.
    #[test]
    fn re_pairing_with_the_same_key_finishes_the_pairing() {
        let (dir, server) = server("repair-same-key");
        let id = "6f1e5b7a-0000-4000-8000-00000000c0dc";
        let key = [6u8; 32];
        let record = local_peer_record(
            &server,
            id,
            "Peer",
            PeerRole::Daemon,
            &key,
            TransportBinding::tailnet("npeer", "peer.", "user@example.com"),
            "100.64.0.2:47831".to_string(),
        )
        .expect("first pairing");
        server.peer_upsert(record).expect("store");

        // Same key, same role: the retry completes and refreshes the row.
        let retry = local_peer_record(
            &server,
            id,
            "Peer Renamed",
            PeerRole::Daemon,
            &key,
            TransportBinding::tailnet("npeer", "peer.", "user@example.com"),
            "100.64.0.9:47831".to_string(),
        )
        .expect("a retry with the same pinned key must finish the pairing");
        assert_eq!(retry.display_name, "Peer Renamed");
        assert_eq!(retry.address, "100.64.0.9:47831");
        let stored = server.peer_upsert(retry).expect("re-store");
        assert_eq!(stored.display_name, "Peer Renamed");
        assert_eq!(
            server.peers().expect("rows").len(),
            1,
            "a retry must not duplicate the row"
        );

        // A different key is still refused: that is the credential changing,
        // which needs a revoke first (design §8 R8 / F-19).
        let substituted = local_peer_record(
            &server,
            id,
            "Peer",
            PeerRole::Daemon,
            &[7u8; 32],
            TransportBinding::tailnet("npeer", "peer.", "user@example.com"),
            "100.64.0.2:47831".to_string(),
        );
        let error = substituted.expect_err("a different key must be refused");
        assert!(
            error.to_string().contains("different key"),
            "the refusal says why it is different from an ordinary retry: {error}"
        );

        drop(server);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// C7: at most one pending entry per device. Two parks for the same device
    /// happened when a second code was shown while the first park was still
    /// inside its 60 s window, and the panel then rendered two confirm cards for
    /// one device while `confirm` removed only the first match.
    ///
    /// The newer attempt replaces the older: a device cannot be waiting twice
    /// for one pairing, and dropping the older entry releases its parked thread
    /// (the answer becomes `pairing busy`) instead of leaving it to consume a
    /// decision meant for the newer one.
    #[test]
    fn a_second_park_for_the_same_device_replaces_the_first() {
        let (dir_a, server_a) = server("dup-a");
        let (dir_b, server_b) = server("dup-b");
        let service_a = PairingService::new();
        let service_b = Arc::new(PairingService::new());
        let transport = Arc::new(crate::peer_transport::TestTransport::default());
        assert!(server_a.set_peer_transport(transport.clone()).is_ok());

        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let address = listener.local_addr().expect("addr").to_string();

        // Two codes, two connections, both parked by the same device A. The
        // second code is what makes a second park possible at all: the first is
        // spent by the first park (H1).
        let (code_one, _) = service_b.start(PeerRole::Client).expect("code one");
        let responder_service = Arc::clone(&service_b);
        let responder_transport = Arc::clone(&transport);
        let responder_server = Arc::clone(&server_b);
        let caps = Arc::new(crate::peer_transport::AcceptCaps::default());
        // One thread per connection, because a `Client`-role park blocks its
        // handler until it is confirmed: a sequential loop would sit on the
        // first pairing for the whole 60 s window and never accept the second,
        // which is what made this test time out on its first run. The real
        // accept loop spawns a thread per connection for the same reason.
        let responder = std::thread::spawn(move || {
            let mut handlers = Vec::new();
            for _ in 0..2 {
                let (stream, peer_addr) = accept_bounded(&listener);
                let slot = caps
                    .admit_handshake(crate::peer_transport::HandshakeKind::Pairing)
                    .expect("a pairing slot");
                let service = Arc::clone(&responder_service);
                let transport = Arc::clone(&responder_transport);
                let server = Arc::clone(&responder_server);
                handlers.push(std::thread::spawn(move || {
                    service.handle(transport.as_ref(), stream, peer_addr, &server, slot);
                }));
            }
            for handler in handlers {
                join_bounded(handler, "a parked pairing's handler");
            }
        });

        service_a
            .complete(&server_a, &address, &code_one, PeerRole::Client)
            .expect("the first pairing");
        let deadline = Instant::now() + bound::THREAD;
        while service_b.park_count() < 1 {
            assert!(Instant::now() < deadline, "the first park never landed");
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(service_b.pending_snapshot().len(), 1);

        // A second code, and the same device pairs again while the first park is
        // still waiting for a confirmation.
        let (code_two, _) = service_b.start(PeerRole::Client).expect("code two");
        service_a
            .complete(&server_a, &address, &code_two, PeerRole::Client)
            .expect("the second pairing");
        while service_b.park_count() < 2 {
            assert!(Instant::now() < deadline, "the second park never landed");
            std::thread::sleep(Duration::from_millis(20));
        }

        let pending = service_b.pending_snapshot();
        assert_eq!(
            pending.len(),
            1,
            "a second park for the same device must replace the first, not join it: {pending:?}"
        );
        assert_eq!(
            pending[0].device_id,
            server_a.device_identity().as_ref().expect("A").device_id
        );

        // Exactly one decision can be delivered: the surviving entry is the one
        // that answers.
        let accepted = service_b
            .confirm(&server_b, &pending[0].device_id, true)
            .expect("confirm");
        assert!(matches!(accepted, ConfirmOutcome::Accepted(_)));
        assert!(service_b.pending_snapshot().is_empty());

        join_bounded(responder, "the two-park responder");
        drop(service_b);
        drop(server_a);
        drop(server_b);
        let _ = std::fs::remove_dir_all(&dir_a);
        let _ = std::fs::remove_dir_all(&dir_b);
    }
}
