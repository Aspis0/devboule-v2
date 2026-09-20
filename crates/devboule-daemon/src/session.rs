//! Daemon-owned PTY sessions.
//!
//! This is the M2 terminal backend moved out of the Tauri process. The PTY
//! plumbing follows the permissively licensed `portable-pty` pattern used by
//! terax-ai (Apache-2.0): `native_pty_system`/`openpty`, an explicit
//! `PtySize`, `CommandBuilder`, `take_writer`, `try_clone_reader`, and a
//! reader thread. v2 deliberately has no sandbox/AppContainer broker, so
//! Windows and Unix use the same native portable-pty path.
//!
//! SCREEN STATE (M3.5):
//! Every output chunk is applied to a headless terminal emulator
//! ([`crate::screen::Screen`]) under the session state lock. The emulator is
//! the screen authority, the same shape Zed's pty-host RFC and tmux use: on
//! attach the client gets one `Snapshot(as_of_seq)` of the visible grid, then
//! ordinary live output chunks with strictly greater sequences. There is no
//! byte replay for a live screen and no replay cursor. Coalesced frames are
//! additionally enqueued to the conversation journal off this thread
//! (`try_send`, never a disk wait); the journal stays the durable transcript.
//! A recovered session has no emulator and replays the journal instead.
//! Terminal bytes are converted with UTF-8-lossy at the coalesced-flush
//! boundary so a read that splits a UTF-8 codepoint cannot panic.
//!
//! THE INVARIANT:
//! A snapshot carrying `as_of_seq = N` is exactly the emulator state after
//! every chunk with sequence `<= N` has been applied and before any chunk
//! with sequence `> N`. The boundary is on application to the emulator — not
//! the pipe write, not the journal commit, not client receipt. Capture of the
//! screen and registration of a new attachment happen under ONE hold of the
//! state lock, so output can never fall into neither the snapshot nor the
//! attachment's unsent queue. When an attachment's unsent queue exceeds
//! [`PENDING_OUTPUT_BUDGET_BYTES`], the unsent suffix is discarded and
//! replaced by a fresh snapshot at the current boundary.
//!
//! DEVICE STATUS REPLIES:
//! The emulator answers terminal queries (ConPTY's startup `ESC[6n` among
//! them) with `PtyWrite` events. Those replies go straight back to the PTY
//! writer from the publish path — never through the journal, a snapshot, or
//! a client pipe. ConPTY stalls its render pipeline until the query is
//! answered; the daemon is the single responder.
//!
//! LOCKING ORDER:
//! - The session registry lock is never held across blocking PTY I/O.
//! - `writer` and `master` are cloned under the registry lock, then their
//!   locks are taken after the registry lock has been released.
//! - Teardown removes the session first, then kills, drops writer/master,
//!   waits for the child, and only then bounded-joins the reader. This
//!   order is load-bearing on Windows because waiting while a ConPTY
//!   master remains open can deadlock.
//!
//! STREAMING:
//! M2 rejected coalescing because the in-process Channel was free (ConPTY
//! itself was the floor at ~0.52 MiB/s, ~7k msg/s, median 67-byte chunks).
//! M3b puts NDJSON and a named pipe on that path; 7k tiny frames/s is a
//! different proposition. The reader coalesces into one seq-assigned chunk
//! per [`COALESCE_MAX_BYTES`] or [`COALESCE_FLUSH`], whichever comes first.
//! Seq is assigned at flush so the stream stays contiguous.
//!
//! Unsent live output waits in one bounded per-attachment queue
//! ([`StreamState::pending`]), not in a byte-history ring. The connection
//! writer pulls at most [`PULL_BATCH`] items per turn, so a slow client
//! leaves the bulk of the backlog inside the budgeted queue, where the
//! snapshot replacement above can coalesce it. Blocking the PTY reader is
//! wrong (it stalls ConPTY's render pipeline), so back-pressure is expressed
//! as state: the slow viewer is resynchronised, the process is never stalled.

use std::collections::{BTreeMap, HashMap};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use portable_pty::{Child, ChildKiller, MasterPty, PtySize};

#[cfg(test)]
use devboule_protocol::CursorShape;
use devboule_protocol::{
    compose_session_id, cursor_replay_ok, validate_attachment_references, validate_attachments,
    validate_session_id, ActiveTurnBehavior, AgentActivityState, AgentTaskState,
    AttachmentReference, Cursor, DelegationRunState, DelegationState, ErrorCode, ErrorDetails,
    FinishArtifact, FinishArtifactPart, FinishArtifactPartMetadata, JournalRetention, JournalStats,
    OwnerId, PermissionOutcome, Project, PromptAttachment, RetentionPatch, Session, SessionEvent,
    SessionKind, SessionModel, SessionOrigin, SessionOriginKind, SessionState,
    SessionStateSnapshot, StoredAttachment, UnattendedState, UserMessageAuthor, UserMessageKind,
    WireError, Workspace, WorkspaceIsolation, MAX_WRITE_BYTES,
};
#[cfg(test)]
use std::sync::Barrier;

use crate::attachment_store::AttachmentStore;
use crate::journal::{new_session_record, Journal, PersistStatus, SessionRecord};
use crate::mcp_broker::McpSessionGuard;
use crate::paths::RuntimePaths;
use crate::peer_policy::{ConnPeer, PeerRole};
use crate::process_tree::{JobObject, ProcessHandle};
#[cfg(test)]
use crate::screen::Screen;
use crate::server::ServerState;
#[cfg(test)]
use devboule_protocol::TranscriptIntegrity;

#[path = "permission_broker.rs"]
mod permission_broker;
/// The daemon-wide peer card allowance, re-exported for its disconnect call
/// site: the boundary that drops a peer connection lives in `server.rs`, and
/// the counters live beside the brokers that spend them (H2).
pub(crate) use permission_broker::release_peer_cards;
#[path = "session_runtime.rs"]
mod session_runtime;
pub(crate) use session_runtime::{
    roster_task_state, AgentMessageSnapshot, SessionRuntime, TurnToken,
};
#[path = "acp_client.rs"]
mod acp_client;
/// One function out of a private module, under test only: the four tests that
/// write the ACP override environment live in three different files and must
/// serialise against each other.
#[cfg(test)]
pub(crate) use acp_client::lock_acp_env;
#[path = "acp_host.rs"]
mod acp_host;
#[path = "claude_client.rs"]
mod claude_client;
#[path = "codex_client.rs"]
mod codex_client;
#[path = "event_pull.rs"]
mod event_pull;
#[path = "pi_client.rs"]
mod pi_client;
/// The class-level provider seam: the `Provider` trait, one implementation
/// per family, and the registry the spawn road resolves through. Declared
/// from here like the other children; since pass 2b the mode lists live in
/// the impls, and `peer_policy`'s mode functions read them through the
/// registry — so the registry lookup is re-exported for that caller.
#[path = "provider.rs"]
mod provider;
/// Pi's mode dictionary, re-exported for the `unattended` derivation: the
/// vocabulary lives in the client that writes the permission extension, and
/// `peer_policy::unattended_mode` reads it from there without this module
/// growing any judgement of its own.
pub(crate) use pi_client::unattended_answer as pi_unattended_answer;
pub(crate) use provider::{
    apply_user_rows, catalog_registry, native_family_ids, session_resumable, ProviderRegistry,
};
#[path = "session_types.rs"]
mod session_types;
#[path = "session_workspaces.rs"]
mod session_workspaces;
#[path = "shell_command.rs"]
mod shell_command;
#[cfg(test)]
use session_workspaces::refuse_worktree_unless_live_git_allows;
use session_workspaces::{map_workspace_spawn_wire_error, workspace_spawn_error};
#[path = "session_envelopes.rs"]
mod session_envelopes;
#[cfg(test)]
pub(crate) use session_envelopes::TITLE_LINE_MAX_CHARS;
use session_envelopes::{
    agent_finished_envelope, agent_input_required_envelope, agent_message_envelope,
    agent_permission_request_envelope, agent_quiet_envelope, bound_finish_envelope,
    child_finish_state, summary_of,
};
#[cfg(test)]
use session_envelopes::{
    cap_excerpt_scalars, excerpt, stop_reason_state, MAX_FINISH_ENVELOPE_CHARS,
    MAX_STOP_REASON_IN_NOTE,
};
pub(crate) use session_envelopes::{neutralise_envelope_text, single_line_header};
use session_spawn::{
    spawn_async_end_marker, spawn_failure_is_provider_health, spawn_os_liveness_sweeper,
    spawn_resumed_session, spawn_session, teardown_session, teardown_session_for_resume,
    terminate_spawned_child, ResumedSessionContext,
};
#[cfg(test)]
use session_spawn::{spawn_codex_verify_thread, start_spawned_session};
#[path = "session_children.rs"]
mod session_children;
use session_messaging::forget_message_brake_target;
#[cfg(test)]
use session_messaging::{
    boundary_reached_message_slot, finish_message_delivery, rearm_message_slot_boundary,
    reserve_message_brake,
};
#[path = "session_items.rs"]
mod session_items;
#[path = "session_messaging.rs"]
mod session_messaging;
/// The attachment and prompt planning carved out of `session_items`: the path
/// lines a prompt carries, the reference resolution behind them, and the ACP
/// prompt plan. A move, not a rewrite - its proof is the byte comparison
/// against `session_items.rs` at `7235ff8`, not a test.
#[path = "session_prompt_planning.rs"]
mod session_prompt_planning;
/// The registry's standing state carved out of `session_items`: the caches it
/// holds, the message brake table and the agent creation table with its guards,
/// tickets and records. A move, not a rewrite - same proof.
#[path = "session_registry_state.rs"]
mod session_registry_state;
#[path = "session_spawn.rs"]
mod session_spawn;
#[cfg(test)]
pub(crate) use session_items::session_unique_for_test;
use session_items::{
    agent_message_target_entry, check_attached, check_resize_owner, check_user_owner,
    classify_agent_message_target, is_child_of, live_session_view, mint_session_unique,
    not_found_while_configuring, owner_from_session_id, peer_entry, peer_entry_mut, process_gone,
    session_metadata_for_resume, unauthorized, write_child_stdin, AgentMessageSourceNamespace,
    AgentMessageTargetClass, PtyKiller, PtySession, PtyWaitableChild, TerminalReaderDispatch,
    UnsupportedSteerer, COALESCE_EAGER_BYTES, EXIT_DRAIN, INITIAL_COLS, INITIAL_ROWS, PULL_BATCH,
    READER_JOIN_BUDGET, READ_CHUNK,
};
#[cfg(test)]
use session_items::{check_owner, elapsed_ms_since_last_life, session_nonce, session_unique};
pub(crate) use session_items::{
    session_origin_for, ModelSwitcher, ReaderDispatch, SessionKiller, SessionSteerer,
    SpawnedSession, StderrSource, StdioWaitableChild,
};
pub use session_items::{
    COALESCE_FLUSH, COALESCE_MAX_BYTES, PENDING_OUTPUT_BUDGET_BYTES, PENDING_OUTPUT_BUDGET_FRAMES,
    SESSION_OS_SWEEP_INTERVAL, SESSION_SILENCE_THRESHOLD,
};
pub(crate) use session_prompt_planning::StaticImageSink;
use session_prompt_planning::{
    plan_structured_prompt, prompt_text_with_fallback_paths, push_reference_path_lines,
    resolve_attachment_references, with_attachment_paths, AcpImageBlock, AcpPromptSink,
    ImageDelivery, PlannedStaticPrompt,
};
use session_registry_state::{
    compose_first_prompt, AgentChild, AgentCreationTable, AgentCreatorCaps, ConnectionPresence,
    CreationGate, CreationKeyHold, DeferredChildEnd, JournalRosterCache, MessageBrake,
    MessageBrakeTable, MessageSlotRef, OutstandingMessage, ProviderProvenance, Recipient,
    SendRequest, SessionCreateMeta, TransitionSink, WorkspacePathCache, MAX_CREATIONS_PER_WINDOW,
    MAX_LIVE_AGENT_SESSIONS, MAX_LIVE_CHILDREN_PER_CREATOR, MAX_MESSAGE_OUTSTANDING,
    MAX_MESSAGE_RECIPIENTS, MAX_MESSAGE_SENT_PER_WINDOW, MESSAGE_RATE_WINDOW, MESSAGE_SLOT_EXPIRY,
};
pub(crate) use session_registry_state::{
    AgentCreation, AgentCreationTicket, AgentCreator, ChildProfileFacts, LiveAgentEntry,
    PermissionResponse, MAX_AGENT_ARTIFACT_BYTES, MAX_AGENT_DEPTH,
};
#[cfg(test)]
use session_registry_state::{
    AgentMessageAfterAdmissionHook, DepositAfterOwnershipHook, JournalRosterAfterListHook,
    CREATION_WINDOW, DEFERRED_SLOT_EXPIRY, WORKSPACE_PATH_CACHE_CAP,
};
/// The move road's named phases: `set_agent_child_profile` in the parent is
/// the thin sequence, and this sibling holds the phases it composes. A
/// rewrite rather than a move — its proof is the characterisation tests in
/// `session_child_profile_tests.rs`, not a byte comparison.
#[path = "session_child_profile.rs"]
mod session_child_profile;
/// The create road's named phases: `create_with_provider_env` in the parent
/// is the thin sequence, and this sibling holds the phases it composes. A
/// rewrite rather than a move — its proof is the characterisation tests in
/// `session_create_tests.rs`, not a byte comparison.
#[path = "session_create.rs"]
mod session_create;
#[cfg(test)]
#[path = "session_create_phase_tests.rs"]
mod session_create_phase_tests;
#[cfg(test)]
#[path = "session_create_tests.rs"]
mod session_create_tests;
use session_child_profile::{manifest_arrived, model_ask_needed};
/// The delegated answer's named phases: `answer_child_permission` in the
/// parent is the thin sequence, and this sibling holds the phases it
/// composes. A rewrite rather than a move — its proof is the
/// characterisation tests in `session_child_permission_tests.rs`, not a
/// byte comparison.
#[path = "session_child_permission.rs"]
mod session_child_permission;
#[cfg(test)]
#[path = "session_child_profile_phase_tests.rs"]
mod session_child_profile_phase_tests;
#[cfg(test)]
#[path = "session_child_profile_tests.rs"]
mod session_child_profile_tests;
use session_child_permission::child_answer_caps_refusal;
#[cfg(test)]
#[path = "session_child_permission_phase_tests.rs"]
mod session_child_permission_phase_tests;
#[cfg(test)]
#[path = "session_child_permission_tests.rs"]
mod session_child_permission_tests;
#[cfg(test)]
#[path = "session_child_slot_tests.rs"]
mod session_child_slot_tests;
/// The envelope-grammar tests carved out of `session_tests` (its lines
/// 1378-1508 at `9a8c667`): the header-and-fence grammar the app parses, a
/// hostile excerpt that must not close its fence, and the excerpt cap counted
/// in scalars. A move, not a rewrite - its proof is the byte comparison
/// against `session_tests.rs` at the commit before it, not a test.
#[cfg(test)]
#[path = "session_envelope_card_tests.rs"]
mod session_envelope_card_tests;
/// The resume road's named phases: `resume` in the parent is the thin
/// sequence, and this sibling holds the phases it composes. A rewrite rather
/// than a move — its proof is the characterisation tests in
/// `session_resume_tests.rs`, not a byte comparison.
#[path = "session_resume.rs"]
mod session_resume;
/// Test support for the resume road's three characterisation files: the
/// fixture they share and the ACP override harness the spawn arms drive.
#[cfg(test)]
#[path = "session_resume_fixture.rs"]
mod session_resume_fixture;
#[cfg(test)]
#[path = "session_resume_phase_tests.rs"]
mod session_resume_phase_tests;
#[cfg(test)]
#[path = "session_resume_spawn_tests.rs"]
mod session_resume_spawn_tests;
#[cfg(test)]
#[path = "session_resume_tests.rs"]
mod session_resume_tests;
/// The refused-spawn tests carved out of `session_tests` (its lines 1510-1862):
/// the journal row an ordinary spawn failure must end before the refusal
/// returns, its non-blocking variant, the two creation-time profile refusals
/// and the pre-card tick refusal. A move, not a rewrite - its proof is the
/// byte comparison against `session_tests.rs` at `8b37de9`, not a test.
#[cfg(test)]
#[path = "session_spawn_refusal_tests.rs"]
mod session_spawn_refusal_tests;
#[cfg(test)]
#[path = "session_tests.rs"]
mod tests;
use session_resume::resume_end_generation_detached;

