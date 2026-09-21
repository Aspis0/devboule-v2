//! The class-level provider seam: one [`Provider`] implementation per spawn
//! family (ACP, Claude, Pi, Codex, Terminal/PTY), and the [`ProviderRegistry`]
//! the spawn road resolves through.
//!
//! Per-family knowledge stays in the client modules: every method body here
//! delegates to today's code (`claude_client`, `pi_client`, `codex_client`,
//! `acp_client`, `peer_policy`, `mcp_broker`, `provider_vocabulary`), and a
//! method whose fact is still keyed by kind in shared code calls that shared
//! function rather than copying the list (`prompt_skipping_mode` -> the
//! `peer_policy` lists; the MOVE of those lists is pass 2b's business). This
//! module never grows a fact a client module should own; its one job is that
//! a create's road from request to running child selects its family through
//! the registry instead of an `if`/`match` on `SessionKind` or provider
//! strings in shared code.
//!
//! Three surfaces the design's §3.2 sketch did not carry (it was measured
//! before slice 5b) are spelled as impl-side facts rather than shared-code
//! literals, so the historical behaviour they encode survives the seam:
//!
//! - [`Provider::acp_create_remap_rank`] — the `Acp`-create remap's
//!   requested/env asymmetry (Claude accepts the env override as a native
//!   selection but not a request naming it) and the pi-then-codex arm order,
//!   both behaviour-identical with the pre-trait spelling and both invisible
//!   in a `wire_kind`-only sketch.
//! - [`Provider::stamp_session_provider`] — the provider id stamped on the
//!   record. Claude/Pi/Codex stamp their own id; the ACP family stamps the
//!   named id (or the command's resolved one — it is a fallthrough family
//!   with no id of its own); a terminal stamps nothing.
//! - [`Provider::resolves_named_from_catalog`] — the one family whose named
//!   command road consults the shared catalog, which is the road the npx
//!   consent gate guards (the gate itself stays catalog policy in
//!   `session.rs`, per the design's §3.3.5).

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock, RwLock};

use devboule_protocol::{
    ErrorCode, SessionKind, UnattendedState, VocabularyModels, VocabularyModes, WireError,
};
use portable_pty::PtySize;

use crate::mcp_broker::{McpLaunchConfig, McpProviderConfig};
use crate::paths::RuntimePaths;
use crate::process_tree::{JobObject, ProcessHandle};
use crate::profile_delivery::ProfileDelivery;
use crate::server::ServerState;

use super::{ProviderProvenance, SpawnedSession};

/// The unattended design's tri-state, keyed on authorship (`DESIGN-what-
/// unattended-means.md` §2): `Yes` — the daemon answers, by route A (its own
/// broker honours the mode) or route B (the daemon authored the knob); `No` —
/// the daemon authored a knob that asks; `CannotEstablish` — the vocabulary
/// is the provider's own, so the daemon cannot promise either way. The
/// epistemics sibling of `prompt_skipping_mode`: same home, different
/// question, never merged into one predicate.
///
/// `allow(dead_code)` until pass 2d converts `unattended_mode`'s call site —
/// the mapping exists so that conversion is a caller swap, not a redesign.
#[allow(dead_code)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum UnattendedAnswer {
    Yes,
    No,
    CannotEstablish,
}

impl From<UnattendedState> for UnattendedAnswer {
    fn from(state: UnattendedState) -> Self {
        match state {
            UnattendedState::Yes => Self::Yes,
            UnattendedState::No => Self::No,
            UnattendedState::Unknown => Self::CannotEstablish,
        }
    }
}

impl From<UnattendedAnswer> for UnattendedState {
    fn from(answer: UnattendedAnswer) -> Self {
        match answer {
            UnattendedAnswer::Yes => Self::Yes,
            UnattendedAnswer::No => Self::No,
            UnattendedAnswer::CannotEstablish => Self::Unknown,
        }
    }
}

/// Route A of the `unattended` marker, shared by the four agent impls: the
/// daemon's own broker answers the delivered mode itself, whatever family
/// the child belongs to. Route B — the daemon authored the knob — is the
/// per-impl `dictionary` each family passes. A terminal never reaches this
/// helper: it has no permission mechanism, so no mode id can make route A
/// true for it, and its impl answers `No` without consulting anything.
fn agent_unattended_mode(
    delivered_mode: Option<&str>,
    dictionary: impl FnOnce(Option<&str>) -> UnattendedState,
) -> UnattendedAnswer {
    let delivered_mode = delivered_mode.filter(|mode| !mode.is_empty());
    if delivered_mode.is_some_and(crate::provider_catalog::mode_is_auto_answered) {
        return UnattendedAnswer::Yes;
    }
    dictionary(delivered_mode).into()
}

/// One provider family, at class level: the facts a create needs before any
/// child exists. Sync, object-safe, handed around as `Arc<dyn Provider>` —
/// the house trait style (`transport.rs`, `peer_transport.rs`, the
/// per-session traits on `SpawnedSession`). Per-session state (killers,
/// steerers, switchers, sinks) deliberately stays on the per-session traits;
/// this trait answers only what is true of the family.
///
/// `allow(dead_code)` on the trait, precisely because 2a is the first slice
/// of pass 2: the spawn road (identity, remap, command resolution, stamping,
/// spawn) routes through here today, while the mode-vocabulary, image, MCP
/// and lifecycle surfaces keep their delegation bodies for passes 2b/2c,
/// whose call-site conversions are the callers those methods wait for. Each
/// body is live delegation to today's code, written now so 2b/2c convert a
/// caller, not a signature.
#[allow(dead_code)]
pub(crate) trait Provider: Send + Sync {
    /// The family's own id, exactly as the catalog spells the rows that bind
    /// to it (`"claude"`, `"codex"`, `"pi"`). The ACP family is a fallthrough
    /// serving many catalog rows and has no id of its own.
    fn id(&self) -> &'static str;

    /// The kind sessions of this provider are created as. The one place the
    /// kind is derived from a provider — the wire and the journal keep
    /// `SessionKind`; the daemon's decisions stop matching on it.
    fn wire_kind(&self) -> SessionKind;

    /// The family's rank in the `Acp`-create remap (`resolve_session_provider`),
    /// or `None` when this provenance never remaps. The rank is the historical
    /// arm order (pi before codex before claude) and the `None` carries the
    /// asymmetry the literal spelling encoded: Claude remaps only from the env
    /// override, never from a request naming it, and the fallthrough/terminal
    /// families never remap at all.
    fn acp_create_remap_rank(&self, provenance: ProviderProvenance) -> Option<u8>;

    /// Whether the family's named command road consults the shared catalog —
    /// the road the npx consent gate guards. `true` only for ACP, whose named
    /// road is the one a catalog npx wrapper can resolve through.
    fn resolves_named_from_catalog(&self) -> bool;

    /// Resolve the command this family's child starts with. `named` is the
    /// provider id a create named, when one survived resolution; families
    /// with a fixed command ignore it, and the ACP family resolves the named
    /// catalog row (or its default road when `None`).
    fn resolve_command(
        &self,
        paths: &RuntimePaths,
        named: Option<&str>,
    ) -> Result<super::PtyCommand, WireError>;

