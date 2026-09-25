//! The ACP feature read: one provider's declared list, learned by starting the
//! provider once, and the cache that makes "once" mean something.
//!
//! Split from [`crate::provider_features`] because the two answer different
//! questions: that module asks *what does a family offer*, and is a table; this
//! one asks *how does an ACP provider's own answer reach the form without
//! starting a process per keystroke*, and is a cache and a thread. The rule that
//! keeps the read honest is stated here in full, and the list it produces is fed
//! to the other module's `acp_axis`, which is what decides the rows' authorship.

use devboule_protocol::{VocabularyFeature, VocabularyFeatures};

use crate::provider_features;

/// The cached ACP declarations for one provider id, if the read has answered
/// it. `Some(vec![])` is a real answer — the agent declared nothing — while
/// `None` is "nobody has asked yet", and the difference decides whether the
/// store may prune at all ([`crate::provider_features::prune_for`]).
pub(crate) fn cached_declarations(provider: &str) -> Option<Vec<VocabularyFeature>> {
    // The store has no handle on `ServerState`, and reaching for one to ask
    // this question would put a process-spawning read on the profile write
    // path. So the store reads what a previous vocabulary query already left
    // behind, which is the only answer that can exist without a spawn, and an
    // unread provider is pruned at spawn instead.
    ACP_PROBE_ANSWERS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .get(provider)
        .cloned()
}

/// The last answer each ACP read produced, kept beside the cache that serves
/// the form so the profile store can prune with it. Written by [`acp_axis_for`]
/// when a read finishes.
static ACP_PROBE_ANSWERS: std::sync::LazyLock<
    std::sync::Mutex<std::collections::HashMap<String, Vec<VocabularyFeature>>>,
> = std::sync::LazyLock::new(|| std::sync::Mutex::new(std::collections::HashMap::new()));

/// The feature axis of one named ACP provider: the cached answer if there is
/// one, otherwise "being read now" and a started read.
///
/// This is where `D1`'s cache lives, and the three rules that make a
/// process-spawning read safe to hang off a Settings panel are its:
///
/// - **It never blocks the ask.** The vocabulary reply is a synchronous
///   round-trip on the connection's own thread, so the first read claims the
///   slot, answers `probing`, and starts the process on a worker. A form that
///   waited on an `npx` cold start would be a frozen Settings panel.
/// - **One read per provider, not one per keystroke.** The cache is keyed on
///   the provider id alone, because the read opens `session/new` with no
///   model: no model choice can change what the agent answers. Paseo re-asks
///   per draft model and re-spawns per draft model; the model-dependent half of
///   the answer here is the static gate, which is a table and costs nothing.
/// - **A failed read is not an answer.** A provider that is not installed, or
///   died, or refused the handshake leaves no cached failure: the next ask
///   claims the slot again, so installing it is picked up without anyone
///   reaching for a refresh button.
pub(crate) fn acp_axis_for(
    state: &std::sync::Arc<crate::server::ServerState>,
    provider: &str,
) -> VocabularyFeatures {
    let cache = &state.acp_features;
    if let Some(probe) = cache.peek(provider) {
        return acp_axis_of(probe);
    }
    if !cache.claim(provider) {
        // Lost the claim to a concurrent ask. Whatever it has left is the
        // answer, and a second provider process is never started for one
        // question.
        return acp_axis_of(cache.peek(provider).unwrap_or(Probe::Running));
    }
    let worker_state = std::sync::Arc::clone(state);
    let worker_cache = std::sync::Arc::clone(cache);
    let worker_provider = provider.to_string();
    let started = std::thread::Builder::new()
        .name("acp-feature-read".to_string())
        .spawn(move || {
            let answer = match crate::session::probe_declarations(&worker_state, &worker_provider) {
                Ok(declared) => {
                    // Recorded for the profile store as well as the form: the
                    // store prunes an ACP profile's keys against this list and
                    // must not spawn a process to find it.
                    if let Ok(mut answers) = ACP_PROBE_ANSWERS.lock() {
                        answers.insert(worker_provider.clone(), declared.clone());
                    }
                    Probe::Answered(declared)
                }
                Err(error) => {
                    // One line naming the provider and what stopped the read:
                    // the reply the form sees is "nothing was learned" in every
                    // case, and only a support session can tell "not installed"
                    // from "silent" from "wrong protocol version".
                    eprintln!(
                        "acp feature read: {worker_provider} declared no features ({})",
                        error.message
                    );
                    Probe::Unavailable
                }
            };
            worker_cache.finish(&worker_provider, answer);
        });
    if started.is_err() {
        // No thread to ask with. Release the claim so the next ask retries, and
        // say what a failed read says: nothing was learned.
        cache.invalidate(provider);
        return provider_features::unavailable_axis();
    }
    provider_features::probing_axis()
}

fn acp_axis_of(probe: Probe) -> VocabularyFeatures {
    match probe {
        Probe::Answered(declared) => provider_features::acp_axis(declared),
        Probe::Running => provider_features::probing_axis(),
        Probe::Unavailable => provider_features::unavailable_axis(),
    }
}

