//! Devboule daemon protocol: wire types shared by the app and the daemon.
//!
//! This crate has **no I/O**, no tokio, and no Tauri. Both sides depend on it
//! so a request/response/event/error/handshake disagreement is a compile error
//! rather than a runtime deserialization failure in a user's hands.
//!
//! # Version compatibility
//!
//! Handshake is bidirectional. The client states the highest protocol version
//! it speaks and the lowest it can accept; the daemon does the same. The
//! agreed version is `min(client.protocol_version, daemon.protocol_version)`.
//! Handshake fails if that value is below either side's minimum.
//!
//! - **Daemon newer than the app**, ranges overlap: they speak the app's
//!   version. Extra daemon capabilities are ignored.
//! - **Daemon older than the app**, ranges overlap: they speak the daemon's
//!   version. The app must not send ops the daemon did not advertise.
//! - **No overlap**: the daemon replies with
//!   [`ErrorCode::ProtocolVersionMismatch`] and closes. The error names both
//!   versions and which binary to update. Neither side may hang or try to
//!   parse the rest of the stream as the other version.
//!
//! M3a speaks only version [`PROTOCOL_VERSION`] (1), with
//! [`PROTOCOL_MIN_VERSION`] also 1. Bumping `PROTOCOL_MIN_VERSION` is how a
//! future daemon or app *drops* an old dialect; until then, a newer daemon
//! must still accept version 1.
//!
//! Capabilities are an open string set, independently negotiated as the
//! intersection of what both sides listed. Unknown capability names MUST be
//! ignored (not a handshake failure). An RPC whose capability was not agreed
//! returns [`ErrorCode::CapabilityNotSupported`].
//!
//! # Framing (transport, not this crate)
//!
//! The byte transport is **newline-delimited compact JSON**. It is not defined
//! here so a Unix socket can replace a named pipe without touching these
//! types. See `devboule-daemon` for the choice and the max-frame cap.
//!
//! # Session identifiers
//!
//! A session id is an opaque string that **carries the owner**. New ids use
//! [`compose_session_id`]: `s.{owner}.{unique}`. The M2 in-process terminal
//! still mints `session-{pid}-{counter}`; [`validate_session_id`] accepts both.
//!
//! # `detach`, `close`, `stop`
//!
//! Three distinct operations. See [`ClientMessage`] variants. Collapsing any
//! two would change the protocol's meaning, not add a field later.
//!
//! # Cursor and generation
//!
//! Replay uses [`Cursor`]: `generation` names the backing process instance,
//! `seq` is the last output sequence the client has for that instance.
//! Replaying across a generation change is a
//! [`ErrorCode::SessionGenerationMismatch`], never a silent continuation.
//!
//! # Idempotency
//!
//! `session_create`, `session_send`, and `session_permission_respond` carry an
//! optional [`idempotency_key`]. Keys are remembered per owner for
//! [`IDEMPOTENCY_TTL_SECS`] seconds (capped at [`IDEMPOTENCY_MAX_ENTRIES`]).
//! A retry with the same key and the same payload returns the original
//! result; a retry with the same key and a different payload returns
//! [`ErrorCode::IdempotencyConflict`].

mod capability;
mod error;
mod handshake;
mod ids;
mod messages;
mod session;

pub use capability::{intersect_capabilities, Capability};
pub use error::{ErrorCode, ErrorDetails, WireError};
pub use handshake::{negotiate, ClientHello, DaemonHello, Negotiation};
pub use ids::{
    compose_session_id, validate_idempotency_key, validate_owner_token, validate_session_id,
    OwnerId,
};
pub use messages::{ClientMessage, DaemonMessage, DaemonStatusBody, SessionEventEnvelope};
pub use session::{
    cursor_replay_ok, Cursor, PermissionOutcome, Persistence, PersistenceKind, ResumeResult,
    Session, SessionEvent, SessionKind,
};

/// Current protocol dialect spoken by this crate.
pub const PROTOCOL_VERSION: u32 = 1;
/// Oldest dialect this crate still accepts. Equal to [`PROTOCOL_VERSION`] in
/// M3a; a future bump is how an old dialect is dropped.
pub const PROTOCOL_MIN_VERSION: u32 = 1;

/// Well-known capability names. These are strings on the wire so a peer that
/// does not know a name can still complete the handshake.
pub mod caps {
    pub const PING: &str = "ping";
    pub const STATUS: &str = "status";
    pub const SHUTDOWN: &str = "shutdown";
    /// Session RPCs (create/attach/detach/close/stop/send/…). Advertised
    /// from M3b so the app and daemon agree to speak them.
    pub const SESSIONS: &str = "sessions";
    /// Conversation journal. Advertised in M3c.
    pub const JOURNAL: &str = "journal";
}

/// How long the daemon remembers an idempotency key, in seconds.
pub const IDEMPOTENCY_TTL_SECS: u64 = 15 * 60;
/// Maximum remembered idempotency entries per daemon process. Evict oldest.
pub const IDEMPOTENCY_MAX_ENTRIES: usize = 4096;

/// Compact JSON frames larger than this are a protocol error (1 MiB).
pub const MAX_FRAME_BYTES: usize = 1024 * 1024;

/// Capabilities this crate's daemon and app currently serve.
///
/// Named `m3a_*` because the handshake helpers were introduced in M3a; M3b
/// adds [`caps::SESSIONS`] without changing the helper names so a peer
/// built against this crate still calls the same constructors.
pub fn m3a_daemon_capabilities() -> Vec<Capability> {
    vec![
        Capability::new(caps::PING),
        Capability::new(caps::STATUS),
        Capability::new(caps::SHUTDOWN),
        Capability::new(caps::SESSIONS),
    ]
}

/// Capabilities the M3a app client offers.
pub fn m3a_client_capabilities() -> Vec<Capability> {
    m3a_daemon_capabilities()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protocol_version_is_one_and_min_matches() {
        assert_eq!(PROTOCOL_VERSION, 1);
        assert_eq!(PROTOCOL_MIN_VERSION, 1);
    }

    #[test]
    fn daemon_and_client_advertise_sessions() {
        let daemon = m3a_daemon_capabilities();
        let client = m3a_client_capabilities();
        assert!(daemon.iter().any(|cap| cap.as_str() == caps::SESSIONS));
        assert!(client.iter().any(|cap| cap.as_str() == caps::SESSIONS));
        assert_eq!(daemon, client);
    }
}