    /// Spawn the child. The MCP launch config and the typed delivery thread
    /// through unchanged: the per-family carriers are built inside each
    /// client's `spawn_process` (S5/S6), which this method wraps. The
    /// workspace id rides in so each family's spawn error keeps the exact
    /// workspace mapping it had before the seam — the four client roads
    /// mapped the whole client call, the terminal road only its child-spawn
    /// step — which is why the mapping lives inside the impls, not at the
    /// shared call site.
    fn spawn(
        &self,
        state: &Arc<ServerState>,
        command: super::PtyCommand,
        mcp: Option<McpLaunchConfig>,
        delivery: ProfileDelivery,
        workspace_id: Option<&str>,
    ) -> Result<SpawnedSession, WireError>;

    /// The provider id stamped on the session record. `named` is the resolved
    /// provider field, `resolved` the command's own provider id; the default
    /// (Claude, Pi, Codex) is the family's own id, the ACP family passes the
    /// named id through with the command's as fallback, and a terminal
    /// stamps none.
    fn stamp_session_provider(
        &self,
        named: Option<String>,
        resolved: Option<String>,
    ) -> Option<String> {
        let _ = (named, resolved);
        Some(self.id().to_string())
    }

    /// The mode a child starts in when the create named none. `None` for the
    /// families whose vocabulary is the agent's own (ACP) or which have no
    /// modes (terminal).
    fn default_mode(&self) -> Option<&'static str>;

    /// Whether a mode id may be set on this family's sessions. ACP's runtime
    /// vocabulary cannot be vetted here (`modes_unvetted` is the judgement);
    /// a terminal has no modes at all.
    fn validate_mode(&self, mode_id: &str) -> Result<(), WireError>;

    /// Whether this mode skips permission prompts for a peer. The lists are
    /// per-family facts and live here since pass 2b — `peer_policy`'s mode
    /// functions read them through the registry, so the walking tests and
    /// the peer gate answer from one source: these impls.
    fn prompt_skipping_mode(&self, mode_id: &str) -> bool;

    /// Whether this family's modes are defined at run time by the agent and
    /// therefore unvetted by the daemon: the peer gate's fail-closed ACP arm.
    fn modes_unvetted(&self) -> bool;

    /// The image delivery this family is authorised for at class level. The
    /// per-session truth can only be more specific than this answer (an ACP
    /// handshake's negotiated verdict, a Pi model's tri-state), never less
    /// honest: the class answer here is the fail-closed one those sources
    /// answer before they have spoken.
    fn image_delivery(&self) -> super::ImageDelivery;

    /// Whether sessions of this family can host the daemon's MCP broker.
    /// Answered per impl since pass 2c: every agent family hosts, a
    /// terminal hosts nothing. The broker's `hosts_mcp` predicate reads
    /// this through the registry, so these impls are the single source.
    fn hosts_mcp(&self) -> bool;

    /// Whether this family's first prompt may wait on the broker.
    /// Deliberately narrower than `hosts_mcp` (ACP/Claude only): carriers
    /// are best-effort and slow (Codex measured ~7.4 s against a dead
    /// broker); blocking a healthy pi/Codex child's first prompt on them
    /// would make an outage of the broker an outage of the child. The
    /// broker's `mcp_gates_first_prompt` reads this through the registry,
    /// and the twin never-block tests pin the rule.
    fn mcp_gates_first_prompt(&self) -> bool;

    /// Build this family's MCP carrier from the broker's launch config. Pi
    /// and Codex own real carriers (bridge file / per-session home); the ACP
    /// and Claude shapes ride the handshake and the launch argv inside their
    /// clients and have no carrier to mint here (pass 2c moves the shapes).
    fn mcp_launch(
        &self,
        config: &McpLaunchConfig,
        runtime_dir: &Path,
    ) -> Result<McpProviderConfig, WireError>;

    /// Whether this family's sessions support resume. Admitted per family;
    /// the refusal site (`resume_handle`) asks this.
    fn resumable(&self) -> bool;

    /// Respawning a dead session of this family onto its existing row: same
    /// id, same workspace, same transcript, new generation. Only families
    /// whose `resumable()` is true carry a real one; the rest refuse with
    /// their own sentence, so flipping a family to resumable without writing
    /// its spawn is a loud failure, never a silent new session.
    fn spawn_resuming(
        &self,
        state: &Arc<ServerState>,
        command: super::PtyCommand,
        peer_session_id: String,
        mcp: Option<McpLaunchConfig>,
    ) -> Result<SpawnedSession, WireError>;

    /// The wording of this family's resume refusal. NOT a second source of
    /// the decision — the fact stays `resumable()`; this method is only its
    /// explanation, so the yes/no is never expressible in two places. The
    /// default is the generic sentence; no family overrides it today.
    fn resume_refusal(&self) -> &'static str {
        "only ACP, Claude and Codex sessions support this resume path"
    }

    /// The refusal a non-resumable family's `spawn_resuming` answers with:
    /// the same sentence `resume_handle` refused with, so the gate and the
    /// spawn cannot disagree about why. One spelling, shared by the impls
    /// (`session.rs::cannot_resume` states it for the gate; this states it
    /// for the spawn).
    fn resume_refused(&self) -> WireError {
        WireError::new(
            ErrorCode::InvalidRequest,
            format!("This session cannot be resumed: {}.", self.resume_refusal()),
        )
    }

    /// Whether a successful spawn of this family measures provider health
    /// (ACP/Pi/Codex yes; Claude records failures only). The create path's
    /// success arm asks this; the failure arm judges the error, not the
    /// family, and stays a free function (`spawn_failure_is_provider_health`).
    fn spawn_measures_health(&self) -> bool;

    /// The vocabulary axes this family can answer a vocabulary query with.
    /// Answered per impl since pass 2d: Claude's disk scrape
    /// (`provider_vocabulary::claude_axes`), every other family `absent`
    /// (`provider_vocabulary::absent_axes`). The query's cache/request layer
    /// stays on `ServerState`; the reply asks this through the registry, so
    /// these impls are the single source.
    fn vocabulary(&self, state: &Arc<ServerState>) -> (VocabularyModels, VocabularyModes);

    /// Validate what a profile delivery asks this family to impose. The
    /// refusals are the client's own (`validate_delivery` free functions,
    /// landed with slice 5b); the application stays inside the client's
    /// `spawn_process`. ACP judges post-handshake and refuses nothing here;
    /// a terminal receives no delivery.
    fn validate_delivery(
        &self,
        state: &Arc<ServerState>,
        spec: &ProfileDelivery,
    ) -> Result<(), WireError>;

    /// The `unattended` marker's answer for a mode delivered to this family.
    /// Answered per impl since pass 2d; the epistemics sibling of
    /// `prompt_skipping_mode` — same home, different question, never merged
    /// into one predicate. Route A (the daemon answers itself) is the shared
    /// helper every agent impl calls; route B (the daemon authored the knob)
    /// is each family's own dictionary; an unauthored id answers
    /// `CannotEstablish`, never `No`. `peer_policy::unattended_mode` keeps
    /// its signature and answers through the registry, so the birth marker,
    /// the child road, the profile prediction and the tests read one source:
    /// these impls.
    fn unattended_mode(&self, delivered_mode: Option<&str>) -> UnattendedAnswer;
}