/// The one answer to "has this probe finished, and with what". Kept as a
/// three-way type rather than an `Option<Vec<_>>` because "still running",
/// "answered empty" and "failed" are three different facts the form renders
/// three different ways — the same distinction [`VocabularyState`] draws for
/// the axes, and for the same reason.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Probe {
    Running,
    Answered(Vec<VocabularyFeature>),
    Unavailable,
}

/// The ACP probe answers, one per provider, for the life of the daemon.
///
/// Keyed on the provider id and **not** on the model, which is where this
/// cache earns its keep over reusing [`crate::provider_vocabulary`]'s: the
/// read is a `session/new` on the agent's own default model, so no model
/// choice changes what it answers, and keying it by model would start a
/// provider process for every model a human types into a free-text field.
/// The model-dependent half of the answer is the static gate, which costs
/// nothing to re-derive.
///
/// `Running` holds the slot open so ten fast asks start ten probes; a failed
/// read is not cached, because a provider that was not installed a moment ago
/// may be installed now, and `Unavailable` must never outlive the read that
/// produced it.
#[derive(Default)]
pub(crate) struct AcpProbeCache {
    answers: std::sync::Mutex<std::collections::HashMap<String, Probe>>,
}

impl AcpProbeCache {
    /// The cached answer, and `None` when nothing is known — which the caller
    /// turns into a read through `start`.
    pub(crate) fn peek(&self, provider: &str) -> Option<Probe> {
        self.answers
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(provider)
            .cloned()
    }

    /// Claim the slot for `provider` and say whether this caller is the one
    /// that must run the read. A slot already `Running` or `Answered` stays
    /// what it is; an `Unavailable` one is re-claimable, so a provider the
    /// human has since installed is answered on the next ask rather than
    /// pinned to the first failure.
    pub(crate) fn claim(&self, provider: &str) -> bool {
        let mut answers = self
            .answers
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        match answers.get(provider) {
            Some(Probe::Running) | Some(Probe::Answered(_)) => false,
            Some(Probe::Unavailable) | None => {
                answers.insert(provider.to_string(), Probe::Running);
                true
            }
        }
    }

    pub(crate) fn finish(&self, provider: &str, probe: Probe) {
        debug_assert!(
            !matches!(probe, Probe::Running),
            "a read finishes with an answer or a failure, never still running"
        );
        let mut answers = self
            .answers
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        // `claim` put `Running` in; a finish that found no slot means the
        // cache was cleared underneath the read (a provider update), and the
        // answer of a process that no longer describes the provider is not
        // worth putting back.
        if answers.contains_key(provider) {
            answers.insert(provider.to_string(), probe);
        }
    }

    /// Forget every answer. The provider update's callback: a new version of
    /// an agent may declare a different surface, and the next ask re-reads it.
    pub(crate) fn invalidate(&self, provider: &str) {
        self.answers
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(provider);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A provider-declared select, for the cache's own tests. The cache stores
    /// exactly what a read returned, so the value's contents do not matter to
    /// them; what matters is the difference between `Answered(vec![...])` and
    /// `Answered(vec![])`, which the claim tests read as two distinct states.
    fn feature(id: &str) -> VocabularyFeature {
        VocabularyFeature {
            id: id.to_string(),
            label: id.to_string(),
            author: devboule_protocol::VocabularyOrigin::Provider,
            control: devboule_protocol::VocabularyFeatureControl::Select,
            options: vec![devboule_protocol::VocabularyFeatureOption {
                id: "a".to_string(),
                label: "A".to_string(),
            }],
            models: None,
        }
    }

    /// The probe cache answers a claim exactly once, so ten opens of the form
    /// start one provider process, and a failed read is retryable.
    #[test]
    fn one_claim_per_running_read_and_a_failure_retries() {
        let cache = AcpProbeCache::default();
        assert!(cache.claim("grok"), "the first ask claims the slot");
        assert!(!cache.claim("grok"), "and the next ten do not start again");
        cache.finish("grok", Probe::Answered(vec![feature("engine")]));
        assert!(
            !cache.claim("grok"),
            "an answered read is not re-run by a claim"
        );
        assert!(matches!(cache.peek("grok"), Some(Probe::Answered(_))));

        cache.invalidate("grok");
        assert!(cache.claim("grok"), "a provider update re-opens the read");
        cache.finish("grok", Probe::Unavailable);
        assert!(
            cache.claim("grok"),
            "a failure is not cached: a provider installed since then is answered now"
        );
        assert_eq!(cache.peek("grok"), Some(Probe::Running));
    }

    /// A finish that arrives after the cache was cleared underneath the read
    /// puts nothing back: the process it asked is no longer the provider the
    /// entry would describe.
    #[test]
    fn a_finish_after_an_invalidate_writes_nothing() {
        let cache = AcpProbeCache::default();
        assert!(cache.claim("trae"));
        cache.invalidate("trae");
        cache.finish("trae", Probe::Answered(vec![feature("fast")]));
        assert_eq!(cache.peek("trae"), None, "the stale answer is dropped");
    }
}
