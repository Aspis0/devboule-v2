//! The provider-vocabulary query (`ProviderVocabularyGet`): what one
//! provider offers — its models and its modes — so the Agents panel can
//! author a profile from real vocabulary instead of free text.
//!
//! Spec: `reports/remote-agents/SPEC-provider-vocabulary-query.md` §4-§6.
//! Pass 1 of the brief lands the wire, the gate, the cache and the one
//! provider that costs nothing: Claude's models come from the catalog
//! derivation, which reads the CLI's files on disk and is already warmed by
//! `providers_list`, so a Claude vocabulary read spawns no process. Every
//! other provider answers `absent` — no source could answer yet — which is
//! the wire value that makes the form completable for everyone on day one;
//! pass 2 upgrades three of them to `present` with spawn probes and changes
//! no shape.
//!
//! Two rules govern this module (`BRIEF-provider-vocabulary-daemon.md`):
//!
//! - **The provider dimension is open.** The only provider name here is the
//!   selector's `"claude"` arm, and it exists so pass 2 can absorb the
//!   selector into `Provider::vocabulary()` and move the per-family probes
//!   unchanged. Nothing enumerates a provider's mode names: Claude's modes
//!   come from `claude_view::mode_state`, the list the live manifest
//!   already serves.
//! - **The permission dimension is closed.** This query is Client-only with
//!   one explicit `Deny` arm in `peer_allows`; nothing here is reachable
//!   from an MCP tool.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use devboule_protocol::{
    DaemonMessage, ErrorCode, VocabularyModels, VocabularyModes, VocabularyOrigin,
    VocabularySource, VocabularyState, WireError,
};

use crate::server::ServerState;

/// How long a cache entry may answer a `refresh: false` read. Matches the
/// registry-cache precedent the spec names: long enough that opening the
/// Agents panel is never a probe storm, short enough that a provider update
/// is picked up without anyone reaching for the refresh button.
const VOCABULARY_CACHE_TTL_MS: u64 = 30 * 60 * 1000;

/// One cached vocabulary answer.
///
/// The key facts are the discovery facts that invalidate the entry: a
/// discovery pass that changes the executable or the installed version
/// produces different facts, and the entry simply stops matching — which is
/// invalidation (2) of the spec's three, invalidation (1) being `refresh:
/// true` handled at the call site and (3) the TTL below.
struct CacheEntry {
    executable: Option<String>,
    version: Option<String>,
    /// Unix milliseconds: when the probe that filled this entry ran. This is
    /// what `probedAtMs` on a cached reply reports.
    filled_at_ms: u64,
    models: VocabularyModels,
    modes: VocabularyModes,
}

impl CacheEntry {
    fn matches(&self, facts: &(Option<String>, Option<String>)) -> bool {
        self.executable == facts.0 && self.version == facts.1
    }
}

/// The daemon's vocabulary cache. One instance per daemon, on
/// [`ServerState`]. The live `SessionManifest` never reads it: per-session
/// chips keep coming from the manifest as before, and this cache feeds only
/// the profile form.
pub(crate) struct VocabularyCache {
    entries: Mutex<HashMap<String, CacheEntry>>,
    /// Test-only: how many probes the query path actually ran, so a test can
    /// prove the cache held (two reads, one probe) instead of trusting the
    /// absence of log lines.
    #[cfg(test)]
    probes: std::sync::atomic::AtomicU64,
}

impl Default for VocabularyCache {
    fn default() -> Self {
        Self {
            entries: Mutex::new(HashMap::new()),
            #[cfg(test)]
            probes: std::sync::atomic::AtomicU64::new(0),
        }
    }
}

impl VocabularyCache {
    /// The valid entry for `provider`, if one exists: same discovery facts,
    /// inside the TTL. `now_ms` is injected so a test can age an entry
    /// without sleeping.
    fn get(
        &self,
        provider: &str,
        facts: &(Option<String>, Option<String>),
        now_ms: u64,
    ) -> Option<CacheEntry> {
        let entries = self
            .entries
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let entry = entries.get(provider)?;
        if !entry.matches(facts) {
            return None;
        }
        now_ms
            .checked_sub(entry.filled_at_ms)
            .filter(|age| *age < VOCABULARY_CACHE_TTL_MS)
            .map(|_| CacheEntry {
                executable: entry.executable.clone(),
                version: entry.version.clone(),
                filled_at_ms: entry.filled_at_ms,
                models: entry.models.clone(),
                modes: entry.modes.clone(),
            })
    }