/// The daemon's resume verdict for one session: the process is gone, the
/// family is resumable, the provider and peer id were persisted non-empty,
/// and the peer id is not the one a provider refused. The one projection
/// every surface reads — journal rows, live views, diagnostics — so the
/// family yes/no stays `Provider::resumable()` and is never re-spelled
/// beside it.
pub(crate) fn session_resumable(
    kind: &SessionKind,
    provider: Option<&str>,
    peer_session_id: Option<&str>,
    is_live: bool,
    disowned_peer_session_id: Option<&str>,
) -> bool {
    !is_live
        && catalog_registry().provider_for_kind(kind).resumable()
        && provider.is_some_and(|id| !id.is_empty())
        && peer_session_id.is_some_and(|id| !id.is_empty())
        // A fact learned from the provider, not another prediction: this
        // exact handle is the one a resume was refused for. It rides beside
        // the handle — which is never destroyed — and the next announce of a
        // different handle clears it.
        && peer_session_id != disowned_peer_session_id
}

/// The ACP family: the fallthrough implementation. Every provider the catalog
/// publishes that no native family binds is an ACP provider — that is the
/// open provider dimension — so this impl carries no id a catalog row can
/// bind to and its per-session facts are the conservative ones (nothing is
/// known before the handshake speaks).
struct AcpProvider;

impl Provider for AcpProvider {
    fn id(&self) -> &'static str {
        "acp"
    }

    fn wire_kind(&self) -> SessionKind {
        SessionKind::Acp
    }

    fn acp_create_remap_rank(&self, _provenance: ProviderProvenance) -> Option<u8> {
        // An Acp create naming an ACP provider is not a remap; the remap
        // exists to promote an Acp create to a native family.
        None
    }

    fn resolves_named_from_catalog(&self) -> bool {
        true
    }

    fn resolve_command(
        &self,
        paths: &RuntimePaths,
        named: Option<&str>,
    ) -> Result<super::PtyCommand, WireError> {
        match named {
            Some(id) => super::acp_client::resolve_named(id, paths),
            None => super::acp_client::resolve_command(paths),
        }
    }

    fn spawn(
        &self,
        state: &Arc<ServerState>,
        command: super::PtyCommand,
        mcp: Option<McpLaunchConfig>,
        delivery: ProfileDelivery,
        workspace_id: Option<&str>,
    ) -> Result<SpawnedSession, WireError> {
        let workspace_path = command.cwd.clone();
        super::acp_client::spawn_process(state, command, mcp, delivery).map_err(|error| {
            super::map_workspace_spawn_wire_error(workspace_id, &workspace_path, error)
        })
    }

    fn stamp_session_provider(
        &self,
        named: Option<String>,
        resolved: Option<String>,
    ) -> Option<String> {
        // No id of its own: the create's named id stands, else the command's
        // resolved one (the catalog row, the ACP override, or nothing).
        named.or(resolved)
    }

    fn default_mode(&self) -> Option<&'static str> {
        // The agent defines its modes at run time and names its own default
        // in `initialize`; the daemon has no list to name one from.
        None
    }

    fn validate_mode(&self, _mode_id: &str) -> Result<(), WireError> {
        // The vocabulary is the agent's own; the daemon cannot vet an id
        // against a list it does not have. The peer gate's fail-closed arm
        // (`modes_unvetted`) is the judgement, not this check.
        Ok(())
    }

    fn modes_unvetted(&self) -> bool {
        true
    }
    fn prompt_skipping_mode(&self, _mode_id: &str) -> bool {
        // Peer-authored vocabulary: this function cannot answer for ACP
        // modes, and the peer gate's refusal comes from `modes_unvetted`.
        false
    }

    fn image_delivery(&self) -> super::ImageDelivery {
        // Before the handshake speaks, nothing is negotiated: the same
        // `from_negotiated` verdict an `Absent` answer produces. The
        // session's negotiated verdict overrides per session.
        super::ImageDelivery::PathLine
    }

    fn hosts_mcp(&self) -> bool {
        true
    }

    fn mcp_gates_first_prompt(&self) -> bool {
        true
    }

    fn mcp_launch(
        &self,
        _config: &McpLaunchConfig,
        _runtime_dir: &Path,
    ) -> Result<McpProviderConfig, WireError> {
        // ACP's MCP shape is the payload form applied at the handshake inside
        // the client (`acp_server_value` into `session/new`/`session/load`);
        // no carrier is minted outside it. Pass 2c moves the shape.
        Ok(McpProviderConfig::default())
    }

    fn resumable(&self) -> bool {
        // One of the two families the resume gate admits (`resume_handle`
        // asks this); the spawn half is `spawn_process_resuming`.
        true
    }

    fn spawn_resuming(
        &self,
        state: &Arc<ServerState>,
        command: super::PtyCommand,
        peer_session_id: String,
        mcp: Option<McpLaunchConfig>,
    ) -> Result<SpawnedSession, WireError> {
        super::acp_client::spawn_process_resuming(state, command, peer_session_id, mcp)
    }

    fn spawn_measures_health(&self) -> bool {
        // A completed handshake proves the provider started and accepted a
        // session, so it measures provider health.
        true
    }

    fn vocabulary(&self, _state: &Arc<ServerState>) -> (VocabularyModels, VocabularyModes) {
        // No source could answer yet: `absent` is a wire value, never an
        // empty `present` and never `none` — the app renders it as a
        // free-text field with the sentence that says why.
        crate::provider_vocabulary::absent_axes()
    }

    fn validate_delivery(
        &self,
        _state: &Arc<ServerState>,
        _spec: &ProfileDelivery,
    ) -> Result<(), WireError> {
        // ACP judges the delivery post-handshake (`apply_profile_delivery`):
        // only the agent can say whether a mode or model it defined exists,
        // so there is nothing to refuse before the child speaks.
        Ok(())
    }

    fn unattended_mode(&self, delivered_mode: Option<&str>) -> UnattendedAnswer {
        // An ACP agent's modes are prose the agent authored; no table here
        // judges them, so outside the route-A ids the answer is
        // `CannotEstablish`.
        agent_unattended_mode(delivered_mode, |_| UnattendedState::Unknown)
    }
}

/// The Claude family: stream-json road, launcher-owned argv.
struct ClaudeProvider;