pub use event_pull::ConnHandle;
pub(crate) use session_types::PendingEvent;
pub use session_types::PtyCommand;
use session_types::{
    Disposition, OutputMetrics, PendingItem, PullState, RegistryEntry, TranscriptSession,
};
pub use shell_command::write_test_pty_command;

#[derive(Clone)]
pub struct SessionRegistry {
    inner: Arc<Mutex<HashMap<String, RegistryEntry>>>,
    paths: RuntimePaths,
    journal: Option<Arc<Journal>>,
    /// The bytes of prompt attachments, on disk under the runtime dir. Files
    /// are written here and never in the workspace: a workspace is a git
    /// checkout whose `git status` the user reads.
    attachments: AttachmentStore,
    transition_sink: Arc<Mutex<Option<TransitionSink>>>,
    presence: Arc<Mutex<HashMap<u64, ConnectionPresence>>>,
    /// Journal rows are the slow, mostly-static half of a roster. Keep them
    /// out of live-session transition broadcasts; lifecycle operations below
    /// invalidate this cache when they can change the row set.
    journal_roster: JournalRosterCache,
    /// Workspace paths change only through workspace mutations. Cache them
    /// after the first successful lookup so session creation does not enqueue
    /// a blocking SQLite RPC for every new process.
    workspace_paths: Arc<Mutex<WorkspacePathCache>>,
    /// Once materialized, a user's full wire roster is updated in place for
    /// one live-session transition. This keeps the full-snapshot contract
    /// while avoiding a second walk over every live entry.
    state_roster_cache: Arc<Mutex<HashMap<String, Vec<SessionStateSnapshot>>>>,
    message_brakes: Arc<Mutex<MessageBrakeTable>>,
    /// The creation budget of agent-created sessions (`S5` decision 5), beside
    /// the message brakes and under the same discipline: one lock over the
    /// whole table, taken on its own and never across another.
    creations: Arc<Mutex<AgentCreationTable>>,
    #[cfg(test)]
    journal_list_calls: Arc<AtomicU64>,
    #[cfg(test)]
    full_roster_builds: Arc<AtomicU64>,
    #[cfg(test)]
    journal_roster_after_list_hook: Arc<Mutex<Option<JournalRosterAfterListHook>>>,
    #[cfg(test)]
    agent_message_after_admission_hook: Arc<Mutex<Option<AgentMessageAfterAdmissionHook>>>,
    #[cfg(test)]
    deposit_after_ownership_hook: Arc<Mutex<Option<DepositAfterOwnershipHook>>>,
    /// The agent-profile store, attached by `ServerState` once both exist
    /// (`create-from-profile`).
    ///
    /// `SessionRegistry::new` cannot take it: the registry is a field of the
    /// state that holds the store, so the two are built in one expression and
    /// the store is attached immediately afterwards. A registry without one —
    /// every unit test that builds its own — has no standing instructions, which
    /// is the honest reading of "no store, no rules": nothing is cached, and the
    /// store is asked again on the next session's first prompt.
    agent_profiles: std::sync::OnceLock<Arc<crate::agent_profiles::AgentProfilesStore>>,
    /// The delegation switch, attached by `ServerState` like the profile
    /// store above. The handle is the store, never a copy of the boolean:
    /// every reader here asks it at the moment it decides, per the
    /// read-cadence rule at `delegation_store.rs`.
    delegation: std::sync::OnceLock<Arc<crate::delegation_store::DelegationStore>>,
}

impl SessionRegistry {
    pub(crate) fn runtime_dir(&self) -> &std::path::Path {
        &self.paths.dir
    }

    /// Sweep attachment folders left behind by a session that never closed.
    ///
    /// Returns what it reclaimed — one entry per swept session, with the bytes
    /// it held, or `None` when the folder could not be read and the size is
    /// unknown. A count alone would not be enough: whoever meters deposits per
    /// device decrements against these numbers, and a sweep that did not say
    /// what it freed would leave that counter charging for bytes that no longer
    /// exist. An unreadable folder is `None` rather than zero for the same
    /// reason — unknown is not empty.
    pub(crate) fn sweep_attachments(
        &self,
        now: std::time::SystemTime,
    ) -> Vec<(String, Option<u64>)> {
        self.attachments
            .sweep_older_than(now, crate::attachment_store::ATTACHMENT_RETENTION)
    }

    pub(crate) fn pipe_name(&self) -> &str {
        &self.paths.pipe_name
    }

    /// The journal worker owns SQLite, so its file-size query is a bounded
    /// RPC just like `list`. Diagnostics reports the failure instead of
    /// inventing a zero-sized database.
    pub(crate) fn journal_file_bytes(&self) -> Option<Result<u64, String>> {
        self.journal
            .as_ref()
            .map(|journal| journal.file_len().map_err(|error| error.to_string()))
    }

    pub fn new(paths: RuntimePaths, journal: Option<Arc<Journal>>) -> Self {
        let registry = Self {
            inner: Arc::new(Mutex::new(HashMap::new())),
            attachments: AttachmentStore::new(&paths.dir),
            paths,
            journal,
            transition_sink: Arc::new(Mutex::new(None)),
            presence: Arc::new(Mutex::new(HashMap::new())),
            journal_roster: Arc::new(Mutex::new(None)),
            workspace_paths: Arc::new(Mutex::new(WorkspacePathCache::default())),
            state_roster_cache: Arc::new(Mutex::new(HashMap::new())),
            message_brakes: Arc::new(Mutex::new(MessageBrakeTable::default())),
            creations: Arc::new(Mutex::new(AgentCreationTable::default())),
            #[cfg(test)]
            journal_list_calls: Arc::new(AtomicU64::new(0)),
            #[cfg(test)]
            full_roster_builds: Arc::new(AtomicU64::new(0)),
            #[cfg(test)]
            journal_roster_after_list_hook: Arc::new(Mutex::new(None)),
            #[cfg(test)]
            agent_message_after_admission_hook: Arc::new(Mutex::new(None)),
            #[cfg(test)]
            deposit_after_ownership_hook: Arc::new(Mutex::new(None)),
            agent_profiles: std::sync::OnceLock::new(),
            delegation: std::sync::OnceLock::new(),
        };
        spawn_os_liveness_sweeper(&registry);
        registry.reconcile_worktree_journal();
        registry
    }

    /// Hand the registry the agent-profile store (`create-from-profile`).
    ///
    /// Called once, by `ServerState`, right after both exist. The registry reads
    /// the store at exactly one moment — a session's first prompt — and never
    /// keeps a copy of its document, so an edit to the profiles takes effect on
    /// the next session's first prompt rather than at the next restart.
    pub(crate) fn attach_agent_profiles(
        &self,
        store: Arc<crate::agent_profiles::AgentProfilesStore>,
    ) {
        let _ = self.agent_profiles.set(store);
    }

    /// Attach the delegation switch, exactly like the profile store above.
    pub(crate) fn attach_delegation(&self, store: Arc<crate::delegation_store::DelegationStore>) {
        let _ = self.delegation.set(store);
    }

    /// The switch, read **now** — the one getter, for the one decision this
    /// call is making. `false` when no store is attached, which is the safe
    /// direction for every caller (a test registry surfaces and answers
    /// nothing).
    pub(crate) fn delegation_enabled(&self) -> bool {
        self.delegation
            .get()
            .map(|store| store.get().0)
            .unwrap_or(false)
    }

    /// The human's standing instructions, read **now**, or nothing when this
    /// registry has no store.
    ///
    /// A copy of the string, not a borrowed handle: the caller puts it in front of
    /// a prompt that is about to be written, and the store may be replaced while
    /// that prompt is being composed.
    pub(crate) fn standing_instructions(&self) -> String {
        self.agent_profiles
            .get()
            .map(|store| store.document().standing_instructions)
            .unwrap_or_default()
    }

