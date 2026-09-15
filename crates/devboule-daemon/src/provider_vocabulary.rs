//! The provider-vocabulary query (`ProviderVocabularyGet`): what one
//! provider offers — its models and its modes — so the Agents panel can
//! author a profile from real vocabulary instead of free text.
//!
//! Spec: `reports/remote-agents/SPEC-provider-vocabulary-query.md` §4-§6.
//! Pass 1 of the brief lands the wire, the gate, the cache and the one
//! provider that costs almost nothing: Claude's models come from the catalog
//! derivation, which reads the CLI's files on disk. The one process a
//! Claude read can start is the native version probe, and only while the
//! installed version is still unknown — the same one-shot probe
//! `providers_list` starts, and once the version is settled a read costs
//! file reads only. Every other provider answers `absent` — no source could
//! answer yet — which is the wire value that makes the form completable for
//! everyone on day one; pass 2 upgrades three of them to `present` with
//! spawn probes and changes no shape.
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

/// The longest provider string this handler will echo in a refusal. Both
/// sibling refusals cap before they echo (`MAX_PROFILE_FIELD_BYTES` in
/// `agent_profiles.rs`, `MAX_POLICY_NAME_BYTES` in `tool_policy.rs`): the
/// string is caller input with nothing bounding its length, and an uncapped
/// echo builds an error frame at least as large as the request — larger,
/// when the bytes JSON-escape — which `MAX_FRAME_BYTES` then refuses,
/// dropping the connection the reply was about to travel on.
const MAX_PROVIDER_ECHO_BYTES: usize = 128;

