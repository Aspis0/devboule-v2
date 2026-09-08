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
//! This crate speaks only version [`PROTOCOL_VERSION`] (3), with
//! [`PROTOCOL_MIN_VERSION`] also 3. Older dialects are refused: required
//! fields (`created_at_ms`, `Workspace.path`) were added and the daemon
//! always serializes the current struct, so agreeing on an older version
//! would not produce an old-shaped payload. Bumping
//! `PROTOCOL_MIN_VERSION` is how an old dialect is dropped.
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
//! # Screen snapshots (M3.5)
//!
//! On attach the daemon sends a [`SessionEvent::Snapshot`] with the current
//! emulator state instead of replaying past frames. Its `as_of_seq` field
//! is the sequence boundary on **application to the emulator**; the type's
//! documentation carries the invariant and the reason it is not allowed to
//! drift to pipe write, journal commit, or client receipt.
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
mod plugin;
mod project;
mod session;
#[cfg(test)]
mod session_event_guard;

pub use capability::{intersect_capabilities, Capability};
pub use error::{ErrorCode, ErrorDetails, WireError};
pub use handshake::{negotiate, ClientHello, DaemonHello, Negotiation};
pub use ids::{
    compose_session_id, validate_idempotency_key, validate_owner_token, validate_session_id,
    OwnerId,
};
pub use messages::{
    ClientMessage, DaemonMessage, DaemonStatusBody, JournalLimits, JournalRetention,
    JournalSessionUsage, JournalStats, JournalUsage, ProviderInfo, RetentionLimit, RetentionPatch,
    RetentionSource, SessionEventEnvelope, Unreclaimable,
};
pub use plugin::WorkspaceRootBody;
pub use project::{Project, Workspace, WorkspaceIsolation};
pub use session::{
    cursor_replay_ok, AgentActivityState, Attention, AttentionReason, AvailableCommandView, Cursor,
    CursorShape, PermissionEnvVar, PermissionOption, PermissionOutcome, Persistence,
    PersistenceKind, ResumeResult, ScreenCursor, Session, SessionEvent, SessionKind,
    SessionModeStateView, SessionModeView, SessionModel, SessionModelEffort, SessionState,
    SessionStateSnapshot, ToolLocation, TranscriptIntegrity, TurnUsage,
};

/// Current protocol dialect spoken by this crate.
///
/// A field added with `#[serde(default)]` is backward compatible and needs
/// no bump (`cwd`). A required field is a breaking change and requires
/// bumping both this constant and [`PROTOCOL_MIN_VERSION`] (`created_at_ms`,
/// `Workspace.path`).
/// The daemon always serializes the current struct regardless of the agreed
/// version, so negotiating down does not produce an old-shaped payload;
/// refusing the handshake is the only protection.
pub const PROTOCOL_VERSION: u32 = 3;
/// Oldest dialect this crate still accepts. Equal to [`PROTOCOL_VERSION`]
/// after a required-field change: agreeing on an older version would still
/// emit the new struct, and the peer would fail to parse it.
pub const PROTOCOL_MIN_VERSION: u32 = 3;

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

    /// Plugin-backend tenant. The host grants these at handshake from what
    /// the plugin manifest requested; a name the host does not know is
    /// ignored, not a handshake failure. Same open-set rule as the daemon.
    pub const WORKSPACE_ROOT: &str = "workspace.root";
    pub const CITY_GET: &str = "city.get";
    pub const FINDINGS_GET: &str = "findings.get";
    pub const FINDING_INSPECT: &str = "finding.inspect";
    pub const ORACLE_SEARCH: &str = "oracle.search";
    pub const GRAPH_IMPORTS: &str = "graph.imports";
    pub const SESSIONS_WATCH: &str = "sessions.watch";
    pub const AGENT_RUN: &str = "agent.run";
    pub const TYPED_PERMISSIONS: &str = "typed_permissions";
}

/// How long the daemon remembers an idempotency key, in seconds.
pub const IDEMPOTENCY_TTL_SECS: u64 = 15 * 60;
/// Maximum remembered idempotency entries per daemon process. Evict oldest.
pub const IDEMPOTENCY_MAX_ENTRIES: usize = 4096;

/// Compact JSON frames larger than this are a protocol error (1 MiB).
///
/// The largest ordinary frame is a screen snapshot: a dense 200x50 screen
/// where every cell repaints its 24-bit colours escapes to roughly 490 KiB
/// of JSON. That fits, but it is orders of magnitude larger than a typical
/// output frame — see the frame-cap test in the `session` module before
/// assuming snapshots are always small.
pub const MAX_FRAME_BYTES: usize = 1024 * 1024;

