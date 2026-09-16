use std::collections::{HashMap, HashSet, VecDeque};
use std::path::Path;
#[cfg(not(test))]
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, SyncSender, TryRecvError};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use devboule_protocol::{
    caps, m3a_daemon_capabilities, negotiate, validate_idempotency_key, AgentMessageState,
    AttachmentReference, ClientMessage, DaemonHello, DaemonMessage, DaemonStatusBody, ErrorCode,
    JournalLimits as WireJournalLimits, JournalSessionUsage as WireJournalSessionUsage,
    JournalUsage as WireJournalUsage, OwnerId, PersistenceKind, PromptAttachment, ResumeResult,
    RetentionPatch, SessionEvent, SessionEventEnvelope, SessionKind,
    Unreclaimable as WireUnreclaimable, WireError, PROTOCOL_MIN_VERSION, PROTOCOL_VERSION,
};

use crate::device_identity::RemoteState;
use crate::diagnostics::{DiagnosticsInput, DiagnosticsReport};
use crate::error::DaemonError;
use crate::framing::Framed;
use crate::idempotency::{IdempotencyOutcome, IdempotencyStore};
use crate::journal::{AuditRecord, Journal, PeerMutation, PeerRecord, JOURNAL_SCHEMA_VERSION};
use crate::lock::SingleInstanceLock;
use crate::login_shell_env::login_shell_capture_outcome;
use crate::outbound::ConnOut;
use crate::paths::RuntimePaths;
use crate::peer_policy::{peer_allows, ConnPeer, PeerDecision, PeerRole};
use crate::peer_transport::{accept_peers, TokenBucket};
use crate::process_tree::JobObject;
use crate::provider_update::{NpmInstallRunner, ProcessNpmInstallRunner};
use crate::secret_store::SecretStore;
use crate::session::{ConnHandle, PendingEvent, SessionRegistry};
use crate::transport::{self, Listener};
use crate::IDLE_SHUTDOWN_GRACE;

const JOIN_SLICE: Duration = Duration::from_millis(10);
const JOIN_BUDGET: Duration = Duration::from_millis(500);
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(2);
/// How long a loaded `peers` table is trusted by the accept path (M1). Every
/// peer mutation also invalidates it, so this is a backstop for a mutation path
/// that does not, not the mechanism that keeps the table current.
const PEER_TABLE_TTL: Duration = Duration::from_secs(10);

#[path = "server/state.rs"]
mod state;
pub use state::ServerState;
use state::*;

#[path = "server/lifecycle.rs"]
mod lifecycle;
pub use lifecycle::run;
use lifecycle::*;

#[path = "server/diagnostics.rs"]
mod diagnostics;
use diagnostics::*;

#[path = "server/connection.rs"]
mod connection;
pub(crate) use connection::handle_client;
use connection::*;

#[path = "server/dispatch.rs"]
mod dispatch;
use dispatch::*;

#[path = "server/stores.rs"]
mod stores;
use stores::*;

#[path = "server/providers.rs"]
mod providers;
use providers::*;

#[path = "server/devices.rs"]
mod devices;
use devices::*;

#[path = "server/peer_gate.rs"]
mod peer_gate;
use peer_gate::*;

#[path = "server/journal_domain.rs"]
mod journal_domain;
use journal_domain::*;

#[path = "server/sessions.rs"]
mod sessions;
pub(crate) use sessions::unix_millis;
use sessions::*;
pub(crate) use sessions::{
    creation_retry_key, idempotent_creation_session, remember_creation_session,
};

#[cfg(test)]
#[path = "server_tests.rs"]
mod tests;