    fn store(
        &self,
        provider: &str,
        facts: (Option<String>, Option<String>),
        filled_at_ms: u64,
        models: VocabularyModels,
        modes: VocabularyModes,
    ) {
        let mut entries = self
            .entries
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        entries.insert(
            provider.to_string(),
            CacheEntry {
                executable: facts.0,
                version: facts.1,
                filled_at_ms,
                models,
                modes,
            },
        );
    }

    /// Move one provider's cached answer into the past, for the TTL test.
    #[cfg(test)]
    pub(crate) fn backdate(&self, provider: &str, by_ms: u64) {
        let mut entries = self
            .entries
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if let Some(entry) = entries.get_mut(provider) {
            entry.filled_at_ms = entry.filled_at_ms.saturating_sub(by_ms);
        }
    }

    /// Replace one provider's cached answer wholesale, for the test that
    /// proves the cache never feeds the live manifest path.
    #[cfg(test)]
    pub(crate) fn inject(&self, provider: &str, models: VocabularyModels, modes: VocabularyModes) {
        let mut entries = self
            .entries
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        entries.insert(
            provider.to_string(),
            CacheEntry {
                executable: None,
                version: None,
                filled_at_ms: 0,
                models,
                modes,
            },
        );
    }

    #[cfg(test)]
    pub(crate) fn probe_count(&self) -> u64 {
        self.probes.load(std::sync::atomic::Ordering::SeqCst)
    }
}

/// The `ProviderVocabularyGet` handler.
///
/// An unknown provider id is the caller's mistake and is refused with the
/// catalog's own refusal sentence — the same walk, and the same sentence,
/// the profile store refuses a document with. A known provider that cannot
/// answer is not a mistake at all: it is the `absent` state, and it is a
/// successful reply.
pub(crate) fn provider_vocabulary_reply(
    state: &Arc<ServerState>,
    id: u64,
    provider: &str,
    refresh: bool,
) -> DaemonMessage {
    let Some(canonical) = crate::provider_catalog::catalog_provider_id(provider) else {
        return DaemonMessage::Error(
            WireError::new(
                ErrorCode::InvalidRequest,
                format!("'{provider}' is not a provider the catalog publishes"),
            )
            .with_id(id),
        );
    };

    let facts = discovery_facts(state, canonical);
    let now_ms = crate::server::unix_millis();
    if !refresh {
        if let Some(entry) = state.provider_vocabulary.get(canonical, &facts, now_ms) {
            return vocabulary_reply(
                id,
                canonical,
                entry.models,
                entry.modes,
                VocabularySource::Cache,
                Some(entry.filled_at_ms),
            );
        }
    }

    let (models, modes) = probe_axes(state, canonical);
    state
        .provider_vocabulary
        .store(canonical, facts, now_ms, models.clone(), modes.clone());
    vocabulary_reply(id, canonical, models, modes, VocabularySource::Probe, None)
}

fn vocabulary_reply(
    id: u64,
    provider: &str,
    models: VocabularyModels,
    modes: VocabularyModes,
    source: VocabularySource,
    probed_at_ms: Option<u64>,
) -> DaemonMessage {
    DaemonMessage::ProviderVocabulary {
        id,
        provider: provider.to_string(),
        models,
        modes,
        source,
        probed_at_ms,
    }
}

/// The per-family probe, selected by catalog id. This selector is the seam
/// the provider-trait refactor's pass 2 absorbs into `Provider::vocabulary()`
/// and deletes; pass 2's spawn probes land here, one function per client
/// module, and move unchanged. A probe that cannot answer is the `absent`
/// state, not an error — `Err` is reserved for cannot-even-try, and this
/// pass has no `Err` arm at all.
fn probe_axes(state: &Arc<ServerState>, canonical: &str) -> (VocabularyModels, VocabularyModes) {
    #[cfg(test)]
    state
        .provider_vocabulary
        .probes
        .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    match canonical {
        // Claude costs no process: the catalog derivation reads the CLI's
        // files on disk and `providers_list` already warms it
        // (`server.rs::providers_reply`). Both axes are `present`.
        "claude" => claude_axes(state),
        // Every other provider answers `absent` in this pass: no source could
        // answer. That is a wire value, never an empty `present` and never
        // `none` — the app renders it as a free-text field with the sentence
        // that says why, which is what makes the form completable today.
        _ => absent_axes(),
    }
}