/// One cached vocabulary answer.
///
/// The key facts are the discovery facts that invalidate the entry: a
/// discovery pass that changes the executable or the installed version
/// produces different facts, and the entry simply stops matching — which is
/// invalidation (2) of the spec's three, invalidation (1) being `refresh:
/// true` handled at the call site and (3) the TTL below. The fourth
/// invalidation is Claude's catalog derivation completing: it changes the
/// answer without changing any fact here, so it cannot be a key — the
/// completion callback evicts the entry outright
/// ([`VocabularyCache::invalidate`]).
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

    /// Drop one provider's cached answer outright. This is the fourth
    /// invalidation: the catalog derivation completing changes Claude's
    /// answer without changing the executable or the version, so it cannot
    /// be expressed as a key fact. The completion callback evicts instead,
    /// and the next read re-probes into the derived catalog — a human is
    /// never served the provisional fallback, labelled `cache`, after the
    /// provider's own list is already available.
    pub(crate) fn invalidate(&self, provider: &str) {
        let mut entries = self
            .entries
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        entries.remove(provider);
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
    // Canonicalised the way the profile store canonicalises one: trimmed
    // first, then resolved by the catalog's own walk — and never echoed
    // past the cap, because the refusal sentence would otherwise repeat an
    // unbounded caller string into a frame the app renders.
    let provider = provider.trim();
    if provider.len() > MAX_PROVIDER_ECHO_BYTES {
        return DaemonMessage::Error(
            WireError::new(
                ErrorCode::InvalidRequest,
                format!(
                    "the provider is {} bytes, over the {MAX_PROVIDER_ECHO_BYTES}-byte cap",
                    provider.len()
                ),
            )
            .with_id(id),
        );
    }
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
///
/// `pub(crate)` for the provider trait's `vocabulary` delegation
/// (`provider.rs`): the impl resolves its family and asks this selector with
/// its own id, keeping this function the one probe home until the selector
/// itself is absorbed.
pub(crate) fn probe_axes(
    state: &Arc<ServerState>,
    canonical: &str,
) -> (VocabularyModels, VocabularyModes) {
    #[cfg(test)]
    state
        .provider_vocabulary
        .probes
        .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    match canonical {
        // Claude costs (almost) no process: the catalog derivation reads the
        // CLI's files on disk. The one process a read can start is the
        // native version probe, inside `claude_models`, and only while the
        // installed version is still unknown. Both axes are `present`.
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
    let models = claude_models_axis(state.claude_models());
    // The current mode is a live-session fact and belongs to the manifest;
    // the vocabulary carries only the available list, so the "current" id
    // the shared builder requires is filled and dropped.
    let modes = VocabularyModes::new(
        VocabularyState::Present,
        Some(VocabularyOrigin::Daemon),
        crate::claude_view::mode_state("default").available_modes,
    )
    .expect("a present modes axis always carries its origin");
    (models, modes)
}

/// The models axis a catalog snapshot maps to — the whole of the origin
/// honesty on the only real vocabulary in this pass, as a pure function so
/// a test drives the mapping itself rather than two literals: `Derived` is
/// the provider's own answer, `Provisional` the daemon's fallback table,
/// and the items pass through unchanged. `claude_axes` feeds this from
/// `ServerState::claude_models`.
fn claude_models_axis(snapshot: crate::claude_catalog::ClaudeCatalogSnapshot) -> VocabularyModels {
    let origin = match snapshot.state {
        crate::claude_catalog::ClaudeCatalogState::Derived => VocabularyOrigin::Provider,
        crate::claude_catalog::ClaudeCatalogState::Provisional => VocabularyOrigin::Daemon,
    };
    VocabularyModels::new(VocabularyState::Present, Some(origin), snapshot.models)
        .expect("a present models axis always carries its origin")
}

/// The `absent` answer: items empty, origin omitted — both, in both
/// directions, exactly as the biconditional requires.
fn absent_axes() -> (VocabularyModels, VocabularyModes) {
    (
        VocabularyModels::new(VocabularyState::Absent, None, Vec::new())
            .expect("an absent axis carries no origin"),
        VocabularyModes::new(VocabularyState::Absent, None, Vec::new())
            .expect("an absent axis carries no origin"),
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

    /// Models origin follows the catalog state, driven through the real
    /// mapping `claude_axes` uses: an extraction that worked is the
    /// provider's own answer; the fallback table is ours. Both catalog
    /// states are fed through [`claude_models_axis`] and the whole axis is
    /// walked — state, origin, and the items carried through — because a
    /// test of two literals cannot see the mapping invert and label the
    /// daemon's aliases as the provider's answer mid-consent.
    #[test]
    fn claude_models_origin_follows_the_catalog_state() {
        let derived = claude_models_axis(crate::claude_catalog::ClaudeCatalogSnapshot::derived(
            crate::claude_catalog::fallback_models(),
        ));
        assert_eq!(derived.state, VocabularyState::Present);
        assert_eq!(
            derived.origin,
            Some(VocabularyOrigin::Provider),
            "a derived catalog is the provider's own answer"
        );
        assert_eq!(
            derived
                .items
                .iter()
                .map(|model| model.model_id.as_str())
                .collect::<Vec<_>>(),
            ["opus", "sonnet", "haiku"],
            "the mapping carries the snapshot's items through unchanged"
        );

        let provisional =
            claude_models_axis(crate::claude_catalog::ClaudeCatalogSnapshot::provisional(
                crate::claude_catalog::fallback_models(),
            ));
        assert_eq!(provisional.state, VocabularyState::Present);
        assert_eq!(
            provisional.origin,
            Some(VocabularyOrigin::Daemon),
            "the fallback table is the daemon's answer, never labelled the provider's"
        );
        assert_eq!(provisional.items, derived.items);
        assert_ne!(
            provisional.origin, derived.origin,
            "Derived and Provisional must not share an origin"
        );

        // The same snapshot shapes `claude_axes` serves, through the real
        // builder: every axis it produces satisfies the constructor's
        // biconditional.
        let state = state();
        let (models, modes) = claude_axes(&state);
        assert!(VocabularyModels::new(models.state, models.origin, models.items).is_ok());
        assert!(VocabularyModes::new(modes.state, modes.origin, modes.items).is_ok());
        let runtime_dir = state.sessions.runtime_dir().to_path_buf();
        drop(state);
        let _ = std::fs::remove_dir_all(runtime_dir);
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

    /// The fourth invalidation: the catalog derivation completing evicts the
    /// cached Claude answer. The derivation changes the answer without
    /// changing any discovery fact — the executable and the version are
    /// byte-identical before and after — so the facts cannot catch it; an
    /// entry filled from the provisional (fallback-alias) answer while the
    /// derivation ran must not survive the completion callback, or a consent
    /// card is shown the daemon's own aliases labelled `cache` for a full
    /// TTL after the provider's own list is already available.
    #[test]
    fn a_completed_catalog_derivation_evicts_the_cached_claude_answer() {
        let state = state();
        let facts = (
            Some("C:\\fake\\claude.exe".to_string()),
            Some("2.1.0".to_string()),
        );
        let now = crate::server::unix_millis();
        state.provider_vocabulary.store(
            "claude",
            facts.clone(),
            now,
            VocabularyModels {
                state: VocabularyState::Present,
                origin: Some(VocabularyOrigin::Daemon),
                items: crate::claude_catalog::fallback_models(),
            },
            VocabularyModes {
                state: VocabularyState::Present,
                origin: Some(VocabularyOrigin::Daemon),
                items: crate::claude_view::mode_state("default").available_modes,
            },
        );
        // The provisional answer is live: the facts a real cold read stored
        // match the next read.
        assert!(
            state
                .provider_vocabulary
                .get("claude", &facts, now)
                .is_some(),
            "the entry must be served before the derivation completes"
        );
        // The derivation delivers. This is the exact callback
        // `start_claude_derivation` installs on the worker thread.
        state.claude_catalog_derived(crate::claude_catalog::fallback_models());
        assert!(
            state
                .provider_vocabulary
                .get("claude", &facts, now)
                .is_none(),
            "the derived answer must supersede the cached provisional one"
        );

        let runtime_dir = state.sessions.runtime_dir().to_path_buf();
        drop(state);
        let _ = std::fs::remove_dir_all(runtime_dir);
    }

    /// The refusal never echoes an uncapped caller string. A provider field
    /// just under `MAX_FRAME_BYTES` of `"` characters is a legal frame, but
    /// JSON escaping makes an echoed reply *larger* than the request, so an
    /// uncapped refusal builds an error frame the framing layer refuses —
    /// dropping the connection. Both sibling refusals cap before they echo;
    /// this one does too.
    #[test]
    fn an_over_cap_provider_is_refused_without_echoing_it_whole() {
        let state = state();
        let flood = "\"".repeat(700_000);
        let reply = provider_vocabulary_reply(&state, 90, &flood, false);
        let DaemonMessage::Error(error) = &reply else {
            panic!("an over-cap provider must be refused, got {reply:?}");
        };
        assert_eq!(error.code, ErrorCode::InvalidRequest);
        assert_eq!(error.id, Some(90));
        assert_eq!(
            error.message, "the provider is 700000 bytes, over the 128-byte cap",
            "the cap refusal names the size, not the string"
        );
        let frame = serde_json::to_vec(&reply).expect("the refusal serialises");
        assert!(
            frame.len() <= devboule_protocol::MAX_FRAME_BYTES,
            "the refusal frame must stay under MAX_FRAME_BYTES, got {} bytes",
            frame.len()
        );

        let runtime_dir = state.sessions.runtime_dir().to_path_buf();
        drop(state);
        let _ = std::fs::remove_dir_all(runtime_dir);
    }

    /// `provider` is canonicalised the way the profile store canonicalises
    /// one: trimmed first. The document the app reads promises this, and a
    /// padded but known id is a provider the catalog publishes, not a
    /// refusal.
    #[test]
    fn a_padded_known_provider_is_trimmed_before_the_catalog_walk() {
        let state = state();
        let reply = provider_vocabulary_reply(&state, 91, "  claude  ", false);
        let DaemonMessage::ProviderVocabulary { provider, .. } = &reply else {
            panic!("a padded known provider must be served, got {reply:?}");
        };
        assert_eq!(provider, "claude", "the reply carries the canonical id");

        let runtime_dir = state.sessions.runtime_dir().to_path_buf();
        drop(state);
        let _ = std::fs::remove_dir_all(runtime_dir);
    }
}