/// Default plugin-invoke payload budget, in bytes (16 MiB).
///
/// This is not the daemon's default NDJSON framing cap. Plugin backends talk
/// to the host over their own named pipe; their frame limit is derived from
/// this budget when the host starts that pipe.
///
/// What it is for: one structured JSON value on `plugin_invoke` — a city
/// graph, a findings ledger, a table of measurements. ~50,000 entities at
/// ~300 bytes of JSON each is about 15 MiB; 16 MiB is that working set with
/// a little headroom. A plugin that needs more declares it in its manifest,
/// and the host clamps the declaration to [`PLUGIN_PAYLOAD_CEILING_BYTES`].
///
/// What it is not for: binary assets. Images and micrographs must not travel
/// as base64 in this JSON; they go through the plugin asset server.
pub const DEFAULT_PLUGIN_PAYLOAD_BYTES: usize = 16 * 1024 * 1024;

/// Absolute host ceiling for a plugin-declared payload budget (64 MiB).
///
/// A manifest may ask for more; the host grants at most this. 64 MiB of
/// JSON is already a large structured document. Asking for gigabytes is
/// clamped, not a parse failure, and the clamp is visible on the manifest.
pub const PLUGIN_PAYLOAD_CEILING_BYTES: usize = 64 * 1024 * 1024;

/// Space reserved for the JSON envelope around a plugin invoke or result.
/// The payload budget measures the nested `Value`; `Framed` measures the
/// complete message. Capability names are capped at 128 characters by the
/// manifest parser, so this leaves ample room for the method and request id.
pub const PLUGIN_FRAME_HEADROOM_BYTES: usize = 4 * 1024;

/// The per-plugin transport cap corresponding to a payload budget.
pub const fn plugin_frame_limit_for_payload(payload_bytes: usize) -> usize {
    payload_bytes.saturating_add(PLUGIN_FRAME_HEADROOM_BYTES)
}

// The former DEFAULT_PLUGIN_PAYLOAD_BYTES > MAX_FRAME_BYTES assertion is gone:
// it was true while plugin Framed still used the daemon cap, so it certified
// numerical inequality instead of separate pipe boundaries. There is
// deliberately no compile-time comparison between these values: the plugin
// pipe's limit is derived above and carried through ClientHello, while daemon
// Framed::new keeps MAX_FRAME_BYTES.
const _: () = assert!(
    PLUGIN_PAYLOAD_CEILING_BYTES >= DEFAULT_PLUGIN_PAYLOAD_BYTES,
    "host ceiling must be at least the default budget"
);

/// Effective budget for one plugin: `min(declared.unwrap_or(default), ceiling)`.
///
/// Zero is refused by the manifest parser before this runs. This function
/// does not re-check: a `Some(0)` here would mean a caller bypassed that
/// parser, and a second refusal would hide the bypass.
pub fn effective_plugin_payload_bytes(declared: Option<u64>) -> usize {
    let asked = declared.unwrap_or(DEFAULT_PLUGIN_PAYLOAD_BYTES as u64);
    let capped = asked.min(PLUGIN_PAYLOAD_CEILING_BYTES as u64);
    usize::try_from(capped).unwrap_or(PLUGIN_PAYLOAD_CEILING_BYTES)
}

/// True when the manifest asked for more than the host will grant.
pub fn plugin_payload_budget_clamped(declared: Option<u64>) -> bool {
    declared.is_some_and(|asked| asked > PLUGIN_PAYLOAD_CEILING_BYTES as u64)
}

/// Names the limit that actually applies, and whether it came from the
/// host default, the manifest, or the host ceiling.
pub fn plugin_payload_limit_reason(declared: Option<u64>) -> String {
    let effective = effective_plugin_payload_bytes(declared);
    match declared {
        None => format!("maximum {effective} bytes (host default)"),
        Some(asked) if asked > PLUGIN_PAYLOAD_CEILING_BYTES as u64 => {
            format!("maximum {effective} bytes (host ceiling; manifest asked for {asked})")
        }
        Some(_) => format!("maximum {effective} bytes (plugin manifest)"),
    }
}

/// Whether a plugin invoke payload fits `limit` **serialized** JSON bytes.
///
/// This counts the bytes `serde_json` would write, not the in-memory size of
/// the [`serde_json::Value`]. It does not bound host memory: on the request
/// path the `Value` is already built by the time this runs.
///
/// Writes into a counting sink and aborts as soon as the count would exceed
/// `limit`, so an oversized payload is rejected without materialising it.
/// A payload whose serialized length equals `limit` is accepted. `None`
/// (no payload) always fits.
///
/// A serialization failure is treated as over-limit — the same `false` as
/// exceeding the budget. The sink still records whether it aborted because
/// the payload was too long (`exceeded`); this predicate does not expose
/// that distinction.
pub fn plugin_payload_within_limit(payload: Option<&serde_json::Value>, limit: usize) -> bool {
    let Some(value) = payload else {
        return true;
    };
    let mut sink = PayloadLimitSink {
        written: 0,
        limit,
        exceeded: false,
    };
    match serde_json::to_writer(&mut sink, value) {
        Ok(()) => true,
        Err(_) => false,
    }
}