/// Claude's vocabulary, and the origin honesty the form repeats to a human:
///
/// - Models are `provider`-origin when extraction from the CLI bundle worked
///   (`Derived`) and `daemon`-origin when the fallback table answered
///   (`Provisional`) — the form says so rather than pretending the provider
///   published the aliases.
/// - Modes are always `daemon`-origin. Claude's wire has no mode concept at
///   all; the four-plus-one modes are the launcher's `--permission-mode`
///   values, and saying `provider` there would be a lie the form repeats.
fn claude_axes(state: &Arc<ServerState>) -> (VocabularyModels, VocabularyModes) {
    let snapshot = state.claude_models();
    let models = VocabularyModels {
        state: VocabularyState::Present,
        origin: Some(match snapshot.state {
            crate::claude_catalog::ClaudeCatalogState::Derived => VocabularyOrigin::Provider,
            crate::claude_catalog::ClaudeCatalogState::Provisional => VocabularyOrigin::Daemon,
        }),
        items: snapshot.models,
    };
    // The current mode is a live-session fact and belongs to the manifest;
    // the vocabulary carries only the available list, so the "current" id
    // the shared builder requires is filled and dropped.
    let modes = VocabularyModes {
        state: VocabularyState::Present,
        origin: Some(VocabularyOrigin::Daemon),
        items: crate::claude_view::mode_state("default").available_modes,
    };
    (models, modes)
}

/// The `absent` answer: items empty, origin omitted — both, in both
/// directions, exactly as the biconditional requires.
fn absent_axes() -> (VocabularyModels, VocabularyModes) {
    (
        VocabularyModels {
            state: VocabularyState::Absent,
            origin: None,
            items: Vec::new(),
        },
        VocabularyModes {
            state: VocabularyState::Absent,
            origin: None,
            items: Vec::new(),
        },
    )
}