impl Provider for ClaudeProvider {
    fn id(&self) -> &'static str {
        "claude"
    }

    fn wire_kind(&self) -> SessionKind {
        SessionKind::Claude
    }

    fn acp_create_remap_rank(&self, provenance: ProviderProvenance) -> Option<u8> {
        // Claude accepts the env override as a native selection but not a
        // request naming it — the asymmetry the literal spelling encoded
        // (`requested == "claude"` never opened the remap guard; env
        // `"claude"` did). Preserved verbatim, as provider policy.
        matches!(provenance, ProviderProvenance::Env).then_some(2)
    }

    fn resolves_named_from_catalog(&self) -> bool {
        false
    }

    fn resolve_command(
        &self,
        paths: &RuntimePaths,
        _named: Option<&str>,
    ) -> Result<super::PtyCommand, WireError> {
        super::claude_client::resolve_command(paths)
    }

    fn spawn(
        &self,
        state: &Arc<ServerState>,
        command: super::PtyCommand,
        mcp: Option<McpLaunchConfig>,
        delivery: ProfileDelivery,
        workspace_id: Option<&str>,
    ) -> Result<SpawnedSession, WireError> {
        let workspace_path = command.cwd.clone();
        super::claude_client::spawn_process(state, command, mcp, delivery).map_err(|error| {
            super::map_workspace_spawn_wire_error(workspace_id, &workspace_path, error)
        })
    }

    fn default_mode(&self) -> Option<&'static str> {
        Some(crate::claude_view::DEFAULT_MODE)
    }

    fn validate_mode(&self, mode_id: &str) -> Result<(), WireError> {
        // The same walk the delivery validation applies: a mode is available
        // when the launcher's mode list carries it.
        if crate::claude_view::mode_state(mode_id)
            .available_modes
            .iter()
            .any(|mode| mode.id == mode_id)
        {
            Ok(())
        } else {
            Err(WireError::new(
                ErrorCode::InvalidRequest,
                format!("Claude session mode '{mode_id}' is not available."),
            ))
        }
    }

    fn modes_unvetted(&self) -> bool {
        false
    }
    fn prompt_skipping_mode(&self, mode_id: &str) -> bool {
        matches!(mode_id, "acceptEdits" | "auto" | "bypassPermissions")
    }

    fn image_delivery(&self) -> super::ImageDelivery {
        super::claude_client::claude_delivery()
    }

    fn hosts_mcp(&self) -> bool {
        true
    }

    fn mcp_gates_first_prompt(&self) -> bool {
        true
    }

    fn mcp_launch(
        &self,
        _config: &McpLaunchConfig,
        _runtime_dir: &Path,
    ) -> Result<McpProviderConfig, WireError> {
        // Claude's MCP shape is the `--mcp-config` argv pair, pointed at the
        // path the broker minted into the launch config and applied inside
        // the client's spawn. No carrier is built outside it; pass 2c moves
        // the shape.
        Ok(McpProviderConfig::default())
    }

    fn resumable(&self) -> bool {
        // The second family the resume gate admits: the provider keeps its
        // own conversation on disk and takes it back by `--resume`.
        true
    }

    fn spawn_resuming(
        &self,
        state: &Arc<ServerState>,
        command: super::PtyCommand,
        peer_session_id: String,
        mcp: Option<McpLaunchConfig>,
    ) -> Result<SpawnedSession, WireError> {
        super::claude_client::spawn_process_resuming(state, command, peer_session_id, mcp)
    }

    fn spawn_measures_health(&self) -> bool {
        // A process spawn proves nothing about the provider, so Claude
        // records failures only (at the create path's failure arm).
        false
    }

    fn vocabulary(&self, state: &Arc<ServerState>) -> (VocabularyModels, VocabularyModes) {
        // Claude costs (almost) no process: the catalog derivation reads the
        // CLI's files on disk. The one process a read can start is the
        // native version probe, inside `claude_models`, and only while the
        // installed version is still unknown. Both axes are `present`.
        crate::provider_vocabulary::claude_axes(state)
    }

    fn validate_delivery(
        &self,
        state: &Arc<ServerState>,
        spec: &ProfileDelivery,
    ) -> Result<(), WireError> {
        super::claude_client::validate_delivery(&state.claude_models(), spec)
    }

    fn unattended_mode(&self, delivered_mode: Option<&str>) -> UnattendedAnswer {
        // Route B is the launcher's own mode table — the vocabulary the
        // daemon delivers and therefore knows.
        agent_unattended_mode(delivered_mode, crate::claude_view::unattended_answer)
    }
}

/// The Pi family: RPC road, permission extension at spawn.
struct PiProvider;

impl Provider for PiProvider {
    fn id(&self) -> &'static str {
        "pi"
    }

    fn wire_kind(&self) -> SessionKind {
        SessionKind::Pi
    }

    fn acp_create_remap_rank(&self, _provenance: ProviderProvenance) -> Option<u8> {
        // The historical arm order: a pi id wins the remap over every other
        // family, from either provenance.
        Some(0)
    }

    fn resolves_named_from_catalog(&self) -> bool {
        false
    }

    fn resolve_command(
        &self,
        paths: &RuntimePaths,
        _named: Option<&str>,
    ) -> Result<super::PtyCommand, WireError> {
        super::pi_client::resolve_command(paths)
    }

    fn spawn(
        &self,
        state: &Arc<ServerState>,
        command: super::PtyCommand,
        mcp: Option<McpLaunchConfig>,
        delivery: ProfileDelivery,
        workspace_id: Option<&str>,
    ) -> Result<SpawnedSession, WireError> {
        let workspace_path = command.cwd.clone();
        super::pi_client::spawn_process(state, command, mcp, delivery).map_err(|error| {
            super::map_workspace_spawn_wire_error(workspace_id, &workspace_path, error)
        })
    }

    fn default_mode(&self) -> Option<&'static str> {
        Some(super::pi_client::DEFAULT_MODE)
    }

    fn validate_mode(&self, mode_id: &str) -> Result<(), WireError> {
        if super::pi_client::mode_is_known(mode_id) {
            Ok(())
        } else {
            Err(WireError::new(
                ErrorCode::InvalidRequest,
                format!("Pi session mode '{mode_id}' is not available."),
            ))
        }
    }

    fn modes_unvetted(&self) -> bool {
        false
    }
    fn prompt_skipping_mode(&self, mode_id: &str) -> bool {
        mode_id == "bypass"
    }

    fn image_delivery(&self) -> super::ImageDelivery {
        // The true delivery is per-model (`pi_delivery(catalog, model)`),
        // read at prompt time; the class answer is the absent-catalog arm
        // that function itself answers with.
        super::ImageDelivery::PathLine
    }

    fn hosts_mcp(&self) -> bool {
        true
    }

    fn mcp_gates_first_prompt(&self) -> bool {
        // A best-effort carrier: slow against a dead broker, so a healthy
        // pi child's first prompt never blocks on it.
        false
    }

    fn mcp_launch(
        &self,
        config: &McpLaunchConfig,
        runtime_dir: &Path,
    ) -> Result<McpProviderConfig, WireError> {
        super::pi_client::mcp_launch(config, runtime_dir)
    }

    fn resumable(&self) -> bool {
        // Deliberate, not accidental: pi can resume on its own wire, but the
        // end-to-end design is not done (`resume_handle`'s refusal comment).
        false
    }

    fn spawn_resuming(
        &self,
        _state: &Arc<ServerState>,
        _command: super::PtyCommand,
        _peer_session_id: String,
        _mcp: Option<McpLaunchConfig>,
    ) -> Result<SpawnedSession, WireError> {
        Err(self.resume_refused())
    }

    fn spawn_measures_health(&self) -> bool {
        true
    }

    fn vocabulary(&self, _state: &Arc<ServerState>) -> (VocabularyModels, VocabularyModes) {
        // No source could answer yet: `absent` is a wire value, never an
        // empty `present` and never `none` — the app renders it as a
        // free-text field with the sentence that says why.
        crate::provider_vocabulary::absent_axes()
    }

    fn validate_delivery(
        &self,
        _state: &Arc<ServerState>,
        spec: &ProfileDelivery,
    ) -> Result<(), WireError> {
        super::pi_client::validate_delivery(spec)
    }

    fn unattended_mode(&self, delivered_mode: Option<&str>) -> UnattendedAnswer {
        // Route B is the family's own mode table — the vocabulary the
        // daemon delivers and therefore knows.
        agent_unattended_mode(delivered_mode, crate::session::pi_unattended_answer)
    }
}

/// The Codex family: app-server road, per-session home at spawn.
struct CodexProvider;

