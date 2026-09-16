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

use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock};

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
/// `allow(dead_code)` until pass 2b converts `unattended_mode`'s call site —
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
    /// Delegated to the broker's single-source predicate until pass 2c
    /// re-homes it.
    fn hosts_mcp(&self) -> bool {
        crate::mcp_broker::hosts_mcp(&self.wire_kind())
    }

    /// Build this family's MCP carrier from the broker's launch config. Pi
    /// and Codex own real carriers (bridge file / per-session home); the ACP
    /// and Claude shapes ride the handshake and the launch argv inside their
    /// clients and have no carrier to mint here (pass 2c moves the shapes).
    fn mcp_launch(
        &self,
        config: &McpLaunchConfig,
        runtime_dir: &Path,
    ) -> Result<McpProviderConfig, WireError>;

    /// Whether this family's sessions support resume. Today's road refuses
    /// resume for everything but ACP; the refusal site converts in pass 2c.
    fn resumable(&self) -> bool;

    /// Whether a successful spawn of this family measures provider health.
    /// Today's raw match (ACP/Pi/Codex yes, Claude failures only) stays in
    /// `session.rs` until pass 2b/2c; this is the fact it will consult.
    fn spawn_measures_health(&self) -> bool;

    /// The vocabulary axes this family can answer a vocabulary query with.
    /// Delegated to the selector the vocabulary pass landed (one `"claude"`
    /// arm, everything else absent) — the seam its own comment commissions
    /// this trait to absorb; the cache/request layer stays on `ServerState`.
    fn vocabulary(&self, state: &Arc<ServerState>) -> (VocabularyModels, VocabularyModes) {
        crate::provider_vocabulary::probe_axes(state, self.id())
    }

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
    /// Delegated to `peer_policy::unattended_mode` — the route-A guard, the
    /// per-family dictionaries and the terminal `No` exactly as landed — so
    /// the re-homing in pass 2b moves one caller, not the rule.
    fn unattended_mode(&self, delivered_mode: Option<&str>) -> UnattendedAnswer {
        crate::peer_policy::unattended_mode(self.wire_kind(), delivered_mode).into()
    }
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
        crate::mcp_broker::hosts_mcp(&self.wire_kind())
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
        // Resume is ACP-only today (`resume_handle` refuses the rest); the
        // refusal site converts in pass 2c.
        true
    }

    fn spawn_measures_health(&self) -> bool {
        true
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
        false
    }

    fn spawn_measures_health(&self) -> bool {
        // Claude records spawn failures only (the raw match this fact
        // belongs to still sits in `session.rs` until 2b/2c).
        false
    }

    fn validate_delivery(
        &self,
        state: &Arc<ServerState>,
        spec: &ProfileDelivery,
    ) -> Result<(), WireError> {
        super::claude_client::validate_delivery(&state.claude_models(), spec)
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

    fn mcp_launch(
        &self,
        config: &McpLaunchConfig,
        runtime_dir: &Path,
    ) -> Result<McpProviderConfig, WireError> {
        super::pi_client::mcp_launch(config, runtime_dir)
    }

    fn resumable(&self) -> bool {
        // Deliberate: pi resume is undesigned (`resume_handle`'s refusal
        // comment); the conversion of that site is pass 2c's.
        false
    }

    fn spawn_measures_health(&self) -> bool {
        true
    }

    fn validate_delivery(
        &self,
        _state: &Arc<ServerState>,
        spec: &ProfileDelivery,
    ) -> Result<(), WireError> {
        super::pi_client::validate_delivery(spec)
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

    fn mcp_launch(
        &self,
        config: &McpLaunchConfig,
        runtime_dir: &Path,
    ) -> Result<McpProviderConfig, WireError> {
        super::codex_client::mcp_launch(config, runtime_dir)
    }

    fn resumable(&self) -> bool {
        // "app-server sessions do not support resume" — the refusal lives at
        // the resume site until pass 2c.
        false
    }

    fn spawn_measures_health(&self) -> bool {
        true
    }

    fn validate_delivery(
        &self,
        _state: &Arc<ServerState>,
        spec: &ProfileDelivery,
    ) -> Result<(), WireError> {
        super::codex_client::validate_delivery(spec)
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

    fn spawn_measures_health(&self) -> bool {
        false
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
    entries: Vec<(&'static str, Arc<dyn Provider>)>,
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
                (agent.id, provider)
            })
            .collect();
        Self { entries }
    }

    /// The provider a create's id names. An id the catalog does not publish
    /// falls through to the ACP implementation — the open dimension's rule
    /// that ACP is the one implementation needing no per-provider code; the
    /// refusal for an unresolvable id is the resolution's to raise, not the
    /// lookup's.
    pub(crate) fn provider_for(&self, provider_id: &str) -> Arc<dyn Provider> {
        self.entries
            .iter()
            .find(|(id, _)| *id == provider_id)
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

/// The registry the spawn road reads: built once from the catalog's
/// enumeration. The catalog is compile-time closed until pass 2 step 6, so
/// the registry is immutable for the process's life.
pub(crate) fn catalog_registry() -> &'static ProviderRegistry {
    static CATALOG_REGISTRY: OnceLock<ProviderRegistry> = OnceLock::new();
    CATALOG_REGISTRY.get_or_init(ProviderRegistry::catalog_default)
}