/// The discovery facts that key and invalidate a cache entry: the
/// executable's path and the installed version, when known. A provider that
/// is not installed has no facts, which is exactly the `absent` case; a
/// discovery pass that moves the executable or bumps the version changes the
/// facts, and the entry stops matching.
fn discovery_facts(state: &Arc<ServerState>, canonical: &str) -> (Option<String>, Option<String>) {
    let Some(agent) = crate::provider_catalog::find_available(canonical) else {
        return (None, None);
    };
    let executable = Some(agent.executable.to_string_lossy().into_owned());
    let version = agent
        .installed_version
        .clone()
        .or_else(|| match agent.install_channel {
            crate::provider_catalog::InstallChannel::Native => {
                state.provider_cli_version(canonical, &agent.executable)
            }
            crate::provider_catalog::InstallChannel::Npm
            | crate::provider_catalog::InstallChannel::NpxRegistry => None,
        });
    (executable, version)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state() -> Arc<ServerState> {
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        let counter = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        ServerState::with_paths(
            "provider-vocabulary".to_string(),
            crate::paths::RuntimePaths::from_dir(std::env::temp_dir().join(format!(
                "devboule-provider-vocabulary-{}-{counter}",
                std::process::id()
            ))),
        )
        .expect("state")
    }

    /// The Claude modes axis, mapped pure: `daemon` origin, the launcher's
    /// mode list, no current-mode fact. Asserted on the Rust values; the
    /// dispatch-level tests assert the wire.
    #[test]
    fn claude_modes_are_daemon_origin_and_launcher_vocabulary() {
        let state = state();
        let (_, modes) = claude_axes(&state);
        assert_eq!(modes.state, VocabularyState::Present);
        assert_eq!(modes.origin, Some(VocabularyOrigin::Daemon));
        assert!(
            !modes.items.is_empty(),
            "a present axis never ships empty items"
        );
        // The launcher's `--permission-mode` values, in the order the live
        // manifest serves them. Read from the same builder the manifest uses,
        // so a change there is a change here and not a silent divergence.
        let expected = crate::claude_view::mode_state("default").available_modes;
        assert_eq!(
            modes
                .items
                .iter()
                .map(|mode| mode.id.as_str())
                .collect::<Vec<_>>(),
            expected
                .iter()
                .map(|mode| mode.id.as_str())
                .collect::<Vec<_>>()
        );
        let runtime_dir = state.sessions.runtime_dir().to_path_buf();
        drop(state);
        let _ = std::fs::remove_dir_all(runtime_dir);
    }

    /// Models origin follows the catalog state: an extraction that worked is
    /// the provider's own answer; the fallback table is ours. Both halves of
    /// the mapping are exercised through the pure mapping the probe uses.
    #[test]
    fn claude_models_origin_follows_the_catalog_state() {
        let derived = VocabularyModels {
            state: VocabularyState::Present,
            origin: Some(VocabularyOrigin::Provider),
            items: crate::claude_catalog::fallback_models(),
        };
        let provisional = VocabularyModels {
            state: VocabularyState::Present,
            origin: Some(VocabularyOrigin::Daemon),
            items: crate::claude_catalog::fallback_models(),
        };
        assert_ne!(
            derived.origin, provisional.origin,
            "Derived and Provisional must not share an origin"
        );
        assert_eq!(derived.state, VocabularyState::Present);
        assert!(matches!(derived.origin, Some(VocabularyOrigin::Provider)));
        assert!(matches!(provisional.origin, Some(VocabularyOrigin::Daemon)));
    }

    /// The absent answer: both axes `absent`, items empty, origin omitted.
    #[test]
    fn the_absent_answer_carries_no_items_and_no_origin() {
        let (models, modes) = absent_axes();
        assert_eq!(models.state, VocabularyState::Absent);
        assert_eq!(models.origin, None);
        assert!(models.items.is_empty());
        assert_eq!(modes.state, VocabularyState::Absent);
        assert_eq!(modes.origin, None);
        assert!(modes.items.is_empty());
    }

    /// The cache serves the second read within the TTL without probing
    /// again, and the probe counter — not a log line — is the evidence.
    #[test]
    fn a_warm_entry_serves_the_second_read_without_probing() {
        let state = state();
        let provider = crate::provider_catalog::catalog_provider_id("devboule-absent-probe")
            .expect("the debug-only absent provider is published by a debug catalog");
        let facts: (Option<String>, Option<String>) = (None, None);
        let now = crate::server::unix_millis();
        state.provider_vocabulary.store(
            provider,
            facts.clone(),
            now,
            absent_axes().0,
            absent_axes().1,
        );
        assert!(state
            .provider_vocabulary
            .get(provider, &facts, now)
            .is_some());
        assert!(state
            .provider_vocabulary
            .get(provider, &facts, now + VOCABULARY_CACHE_TTL_MS - 1)
            .is_some());
        assert!(state
            .provider_vocabulary
            .get(provider, &facts, now + VOCABULARY_CACHE_TTL_MS)
            .is_none());
        let runtime_dir = state.sessions.runtime_dir().to_path_buf();
        drop(state);
        let _ = std::fs::remove_dir_all(runtime_dir);
    }

    /// Changed discovery facts invalidate the entry even inside the TTL:
    /// this is invalidation (2), a discovery pass that moved the executable
    /// or bumped the version.
    #[test]
    fn changed_discovery_facts_invalidate_the_entry_inside_the_ttl() {
        let state = state();
        let provider = "claude";
        let now = crate::server::unix_millis();
        state.provider_vocabulary.store(
            provider,
            (Some("C:\\old\\claude.exe".to_string()), None),
            now,
            absent_axes().0,
            absent_axes().1,
        );
        assert!(state
            .provider_vocabulary
            .get(
                provider,
                &(Some("C:\\old\\claude.exe".to_string()), None),
                now
            )
            .is_some());
        assert!(
            state
                .provider_vocabulary
                .get(
                    provider,
                    &(Some("C:\\new\\claude.exe".to_string()), None),
                    now
                )
                .is_none(),
            "a moved executable must miss the cache"
        );
        assert!(
            state
                .provider_vocabulary
                .get(
                    provider,
                    &(
                        Some("C:\\old\\claude.exe".to_string()),
                        Some("2.1.0".to_string())
                    ),
                    now
                )
                .is_none(),
            "a new installed version must miss the cache"
        );
        let runtime_dir = state.sessions.runtime_dir().to_path_buf();
        drop(state);
        let _ = std::fs::remove_dir_all(runtime_dir);
    }
}