impl Provider for CodexProvider {
    fn id(&self) -> &'static str {
        "codex"
    }

    fn wire_kind(&self) -> SessionKind {
        SessionKind::Codex
    }

    fn acp_create_remap_rank(&self, _provenance: ProviderProvenance) -> Option<u8> {
        // Second in the historical arm order: a codex id remaps unless a pi
        // id was named on either road.
        Some(1)
    }

    fn resolves_named_from_catalog(&self) -> bool {
        false
    }

    fn resolve_command(
        &self,
        paths: &RuntimePaths,
        _named: Option<&str>,
    ) -> Result<super::PtyCommand, WireError> {
        super::codex_client::resolve_command(paths)
    }

    fn spawn(
        &self,
        state: &Arc<ServerState>,
        command: super::PtyCommand,
        mcp: Option<McpLaunchConfig>,
        delivery: ProfileDelivery,
        workspace_id: Option<&str>,
    ) -> Result<SpawnedSession, WireError> {
        let workspace_path = command.cwd.clone();
        super::codex_client::spawn_process(state, command, mcp, delivery).map_err(|error| {
            super::map_workspace_spawn_wire_error(workspace_id, &workspace_path, error)
        })
    }

    fn default_mode(&self) -> Option<&'static str> {
        Some(crate::codex_view::DEFAULT_MODE)
    }

    fn validate_mode(&self, mode_id: &str) -> Result<(), WireError> {
        crate::codex_view::validate_mode(mode_id)
    }

    fn modes_unvetted(&self) -> bool {
        false
    }
    fn prompt_skipping_mode(&self, mode_id: &str) -> bool {
        // `auto` is deliberately absent: it still prompts (`on-request` +
        // workspaceWrite), so a peer may set it. Only the never-ask modes
        // are prompt-skipping.
        matches!(mode_id, "auto-review" | "full-access")
    }

    fn image_delivery(&self) -> super::ImageDelivery {
        super::codex_client::codex_delivery()
    }

    fn hosts_mcp(&self) -> bool {
        true
    }

    fn mcp_gates_first_prompt(&self) -> bool {
        // A best-effort carrier (measured ~7.4 s against a dead broker): a
        // healthy Codex child's first prompt never blocks on it.
        false
    }

    fn mcp_launch(
        &self,
        config: &McpLaunchConfig,
        runtime_dir: &Path,
    ) -> Result<McpProviderConfig, WireError> {
        super::codex_client::mcp_launch(config, runtime_dir)
    }

    fn resumable(&self) -> bool {
        // `thread/resume` loads the thread from disk by the `threadId` this
        // family already persists; the rollout is the conversation, and no
        // history rides our journal onto the wire.
        true
    }

    fn spawn_resuming(
        &self,
        state: &Arc<ServerState>,
        command: super::PtyCommand,
        peer_session_id: String,
        mcp: Option<McpLaunchConfig>,
    ) -> Result<SpawnedSession, WireError> {
        super::codex_client::spawn_process_resuming(state, command, peer_session_id, mcp)
    }

    fn spawn_measures_health(&self) -> bool {
        true
    }

    fn vocabulary(&self, _state: &Arc<ServerState>) -> (VocabularyModels, VocabularyModes) {
        // No source could answer yet: `absent` is a wire value, never an
        // empty `present` and never `none` — the app renders it as a
        // free-text field with the sentence that says why.
        crate::provider_vocabulary::absent_axes()
    }

    fn validate_delivery(
        &self,
        _state: &Arc<ServerState>,
        spec: &ProfileDelivery,
    ) -> Result<(), WireError> {
        super::codex_client::validate_delivery(spec)
    }

    fn unattended_mode(&self, delivered_mode: Option<&str>) -> UnattendedAnswer {
        // Route B is the family's own mode table — the vocabulary the
        // daemon delivers and therefore knows.
        agent_unattended_mode(delivered_mode, crate::codex_view::unattended_answer)
    }
}

/// The Terminal/PTY family: the plain shell road, no agent machinery.
struct TerminalProvider;

impl Provider for TerminalProvider {
    fn id(&self) -> &'static str {
        "terminal"
    }

    fn wire_kind(&self) -> SessionKind {
        SessionKind::Terminal
    }

    fn acp_create_remap_rank(&self, _provenance: ProviderProvenance) -> Option<u8> {
        None
    }

    fn resolves_named_from_catalog(&self) -> bool {
        false
    }

    fn resolve_command(
        &self,
        paths: &RuntimePaths,
        _named: Option<&str>,
    ) -> Result<super::PtyCommand, WireError> {
        super::shell_command::resolve_pty_command(paths)
    }

    fn spawn(
        &self,
        state: &Arc<ServerState>,
        command: super::PtyCommand,
        _mcp: Option<McpLaunchConfig>,
        _delivery: ProfileDelivery,
        workspace_id: Option<&str>,
    ) -> Result<SpawnedSession, WireError> {
        // The PTY road, moved verbatim out of `spawn_session`'s fallthrough
        // (a move, not a rewrite): the same openpty, the same Windows job
        // containment, the same writer/reader wiring, the same teardowns.
        open_pty_session(state, command, workspace_id)
    }

    fn stamp_session_provider(
        &self,
        named: Option<String>,
        resolved: Option<String>,
    ) -> Option<String> {
        let _ = (named, resolved);
        // A terminal has no provider.
        None
    }

    fn default_mode(&self) -> Option<&'static str> {
        None
    }

    fn validate_mode(&self, _mode_id: &str) -> Result<(), WireError> {
        Err(WireError::new(
            ErrorCode::InvalidRequest,
            "a terminal session has no modes.",
        ))
    }

    fn modes_unvetted(&self) -> bool {
        false
    }
    fn prompt_skipping_mode(&self, _mode_id: &str) -> bool {
        false
    }

    fn image_delivery(&self) -> super::ImageDelivery {
        super::ImageDelivery::PathLine
    }

    fn hosts_mcp(&self) -> bool {
        false
    }

    fn mcp_gates_first_prompt(&self) -> bool {
        false
    }

    fn mcp_launch(
        &self,
        _config: &McpLaunchConfig,
        _runtime_dir: &Path,
    ) -> Result<McpProviderConfig, WireError> {
        // A terminal hosts no MCP broker.
        Ok(McpProviderConfig::default())
    }

    fn resumable(&self) -> bool {
        false
    }

    fn spawn_resuming(
        &self,
        _state: &Arc<ServerState>,
        _command: super::PtyCommand,
        _peer_session_id: String,
        _mcp: Option<McpLaunchConfig>,
    ) -> Result<SpawnedSession, WireError> {
        Err(self.resume_refused())
    }

    fn spawn_measures_health(&self) -> bool {
        false
    }

    fn vocabulary(&self, _state: &Arc<ServerState>) -> (VocabularyModels, VocabularyModes) {
        // No source could answer yet: `absent` is a wire value, never an
        // empty `present` and never `none` — the app renders it as a
        // free-text field with the sentence that says why.
        crate::provider_vocabulary::absent_axes()
    }

    fn validate_delivery(
        &self,
        _state: &Arc<ServerState>,
        _spec: &ProfileDelivery,
    ) -> Result<(), WireError> {
        // A terminal receives no profile delivery (a create that resolved no
        // profile delivers nothing, and terminals resolve no profiles).
        Ok(())
    }

    fn unattended_mode(&self, _delivered_mode: Option<&str>) -> UnattendedAnswer {
        // A terminal has no permission mechanism at all: no mode id —
        // including a route-A id — can make one exist.
        UnattendedAnswer::No
    }
}