    fn reconcile_worktree_journal(&self) {
        let Some(journal) = &self.journal else {
            return;
        };
        let Ok(projects) = journal.projects_list() else {
            return;
        };
        for project in projects {
            let Ok(workspaces) = journal.workspaces_list(&project.id) else {
                continue;
            };
            let project_path = PathBuf::from(&project.path);
            let root = crate::worktree::worktree_root_beside_project(&project_path);
            let mut known_checkouts = Vec::new();
            for workspace in workspaces {
                if workspace.isolation != WorkspaceIsolation::Worktree {
                    continue;
                }
                let checkout = PathBuf::from(&workspace.path);
                known_checkouts.push(checkout.clone());
                if !checkout.exists() {
                    eprintln!(
                        "worktree row '{}' points at missing checkout '{}'; detaching the row",
                        workspace.id, workspace.path
                    );
                    let _ = journal.workspace_delete(&workspace.id);
                }
            }
            let Some(root) = root else {
                continue;
            };
            let Ok(entries) = std::fs::read_dir(&root) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if !path.is_dir() {
                    continue;
                }
                let known = known_checkouts.iter().any(|known| {
                    crate::worktree::canonical_or_original(known)
                        == crate::worktree::canonical_or_original(&path)
                });
                if !known {
                    eprintln!(
                        "orphan worktree checkout '{}' has no journal row; leaving it on disk",
                        path.display()
                    );
                }
            }
        }
    }

    #[cfg(test)]
    fn journal_list_call_count(&self) -> u64 {
        self.journal_list_calls.load(Ordering::Relaxed)
    }

    #[cfg(test)]
    fn full_roster_build_count(&self) -> u64 {
        self.full_roster_builds.load(Ordering::Relaxed)
    }

    #[cfg(test)]
    fn set_journal_roster_after_list_hook(&self, hook: JournalRosterAfterListHook) {
        *self
            .journal_roster_after_list_hook
            .lock()
            .expect("journal roster test hook") = Some(hook);
    }

    /// Arm a one-shot callback that runs after an agent message's brake admission
    /// and before its delivery (S4-10).
    #[cfg(test)]
    fn set_agent_message_after_admission_hook(&self, hook: AgentMessageAfterAdmissionHook) {
        *self
            .agent_message_after_admission_hook
            .lock()
            .expect("agent message test hook") = Some(hook);
    }

    #[cfg(test)]
    fn fire_agent_message_after_admission_hook(&self) {
        let hook = self
            .agent_message_after_admission_hook
            .lock()
            .ok()
            .and_then(|mut hook| hook.take());
        if let Some(hook) = hook {
            hook();
        }
    }

    /// Arm a one-shot callback that runs after a deposit's ownership check and
    /// before the store write (HND-01).
    #[cfg(test)]
    fn set_deposit_after_ownership_hook(&self, hook: DepositAfterOwnershipHook) {
        *self
            .deposit_after_ownership_hook
            .lock()
            .expect("deposit test hook") = Some(hook);
    }

    #[cfg(test)]
    fn fire_deposit_after_ownership_hook(&self) {
        let hook = self
            .deposit_after_ownership_hook
            .lock()
            .ok()
            .and_then(|mut hook| hook.take());
        if let Some(hook) = hook {
            hook();
        }
    }

    fn invalidate_journal_roster(&self) {
        // A poisoned cache is not a reason to keep serving possibly stale
        // roster data. Recover the guard and clear it so the next read is
        // forced to consult the journal again.
        *self
            .journal_roster
            .lock()
            .unwrap_or_else(|poison| poison.into_inner()) = None;
        self.invalidate_state_roster();
    }

    fn invalidate_state_roster(&self) {
        self.state_roster_cache
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .clear();
    }

    fn cached_workspace_path(&self, workspace_id: &str) -> Option<PathBuf> {
        self.workspace_paths
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .get(workspace_id)
    }

    fn remember_workspace_path(&self, workspace_id: &str, path: PathBuf) {
        self.workspace_paths
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .insert(workspace_id.to_string(), path);
    }

    fn invalidate_workspace_path(&self, workspace_id: &str) {
        self.workspace_paths
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .remove(workspace_id);
    }

    fn invalidate_stale_journal_roster(&self) {
        let Some(journal) = self.journal.as_ref() else {
            return;
        };
        let revision = journal.session_set_revision();
        let stale = self
            .journal_roster
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .as_ref()
            .is_some_and(|(cached, _)| *cached != revision);
        if stale {
            self.invalidate_journal_roster();
        }
    }

    fn journal_roster(&self) -> Option<Vec<SessionRecord>> {
        let journal = self.journal.as_ref()?;
        let before_revision = journal.session_set_revision();
        let cached_rows = self
            .journal_roster
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .as_ref()
            .and_then(|(cached_revision, rows)| {
                (*cached_revision == before_revision).then(|| rows.clone())
            });
        if cached_rows.is_some() {
            return cached_rows;
        }

        #[cfg(test)]
        self.journal_list_calls.fetch_add(1, Ordering::Relaxed);
        let rows = journal.list().ok()?;

        #[cfg(test)]
        if let Some(hook) = self
            .journal_roster_after_list_hook
            .lock()
            .ok()
            .and_then(|mut hook| hook.take())
        {
            hook();
        }

        let after_revision = journal.session_set_revision();
        if after_revision != before_revision {
            // The rows and revision came from different points in the
            // journal's mutation stream. Returning this point-in-time result
            // is safe, but caching it under either revision would make the
            // next reader trust data that it did not actually read at that
            // revision. Let the next call retry instead.
            return Some(rows);
        }

        let mut cache = self
            .journal_roster
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        if let Some((cached_revision, cached)) = cache.as_ref() {
            if *cached_revision == after_revision {
                return Some(cached.clone());
            }
        }
        *cache = Some((before_revision, rows.clone()));
        Some(rows)
    }

    pub(crate) fn set_transition_sink(&self, sink: TransitionSink) {
        if let Ok(mut current) = self.transition_sink.lock() {
            *current = Some(sink);
        }
    }

    fn emit_transition(&self, owner: &OwnerId) {
        let sink = self
            .transition_sink
            .lock()
            .ok()
            .and_then(|current| current.clone());
        if let Some(sink) = sink {
            sink(owner.clone());
        }
    }

    fn notify_session_transition(&self, owner: &OwnerId, session_id: &str) {
        self.refresh_state_snapshot(owner, session_id);
        self.emit_transition(owner);
    }

    /// The delegation facts one snapshot row carries, or `None` for a session
    /// that is not an agent-created child — an absence that must never be
    /// read as `off` (a child whose switch a human turned off). The
    /// `unattended` state is the birth fact the row already carries (read
    /// from the journal's ratcheted column, never recomputed and never
    /// derived from the live switch); the switch itself is asked **now**,
    /// per the read-cadence rule; the count is read from the resolution
    /// ledger the replay reads back.
    fn delegation_state_for(&self, session: &Session) -> Option<DelegationState> {
        session.created_by.as_ref()?;
        let state = if session.unattended == UnattendedState::Yes {
            DelegationRunState::Unattended
        } else if self.delegation_enabled() {
            DelegationRunState::Active
        } else {
            DelegationRunState::Off
        };
        let answered = self
            .journal
            .as_ref()
            .and_then(|journal| journal.permission_count(&session.id).ok())
            .unwrap_or(0);
        Some(DelegationState { answered, state })
    }

    /// Drop the cached roster, so the next snapshot rebuilds. The delegation
    /// facts ride every row, and a switch flip must not be served stale from
    /// a cache a transition never invalidated: `DelegationSet` clears this
    /// before the watchers are re-pushed.
    pub(crate) fn invalidate_state_roster_cache(&self) {
        if let Ok(mut cache) = self.state_roster_cache.lock() {
            cache.clear();
        }
    }

    pub(crate) fn state_snapshots(&self, owner: &OwnerId) -> Vec<SessionStateSnapshot> {
        self.invalidate_stale_journal_roster();
        if let Ok(cache) = self.state_roster_cache.lock() {
            if let Some(snapshots) = cache.get(&owner.user) {
                return snapshots.clone();
            }
        }

        let snapshots = self.build_state_snapshots(owner);
        // A failed journal read must remain retryable. Live state is still
        // useful to return now, but do not let that partial roster become an
        // unbounded cache entry.
        let journal_is_cached = self.journal.is_none()
            || self
                .journal_roster
                .lock()
                .ok()
                .is_some_and(|cache| cache.is_some());
        if journal_is_cached {
            if let Ok(mut cache) = self.state_roster_cache.lock() {
                cache.insert(owner.user.clone(), snapshots.clone());
            }
        }
        snapshots
    }

    fn build_state_snapshots(&self, owner: &OwnerId) -> Vec<SessionStateSnapshot> {
        #[cfg(test)]
        self.full_roster_builds.fetch_add(1, Ordering::Relaxed);
        let (sessions_from_map, hidden_ids) = self
            .inner
            .lock()
            .map(|map| {
                let hidden_ids: std::collections::HashSet<String> = map
                    .values()
                    .filter(|entry| entry.is_configuring())
                    .map(|entry| entry.metadata().id.clone())
                    .collect();
                let sessions = map
                    .values()
                    .filter(|entry| entry.owner().user == owner.user && !entry.is_configuring())
                    .map(|entry| (entry.to_session(), entry.runtime().attention()))
                    .collect::<Vec<_>>();
                (sessions, hidden_ids)
            })
            .unwrap_or_default();
        let mut sessions = sessions_from_map;
        let live_ids = sessions
            .iter()
            .map(|(session, _)| session.id.clone())
            .collect::<std::collections::HashSet<_>>();
        if let Some(rows) = self.journal_roster() {
            sessions.extend(rows.into_iter().filter_map(|row| {
                if live_ids.contains(&row.id) || hidden_ids.contains(&row.id) {
                    return None;
                }
                (row.owner == owner.user).then(|| (row.to_session(), None))
            }));
        }
        sessions.sort_by(|left, right| left.0.id.cmp(&right.0.id));
        sessions
            .into_iter()
            .map(|(session, attention)| {
                let delegation = self.delegation_state_for(&session);
                SessionStateSnapshot {
                    id: session.id,
                    workspace_id: session.workspace_id,
                    kind: session.kind,
                    title: session.title,
                    state: session.state,
                    elapsed_ms: session.elapsed_ms,
                    attention,
                    origin: session.origin,
                    // The two fields a push-only row needs (S5-09, S5-04): the row
                    // this client is sent must name the child and its creator, not
                    // only the row the next list would build. The
                    // creation-from-profile facts travel with them for the same
                    // reason: a child created while the app is open arrives as a
                    // push-only row, and a row without its profile, its context, its
                    // marker and its labels would stay that way until the next full
                    // list.
                    display_name: session.display_name,
                    created_by: session.created_by,
                    profile_id: session.profile_id,
                    context_id: session.context_id,
                    unattended: session.unattended,
                    labels: session.labels,
                    delegation,
                }
            })
            .collect()
    }

    fn refresh_state_snapshot(&self, owner: &OwnerId, session_id: &str) {
        let snapshot = self.inner.lock().ok().and_then(|map| {
            map.get(session_id)
                .filter(|entry| entry.owner().user == owner.user)
                .filter(|entry| !entry.is_configuring())
                .map(|entry| {
                    let session = entry.to_session();
                    let delegation = self.delegation_state_for(&session);
                    SessionStateSnapshot {
                        id: session.id,
                        workspace_id: session.workspace_id,
                        kind: session.kind,
                        title: session.title,
                        state: session.state,
                        elapsed_ms: session.elapsed_ms,
                        attention: entry.runtime().attention(),
                        origin: session.origin,
                        display_name: session.display_name,
                        created_by: session.created_by,
                        profile_id: session.profile_id,
                        context_id: session.context_id,
                        unattended: session.unattended,
                        labels: session.labels,
                        delegation,
                    }
                })
        });
        if let Ok(mut cache) = self.state_roster_cache.lock() {
            let Some(roster) = cache.get_mut(&owner.user) else {
                return;
            };
            roster.retain(|session| session.id != session_id);
            if let Some(snapshot) = snapshot {
                roster.push(snapshot);
                roster.sort_by(|left, right| left.id.cmp(&right.id));
            }
        }
    }

    fn configure_runtime_attention(&self, runtime: &Arc<SessionRuntime>, owner: &OwnerId) {
        let presence = Arc::clone(&self.presence);
        let user = owner.user.clone();
        let session_id = runtime.session_id.clone();
        let suppressed_session_id = session_id.clone();
        let suppressed = Arc::new(move || {
            presence.lock().is_ok_and(|connections| {
                connections.values().any(|connection| {
                    connection.user == user
                        && connection.app_visible
                        && connection.focused_session_id.as_deref()
                            == Some(suppressed_session_id.as_str())
                })
            })
        });
        let registry = self.clone();
        let owner = owner.clone();
        let notify = Arc::new(move || {
            registry.notify_session_transition(&owner, &session_id);
            // The same transition is what a creator is owed a report about
            // (`S5` §3). The claim inside is idempotent, so the many
            // transitions an ordinary session raises cost one hash lookup.
            registry.report_child_events(&session_id);
        });
        runtime.set_attention_hooks(suppressed, notify);
        // The delegated-surfacing observer, installed in the same place with
        // the same facts in scope: one observer per child, called once per
        // parked card, deciding at that moment whether the creator is told.
        let registry = self.clone();
        let child = runtime.session_id.clone();
        runtime.set_permission_park_hook(Arc::new(move |request| {
            registry.notify_creator_of_parked_card(&child, request);
        }));
    }

    /// One parked card, surfaced to its creator under the delegation switch
    /// (§4.3): the `<devboule-system>` `agent_permission_request` envelope
    /// joins the moment a card parks — the same park that raises attention
    /// and, once per child, sends the `input_required` notice.
    ///
    /// The switch is read **here**, at the park: a switch that was on when
    /// the daemon started surfaces nothing after a human turned it off, and
    /// the answer side re-reads it again before it accepts anything (the
    /// read-cadence rule at `delegation_store.rs`). Surfacing was a copy,
    /// never a transfer — a card surfaced to a creator stays pending for the
    /// human exactly as before.
    fn notify_creator_of_parked_card(&self, child: &str, request: &SessionEvent) {
        if !self.delegation_enabled() {
            return;
        }
        let SessionEvent::PermissionRequest {
            tool_call_id,
            title,
            description,
            command,
            ..
        } = request
        else {
            return;
        };
        let Some((session, _runtime, owner)) = self.child_view(child) else {
            return;
        };
        let Some(creator) = session.created_by.clone() else {
            return;
        };
        let display_name = session
            .display_name
            .clone()
            .unwrap_or_else(|| session.title.clone());
        // The child's own words on the card: the description it wrote, or the
        // command it asked to run. Capped and neutralised inside the builder.
        let excerpt = description
            .clone()
            .filter(|text| !text.trim().is_empty())
            .or_else(|| command.clone())
            .unwrap_or_else(|| title.clone());
        let envelope = agent_permission_request_envelope(
            &session.id,
            &session.origin,
            tool_call_id,
            title,
            &display_name,
            &excerpt,
        );
        let _ = self.deliver_to_creator(&creator, &owner, &envelope);
    }

    pub(crate) fn set_presence(
        &self,
        conn_id: u64,
        owner: &OwnerId,
        focused_session_id: Option<String>,
        app_visible: bool,
    ) -> Result<(), WireError> {
        if let Some(session_id) = focused_session_id.as_deref() {
            validate_session_id(session_id)
                .map_err(|message| WireError::new(ErrorCode::InvalidRequest, message))?;
        }
        if let Ok(mut presence) = self.presence.lock() {
            presence.insert(
                conn_id,
                ConnectionPresence {
                    user: owner.user.clone(),
                    focused_session_id: focused_session_id.clone(),
                    app_visible,
                },
            );
        } else {
            return Err(internal("Session state is unavailable."));
        }
        // The presence guard is intentionally released before clearing
        // attention: raises use the global attention -> presence order.
        if app_visible {
            if let Some(session_id) = focused_session_id {
                let runtime = self.inner.lock().ok().and_then(|map| {
                    map.get(&session_id).and_then(|entry| {
                        (entry.owner().user == owner.user).then(|| entry.runtime())
                    })
                });
                if runtime.is_some_and(|runtime| runtime.clear_attention()) {
                    self.notify_session_transition(owner, &session_id);
                }
            }
        }
        Ok(())
    }

    pub(crate) fn clear_presence(&self, conn_id: u64) {
        if let Ok(mut presence) = self.presence.lock() {
            presence.remove(&conn_id);
        }
    }

    pub(crate) fn output_metrics(&self) -> OutputMetrics {
        let Ok(map) = self.inner.lock() else {
            return OutputMetrics::default();
        };
        map.values()
            .map(RegistryEntry::runtime)
            .map(|runtime| runtime.output_metrics())
            .fold(OutputMetrics::default(), |mut total, metrics| {
                total.peak_pending_bytes = total.peak_pending_bytes.max(metrics.peak_pending_bytes);
                total.coalesced_bytes = total
                    .coalesced_bytes
                    .saturating_add(metrics.coalesced_bytes);
                total.coalesced_frames = total
                    .coalesced_frames
                    .saturating_add(metrics.coalesced_frames);
                total
            })
    }

    /// Wire view of the journal writer's counters for the Status reply.
    /// `None` when the journal could not be opened: there is no writer
    /// whose behaviour could be counted, and inventing zeros would claim
    /// an integrity nobody observed.
    pub fn journal_stats(&self) -> Option<JournalStats> {
        self.journal.as_ref().map(|journal| {
            let snapshot = journal.stats();
            JournalStats {
                accepted_frames: snapshot.accepted_frames,
                accepted_bytes: snapshot.accepted_bytes,
                committed_frames: snapshot.committed_frames,
                committed_bytes: snapshot.committed_bytes,
                failed_frames: snapshot.failed_frames,
            }
        })
    }

    pub fn flush_journal(&self) {
        if let Some(journal) = &self.journal {
            let _ = journal.flush();
            journal.shutdown();
        }
    }

    pub fn journal_usage(&self) -> Result<crate::journal::JournalUsage, WireError> {
        self.journal
            .as_ref()
            .ok_or_else(journal_unavailable)?
            .usage()
            .map_err(Into::into)
    }

    pub fn journal_retention_get(&self) -> Result<JournalRetention, WireError> {
        self.journal
            .as_ref()
            .ok_or_else(journal_unavailable)?
            .retention_get()
            .map_err(Into::into)
    }

    pub fn journal_retention_set(
        &self,
        patch: RetentionPatch,
    ) -> Result<JournalRetention, WireError> {
        let result = self
            .journal
            .as_ref()
            .ok_or_else(journal_unavailable)?
            .retention_set(patch)
            .map_err(WireError::from);
        if result.is_ok() {
            self.invalidate_journal_roster();
        }
        result
    }

    pub fn delete_session(&self, session_id: &str, owner: &OwnerId) -> Result<(), WireError> {
        validate_session_id(session_id)
            .map_err(|message| WireError::new(ErrorCode::InvalidRequest, message))?;
        let transcript_in_registry = {
            let map = self
                .inner
                .lock()
                .map_err(|_| internal("Session state is unavailable."))?;
            match map.get(session_id) {
                Some(entry) => {
                    if entry.owner().user != owner.user {
                        return Err(unauthorized());
                    }
                    // Inside the delivery window the delete answers what
                    // every id-addressed peer call answers through
                    // `peer_entry`: `SessionNotFound`. A `Configuring`
                    // entry does not exist for peers — `sessions_list`
                    // never names the id, and no peer legitimately holds it
                    // (the create returns it only after promotion) — so
                    // "close the session before deleting it" would
                    // contradict the roster and confirm an id the caller
                    // should not know. One door, one answer.
                    if entry.is_configuring() {
                        return Err(not_found());
                    }
                    // Past the window the close-first guard stands (the
                    // re-audit's P1-1): a session a peer can see holds a
                    // running child, and an unrefused delete here would
                    // remove that child's row and entry out from under it.
                    if entry.as_peer_visible().is_some() {
                        return Err(WireError::new(
                            ErrorCode::InvalidRequest,
                            "Close the session before deleting it.",
                        ));
                    }
                    true
                }
                None => false,
            }
        };
        let journal = self.journal.as_ref().ok_or_else(journal_unavailable)?;
        if !transcript_in_registry {
            let record = journal
                .list()
                .map_err(WireError::from)?
                .into_iter()
                .find(|record| record.id == session_id)
                .ok_or_else(not_found)?;
            if record.owner != owner.user {
                return Err(unauthorized());
            }
        }
        journal
            .delete_session(session_id)
            .map_err(WireError::from)?;
        self.invalidate_journal_roster();
        if transcript_in_registry {
            if let Ok(mut map) = self.inner.lock() {
                map.remove(session_id);
            }
            journal.unpin(session_id);
        }
        self.notify_session_transition(owner, session_id);
        Ok(())
    }

    /// Env override applies only when the request did not name a provider.
    /// An explicit `provider` is the frontend's choice and must not be
    /// silently replaced by `DEVBOULE_AGENT_PROVIDER`.
    fn resolve_session_provider(
        kind: SessionKind,
        provider: Option<String>,
        env_provider: Option<&str>,
    ) -> (SessionKind, Option<String>, Option<ProviderProvenance>) {
        let requested = provider.filter(|id| !id.is_empty());
        let env_provider = env_provider.filter(|value| !value.is_empty());
        // The family remap routes through the provider registry: the id
        // names a provider, the provider's own `wire_kind` is the kind a
        // native family answers with, and both the requested/env asymmetry
        // and the arm order are the impls' `acp_create_remap_rank`. The
        // env override participates only when the request named nothing or
        // named a native family itself — the gate the literal spelling
        // closed with `requested.is_none()` — and the rank minimum
        // reproduces the arm order (pi, then codex, then claude) for every
        // input, including the ones where the env road outranks the
        // request's.
        let kind = if kind == SessionKind::Acp {
            let providers = provider::catalog_registry();
            let requested_provider = requested.as_deref().map(|id| providers.provider_for(id));
            let request_opens = requested_provider
                .as_ref()
                .map(|candidate| {
                    candidate
                        .acp_create_remap_rank(ProviderProvenance::Request)
                        .is_some()
                })
                .unwrap_or(true);
            let env_candidate = if request_opens {
                env_provider.map(|id| providers.provider_for(id))
            } else {
                None
            };
            requested_provider
                .into_iter()
                .map(|candidate| (candidate, ProviderProvenance::Request))
                .chain(env_candidate.map(|candidate| (candidate, ProviderProvenance::Env)))
                .filter_map(|(candidate, provenance)| {
                    candidate
                        .acp_create_remap_rank(provenance)
                        .map(|rank| (rank, candidate))
                })
                .min_by_key(|(rank, _)| *rank)
                .map(|(_, candidate)| candidate.wire_kind())
                .unwrap_or(SessionKind::Acp)
        } else {
            kind
        };
        let (provider, provenance) = if requested.is_some() {
            let provider = requested.filter(|id| id != "claude" && id != "pi");
            let provenance = provider.as_ref().map(|_| ProviderProvenance::Request);
            (provider, provenance)
        } else {
            let provider = env_provider
                .map(str::to_string)
                .filter(|id| id != "claude" && id != "pi" && !id.is_empty());
            let provenance = provider.as_ref().map(|_| ProviderProvenance::Env);
            (provider, provenance)
        };
        (kind, provider, provenance)
    }

    /// Consent for npx wrappers is explicit `provider` on the request.
    /// An env override must not launch third-party npx code.
    fn env_override_cannot_launch_npx(
        id: &str,
        provenance: Option<ProviderProvenance>,
        origin: Option<crate::provider_catalog::ProviderOrigin>,
    ) -> Result<(), WireError> {
        if provenance == Some(ProviderProvenance::Env)
            && origin == Some(crate::provider_catalog::ProviderOrigin::NpxWrapper)
        {
            return Err(WireError::new(
                ErrorCode::InvalidRequest,
                format!(
                    "provider '{id}' is an npx wrapper; npx wrappers require explicit selection, the env override cannot launch them"
                ),
            ));
        }
        Ok(())
    }

    fn reject_env_npx_wrapper(
        id: &str,
        provenance: Option<ProviderProvenance>,
        paths: &RuntimePaths,
    ) -> Result<(), WireError> {
        let origin = crate::provider_catalog::find_in_catalog(
            id,
            &crate::registry::CdnRegistryFetch,
            &paths.dir,
        )
        .map(|agent| agent.origin);
        Self::env_override_cannot_launch_npx(id, provenance, origin)
    }

    /// Create a session owned by `owner`, originating from `conn_peer` (`None`
    /// for the local pipe).
    ///
    /// The origin is written here, once, and read-only afterwards: the journal
    /// row and the wire metadata must agree on who asked for this session
    /// (`DESIGN-remote-agents.md` §8 R2).
    #[allow(clippy::too_many_arguments)]
    pub fn create(
        &self,
        state: &Arc<ServerState>,
        owner: &OwnerId,
        workspace_id: Option<String>,
        kind: SessionKind,
        provider: Option<String>,
        mode: Option<String>,
        display_name: Option<String>,
        conn_peer: &Option<ConnPeer>,
    ) -> Result<Session, WireError> {
        let env_provider = std::env::var("DEVBOULE_AGENT_PROVIDER").ok();
        let meta = SessionCreateMeta {
            display_name,
            ..SessionCreateMeta::default()
        };
        self.create_with_provider_env(
            state,
            owner,
            workspace_id,
            kind,
            provider,
            crate::profile_delivery::ProfileDelivery::for_request(mode),
            None,
            conn_peer,
            env_provider.as_deref(),
            &meta,
        )
    }

    // Env is a seventh caller argument so tests inject DEVBOULE_AGENT_PROVIDER
    // without mutating process env (which races under cargo's parallel harness).
    #[allow(clippy::too_many_arguments)]
    fn create_with_provider_env(
        &self,
        state: &Arc<ServerState>,
        owner: &OwnerId,
        workspace_id: Option<String>,
        kind: SessionKind,
        provider: Option<String>,
        delivery: crate::profile_delivery::ProfileDelivery,
        command: Option<PtyCommand>,
        conn_peer: &Option<ConnPeer>,
        env_provider: Option<&str>,
        meta: &SessionCreateMeta,
    ) -> Result<Session, WireError> {
        // The create road is the boundary that answers "which providers
        // exist": the file is read here (and at resume) so an edit takes
        // effect on the next creation rather than at the next restart — the
        // liveness rule the profile store states for itself. A read that
        // finds nothing to change swaps nothing.
        crate::user_providers::refresh_user_rows(self.runtime_dir());
        let resolved = self.resolve_creation_inputs(
            owner,
            workspace_id.as_deref(),
            kind,
            provider,
            env_provider,
            command,
            meta,
        )?;
        let (origin, title) = session_create::birth_stamps(meta, conn_peer, &resolved.kind);
        let (record, metadata, record_generation) = session_create::build_birth_record(
            &resolved,
            owner,
            workspace_id,
            &delivery,
            meta,
            origin,
            title,
        );
        // The birth door: the row is created, not upserted, so an id the
        // journal already holds refuses the create loudly instead of merging
        // two sessions into one row — whichever road supplied the id, minted
        // or caller-provided. It runs before anything registers against the
        // id, and blocking, so the answer reaches the caller: a create that
        // cannot own its id fails here, before a child exists. Journaling
        // before spawn also keeps the old guarantee — a short-lived command
        // (cmd /c echo) can EOF and enqueue MarkEnded before spawn returns,
        // and the journal thread must then see a live session, not a missing
        // one (recovered-as-killed on reopen). The row write stays beside the
        // env injection and the spawn: separating the durable boundary from
        // the sequence that fulfils it is the one split with no meaning.
        if let Some(journal) = &self.journal {
            journal.create_session(record).map_err(WireError::from)?;
            self.invalidate_journal_roster();
        }
        let mut command = resolved.command;
        crate::agent_env::inject_session_env(
            &mut command,
            &metadata.id,
            metadata.workspace_id.as_deref(),
            &self.paths,
        );
        let mcp_session = if crate::mcp_broker::hosts_mcp(&resolved.kind) {
            match state.mcp.register_with_provider(
                &metadata.id,
                owner,
                &resolved.kind,
                resolved.session_provider.as_deref(),
                crate::mcp_broker::AgentLineage {
                    depth: meta.depth,
                    overlay: meta.overlay.clone(),
                },
            ) {
                Ok(mcp) => mcp,
                Err(error) => {
                    // The row was born above, so it must still end — but not
                    // here: this is the dispatch thread and the blocking send
                    // is an unbounded 5 ms busy-loop, so the end rides a
                    // throwaway thread like the resume path. The revision bump
                    // on the end wakes roster readers once it lands.
                    if let Some(journal) = &self.journal {
                        spawn_async_end_marker(journal, &resolved.id, record_generation);
                    }
                    return Err(error);
                }
            }
        } else {
            None
        };
        self.note_pending_child_if_creation_pending(&resolved.id, meta);
        // The journal row above is the durable product boundary. A failed
        // spawn must end that row, or the next roster render resurrects a
        // phantom recovered session with zero events.
        match spawn_session(
            state,
            self,
            metadata.clone(),
            owner.clone(),
            command,
            mcp_session,
            delivery,
        ) {
            Ok(()) => {
                // Whose spawn success measures provider health is the impls'
                // `spawn_measures_health`, read through the registry; the
                // per-family reasons live there.
                if provider::catalog_registry()
                    .provider_for_kind(&resolved.kind)
                    .spawn_measures_health()
                {
                    if let Some(provider_id) = &metadata.provider {
                        state.record_provider_health(provider_id, Ok(()));
                    }
                }
            }
            Err(error) => {
                return Err(self.fail_spawn(
                    state,
                    &metadata.id,
                    record_generation,
                    metadata.provider.as_deref(),
                    error,
                ));
            }
        }
        Ok(metadata)
    }

    #[cfg(test)]
    pub fn attach(
        &self,
        session_id: &str,
        from_cursor: Option<Cursor>,
        conn: &ConnHandle,
        owner: &OwnerId,
        typed_permissions: bool,
    ) -> Result<(), WireError> {
        self.attach_with_subscription(
            session_id,
            conn.id,
            from_cursor,
            conn,
            owner,
            typed_permissions,
        )?;
        self.claim_resize_with_subscription(session_id, conn.id, owner, conn)
    }

    pub fn attach_with_subscription(
        &self,
        session_id: &str,
        subscription_id: u64,
        from_cursor: Option<Cursor>,
        conn: &ConnHandle,
        owner: &OwnerId,
        typed_permissions: bool,
    ) -> Result<(), WireError> {
        let runtime = match self.runtime_for_user(session_id, owner, conn) {
            Ok(runtime) => runtime,
            Err(error) if error.code == ErrorCode::SessionNotFound => {
                self.hydrate_transcript(session_id, from_cursor, owner, conn)?
            }
            Err(error) => return Err(error),
        };
        let outcome = runtime.try_attach_with_subscription(
            subscription_id,
            from_cursor,
            conn,
            typed_permissions,
        )?;
        // A terminal attach synchronises the screen (snapshot first, live
        // after). A transcript attach replays its journal. A live headless
        // agent needs the third contract: durable replay through a locked
        // watermark, then the live queue. Keeping these states explicit avoids
        // letting an agent's bounded backlog masquerade as history.
        let transcript = runtime.is_transcript();
        let transcript_cursor = if transcript {
            Some(from_cursor.map(|cursor| cursor.seq).unwrap_or(0))
        } else {
            None
        };
        if let Err(error) = conn.track_with_subscription(
            subscription_id,
            Arc::clone(&runtime),
            transcript,
            transcript_cursor,
            outcome.generation,
            outcome.live_agent_replay,
        ) {
            runtime.detach_subscription(conn.id, subscription_id);
            return Err(error);
        }
        // The journal writer records asynchronous failures in shared state;
        // attach must import that fact before returning even when the PTY is
        // otherwise quiet and no status request or later output occurs.
        runtime.refresh_journal_degradation();
        Ok(())
    }

    pub fn claim_resize_with_subscription(
        &self,
        session_id: &str,
        subscription_id: u64,
        owner: &OwnerId,
        conn: &ConnHandle,
    ) -> Result<(), WireError> {
        let runtime = self.runtime_for_user(session_id, owner, conn)?;
        runtime.claim_resize(conn.id, subscription_id)
    }

    /// The lineage a resumed session re-registers with, read off its journal
    /// row in the same lookup that reads `created_by` — never re-resolved
    /// from the profile store, whose answer may have changed since the birth.
    /// Depth and overlay are both birth facts: gating either on the creator
    /// being live would launder powers through a restart (an orphan resumed
    /// shallow could delegate again), so liveness only decides the
    /// bookkeeping in `readmit_agent_child`, never this lineage. A row that
    /// predates the columns resumes depth-capped, never at a depth nobody
    /// recorded; a human's own row is the root lineage. An unreadable
    /// overlay cell refuses the resume — it must never read as unrestricted —
    /// while the roster, which never reads the cell, keeps listing the row.
    ///
    /// `pub(crate)` for the wiring test: it feeds a real journal row through
    /// this mapping into a real broker registration.
    pub(crate) fn resumed_lineage(
        row: Option<&SessionRecord>,
    ) -> Result<crate::mcp_broker::AgentLineage, WireError> {
        let Some(row) = row else {
            return Ok(crate::mcp_broker::AgentLineage::root());
        };
        if row.created_by.is_none() {
            return Ok(crate::mcp_broker::AgentLineage::root());
        }
        let Some(overlay) = row.overlay.clone() else {
            return Err(WireError::new(
                ErrorCode::Internal,
                format!(
                    "cannot resume session '{}': its stored tool overlay is unreadable (sessions.overlay)",
                    row.id
                ),
            ));
        };
        Ok(crate::mcp_broker::AgentLineage {
            depth: row.depth.unwrap_or(MAX_AGENT_DEPTH),
            overlay,
        })
    }

    pub fn resume(
        &self,
        state: &Arc<ServerState>,
        session_id: &str,
        owner: &OwnerId,
        conn: &ConnHandle,
    ) -> Result<Session, WireError> {
        let (journal, record) = self.resume_locate_record(session_id)?;
        let (provider, peer_session_id) = resume_handle(&record, owner)?;
        let (command, generation) = self.resume_stage_command(&record, &provider)?;
        let had_live_slot = self.resume_evict_previous(&journal, session_id, owner, conn)?;
        conn.untrack_session(session_id);

        if !had_live_slot && !state.session_started() {
            return Err(WireError::new(
                ErrorCode::ShuttingDown,
                "daemon is shutting down",
            ));
        }
        // A resumed agent-created session is still that creator's child (audit
        // S5B-05). The journal has carried `created_by` since the slice-5
        // migration, and the birth overlay and depth since v13, so the
        // lineage is read back instead of being dropped: the session
        // re-registers under the powers it was born with, whether or not
        // its creator is live — losing a restriction because the parent is
        // gone would be the silent escalation this column exists to stop.
        // Whether the creator is live decides only the bookkeeping in
        // `readmit_agent_child` below, never the powers. The *quiet*
        // preference is still not persisted: a resume reports its end.
        // The row is the one this function already holds: re-reading it
        // through the roster cache would trade a propagated error for a
        // silent root lineage on a slow journal.
        let resumed_child = record.created_by.clone();
        let lineage = match Self::resumed_lineage(Some(&record)) {
            Ok(lineage) => lineage,
            Err(error) => {
                state.session_finished();
                return Err(error);
            }
        };
        // S9 kind-preserving fix: the gate above (`resume_handle`) admits the
        // resumable families, so this is Acp or Claude today — but the kind
        // comes from the record, never from a literal, so a resumed session
        // re-registers with its own kind rather than as whatever the last
        // author assumed. Pi/Codex resume stays refused at the gate
        // (deliberate: family resume is undesigned — see `resume_handle`).
        let mcp_session = match state.mcp.register_with_provider(
            session_id,
            owner,
            &record.kind,
            Some(provider.as_str()),
            lineage,
        ) {
            Ok(mcp_session) => mcp_session,
            Err(error) => {
                state.session_finished();
                return Err(error);
            }
        };
        if let Err(error) = journal.start_generation(session_id, generation) {
            drop(mcp_session);
            state.session_finished();
            return Err(error.into());
        }
        self.readmit_agent_child(session_id, resumed_child.as_deref(), owner);
        // Health is measured per provider id; `provider` is moved into the
        // metadata below, so keep a copy for the spawn outcome recording.
        let health_provider = provider.clone();
        // The failed-spawn arm below needs both facts this function already
        // holds, and `record` is consumed by the metadata build: the family
        // decides which wire code the failure keeps, and the handle the
        // resume tried to load is what a retraction may name.
        let resumed_kind = record.kind.clone();
        let attempted_handle = peer_session_id.clone();
        // Resume does not create a session: echo the journal's original
        // created_at_ms. Re-stamping now would break the staleness check
        // this field exists for.
        let metadata = session_metadata_for_resume(
            session_id,
            record,
            &command,
            provider,
            peer_session_id.clone(),
            generation,
        );
        match spawn_resumed_session(
            state,
            self,
            metadata,
            owner.clone(),
            command,
            ResumedSessionContext {
                peer_session_id,
                generation,
                mcp_session,
            },
        ) {
            Ok(()) => {
                state.record_provider_health(&health_provider, Ok(()));
                // The provider honoured this exact handle: a refusal recorded
                // against it is stale — the one fact that can prove a mark
                // wrong — and it is recorded on the same road as the health
                // it was learned with. Without this clear, a mark could hide
                // a working session with no road left that could correct it.
                if let Err(clear_error) =
                    journal.clear_peer_session_disown(session_id, &attempted_handle)
                {
                    eprintln!(
                        "journal could not clear the disown mark for {session_id}: {clear_error}"
                    );
                }
            }
            Err(mut error) => {
                state.session_finished();
                // The one fact a failed resume can carry: the far side's
                // handle names a session it does not have. Two families
                // produce that fact — ACP, whose `session/load` answered
                // ResourceNotFound naming this very session (`acp_client::
                // session_disown` raises it as `SessionNotFound`), and
                // Claude, whose history file the daemon itself proved
                // absent. A spawn failure, a broken pipe, a timeout, or a
                // handshake that never got an answer says nothing about the
                // far session — the handle may still be perfectly good, so
                // it stays and the offer stays with it.
                let peer_disowned = error.code == ErrorCode::SessionNotFound;
                if peer_disowned {
                    // The classification is a channel the daemon reads, never
                    // a statement about this daemon's roster: the row exists —
                    // the mark below is its whole point — and on the wire
                    // `session_not_found` is the app's word for a row the
                    // daemon has lost. Each family's caller keeps the code
                    // and sentence it has always seen for this failure.
                    error.code = match resumed_kind {
                        SessionKind::Claude => ErrorCode::InvalidRequest,
                        _ => ErrorCode::Io,
                    };
                    // The mark is issued HERE, ahead of the failing answer
                    // this arm returns: the bounded rpc returns only after
                    // the write is committed, so the one roster read the app
                    // fires the instant the answer arrives is already behind
                    // it. A healthy queue replies in milliseconds; the bound
                    // is the journal's ordinary RPC_WAIT, the same wait other
                    // control traffic on this thread already accepts. The
                    // handle itself is never destroyed — the refusal is
                    // recorded beside it.
                    if let Err(mark_error) =
                        journal.mark_peer_session_disowned(session_id, &attempted_handle)
                    {
                        // The bounded write can time out on a saturated queue
                        // with the command never enqueued; that failure is
                        // the detached fallback's whole reason to exist, so
                        // it is reported, never swallowed.
                        eprintln!(
                            "journal could not record the disowned handle for {session_id}: {mark_error}; the detached fallback will retry"
                        );
                    }
                    // A client that attached during the spawn window hydrated
                    // a `Transcript` entry from the row as it was BEFORE the
                    // mark — and the live map wins a roster read, so that
                    // entry would serve its stale `resumable` forever. The
                    // entry holds no process, so eviction tears nothing down:
                    // the journal, which now holds the mark and the end
                    // marker, is the one source of truth for everything it
                    // cached.
                    if let Ok(mut map) = self.inner.lock() {
                        if matches!(map.get(session_id), Some(RegistryEntry::Transcript(_))) {
                            map.remove(session_id);
                        }
                    }
                    // The eviction is a live-map change, not a journal write:
                    // a repeated mark answers `Ok(false)` and moves no revision,
                    // so only this clear drops the cached row the eviction removed.
                    self.invalidate_journal_roster();
                }
                // The generation was already started on the journal row; a
                // failed respawn must end it, or the row stays live and the
                // roster renders a phantom recovered session.
                resume_end_generation_detached(
                    &journal,
                    session_id,
                    generation,
                    peer_disowned,
                    attempted_handle,
                );
                state.record_provider_health(&health_provider, Err(&error));
                return Err(error);
            }
        }
        self.resume_read_registered(session_id)
    }

    fn hydrate_transcript(
        &self,
        session_id: &str,
        from_cursor: Option<Cursor>,
        owner: &OwnerId,
        conn: &ConnHandle,
    ) -> Result<Arc<SessionRuntime>, WireError> {
        validate_session_id(session_id)
            .map_err(|message| WireError::new(ErrorCode::InvalidRequest, message))?;
        let journal = self.journal.as_ref().ok_or_else(not_found)?;
        let record = journal
            .list()?
            .into_iter()
            .find(|row| row.id == session_id)
            .ok_or_else(not_found)?;
        let session_owner = owner_from_session_id(session_id, &record.owner)?;
        if session_owner.user != owner.user {
            return Err(unauthorized());
        }
        journal.pin(session_id)?;
        // The store holds the whole history, whatever the cursor says: a
        // cursor is a position inside the current generation — history
        // never advances it — so it cannot certify the history was read.
        // What this reader is owed is the pull's decision, through the
        // owed-row predicate, against the cursor.
        let replay = match journal.replay(session_id) {
            Ok(replay) => replay,
            Err(error) => {
                journal.unpin(session_id);
                return Err(error.into());
            }
        };
        if let Some(cursor) = from_cursor {
            if let Err(error) = cursor_replay_ok(replay.generation, cursor) {
                journal.unpin(session_id);
                return Err(error);
            }
        }
        let metadata = record.to_session();
        let runtime =
            SessionRuntime::from_replay(session_id.to_string(), Some(Arc::clone(journal)), replay);
        // A recovered session carries the origin of the create that made it,
        // so the peer gate and the permission card read the same fact a live
        // session would have had.
        runtime.set_origin(metadata.origin.clone());
        if let Some(peer_session_id) = record.peer_session_id.clone() {
            runtime.restore_peer_session_id(peer_session_id);
        }
        {
            let Ok(mut map) = self.inner.lock() else {
                journal.unpin(session_id);
                return Err(internal("Session state is unavailable."));
            };
            if let Some(existing) = map.get(session_id) {
                check_user_owner(existing, owner, &conn.conn_peer)?;
                // Attach reaches this arm whenever the registry holds the
                // id — including inside the delivery window, where
                // `runtime_for_user` answered `SessionNotFound` and the
                // caller fell back here on exactly that code (the
                // re-audit's P2-2). Handing back the windowed child's
                // runtime through the fallback would re-open the window the
                // door just closed.
                if existing.is_configuring() {
                    journal.unpin(session_id);
                    return Err(not_found());
                }
                journal.unpin(session_id);
                return Ok(existing.runtime());
            }
            map.insert(
                session_id.to_string(),
                RegistryEntry::Transcript(Box::new(TranscriptSession {
                    metadata,
                    owner: session_owner,
                    runtime: Arc::clone(&runtime),
                })),
            );
        }
        Ok(runtime)
    }

    #[cfg(test)]
    pub fn detach(
        &self,
        session_id: &str,
        conn: &ConnHandle,
        owner: &OwnerId,
    ) -> Result<(), WireError> {
        self.detach_with_subscription(session_id, conn.id, conn, owner)
    }

    pub fn detach_with_subscription(
        &self,
        session_id: &str,
        subscription_id: u64,
        conn: &ConnHandle,
        owner: &OwnerId,
    ) -> Result<(), WireError> {
        let runtime = self.runtime_for_user(session_id, owner, conn)?;
        runtime.detach_subscription(conn.id, subscription_id);
        conn.untrack_subscription(subscription_id);
        self.drop_transcript_if_idle(session_id);
        Ok(())
    }

    #[cfg(test)]
    pub fn permission_respond(
        &self,
        session_id: &str,
        request_id: &str,
        outcome: PermissionOutcome,
        conn: &ConnHandle,
        owner: &OwnerId,
    ) -> Result<(), WireError> {
        self.permission_respond_with_subscription(
            PermissionResponse {
                session_id,
                request_id,
                outcome,
                option_id: None,
            },
            conn.id,
            conn,
            owner,
        )
    }

    pub fn permission_respond_with_subscription(
        &self,
        response: PermissionResponse<'_>,
        subscription_id: u64,
        conn: &ConnHandle,
        owner: &OwnerId,
    ) -> Result<(), WireError> {
        let PermissionResponse {
            session_id,
            request_id,
            outcome,
            option_id,
        } = response;
        validate_session_id(session_id)
            .map_err(|message| WireError::new(ErrorCode::InvalidRequest, message))?;
        if request_id.is_empty() {
            return Err(WireError::new(
                ErrorCode::InvalidRequest,
                "Permission request id is required.",
            ));
        }
        let runtime = self.runtime_for_user(session_id, owner, conn)?;
        check_attached(&runtime, conn, subscription_id)?;
        let broker = runtime.permission_broker().ok_or_else(|| {
            WireError::new(
                ErrorCode::InvalidRequest,
                "Session has no live ACP permission broker.",
            )
        })?;
        broker
            .respond_with_option(request_id, outcome, option_id.map(str::to_string))
            .map_err(|error| {
                let code = match error {
                    permission_broker::PermissionResponseError::NotFound => {
                        ErrorCode::InvalidRequest
                    }
                    permission_broker::PermissionResponseError::InvalidRequest(_) => {
                        ErrorCode::InvalidRequest
                    }
                    permission_broker::PermissionResponseError::Io(_) => ErrorCode::Io,
                };
                WireError::new(code, error.to_string())
            })?;
        if runtime.clear_attention() {
            self.notify_session_transition(owner, session_id);
        }
        Ok(())
    }

    /// The delegated answer (`§4.1`): an agent answering its own child's
    /// permission card, through `devboule_answer_permission`.
    ///
    /// Identity is imposed — `creator_session_id` is the caller's bearer-
    /// mapped session, never a tool argument (§0.1) — and every check runs
    /// inside [`permission_broker::PermissionBroker::answer_delegated_on`],
    /// in the spec's order, with this registry's facts supplied as the
    /// testimony the checks consume. A refusal from anywhere in the chain
    /// leaves the card pending and untouched.
    pub(crate) fn answer_child_permission(
        &self,
        creator_session_id: &str,
        card_id: &str,
        outcome: PermissionOutcome,
        device_caps: &dyn Fn(&str) -> Vec<String>,
    ) -> Result<(), String> {
        let (owner_user, creator_origin) = self.caller_identity(creator_session_id)?;
        let (found, child_holders) =
            self.find_card_holder(&owner_user, creator_session_id, card_id)?;
        // The ambiguity guard runs before any broker check, so an ambiguous
        // id is refused as ambiguous even with the switch off.
        if child_holders > 1 {
            return Err(format!(
                "more than one of your live children holds permission card {card_id}; the cards stay pending for the human"
            ));
        }
        // Check 2's closure: the switch, read at the moment the check runs.
        let switch_on = || self.delegation_enabled();
        // Check 3's closure: the ledger, read per request id.
        let resolved_elsewhere = |request_id: &str| self.permission_already_recorded(request_id);
        // Check 4's closure: the session that passes the check is remembered
        // so the attention it was waiting under clears when the answer lands.
        let answered_child: std::cell::RefCell<Option<String>> = std::cell::RefCell::new(None);
        let child_check = |card_session: &str| -> Result<(), String> {
            let target = self.child_answer_target(card_id, card_session, creator_session_id)?;
            *answered_child.borrow_mut() = Some(target);
            Ok(())
        };
        // Check 5's closure: the peer gate, judged with the caller's origin.
        let caps_check = |_: &str| child_answer_caps_refusal(&creator_origin, device_caps);
        permission_broker::PermissionBroker::answer_delegated_on(
            found.as_deref(),
            card_id,
            outcome,
            &switch_on,
            &resolved_elsewhere,
            &child_check,
            &caps_check,
            creator_session_id,
        )?;
        // The cell may be consumed only after the chain returned Ok: the
        // child check runs before the capability check, so a refused answer
        // can leave here with the cell filled and the card still pending.
        if let Some(child) = answered_child.into_inner() {
            self.clear_child_attention_after_answer(&child);
        }
        Ok(())
    }

    /// A creator moves its own live child onto a profile (slice 5b §2, Pass
    /// A), through `devboule_set_agent_profile`.
    ///
    /// Identity is imposed — `creator_session_id` is the caller's bearer-mapped
    /// session, never a tool argument (§0.1) — and the checks run in the
    /// spec's order, each refusal naming its reason and leaving the child
    /// untouched:
    ///
    /// 1. The caller is registered. The MCP registration guarantees it; a row
    ///    that has gone is a refusal, not a panic.
    /// 2. The target resolves **by id or display name among the caller's own
    ///    live children only** — visible, and `created_by` equals the caller.
    ///    The caller itself, a sibling, a grandchild, a human-started session
    ///    and an invented or dead name each get the sentence that case earns
    ///    without leaking anything a roster does not already show the same
    ///    owner; a name two live children share is refused ambiguous rather
    ///    than resolved to one of them.
    /// 3. The profile is resolved **now** by the caller's closure — the
    ///    broker's `resolve_profile`, with the unticked refusal §1.2 demands —
    ///    and never from a list read earlier.
    /// 4. The mode ask goes through [`Self::set_mode`] on the internal
    ///    connection (`ConnHandle::with_peer(0, None)`, the `send_message`
    ///    precedent): the child's **own manifest** must advertise the id, and
    ///    a provider that cannot switch a live session answers on its own
    ///    wire. **The child is never restarted.** A manifest nobody has
    ///    delivered yet is the third state: the daemon cannot say yet, and the
    ///    refusal withholds.
    /// 5. Only after the mode landed is the model asked, through
    ///    [`Self::set_model`]. A refusal there is a **partial** success: the
    ///    answer reports exactly what landed, records **no** profile change —
    ///    and the `unattended` ratchet still fires, because the child has in
    ///    fact been able to run in that mode and that cannot be un-lived.
    ///
    /// On a full success the child's row records the profile's stable id and
    /// the marker is the delivered mode's own judgement — the same
    /// `peer_policy::unattended_mode` the birth calls, raised (never lowered)
    /// through the journal's `MAX` ratchet and in the live metadata the
    /// snapshot serves.
    pub(crate) fn set_agent_child_profile(
        &self,
        creator_session_id: &str,
        target: &str,
        profile_name: &str,
        resolve_profile: &dyn Fn(&str) -> Result<ChildProfileFacts, String>,
    ) -> Result<(), String> {
        let (child_session, child_runtime, child_owner) =
            self.resolve_own_live_child(creator_session_id, target)?;
        // Check 3: the profile, read at the moment of the call — the closure
        // owns the store and the three refusals §1.2 names.
        let facts = resolve_profile(profile_name)?;
        // Check 4's pre-read: a manifest nobody has delivered yet is not "the
        // mode is unavailable" — it is "the daemon cannot say yet", and the
        // refusal withholds. A manifest that arrived and names no modes is the
        // provider's own say-so, and `set_mode`'s sentence for it stands.
        if !manifest_arrived(&child_runtime) {
            return Err(format!(
                "the daemon cannot say yet whether mode '{}' is available on this child: its provider has not reported the session's manifest; ask again once the child is up",
                facts.mode_id
            ));
        }
        let internal_conn = ConnHandle::with_peer(0, None);
        self.set_mode(
            &child_session.id,
            &child_owner,
            &facts.mode_id,
            &internal_conn,
        )
        .map_err(|error| error.message)?;
        // Check 5: the model, only after the mode landed. A child already
        // running the profile's model with no thinking option to deliver asks
        // nothing — there is no ask to make — and every other combination is
        // asserted on the provider's own wire, Claude's effort validation
        // included where it applies.
        if model_ask_needed(child_runtime.session_manifest().as_ref(), &facts) {
            if let Err(error) = self.set_model(
                &child_session.id,
                &child_owner,
                Some(&facts.model),
                facts.thinking_option_id.as_deref(),
            ) {
                return self.record_partial_move(&child_session, &facts, &error.message);
            }
        }
        // Full success: record the profile and raise the marker through the
        // one predicate — the delivered mode's own judgement, the same
        // function the birth calls.
        self.record_child_profile_move(
            &child_session.id,
            &child_session.kind,
            &facts.mode_id,
            Some(&facts.profile_id),
        );
        Ok(())
    }

    /// The recording half of a move: the journal row's `profile_id` and the
    /// `unattended` ratchet, then the live metadata the snapshot serves,
    /// raised — never lowered — with the same rank the SQL `MAX` compares.
    ///
    /// The asks that already landed cannot be un-lived, so a journal that
    /// cannot take the write degrades the recording; it never refuses the
    /// move and never erases the marker.
    fn record_child_profile_move(
        &self,
        child_id: &str,
        child_kind: &SessionKind,
        delivered_mode: &str,
        profile_id: Option<&str>,
    ) {
        let marker = crate::peer_policy::unattended_mode(child_kind.clone(), Some(delivered_mode));
        if let Some(journal) = &self.journal {
            if let Err(error) = journal.set_agent_profile_row(child_id, profile_id, marker) {
                eprintln!("agent profile row update failed for {child_id}: {error}");
            }
        }
        if let Ok(mut map) = self.inner.lock() {
            if let Some(live) = map
                .get_mut(child_id)
                .and_then(RegistryEntry::as_peer_visible_mut)
            {
                if let Some(profile_id) = profile_id {
                    live.metadata.profile_id = Some(profile_id.to_string());
                }
                if crate::journal::unattended_state_rank(marker)
                    > crate::journal::unattended_state_rank(live.metadata.unattended)
                {
                    live.metadata.unattended = marker;
                }
            }
        }
        self.invalidate_journal_roster();
        if let Some((_session, _runtime, owner)) = self.child_view(child_id) {
            self.notify_session_transition(&owner, child_id);
        }
    }

    /// Resolve `target` (exact id or display name) among the caller's own
    /// live children — the scope gate for `stop_agent_child` and
    /// `close_agent_child`.
    ///
    /// Scope is the caller's own children and nothing else: a session the
    /// caller did not create, a dead one, and an invented id all get the
    /// same refusal, so the answer never says whether an id exists. The
    /// caller itself is refused before the scan: a session is not its own
    /// child, and the process waiting for this reply would be the one torn
    /// down.
    fn resolve_own_child(
        &self,
        creator_session_id: &str,
        target: &str,
    ) -> Result<(String, OwnerId), WireError> {
        if target == creator_session_id {
            return Err(WireError::new(
                ErrorCode::InvalidRequest,
                "a session is not its own child; name a session you created",
            ));
        }
        let map = self
            .inner
            .lock()
            .map_err(|_| internal("Session state is unavailable."))?;
        let caller_owner = map
            .get(creator_session_id)
            .map(|entry| entry.owner().clone())
            .ok_or_else(|| {
                WireError::new(
                    ErrorCode::InvalidRequest,
                    "the calling session is not registered on this daemon",
                )
            })?;
        let display = |session: &Session| {
            session
                .display_name
                .clone()
                .unwrap_or_else(|| session.title.clone())
        };
        let mut matches: Vec<(String, OwnerId)> = map
            .values()
            .filter(|entry| entry.owner().user == caller_owner.user)
            .filter_map(|entry| {
                let live = entry.as_peer_visible()?;
                let session = &live.metadata;
                if !is_child_of(session.created_by.as_deref(), creator_session_id) {
                    return None;
                }
                (session.id == target || display(session) == target)
                    .then(|| (session.id.clone(), entry.owner().clone()))
            })
            .collect();
        match matches.len() {
            1 => Ok(matches.pop().expect("exactly one match")),
            0 => Err(WireError::new(
                ErrorCode::SessionNotFound,
                format!("none of your live children is called '{target}'"),
            )),
            _ => Err(WireError::new(
                ErrorCode::InvalidRequest,
                format!(
                    "more than one of your live children is called '{target}'; use the session id"
                ),
            )),
        }
    }

    /// Stop one of the caller's own children (`devboule_stop_agent`): the
    /// child's process tree dies, the session row and its transcript stay.
    /// The everyday supervisor action for a stuck child.
    pub fn stop_agent_child(
        &self,
        creator_session_id: &str,
        target: &str,
    ) -> Result<(), WireError> {
        let (child_id, owner) = self.resolve_own_child(creator_session_id, target)?;
        self.stop(&child_id, &owner)
    }

    /// Close one of the caller's own children (`devboule_close_agent`): the
    /// live session ends, the transcript stays in history. What a finished
    /// child's creator does when it no longer needs the child.
    ///
    /// The removal is this road's own, so it releases the idle-shutdown slot
    /// the child's creation took (`create_session_for_agent`): the wire
    /// `SessionClose` handler that pairs the client road's closes is not on
    /// this road.
    pub fn close_agent_child(
        &self,
        state: &Arc<ServerState>,
        creator_session_id: &str,
        target: &str,
    ) -> Result<bool, WireError> {
        let (child_id, owner) = self.resolve_own_child(creator_session_id, target)?;
        let removed = self.close(&child_id, &owner, &None)?;
        if removed {
            state.session_finished();
        }
        Ok(removed)
    }

    pub fn stop(&self, session_id: &str, owner: &OwnerId) -> Result<(), WireError> {
        validate_session_id(session_id)
            .map_err(|message| WireError::new(ErrorCode::InvalidRequest, message))?;
        let (mut killer, job) = {
            let mut map = self
                .inner
                .lock()
                .map_err(|_| internal("Session state is unavailable."))?;
            let entry = peer_entry_mut(&mut map, session_id, owner, &None)?;
            let session = entry.as_peer_visible_mut().ok_or_else(process_gone)?;
            session.preserve_on_exit.store(true, Ordering::SeqCst);
            (
                session.killer.clone_killer(),
                Arc::clone(&session.process_job),
            )
        };
        killer.kill();
        // The session is preserved, so its job stays open: the kill above
        // stops only the root. Terminate the tree too, or the agent's
        // descendants outlive the stop. Mirrors the on-OS-death handler.
        let _ = job.terminate();
        Ok(())
    }

    pub fn stop_with_subscription(
        &self,
        session_id: &str,
        subscription_id: u64,
        owner: &OwnerId,
        conn: &ConnHandle,
    ) -> Result<(), WireError> {
        validate_session_id(session_id)
            .map_err(|message| WireError::new(ErrorCode::InvalidRequest, message))?;
        let (mut killer, runtime, job) = {
            let mut map = self
                .inner
                .lock()
                .map_err(|_| internal("Session state is unavailable."))?;
            let entry = peer_entry_mut(&mut map, session_id, owner, &conn.conn_peer)?;
            let session = entry.as_peer_visible_mut().ok_or_else(process_gone)?;
            (
                session.killer.clone_killer(),
                Arc::clone(&session.runtime),
                Arc::clone(&session.process_job),
            )
        };
        check_attached(&runtime, conn, subscription_id)?;
        {
            let mut map = self
                .inner
                .lock()
                .map_err(|_| internal("Session state is unavailable."))?;
            // Peer-visible shape on purpose, though this is bookkeeping: a
            // `Configuring` entry here would be a *different* child — the
            // resume that replaced the one just killed — and must not
            // inherit its `preserve_on_exit`.
            if let Some(session) = map
                .get_mut(session_id)
                .and_then(RegistryEntry::as_peer_visible_mut)
            {
                session.preserve_on_exit.store(true, Ordering::SeqCst);
            }
        }
        killer.kill();
        // Same ownership as `stop`: the session is preserved, so its job
        // stays open and the kill above stops only the root.
        let _ = job.terminate();
        Ok(())
    }

    /// Drop every subscription this connection holds. The processes stay.
    pub fn detach_conn(&self, conn: &ConnHandle) {
        let ids = conn.take_attached_ids();
        for (subscription_id, session_id) in ids {
            self.detach_runtime(&session_id, conn.id, subscription_id);
            self.drop_transcript_if_idle(&session_id);
        }
    }

    pub(crate) fn subscription_event_sent(&self, session_id: &str) {
        self.drop_transcript_if_idle(session_id);
    }

    fn detach_runtime(&self, session_id: &str, conn_id: u64, subscription_id: u64) {
        if let Ok(runtime) = self.runtime(session_id) {
            runtime.detach_subscription(conn_id, subscription_id);
        }
    }

    fn drop_transcript_if_idle(&self, session_id: &str) {
        let Ok(mut map) = self.inner.lock() else {
            return;
        };
        let is_idle_transcript = map.get(session_id).is_some_and(|entry| {
            matches!(entry, RegistryEntry::Transcript(session) if {
                session
                    .runtime
                    .stream
                    .lock()
                    .map(|stream| stream.observers.is_empty())
                    .unwrap_or(true)
            })
        });
        if is_idle_transcript {
            map.remove(session_id);
            if let Some(journal) = &self.journal {
                journal.unpin(session_id);
            }
        }
    }

    /// Close a session, or a previous run's row for the same user.
    ///
    /// `conn_peer` is the requestor's connection identity: a paired device
    /// may close only what `check_user_owner` opens to it, and the internal
    /// callers that reap a half-started session pass `&None` because no peer
    /// asked for that close.
    pub fn close(
        &self,
        session_id: &str,
        owner: &OwnerId,
        conn_peer: &Option<ConnPeer>,
    ) -> Result<bool, WireError> {
        validate_session_id(session_id)
            .map_err(|message| WireError::new(ErrorCode::InvalidRequest, message))?;
        let session = {
            let mut map = self
                .inner
                .lock()
                .map_err(|_| internal("Session state is unavailable."))?;
            if let Some(entry) = map.get(session_id) {
                check_user_owner(entry, owner, conn_peer)?;
                // Close is teardown: it reaches through the delivery window
                // exactly like the `Configuring` arm below, so the same
                // child-slot accessor answers for both variants here. (For a
                // windowed child the store is a no-op — `transition_ready`
                // is not raised until the delivery lands and promotes.)
                if let Some(session) = entry.as_child_process() {
                    session
                        .runtime
                        .transition_ready
                        .store(false, Ordering::Release);
                }
            }
            // The closed session's message-brake entries go in the same critical
            // section that takes it out of the map (A2-06): a send that found it
            // here cannot reserve a slot for it afterwards (A2-05).
            forget_message_brake_target(&self.message_brakes, session_id);
            map.remove(session_id)
        };
        self.forget_agent_creator(session_id);
        match session {
            // A `Configuring` entry closes exactly like a live one: the
            // delivery-refusal path tears a half-started child down through
            // this arm, and teardown is the one thing the delivery window
            // must never block.
            Some(RegistryEntry::Live(session)) | Some(RegistryEntry::Configuring(session)) => {
                // The last chance to report this child to its creator (`S5` §3,
                // audit S5-01): the row is out of the map, the runtime is still
                // here, and the report is claimed exactly once, so a child whose
                // turn already reported finds nothing owed.
                //
                // Report *then* release: the claim of the report reads the
                // child's link in the creation table, which the release removes.
                // An end that released first would silently owe the creator
                // nothing but the caps, which is what the audit found.
                self.child_ended_with(
                    session_id,
                    Some(&session.metadata),
                    Some(&session.runtime),
                    Some(owner),
                );
                if let Some(journal) = &self.journal {
                    journal.try_mark_closed(session_id);
                    journal.unpin(session_id);
                    self.invalidate_journal_roster();
                }
                teardown_session(*session);
                // The attachments existed for this session's turns. Removing
                // them here is the normal path; `sweep_attachments` on the next
                // daemon start is the fallback for a close that never ran.
                self.attachments.remove_session(session_id);
                self.notify_session_transition(owner, session_id);
                Ok(true)
            }
            Some(RegistryEntry::Transcript(_)) => {
                // A transcript carries no runtime to report with, so it only
                // gives a slot back. A child that ended by EOF was released by
                // `finish_reader_session` already, and a recovered transcript
                // has no row at all after a restart.
                self.release_agent_child(session_id);
                if let Some(journal) = &self.journal {
                    journal.try_mark_closed(session_id);
                    journal.unpin(session_id);
                    self.invalidate_journal_roster();
                }
                self.attachments.remove_session(session_id);
                self.notify_session_transition(owner, session_id);
                Ok(false)
            }
            None => {
                if let Some(journal) = &self.journal {
                    let known = journal.list()?.into_iter().find(|row| row.id == session_id);
                    if let Some(record) = known {
                        let session_owner = owner_from_session_id(session_id, &record.owner)?;
                        if session_owner.user != owner.user {
                            return Err(unauthorized());
                        }
                        journal.try_mark_closed(session_id);
                        self.invalidate_journal_roster();
                        self.attachments.remove_session(session_id);
                        self.notify_session_transition(owner, session_id);
                        return Ok(false);
                    }
                }
                Err(not_found())
            }
        }
    }

    /// Interrupt the current turn of an agent session without killing the
    /// process. Unlike `stop`, the registry entry stays live and later
    /// turns keep working.
    pub fn interrupt_with_subscription(
        &self,
        session_id: &str,
        subscription_id: u64,
        owner: &OwnerId,
        conn: &ConnHandle,
    ) -> Result<(), WireError> {
        validate_session_id(session_id)
            .map_err(|message| WireError::new(ErrorCode::InvalidRequest, message))?;
        let (mut killer, runtime) = {
            let mut map = self
                .inner
                .lock()
                .map_err(|_| internal("Session state is unavailable."))?;
            let entry = peer_entry_mut(&mut map, session_id, owner, &conn.conn_peer)?;
            let session = entry.as_peer_visible_mut().ok_or_else(process_gone)?;
            if !session.metadata.kind.is_agent() {
                return Err(WireError::new(
                    ErrorCode::InvalidRequest,
                    "Only agent sessions support interrupting a turn.",
                ));
            }
            (session.killer.clone_killer(), Arc::clone(&session.runtime))
        };
        check_attached(&runtime, conn, subscription_id)?;
        killer.interrupt();
        Ok(())
    }

    pub fn set_model(
        &self,
        session_id: &str,
        owner: &OwnerId,
        model_id: Option<&str>,
        effort: Option<&str>,
    ) -> Result<(), WireError> {
        validate_session_id(session_id)
            .map_err(|message| WireError::new(ErrorCode::InvalidRequest, message))?;
        if model_id.is_none() && effort.is_none() {
            return Err(WireError::new(
                ErrorCode::InvalidRequest,
                "A model or effort is required.",
            ));
        }
        let (switcher, kind, runtime) = {
            let mut map = self
                .inner
                .lock()
                .map_err(|_| internal("Session state is unavailable."))?;
            let entry = peer_entry_mut(&mut map, session_id, owner, &None)?;
            let session = entry.as_peer_visible_mut().ok_or_else(process_gone)?;
            if !session.metadata.kind.is_agent() {
                return Err(WireError::new(
                    ErrorCode::InvalidRequest,
                    "Only agent sessions support switching the model or effort.",
                ));
            }
            let switcher = session
                .switcher
                .as_ref()
                .map(|switcher| switcher.clone_switcher())
                .ok_or_else(|| {
                    WireError::new(
                        ErrorCode::InvalidRequest,
                        "This provider does not support switching the model or effort.",
                    )
                })?;
            (
                switcher,
                session.metadata.kind.clone(),
                Arc::clone(&session.runtime),
            )
        };
        if kind == SessionKind::Claude {
            Self::validate_claude_effort(
                runtime.session_manifest().as_ref(),
                runtime.claude_catalog_state(),
                model_id,
                effort,
            )?;
        }
        let result = switcher.set_model(model_id, effort);
        if result.is_ok() {
            if let Some(manifest) = switcher.manifest() {
                let manifest = runtime.store_session_manifest(manifest);
                let _ = runtime.publish_agent_event(manifest, None);
            }
        }
        result
    }

    /// Switch a live agent session's mode.
    ///
    /// The connection is threaded through like `interrupt_with_subscription`
    /// and `close`: `SessionSetMode` is under `CAP_SEND`, so it *is* reachable
    /// from a paired device, and the identity of the caller is part of the
    /// authorization the ownership check makes (§8b A3/A4/A5, H5). Without the
    /// connection the call site could only answer with the owner comparison,
    /// which is what let a mode change arrive with no origin attached.
    pub fn set_mode(
        &self,
        session_id: &str,
        owner: &OwnerId,
        mode_id: &str,
        conn: &ConnHandle,
    ) -> Result<(), WireError> {
        validate_session_id(session_id)
            .map_err(|message| WireError::new(ErrorCode::InvalidRequest, message))?;
        if mode_id.is_empty() {
            return Err(WireError::new(
                ErrorCode::InvalidRequest,
                "A mode is required.",
            ));
        }
        let (switcher, runtime) = {
            let mut map = self
                .inner
                .lock()
                .map_err(|_| internal("Session state is unavailable."))?;
            let entry = peer_entry_mut(&mut map, session_id, owner, &conn.conn_peer)?;
            let session = entry.as_peer_visible_mut().ok_or_else(process_gone)?;
            if !session.metadata.kind.is_agent() {
                return Err(WireError::new(
                    ErrorCode::InvalidRequest,
                    "Only agent sessions support switching the session mode.",
                ));
            }
            let manifest = session.runtime.session_manifest();
            let modes = match manifest.as_ref() {
                Some(SessionEvent::SessionManifest {
                    modes: Some(modes), ..
                }) => modes,
                _ => {
                    return Err(WireError::new(
                        ErrorCode::InvalidRequest,
                        "This provider has not advertised any session modes.",
                    ));
                }
            };
            if !modes.available_modes.iter().any(|mode| mode.id == mode_id) {
                return Err(WireError::new(
                    ErrorCode::InvalidRequest,
                    format!("Session mode '{mode_id}' is not available."),
                ));
            }
            let switcher = session
                .switcher
                .as_ref()
                .map(|switcher| switcher.clone_switcher())
                .ok_or_else(|| {
                    WireError::new(
                        ErrorCode::InvalidRequest,
                        "This provider does not support switching the session mode.",
                    )
                })?;
            (switcher, Arc::clone(&session.runtime))
        };
        switcher.set_mode(mode_id)?;
        let Some(SessionEvent::SessionManifest {
            provider_id,
            current_model_id,
            models,
            modes: Some(mut modes),
        }) = runtime.session_manifest()
        else {
            return Err(WireError::new(
                ErrorCode::InvalidRequest,
                "Session mode state disappeared while switching.",
            ));
        };
        modes.current_mode_id = mode_id.to_string();
        let manifest = runtime.store_session_manifest(SessionEvent::SessionManifest {
            provider_id,
            current_model_id,
            models,
            modes: Some(modes),
        });
        let _ = runtime.publish_agent_event(manifest, None);
        Ok(())
    }

    fn validate_claude_effort(
        manifest: Option<&SessionEvent>,
        catalog_state: crate::claude_catalog::ClaudeCatalogState,
        model_id: Option<&str>,
        effort: Option<&str>,
    ) -> Result<(), WireError> {
        if model_id.is_none() && effort.is_none() {
            return Ok(());
        }
        if catalog_state == crate::claude_catalog::ClaudeCatalogState::Provisional {
            return Ok(());
        }
        let Some(SessionEvent::SessionManifest {
            current_model_id,
            models,
            ..
        }) = manifest
        else {
            return Err(WireError::new(
                ErrorCode::InvalidRequest,
                "Claude has not published its model catalog yet.",
            ));
        };
        let model_id = model_id
            .filter(|model_id| !model_id.is_empty())
            .or(current_model_id.as_deref())
            .ok_or_else(|| {
                WireError::new(
                    ErrorCode::InvalidRequest,
                    "Claude has not reported a current model yet.",
                )
            })?;
        let model = models
            .iter()
            .find(|model| crate::claude_catalog::model_ids_match(&model.model_id, model_id))
            .ok_or_else(|| {
                WireError::new(
                    ErrorCode::InvalidRequest,
                    format!("Claude model '{model_id}' is not in the current catalog."),
                )
            })?;
        let Some(effort) = effort else {
            return Ok(());
        };
        let valid = model
            .efforts
            .as_ref()
            .is_some_and(|efforts| efforts.iter().any(|entry| entry.id == effort));
        if valid {
            Ok(())
        } else {
            Err(WireError::new(
                ErrorCode::InvalidRequest,
                format!("Effort '{effort}' is not supported by Claude model '{model_id}'."),
            ))
        }
    }

    pub fn list(&self, owner: &OwnerId) -> Result<Vec<Session>, WireError> {
        let map = self
            .inner
            .lock()
            .map_err(|_| internal("Session state is unavailable."))?;
        // A session inside its delivery window does not exist for its peers
        // (the re-audit's P2-1): the entry is skipped, and the row the
        // journal wrote before the spawn is skipped with it, so no roster
        // read can hand out an id a prompt would be lost on.
        let hidden: std::collections::HashSet<String> = map
            .values()
            .filter(|entry| entry.is_configuring())
            .map(|entry| entry.metadata().id.clone())
            .collect();
        let mut sessions: Vec<Session> = map
            .values()
            .filter(|entry| entry.owner().user == owner.user && !entry.is_configuring())
            .map(RegistryEntry::to_session)
            .collect();
        drop(map);
        if let Some(journal) = &self.journal {
            if let Ok(rows) = journal.list() {
                for row in rows {
                    if row.owner != owner.user {
                        continue;
                    }
                    if sessions.iter().any(|session| session.id == row.id) {
                        continue;
                    }
                    if hidden.contains(&row.id) {
                        continue;
                    }
                    sessions.push(row.to_session());
                }
            }
        }
        sessions.sort_by(|left, right| left.id.cmp(&right.id));
        Ok(sessions)
    }

    /// Pull asynchronous journal-loop failures into live runtimes so their
    /// attached event channels can report degradation even when the PTY has
    /// gone quiet since the failed write.
    pub fn refresh_journal_degradation(&self) {
        let runtimes = self
            .inner
            .lock()
            .map(|map| map.values().map(RegistryEntry::runtime).collect::<Vec<_>>())
            .unwrap_or_default();
        for runtime in runtimes {
            runtime.refresh_journal_degradation();
        }
    }

    pub fn has_live_journal_degradation(&self) -> bool {
        self.inner
            .lock()
            .map(|map| map.values().any(|entry| entry.runtime().journal_degraded()))
            .unwrap_or(true)
    }

    pub(crate) fn publish_claude_catalog(&self, models: Vec<SessionModel>) {
        let runtimes = self
            .inner
            .lock()
            .map(|map| {
                map.values()
                    .filter_map(|entry| {
                        let session = entry.as_peer_visible()?;
                        (session.metadata.kind == SessionKind::Claude)
                            .then(|| Arc::clone(&session.runtime))
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        for runtime in runtimes {
            let manifest = runtime.store_claude_catalog(
                crate::claude_catalog::manifest_with_current(models.clone(), None),
            );
            runtime.publish_agent_event(manifest, None);
        }
    }

    /// What the peer gate needs to refuse a session that runs without asking
    /// the user's permission (§8b A4/A5): the session's provider kind and the
    /// mode it is in now, when it has advertised one. `None` for a session
    /// this daemon does not know.
    pub(crate) fn session_mode_guard(
        &self,
        session_id: &str,
    ) -> Option<(SessionKind, Option<String>)> {
        let map = self.inner.lock().ok()?;
        let entry = map.get(session_id)?;
        // A session inside its delivery window does not exist for the peer
        // gate either (the re-audit's P2-2): `None` is this function's
        // "the daemon does not know this session", and the caller refuses
        // on that.
        if entry.is_configuring() {
            return None;
        }
        let kind = entry.metadata().kind.clone();
        Some((kind, entry.runtime().current_mode_id()))
    }

    /// The stored origin of the session behind `session_id`, for the MCP tool
    /// door (`mcp_broker.rs`). Read from the registry row, never from the
    /// loopback connection the broker holds: that socket is this machine's own
    /// by construction, so reading it would label a peer's child as local.
    ///
    /// `None` is every way there is no readable row — absent, or a poisoned
    /// lock — and the door refuses it with the pre-existing retryable absence
    /// sentence, never as the local person. Absence is transient by construction:
    /// an agent's first call can land before its own commit (the stub documents
    /// the race and retries exactly that sentence), and a reaped session's
    /// in-flight calls outlive its row; in both cases the row a retry finds a
    /// moment later is judged normally. A stored `Unknown` origin reads back as
    /// itself (`Some`), and is refused hard at the door: unlike absence it never
    /// resolves.
    ///
    /// This deliberately reads through the delivery window (`Configuring`): the
    /// window hides a session from its *targets* (`peer_entry` refuses it, so no
    /// peer path can act on a half-born session), but the caller's own origin
    /// was written at the create before the journal row and is already a fact.
    /// Refusing a configuring caller would turn a transient local birth into a
    /// refusal for the session's own first tool calls.
    pub(crate) fn caller_origin(&self, session_id: &str) -> Option<SessionOrigin> {
        let map = self.inner.lock().ok()?;
        let entry = map.get(session_id)?;
        Some(entry.metadata().origin.clone())
    }

    /// Whether `conn_peer` may reach `session_id` at all — asked *before* any
    /// question about what that session is (`session_mode_guard`, H6).
    ///
    /// The two failures are one answer on purpose. A session owned by someone
    /// else and a session this daemon does not know both give `unauthorized()`
    /// here, so a peer that probes another device's session ids learns nothing
    /// from comparing the replies: without this, `SessionSetMode` on a
    /// reachable-looking id answered "that session exists and runs this
    /// provider" through the mode policy gate, before the ownership check ever
    /// ran (§8b A1/A3).
    ///
    /// It is deliberately not a substitute for the checks the session methods
    /// make: this is the *ordering* the gate needs, and every operation still
    /// authorizes itself again at the point it touches the session.
    pub(crate) fn session_scope(
        &self,
        session_id: &str,
        owner: &OwnerId,
        conn_peer: &Option<ConnPeer>,
    ) -> Result<(), WireError> {
        let map = self
            .inner
            .lock()
            .map_err(|_| internal("Session state is unavailable."))?;
        match map.get(session_id) {
            Some(entry) => {
                check_user_owner(entry, owner, conn_peer)?;
                // The ordering gate must not admit a session that is still
                // inside its delivery window (the re-audit's P2-2): the
                // honest answer is the one the operation behind this gate
                // would give — `SessionNotFound` — while the unknown-id
                // refusal above stays `unauthorized`, so a probe still
                // learns nothing from comparing replies.
                if entry.is_configuring() {
                    return Err(not_found());
                }
                Ok(())
            }
            None => Err(unauthorized()),
        }
    }

    /// The peer gate's ordering check for `AgentMessageSend`. The target
    /// classifier is the one rule shared with the registry and mode consumer:
    /// an allowed local target proceeds, while a relay is refused here with
    /// the same `Unauthorized` as an unknown id. Local callers keep the
    /// ordinary ownership check at this door.
    pub(crate) fn agent_message_target_scope(
        &self,
        session_id: &str,
        owner: &OwnerId,
        conn_peer: &Option<ConnPeer>,
    ) -> Result<(), WireError> {
        let map = self
            .inner
            .lock()
            .map_err(|_| internal("Session state is unavailable."))?;
        let entry = map.get(session_id).ok_or_else(unauthorized)?;
        // Classify before consulting the delivery window so a configuring
        // relay remains indistinguishable from an unknown target.
        let result = match classify_agent_message_target(entry, conn_peer) {
            AgentMessageTargetClass::Local => Ok(()),
            AgentMessageTargetClass::Relay => {
                // A third-device target is refused before mode lookup, with
                // the same frame an unknown target receives; no existence or
                // provider-mode oracle belongs at this gate.
                Err(unauthorized())
            }
            AgentMessageTargetClass::OwnPeer | AgentMessageTargetClass::Other => {
                check_user_owner(entry, owner, conn_peer)
            }
        };
        result?;
        if entry.is_configuring() {
            return Err(not_found());
        }
        Ok(())
    }

    /// Whether the target's mode may be consulted by the peer gate. A
    /// scope-denied target is refused before this lookup, without revealing
    /// its provider or prompt-skipping mode.
    pub(crate) fn agent_message_target_is_mode_visible(
        &self,
        session_id: &str,
        conn_peer: &Option<ConnPeer>,
    ) -> bool {
        let Ok(map) = self.inner.lock() else {
            return false;
        };
        let Some(entry) = map.get(session_id) else {
            return false;
        };
        !matches!(
            classify_agent_message_target(entry, conn_peer),
            AgentMessageTargetClass::Relay
        )
    }

    fn runtime(&self, session_id: &str) -> Result<Arc<SessionRuntime>, WireError> {
        validate_session_id(session_id)
            .map_err(|message| WireError::new(ErrorCode::InvalidRequest, message))?;
        let map = self
            .inner
            .lock()
            .map_err(|_| internal("Session state is unavailable."))?;
        let session = map.get(session_id).ok_or_else(not_found)?;
        Ok(session.runtime())
    }

    fn runtime_for_user(
        &self,
        session_id: &str,
        owner: &OwnerId,
        conn: &ConnHandle,
    ) -> Result<Arc<SessionRuntime>, WireError> {
        validate_session_id(session_id)
            .map_err(|message| WireError::new(ErrorCode::InvalidRequest, message))?;
        let map = self
            .inner
            .lock()
            .map_err(|_| internal("Session state is unavailable."))?;
        // The peer door: attach, resize, detach and permission responses
        // reach a `Configuring` session through here, and the door refuses
        // the delivery window (the re-audit's P2-2). A transcript entry is
        // addressable — it is a roster member — so the door lets it through
        // and the runtime below serves it.
        let entry = peer_entry(&map, session_id, owner, &conn.conn_peer)?;
        Ok(entry.runtime())
    }
}

fn unix_millis() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

fn not_found() -> WireError {
    WireError::new(ErrorCode::SessionNotFound, "No session with that id.")
}

fn cannot_resume(reason: &str) -> WireError {
    WireError::new(
        ErrorCode::InvalidRequest,
        format!("This session cannot be resumed: {reason}."),
    )
}

fn resume_handle(
    record: &crate::journal::SessionRecord,
    owner: &OwnerId,
) -> Result<(String, String), WireError> {
    if record.owner != owner.user {
        return Err(unauthorized());
    }
    // Pi and Codex can resume on their own wires, but their end-to-end
    // design is not done: those families stay refused deliberately, not by
    // accident. The fact is the impls' `resumable()`; `resume_refusal()` is
    // only the wording of the refusal, so the decision is never expressible
    // in two places.
    let family = provider::catalog_registry().provider_for_kind(&record.kind);
    if !family.resumable() {
        return Err(cannot_resume(family.resume_refusal()));
    }
    let provider = record
        .provider
        .clone()
        .ok_or_else(|| cannot_resume("the provider was not persisted"))?;
    let peer_session_id = record
        .peer_session_id
        .clone()
        .ok_or_else(|| cannot_resume("the row has no provider handle to resume from"))?;
    Ok((provider, peer_session_id))
}

fn journal_unavailable() -> WireError {
    WireError::new(
        ErrorCode::Journal,
        "The conversation journal is unavailable.",
    )
}

pub(super) fn internal(message: impl Into<String>) -> WireError {
    WireError::new(ErrorCode::Internal, message)
}

fn pty_wire_error(context: &str, error: impl std::fmt::Display) -> WireError {
    let detail = error.to_string();
    let message = match extract_os_error_code(&detail) {
        Some(code) => {
            eprintln!("{context} (OS error {code})");
            format!(
                "{context} (OS error {code}: {}).",
                os_error_description(code)
            )
        }
        None => {
            eprintln!("{context} (unknown OS error)");
            format!("{context} (unknown OS error).")
        }
    };
    WireError::new(ErrorCode::Io, message)
}

fn extract_os_error_code(detail: &str) -> Option<u32> {
    detail
        .rsplit_once("(os error ")?
        .1
        .strip_suffix(')')?
        .parse()
        .ok()
}

fn os_error_description(code: u32) -> &'static str {
    match code {
        2 => "no such file or directory",
        3 => "path not found",
        8 => "not enough memory",
        232 => "no data",
        1450 => "no system resources",
        267 => "directory name is invalid",
        _ => "unknown error",
    }
}

#[cfg(test)]
pub(crate) fn insert_test_live_agent(
    registry: &SessionRegistry,
    id: &str,
    owner: OwnerId,
) -> Arc<SessionRuntime> {
    tests::insert_live_agent(registry, id, owner)
}

/// Test-only live agent that is somebody's child: `created_by` is the fact
/// the stop/close scope reads, so a test can build a real parent-child pair.
#[cfg(test)]
pub(crate) fn insert_test_child_agent(
    registry: &SessionRegistry,
    id: &str,
    owner: OwnerId,
    creator: &str,
) -> Arc<SessionRuntime> {
    tests::insert_child(registry, id, owner, creator)
}

/// Test-only live agent of one explicit kind (S9): the door reads origin, not
/// kind, so a pi/Codex-kind caller must meet exactly the judgment an ACP-kind
/// caller meets. Delegates to the same helper as the default insert.
#[cfg(test)]
pub(crate) fn insert_test_live_agent_with_kind(
    registry: &SessionRegistry,
    id: &str,
    owner: OwnerId,
    kind: SessionKind,
) -> Arc<SessionRuntime> {
    tests::insert_live_agent_with_kind_and_writer(
        registry,
        id,
        owner,
        kind,
        Box::new(tests::FailingWriter) as Box<dyn Write + Send>,
    )
}

/// One test-only live agent whose delivered bytes a test can read back: the
/// shape the broker-level attribution tests observe the envelope through.
#[cfg(test)]
pub(crate) fn insert_test_live_agent_with_recording_writer(
    registry: &SessionRegistry,
    id: &str,
    owner: OwnerId,
    kind: SessionKind,
) -> Arc<Mutex<Vec<u8>>> {
    let received = Arc::new(Mutex::new(Vec::new()));
    tests::insert_live_agent_with_kind_and_writer(
        registry,
        id,
        owner,
        kind,
        Box::new(tests::RecordingWriter(Arc::clone(&received))),
    );
    received
}

#[cfg(test)]
impl SessionRegistry {
    /// One test-only live agent session that is `creator`'s child, with a
    /// display name — the shape `devboule_answer_permission`'s chain checks.
    pub(crate) fn insert_test_child(
        &self,
        id: &str,
        owner: OwnerId,
        creator: &str,
    ) -> Arc<SessionRuntime> {
        let runtime = tests::insert_live_agent(self, id, owner);
        {
            let mut map = self.inner.lock().expect("registry");
            let live = map
                .get_mut(id)
                .and_then(RegistryEntry::as_peer_visible_mut)
                .expect("live entry");
            live.metadata.created_by = Some(creator.to_string());
            live.metadata.display_name = Some("child".to_string());
        }
        runtime
    }

    /// Test-only: overwrite one live row's stored origin, so an out-of-module
    /// test can drive the tool door as a peer's agent.
    pub(crate) fn set_test_origin(&self, session_id: &str, origin: SessionOrigin) {
        let mut map = self.inner.lock().expect("registry");
        let live = map
            .get_mut(session_id)
            .and_then(RegistryEntry::as_peer_visible_mut)
            .expect("live entry");
        live.metadata.origin = origin;
    }

    /// Test-only: park one permission card on a live session's broker, so an
    /// out-of-module test can answer one.
    pub(crate) fn test_park_card(&self, session_id: &str, card_id: &str) {
        let runtime = self.runtime(session_id).expect("runtime");
        let broker = runtime.permission_broker().expect("broker");
        broker
            .register(1, permission_broker::permission(card_id), &runtime)
            .expect("the card parks");
    }

    /// Test-only: a live agent child of `creator` with a display name, a
    /// manifest advertising `available_modes`, a switcher the move's asks land
    /// on, and the journal row the move's recording updates — the full shape
    /// `devboule_set_agent_profile` reads and records, so an out-of-module
    /// test can drive one end to end. `model_fails` aims the switcher's model
    /// ask at a refusal, for the partial-failure path.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn insert_test_move_child(
        &self,
        id: &str,
        owner: OwnerId,
        creator: &str,
        display_name: &str,
        available_modes: &[&str],
        current_model: Option<&str>,
        model_fails: bool,
    ) -> Arc<SessionRuntime> {
        let journal = self.journal.as_ref().expect("the registry's journal");
        let (runtime, _mode_calls, _model_calls, _order) = tests::insert_move_child(
            self,
            journal,
            id,
            owner,
            creator,
            display_name,
            available_modes,
            current_model,
            true,
            false,
            model_fails,
        );
        runtime
    }
}

/// One test-only live agent session with a writer of the caller's choosing.
///
/// `insert_test_live_agent` deliberately carries a writer that fails, which is
/// what a test about a *write* failure wants. A test that needs the session to
/// accept a prompt (the `AgentMessageSend` receipt path, for one) needs the
/// other half.
#[cfg(test)]
pub(crate) fn insert_test_live_agent_with_writer(
    registry: &SessionRegistry,
    id: &str,
    owner: OwnerId,
    kind: SessionKind,
    writer: Box<dyn Write + Send>,
) -> Arc<SessionRuntime> {
    tests::insert_live_agent_with_kind_and_writer(registry, id, owner, kind, writer)
}