/// Marker carried by the `io::Error` the sink returns when it aborts.
/// Distinct from a real I/O failure so the two cannot be confused by kind
/// alone (`ErrorKind::Other` is too wide).
#[derive(Debug)]
struct PayloadLimitExceeded;

impl std::fmt::Display for PayloadLimitExceeded {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("plugin payload exceeds the allowed serialized size")
    }
}

impl std::error::Error for PayloadLimitExceeded {}

struct PayloadLimitSink {
    written: usize,
    limit: usize,
    /// Set only when `write` refused further bytes because the next one
    /// would pass `limit`. Not set on any other error path.
    exceeded: bool,
}

impl std::io::Write for PayloadLimitSink {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        let remaining = self.limit.saturating_sub(self.written);
        if remaining == 0 {
            self.exceeded = true;
            return Err(std::io::Error::other(PayloadLimitExceeded));
        }
        let n = buf.len().min(remaining);
        self.written += n;
        Ok(n)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Capabilities this crate's daemon and app currently serve.
///
/// Named `m3a_*` because the handshake helpers were introduced in M3a; M3b
/// adds [`caps::SESSIONS`] without changing the helper names so a peer
/// built against this crate still calls the same constructors.
pub fn m3a_daemon_capabilities() -> Vec<Capability> {
    let mut capabilities = vec![
        Capability::new(caps::PING),
        Capability::new(caps::STATUS),
        Capability::new(caps::SHUTDOWN),
        Capability::new(caps::SESSIONS),
        Capability::new(caps::JOURNAL),
    ];
    capabilities.push(Capability::new(caps::TYPED_PERMISSIONS));
    capabilities
}

/// Capabilities the M3a app client offers.
pub fn m3a_client_capabilities() -> Vec<Capability> {
    let mut capabilities = vec![
        Capability::new(caps::PING),
        Capability::new(caps::STATUS),
        Capability::new(caps::SHUTDOWN),
        Capability::new(caps::SESSIONS),
        Capability::new(caps::JOURNAL),
    ];
    capabilities.push(Capability::new(caps::TYPED_PERMISSIONS));
    capabilities
}

/// Capabilities a plugin backend advertises today. The host may grant a
/// subset. Later plugin work adds names here; unknown names on either side
/// still complete the handshake.
pub fn plugin_backend_capabilities() -> Vec<Capability> {
    vec![
        Capability::new(caps::PING),
        Capability::new(caps::WORKSPACE_ROOT),
        Capability::new(caps::CITY_GET),
        Capability::new(caps::FINDINGS_GET),
        Capability::new(caps::FINDING_INSPECT),
    ]
}

/// The invoke method for a capability is the capability name. The host
/// refuses a method that was not in the negotiated set.
pub fn invoke_method_capability(method: &str) -> &str {
    method
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protocol_version_is_three_and_min_matches() {
        assert_eq!(PROTOCOL_VERSION, 3);
        assert_eq!(PROTOCOL_MIN_VERSION, 3);
    }

    #[test]
    fn daemon_and_client_advertise_sessions() {
        let daemon = m3a_daemon_capabilities();
        let client = m3a_client_capabilities();
        assert!(daemon.iter().any(|cap| cap.as_str() == caps::SESSIONS));
        assert!(client.iter().any(|cap| cap.as_str() == caps::SESSIONS));
        assert!(daemon.iter().any(|cap| cap.as_str() == caps::JOURNAL));
        assert!(client.iter().any(|cap| cap.as_str() == caps::JOURNAL));
        assert_eq!(daemon, client);
    }

    #[test]
    fn plugin_backend_is_a_second_tenant_not_a_daemon_capability() {
        let daemon = m3a_daemon_capabilities();
        let plugin = plugin_backend_capabilities();
        assert!(
            !daemon
                .iter()
                .any(|cap| cap.as_str() == caps::WORKSPACE_ROOT),
            "workspace.root belongs to the plugin tenant, not the daemon"
        );
        assert!(plugin.iter().any(|cap| cap.as_str() == caps::PING));
        assert!(plugin
            .iter()
            .any(|cap| cap.as_str() == caps::WORKSPACE_ROOT));
        assert!(plugin.iter().any(|cap| cap.as_str() == caps::CITY_GET));
        assert!(plugin.iter().any(|cap| cap.as_str() == caps::FINDINGS_GET));
        assert!(plugin
            .iter()
            .any(|cap| cap.as_str() == caps::FINDING_INSPECT));
        assert!(!plugin.iter().any(|cap| cap.as_str() == caps::STATUS));
        assert!(!plugin.iter().any(|cap| cap.as_str() == caps::SESSIONS));
        assert_eq!(
            invoke_method_capability("workspace.root"),
            caps::WORKSPACE_ROOT
        );
        assert_eq!(invoke_method_capability("findings.get"), caps::FINDINGS_GET);
        assert_eq!(
            invoke_method_capability("finding.inspect"),
            caps::FINDING_INSPECT
        );
    }

    #[test]
    fn plugin_payloads_are_measured_against_the_limit_they_are_given() {
        let value = serde_json::json!({"k": "v"});
        let size = serde_json::to_vec(&value)
            .expect("fixture must serialize")
            .len();
        assert!(
            size > 1,
            "fixture must have room for a just-under limit, got {size}"
        );
        assert!(
            plugin_payload_within_limit(Some(&value), size),
            "serialized length {size} must pass a limit of {size}"
        );
        assert!(
            plugin_payload_within_limit(Some(&value), size + 1),
            "serialized length {size} must pass a limit of {}",
            size + 1
        );
        assert!(
            !plugin_payload_within_limit(Some(&value), size - 1),
            "serialized length {size} must fail a limit of {}",
            size - 1
        );
        assert!(plugin_payload_within_limit(None, 0));
    }

    #[test]
    fn payload_limit_sink_aborts_past_the_limit_and_not_at_it() {
        use std::io::Write;
        let mut sink = PayloadLimitSink {
            written: 0,
            limit: 4,
            exceeded: false,
        };
        assert_eq!(sink.write(b"abcd").expect("exact fill"), 4);
        assert!(!sink.exceeded);
        let err = sink.write(b"x").expect_err("one past the limit");
        assert!(sink.exceeded);
        assert!(err
            .get_ref()
            .is_some_and(|inner| inner.is::<PayloadLimitExceeded>()));
        let mut partial = PayloadLimitSink {
            written: 0,
            limit: 3,
            exceeded: false,
        };
        assert_eq!(partial.write(b"abcd").expect("short write up to limit"), 3);
        assert!(!partial.exceeded);
        assert!(partial.write(b"d").is_err());
        assert!(partial.exceeded);

        // Through serde_json, which wraps the io::Error. The sink flag is
        // how the host tells a limit abort from a serialize failure:
        // serde_json::Error does not expose the typed payload.
        let value = serde_json::json!({"k": "v"});
        let size = serde_json::to_vec(&value)
            .expect("fixture must serialize")
            .len();
        let mut at_limit = PayloadLimitSink {
            written: 0,
            limit: size,
            exceeded: false,
        };
        serde_json::to_writer(&mut at_limit, &value)
            .expect("serialized length equal to the limit must complete");
        assert_eq!(at_limit.written, size);
        assert!(!at_limit.exceeded);

        let mut over = PayloadLimitSink {
            written: 0,
            limit: size - 1,
            exceeded: false,
        };
        serde_json::to_writer(&mut over, &value).expect_err("one byte past the limit must abort");
        assert!(over.exceeded);
        assert_eq!(over.written, size - 1);
    }

    #[test]
    fn plugin_payload_budget_defaults_and_clamps_independently_of_daemon_frame_cap() {
        assert_eq!(
            effective_plugin_payload_bytes(None),
            DEFAULT_PLUGIN_PAYLOAD_BYTES
        );
        assert_eq!(effective_plugin_payload_bytes(Some(1024)), 1024);
        assert_eq!(
            effective_plugin_payload_bytes(Some(u64::MAX)),
            PLUGIN_PAYLOAD_CEILING_BYTES
        );
        assert!(!plugin_payload_budget_clamped(None));
        assert!(!plugin_payload_budget_clamped(Some(1024)));
        assert!(plugin_payload_budget_clamped(Some(u64::MAX)));
        assert!(
            plugin_payload_limit_reason(None).contains(&DEFAULT_PLUGIN_PAYLOAD_BYTES.to_string())
        );
        assert!(plugin_payload_limit_reason(None).contains("host default"));
        assert!(plugin_payload_limit_reason(Some(1024)).contains("plugin manifest"));
        let clamped = plugin_payload_limit_reason(Some(u64::MAX));
        assert!(clamped.contains(&PLUGIN_PAYLOAD_CEILING_BYTES.to_string()));
        assert!(clamped.contains("host ceiling"));
        assert!(clamped.contains(&u64::MAX.to_string()));
    }

    #[test]
    fn plugin_frame_limit_is_payload_budget_plus_envelope_headroom() {
        assert_eq!(
            plugin_frame_limit_for_payload(4096),
            4096 + PLUGIN_FRAME_HEADROOM_BYTES
        );
        assert_eq!(
            plugin_frame_limit_for_payload(usize::MAX),
            usize::MAX,
            "deriving a transport limit must not wrap"
        );
    }
}