/// The PTY road, moved from `spawn_session`'s fallthrough. Every line is the
/// road it replaced — the ConPTY DSR comment, the two-step job containment,
/// the writer-before-reader ordering, the workspace mapping on the
/// child-spawn step and nowhere else — with the final
/// `start_spawned_session` hand-off staying at the shared call site, exactly
/// as the four client roads hand their `SpawnedSession` back.
fn open_pty_session(
    state: &Arc<ServerState>,
    command: super::PtyCommand,
    workspace_id: Option<&str>,
) -> Result<SpawnedSession, WireError> {
    // On Windows portable-pty selects ConPTY internally. ConPTY may issue a
    // DSR query (`ESC[6n`) at startup and stalls its render pipeline until it
    // is answered. The DAEMON is the single responder: publish_output routes
    // the emulator's PtyWrite replies straight back to this writer. Clients
    // must not answer DSR themselves (a second reply would reach the child).
    let pty_system = portable_pty::native_pty_system();
    let pair = pty_system
        .openpty(PtySize {
            rows: super::INITIAL_ROWS,
            cols: super::INITIAL_COLS,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|error| super::pty_wire_error("Could not open the terminal.", error))?;
    let workspace_path = command.cwd.clone();
    let mut child = pair
        .slave
        .spawn_command(command.to_command_builder())
        .map_err(|error| super::workspace_spawn_error(workspace_id, &workspace_path, error))?;

    // portable-pty 0.9 exposes the native Windows process handle on Child,
    // but does not expose CREATE_SUSPENDED. Assign immediately after spawn so
    // the normal race window is only the interval between CreateProcessW and
    // these calls. Closing it completely would require adapting portable-pty's
    // ConPTY CreateProcessW seam to create suspended and resume after both
    // assignments; that is deliberately not part of this milestone.
    #[cfg(windows)]
    let (process_job, os_handle) = {
        let process_job = match JobObject::new() {
            Ok(process_job) => process_job,
            Err(error) => {
                super::terminate_spawned_child(pair, child);
                return Err(WireError::new(
                    ErrorCode::Io,
                    format!("Could not create the terminal process job: {error}"),
                ));
            }
        };
        let process_handle = match child.as_raw_handle() {
            Some(process_handle) => process_handle,
            None => {
                super::terminate_spawned_child(pair, child);
                return Err(WireError::new(
                    ErrorCode::Io,
                    "The terminal process has no native handle.",
                ));
            }
        };
        if let Err(error) = state
            .process_job
            .assign(process_handle)
            .and_then(|()| process_job.assign(process_handle))
        {
            super::terminate_spawned_child(pair, child);
            return Err(WireError::new(
                ErrorCode::Io,
                format!("Could not contain the terminal process: {error}"),
            ));
        }
        let os_handle = match ProcessHandle::duplicate(process_handle) {
            Ok(handle) => Some(handle),
            Err(error) => {
                eprintln!("could not duplicate terminal process handle for OS liveness: {error}");
                None
            }
        };
        (process_job, os_handle)
    };

    #[cfg(not(windows))]
    let process_job = JobObject::new().map_err(|error| {
        WireError::new(
            ErrorCode::Io,
            format!("Could not create the terminal process job: {error}"),
        )
    })?;
    #[cfg(not(windows))]
    let os_handle = None;

    let killer = child.clone_killer();

    let writer = match pair.master.take_writer() {
        Ok(writer) => writer,
        Err(_) => {
            let mut killer = killer;
            let _ = killer.kill();
            drop(pair.master);
            let _ = child.wait();
            return Err(WireError::new(
                ErrorCode::Io,
                "Could not attach to the terminal.",
            ));
        }
    };
    let reader = match pair.master.try_clone_reader() {
        Ok(reader) => reader,
        Err(_) => {
            let mut killer = killer;
            let _ = killer.kill();
            drop(writer);
            drop(pair.master);
            let _ = child.wait();
            return Err(WireError::new(
                ErrorCode::Io,
                "Could not read from the terminal.",
            ));
        }
    };

    Ok(SpawnedSession {
        process_job,
        master: Some(Arc::new(Mutex::new(pair.master))),
        killer: Box::new(super::PtyKiller { inner: killer }),
        switcher: None,
        child: Box::new(super::PtyWaitableChild { child }),
        writer: Arc::new(Mutex::new(writer)),
        // A terminal's writer is a PTY: nothing there can open a path, so no
        // structured prompt route.
        image_sink: None,
        static_image_sink: None,
        reader,
        reader_dispatch: None,
        stderr: None,
        permission_broker: None,
        os_handle,
        peer_session_id: None,
        agent_version: None,
        pending_delivery: None,
        pending_codex_verify: None,
    })
}

/// The provider registry: the catalog's own enumeration bound to the family
/// implementations. The native families bind by their own `id()` — this
/// module never spells a catalog row's name — and every row no native family
/// claims is an ACP provider, which is what makes the dimension open: a
/// provider the catalog learns about needs no new code to spawn.
pub(crate) struct ProviderRegistry {
    entries: Vec<(Arc<str>, Arc<dyn Provider>)>,
    /// The user rows this snapshot was built from, carried beside `entries`
    /// so one swap can never tear a row's declaration from its registry
    /// entry. The profile lookup answers from `entries` — ids that bind an
    /// implementation, i.e. rows that can spawn — never from here alone
    /// (pass 2e step 3's ordering rule).
    user_rows: BTreeMap<Arc<str>, crate::user_providers::UserProviderRow>,
}

impl ProviderRegistry {
    /// Build the registry from the catalog's provider enumeration
    /// (`KNOWN_AGENTS`, chained with `TEST_ONLY_AGENTS` under debug — the
    /// same walk `catalog_provider_id` serves the profile store with).
    pub(crate) fn catalog_default() -> Self {
        let natives: [Arc<dyn Provider>; 3] = [
            Arc::new(ClaudeProvider),
            Arc::new(CodexProvider),
            Arc::new(PiProvider),
        ];
        let entries = crate::provider_catalog::catalog_provider_rows()
            .map(|agent| {
                let provider = natives
                    .iter()
                    .find(|candidate| candidate.id() == agent.id)
                    .cloned()
                    .unwrap_or_else(|| Arc::new(AcpProvider) as Arc<dyn Provider>);
                (Arc::from(agent.id), provider)
            })
            .collect();
        Self {
            entries,
            user_rows: BTreeMap::new(),
        }
    }

    /// The whole next registry with `rows` joined to the catalog's
    /// enumeration, every row bound to the ACP implementation — the one
    /// family that needs no per-provider code, which is what makes the
    /// provider dimension open. Called by [`apply_user_rows`], which swaps
    /// only when the rows differ from the live snapshot's.
    pub(crate) fn with_user_rows(
        mut self,
        rows: BTreeMap<String, crate::user_providers::UserProviderRow>,
    ) -> Self {
        for (id, row) in rows {
            let id: Arc<str> = Arc::from(id.as_str());
            self.entries
                .push((id.clone(), Arc::new(AcpProvider) as Arc<dyn Provider>));
            self.user_rows.insert(id, row);
        }
        self
    }

    /// Does this snapshot already carry exactly these user rows? The
    /// compare behind the refresh's deep-equal early return: an unchanged
    /// document swaps nothing, so a boundary costs a read and a compare,
    /// not a swap.
    fn user_rows_equivalent(
        &self,
        rows: &BTreeMap<String, crate::user_providers::UserProviderRow>,
    ) -> bool {
        self.user_rows.len() == rows.len()
            && self
                .user_rows
                .iter()
                .zip(rows.iter())
                .all(|((live_id, live_row), (id, row))| &**live_id == id && live_row == row)
    }

    /// The user row a live snapshot carries for `id`, matched the way the
    /// catalog matches names — case-insensitively — and cloned out, so the
    /// caller holds the row however later swaps behave.
    pub(crate) fn user_row_for(&self, id: &str) -> Option<crate::user_providers::UserProviderRow> {
        self.user_rows
            .iter()
            .find(|(live_id, _)| live_id.eq_ignore_ascii_case(id))
            .map(|(_, row)| row.clone())
    }

    /// The ids this snapshot publishes — the entries, i.e. rows that bind an
    /// implementation and can spawn. The one source the profile lookup may
    /// answer from; `user_rows` alone never publishes an id (pass 2e step
    /// 3's ordering rule: a profile cannot name a provider nothing can
    /// spawn).
    pub(crate) fn published_ids(&self) -> impl Iterator<Item = &Arc<str>> {
        self.entries.iter().map(|(id, _)| id)
    }

    /// `InstalledAgent` rows for the live user declarations, so the catalogue
    /// list answers what the spawn road can spawn (`resolve_named` reads the
    /// same rows first). A declaration is launchable by fiat — its argv is
    /// explicit — so every row reads installed and ACP-available; a command
    /// that does not exist surfaces at launch like any PATH row that
    /// vanished, not here. The channel is inert `Npm` with no package: it
    /// runs no `--version` probe on refresh and offers no update, which is
    /// what a row with no installation story wants.
    pub(crate) fn user_agents(&self) -> Vec<crate::provider_catalog::InstalledAgent> {
        self.user_rows
            .iter()
            .map(|(id, row)| {
                let command = row.command.clone().unwrap_or_default();
                crate::provider_catalog::InstalledAgent {
                    id: id.to_string(),
                    aliases: &[],
                    installed: true,
                    executable: command.first().cloned().unwrap_or_default().into(),
                    prefix_args: Vec::new(),
                    acp_command: Some(command),
                    stream_json_command: None,
                    rpc_command: None,
                    app_server_command: None,
                    authentication: crate::provider_catalog::AuthenticationStatus::Unknown,
                    origin: crate::provider_catalog::ProviderOrigin::UserBinary,
                    launch_args: None,
                    pickable: None,
                    installed_version: None,
                    latest_version: None,
                    install_channel: crate::provider_catalog::InstallChannel::Npm,
                    npm_package: None,
                    tools: crate::provider_catalog::mcp_tools_for(id),
                }
            })
            .collect()
    }

    /// The provider a create's id names. An id the catalog does not publish
    /// falls through to the ACP implementation — the open dimension's rule
    /// that ACP is the one implementation needing no per-provider code; the
    /// refusal for an unresolvable id is the resolution's to raise, not the
    /// lookup's.
    pub(crate) fn provider_for(&self, provider_id: &str) -> Arc<dyn Provider> {
        self.entries
            .iter()
            .find(|(id, _)| **id == *provider_id)
            .map(|(_, provider)| Arc::clone(provider))
            .unwrap_or_else(|| Arc::new(AcpProvider))
    }

    /// The provider whose sessions are created as `kind`. The one kind ->
    /// implementation table in the daemon; every dispatch that used to match
    /// the kind itself resolves through here instead.
    pub(crate) fn provider_for_kind(&self, kind: &SessionKind) -> Arc<dyn Provider> {
        match kind {
            SessionKind::Acp => Arc::new(AcpProvider),
            SessionKind::Claude => Arc::new(ClaudeProvider),
            SessionKind::Pi => Arc::new(PiProvider),
            SessionKind::Codex => Arc::new(CodexProvider),
            SessionKind::Terminal => Arc::new(TerminalProvider),
        }
    }
}

/// The registry cell: one whole-registry snapshot behind a lock, initialised
/// on first use from the catalog's enumeration. Readers clone the snapshot
/// out; [`swap_catalog_registry`] replaces it whole.
static CATALOG_REGISTRY: OnceLock<RwLock<Arc<ProviderRegistry>>> = OnceLock::new();

fn catalog_cell() -> &'static RwLock<Arc<ProviderRegistry>> {
    CATALOG_REGISTRY.get_or_init(|| RwLock::new(Arc::new(ProviderRegistry::catalog_default())))
}

