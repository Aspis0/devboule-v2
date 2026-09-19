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
//! 5. inside Noise: `{device_id, display_name, role, public_key, listenPort}`
//!    each way, and the responder answers `{accepted, reason}`.
//!
//! `listenPort` is how the initiator tells the responder which port its own
//! listener is bound to: the responder would otherwise record the ephemeral
//! source port of the pairing connection, which belongs to nothing. The IP is
//! never carried — the responder keeps the one the kernel attested off
//! `accept()`, so a payload cannot point future dials at a third machine.
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
    /// The port this device's peer listener is bound to, so the other side can
    /// store a working address for it. `default` keeps a peer built before
    /// this field existed pairing instead of failing on a field it never
    /// heard of; the responder records port `0` for such a peer and a dial to
    /// `0` refuses, rather than anyone guessing.
    #[serde(default)]
    listen_port: Option<u16>,
}

/// This device's own payload: its identity, its role in this pairing, and the
/// port its peer listener actually bound. The port is read from the listener
/// state the daemon published when it bound — the bound socket is the truth —
/// not re-parsed from the environment.
fn own_payload(
    identity: &crate::device_identity::DeviceIdentity,
    role: PeerRole,
    server: &Arc<ServerState>,
) -> PairPayload {
    PairPayload {
        device_id: identity.device_id.clone(),
        display_name: identity.display_name.clone(),
        role,
        public_key: identity.public_key_b64(),
        listen_port: server.remote_port(),
    }
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
        // A `Daemon` peer promises to be callable, so this device must have a
        // listener to advertise: without one the responder records `:0`, both
        // screens say paired, and the row can never be dialled — re-pairing
        // would reproduce the same `0`. A `Client` peer makes no such promise
        // and may pair without a listener. The retry is the same one showing a
        // code uses, so the instruction below is one the daemon honours.
        if role == PeerRole::Daemon {
            if server.remote_port().is_none() {
                server.ensure_remote_listener();
            }
            if server.remote_port().is_none() {
                return Err(PairingError::Failed(
                    "this device has no tailnet listener to advertise; start Tailscale, then \
                     pair again"
                        .to_string(),
                ));
            }
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

        let payload = own_payload(&identity, role, server);
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
            &own_payload(&identity, responder_role, server),
            deadline,
        )?;

        // The binding is what **our** `whois` says about **their** address.
        // `self_node()` describes this device and is never consulted here.
        let binding = transport
            .binding(&peer_addr)
            .map_err(|error| PairingError::Failed(error.to_string()))?;
        let key_fingerprint = crate::device_identity::key_fingerprint(&remote_static);
        // The IP is the one the kernel attested off `accept()`, and **only the
        // port** may come from the payload. On this responder path, a payload
        // that could set the IP would let a paired device point future dials
        // at a third machine, and the pinned key would not stop the connection
        // being attempted there. (The initiator is different: it stores,
        // verbatim, the address its human typed.) The port is the listener the
        // initiator advertised inside this Noise session — keyed by the spoken
        // code, so only the device actually being paired with could have set
        // it. The accepted socket's own port is the initiator's ephemeral
        // source port and belongs to nothing. An absent advertisement records
        // `0`: a dial to `0` refuses, rather than anyone guessing. The address
        // is composed by `SocketAddr`, which brackets IPv6 by construction.
        let address = crate::peer_transport::compose_peer_address(
            peer_addr.ip(),
            payload.listen_port.unwrap_or(0),
        );

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
#[path = "pairing_tests.rs"]
mod tests;