/// The registry the spawn road reads: a snapshot of the catalogue bound to
/// the family implementations, taken at the instant of the call. The
/// snapshot is an owned `Arc` and stays valid no matter what later swaps
/// do; a reader never observes a half-swapped registry, because a swap
/// replaces the whole snapshot in one step (see [`swap_catalog_registry`]).
pub(crate) fn catalog_registry() -> Arc<ProviderRegistry> {
    Arc::clone(&catalog_cell().read().unwrap())
}

/// Replace the registry snapshot wholesale, returning the snapshot it
/// replaced. The caller builds the *whole* next registry before calling:
/// this function's only work is one `Arc` store under the write lock, so
/// nothing inside the swap can fail — a build that errors, or never
/// happens, swaps nothing, and every `catalog_registry()` reader sees
/// either the previous snapshot or `next`, never a mixture (Paseo's
/// prepare → apply → commit around `mutable-provider-config-owner.ts`: the
/// registry is never left half-swapped). Nothing in production swaps yet:
/// pass 2e step 2's user rows enter through here, and until then only
/// tests call it. Dead in non-test builds until then — the attribute is the
/// machine form of the sentence above.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn swap_catalog_registry(next: ProviderRegistry) -> Arc<ProviderRegistry> {
    let next = Arc::new(next);
    let mut current = catalog_cell().write().unwrap();
    std::mem::replace(&mut *current, next)
}

/// The ids the native families bind by, from the impls' own `id()` — the
/// closed set `user_providers` validates `extends` against without spelling
/// a catalog row's name.
pub(crate) fn native_family_ids() -> Vec<String> {
    let natives: [Arc<dyn Provider>; 3] = [
        Arc::new(ClaudeProvider),
        Arc::new(CodexProvider),
        Arc::new(PiProvider),
    ];
    natives
        .iter()
        .map(|provider| provider.id().to_string())
        .collect()
}

/// Apply one validated user-rows document to the live registry: when the
/// rows differ from the live snapshot's, build the *whole* next registry
/// (builtins + rows, every row bound to `AcpProvider`) and swap it in one
/// step through [`swap_catalog_registry`]; when they do not, swap nothing —
/// an unchanged document costs a compare, not a swap, and a refused or
/// unreadable document never reaches here (the live registry is never
/// emptied by a bad read; `user_providers::refresh_user_rows` owns that
/// rule).
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn apply_user_rows(rows: BTreeMap<String, crate::user_providers::UserProviderRow>) {
    if catalog_registry().user_rows_equivalent(&rows) {
        return;
    }
    let next = ProviderRegistry::catalog_default().with_user_rows(rows);
    swap_catalog_registry(next);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// M2-e1, half one — the fallthrough is a property of the registry
    /// itself, so it is tested on a local one and touches no global state:
    /// an id no row publishes resolves to ACP, the open dimension's rule.
    #[test]
    fn a_registry_with_no_rows_falls_through_to_acp() {
        // The id comes from the impl, never spelled: the catalog owns names.
        let native_id = ClaudeProvider.id();
        let full = ProviderRegistry::catalog_default();
        assert_eq!(full.provider_for(native_id).id(), native_id);
        let empty = ProviderRegistry {
            entries: Vec::new(),
            user_rows: BTreeMap::new(),
        };
        assert_eq!(
            empty.provider_for(native_id).id(),
            AcpProvider.id(),
            "a registry with no rows resolves every id to ACP"
        );
    }

    /// M2-e1, half two — the swap seam is live: it replaces the snapshot the
    /// whole process reads and hands back the one it replaced.
    ///
    /// It swaps in an **equivalent** registry, deliberately, and never a
    /// degraded one. This test shares the process-wide cell with every other
    /// test in the binary, and the suite runs in parallel: a window in which
    /// the live catalogue resolved nothing would make any concurrent test that
    /// resolves a provider id fail at random — `provider_for` is on the resume
    /// stamp and the vocabulary reply, among others. Swapping an equal
    /// registry keeps the seam observable through snapshot *identity* while
    /// leaving every concurrent reader the same answers it would have had.
    ///
    /// It holds the rows lock for the same reason, and pass 2e-2 is what made
    /// that necessary: identity is only observable if nothing else swaps
    /// between the read and the swap. Without the lock this test and the two
    /// that put user rows live would break each other both ways — a foreign
    /// swap in the window makes `replaced` some other snapshot, and this
    /// test's builtins-only replacement would drop their rows mid-assertion.
    /// **The lock is the serialisation point for every swap of the live
    /// registry, not only for the rows file**; anything that swaps takes it.
    #[test]
    fn the_swap_seam_replaces_the_live_snapshot() {
        let native_id = ClaudeProvider.id();
        let _gate = crate::user_providers::lock_rows_state();
        let before = catalog_registry();
        let replaced = swap_catalog_registry(ProviderRegistry::catalog_default());
        let after = catalog_registry();

        assert!(
            Arc::ptr_eq(&before, &replaced),
            "the seam hands back exactly the snapshot it replaced"
        );
        assert!(
            !Arc::ptr_eq(&before, &after),
            "the live snapshot is the new one: a seam that swapped nothing fails here"
        );
        assert_eq!(
            after.provider_for(native_id).id(),
            native_id,
            "and the replacement resolves what it should"
        );
    }

    /// Pass 2e step 3, and the ordering rule the design states for it: the
    /// profile lookup answers for rows that bind an implementation — rows
    /// that can spawn — never for a row that is merely declared. A snapshot
    /// whose row sits in `user_rows` with no registry entry is the exact
    /// shape "lookup widened before the rows can spawn" would publish, and
    /// the lookup refuses it: a profile cannot name a provider nothing can
    /// spawn. (M2-e2-e's target — widening the walk to the declared rows —
    /// turns the first assert red.) Local registries only; no global state.
    #[test]
    fn a_profile_lookup_answers_for_rows_that_bind_an_implementation_only() {
        let rows = crate::user_providers::parse_providers_document(
            br#"{"declared-agent": {"extends": "acp", "command": ["/bin/declared"]}}"#,
            &native_family_ids(),
        )
        .expect("a valid row");

        let mut declared_only = ProviderRegistry::catalog_default();
        let id: Arc<str> = Arc::from("declared-agent");
        declared_only
            .user_rows
            .insert(id, rows["declared-agent"].clone());
        assert_eq!(
            crate::provider_catalog::catalog_provider_id_for(&declared_only, "declared-agent"),
            None,
            "a profile cannot name a provider nothing can spawn"
        );

        // When the row does bind an implementation, the same lookup
        // publishes it, case-insensitively like the built-ins.
        let bound = ProviderRegistry::catalog_default().with_user_rows(rows);
        assert_eq!(
            crate::provider_catalog::catalog_provider_id_for(&bound, "Declared-Agent"),
            Some("declared-agent".to_string()),
            "a live row canonicalises like any other provider id"
        );
        // And the built-ins answer first, exactly as before this pass.
        assert_eq!(
            crate::provider_catalog::catalog_provider_id_for(&bound, ClaudeProvider.id()),
            Some(ClaudeProvider.id().to_string()),
        );
    }

    /// Pass 2c: whose spawn success measures provider health is a per-family
    /// fact living in the impls, read by the create path's success arm
    /// through the registry. A completed handshake proves the provider
    /// started and accepted a session; a bare process spawn proves nothing,
    /// so Claude records failures only. Pins all five answers — without this
    /// walk the move has no test that can go red.
    #[test]
    fn spawn_measures_health_is_a_per_family_fact() {
        for (kind, measures) in [
            (SessionKind::Acp, true),
            (SessionKind::Pi, true),
            (SessionKind::Codex, true),
            (SessionKind::Claude, false),
            (SessionKind::Terminal, false),
        ] {
            assert_eq!(
                catalog_registry()
                    .provider_for_kind(&kind)
                    .spawn_measures_health(),
                measures,
                "spawn_measures_health for {kind:?}"
            );
        }
    }

    /// Stage 1: Claude joins ACP as a resumable family; stage 2 adds Codex
    /// (`thread/resume`); Pi and the terminal stay refused. Pins all five
    /// answers — the flips this test guards went red before they went green.
    #[test]
    fn resumable_is_a_per_family_fact() {
        for (kind, resumable) in [
            (SessionKind::Acp, true),
            (SessionKind::Claude, true),
            (SessionKind::Codex, true),
            (SessionKind::Pi, false),
            (SessionKind::Terminal, false),
        ] {
            assert_eq!(
                catalog_registry().provider_for_kind(&kind).resumable(),
                resumable,
                "resumable for {kind:?}"
            );
        }
    }

    /// The verdict needs all five: a dead process, a resumable family, both
    /// persisted columns non-empty, and a handle that is not the one a
    /// provider refused. Each missing piece refuses on its own, so no
    /// surface can offer a resume the gate would not honour.
    #[test]
    fn session_resumable_needs_a_dead_process_family_and_columns() {
        let live = session_resumable(
            &SessionKind::Claude,
            Some("claude"),
            Some("peer-1"),
            true,
            None,
        );
        assert!(!live, "a running process is never resumable");
        for kind in [SessionKind::Pi, SessionKind::Terminal] {
            assert!(
                !session_resumable(&kind, Some("x"), Some("peer-1"), false, None),
                "an undesigned family is never resumable ({kind:?})"
            );
        }
        for (provider, peer) in [
            (None, Some("peer-1")),
            (Some("claude"), None),
            (Some(""), Some("peer-1")),
            (Some("claude"), Some("")),
        ] {
            assert!(
                !session_resumable(&SessionKind::Claude, provider, peer, false, None),
                "missing columns are never resumable"
            );
        }
        for kind in [SessionKind::Acp, SessionKind::Claude, SessionKind::Codex] {
            assert!(
                session_resumable(&kind, Some("x"), Some("peer-1"), false, None),
                "a dead admitted session with its columns is resumable ({kind:?})"
            );
        }
        // The provider's own refusal, recorded beside the handle: the verdict
        // is a fact learned from the provider, and it waits for a different
        // handle — which clears the refusal at the announce.
        assert!(
            !session_resumable(
                &SessionKind::Acp,
                Some("x"),
                Some("peer-1"),
                false,
                Some("peer-1")
            ),
            "a refused handle does not offer the resume that was refused"
        );
        assert!(
            session_resumable(
                &SessionKind::Acp,
                Some("x"),
                Some("peer-2"),
                false,
                Some("peer-1")
            ),
            "a fresh handle is not silenced by a refusal about another one"
        );
    }
}
