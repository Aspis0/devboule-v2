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

/// One cache key: the provider whose `session/new` answer is being read.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct ProbeKey {
    pub(crate) provider: String,
}

impl ProbeKey {
    pub(crate) fn new(provider: &str) -> Self {
        Self {
            provider: provider.trim().to_string(),
        }
    }
}

/// The cached ACP declarations for one provider, if that read has
/// answered. `Some(vec![])` is a real answer — the agent declared nothing — while
/// `None` is "nobody has asked yet", and the difference decides
/// whether the store may prune at all
/// ([`crate::provider_features::prune_for`]).
///
pub(crate) fn cached_declarations(key: &ProbeKey) -> Option<Vec<VocabularyFeature>> {
    // The store has no handle on `ServerState`, and reaching for one to ask
    // this question would put a process-spawning read on the profile write
    // path. So the store reads what a previous vocabulary query already left
    // behind, which is the only answer that can exist without a spawn, and an
    // unread provider is pruned at spawn instead.
    ACP_PROBE_ANSWERS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .get(key)
        .cloned()
}

/// The last answer each ACP read produced, kept beside the cache that serves
/// the form so the profile store can prune with it. Written by [`acp_axis_for`]
/// when a read finishes.
static ACP_PROBE_ANSWERS: std::sync::LazyLock<
    std::sync::Mutex<std::collections::HashMap<ProbeKey, Vec<VocabularyFeature>>>,
> = std::sync::LazyLock::new(|| std::sync::Mutex::new(std::collections::HashMap::new()));

const MAX_CACHED_PROVIDERS: usize = 128;
const UNAVAILABLE_RETRY_AFTER: std::time::Duration = std::time::Duration::from_secs(30);

/// The feature axis of one named ACP provider: the cached answer
/// for that pair if there is one, otherwise "being read now" and a started read.
///
/// This is where `D1`'s cache lives, and the three rules that make a
/// process-spawning read safe to hang off a Settings panel are its:
///
/// - **It never blocks the ask.** The vocabulary reply is a synchronous
///   round-trip on the connection's own thread, so the first read claims the
///   slot, answers `probing`, and starts the process on a worker. A form that
///   waited on an `npx` cold start would be a frozen Settings panel — Paseo can
///   await its probe only because its server is asynchronous.
/// - **One read per provider.** `session/new` carries no model, so the probe
///   cannot establish a model-specific declaration.
/// - **A failed read expires.** Brief failures are cached to avoid starting a
///   provider on every poll, then a later open can retry.
pub(crate) fn acp_axis_for(
    state: &std::sync::Arc<crate::server::ServerState>,
    key: &ProbeKey,
) -> VocabularyFeatures {
    let cache = &state.acp_features;
    if let Some(probe) = cache.peek(key) {
        return acp_axis_of(probe);
    }
    if !cache.claim(key) {
        // Lost the claim to a concurrent ask for the same pair. Whatever it has
        // left is the answer, and a second provider process is never started for
        // one question.
        return acp_axis_of(cache.peek(key).unwrap_or(Probe::Running));
    }
    let worker_state = std::sync::Arc::clone(state);
    let worker_cache = std::sync::Arc::clone(cache);
    let worker_key = key.clone();
    let started = std::thread::Builder::new()
        .name("acp-feature-read".to_string())
        .spawn(move || {
            let answer =
                match crate::session::probe_declarations(&worker_state, &worker_key.provider) {
                    Ok(declared) => {
                        // Recorded for the profile store as well as the form: the
                        // store prunes an ACP profile's keys against this list and
                        // must not spawn a process to find it.
                        if let Ok(mut answers) = ACP_PROBE_ANSWERS.lock() {
                            if answers.len() >= MAX_CACHED_PROVIDERS {
                                answers.clear();
                            }
                            answers.insert(worker_key.clone(), declared.clone());
                        }
                        Probe::Answered(declared)
                    }
                    Err(error) => {
                        // A fixed sentence, and no provider text. An ACP error
                        // message is provider-supplied, so a misconfigured agent that
                        // echoes a private option or a credential in its error would
                        // put that value in the daemon's log — and no profile feature
                        // needs to be logged to trigger it. The byte length is the
                        // part a support session can use: it tells an empty reply from
                        // a long one without carrying either.
                        eprintln!(
                            "acp feature read: {} learned nothing (provider error of {} bytes)",
                            worker_key.provider,
                            error.message.len()
                        );
                        Probe::Unavailable
                    }
                };
            worker_cache.finish(&worker_key, answer);
        });
    if started.is_err() {
        // No thread to ask with. Release the claim so the next ask retries, and
        // say what a failed read says: nothing was learned.
        cache.invalidate(&key.provider);
        return provider_features::unavailable_axis();
    }
    provider_features::probing_axis()
}

/// Record an answer for one key without running a read. Test-only: the store's
/// prune reads [`cached_declarations`], and a test that wants to see whether a
/// list read for one model can answer a prune for another has to be able to
/// place an answer at a chosen key — which is exactly the thing the key's
/// shape decides.
#[cfg(test)]
pub(crate) fn record_answer_for_test(key: &ProbeKey, declared: Vec<VocabularyFeature>) {
    if let Ok(mut answers) = ACP_PROBE_ANSWERS.lock() {
        answers.insert(key.clone(), declared);
    }
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

/// Bounded per-provider ACP answers. The request has no model field, and brief
/// failures expire so a later form open can retry.
#[derive(Default)]
pub(crate) struct AcpProbeCache {
    answers: std::sync::Mutex<std::collections::HashMap<ProbeKey, ProbeEntry>>,
}

#[derive(Clone)]
struct ProbeEntry {
    probe: Probe,
    retry_after: Option<std::time::Instant>,
}

impl AcpProbeCache {
    /// The cached answer, and `None` when nothing is known — which the caller
    /// turns into a read through `start`.
    pub(crate) fn peek(&self, key: &ProbeKey) -> Option<Probe> {
        self.answers
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(key)
            .filter(|entry| {
                entry
                    .retry_after
                    .is_none_or(|at| at > std::time::Instant::now())
            })
            .map(|entry| entry.probe.clone())
    }

    /// Claim the slot for `provider` and say whether this caller is the one
    /// that must run the read. A slot already `Running` or `Answered` stays
    /// what it is; an `Unavailable` one is re-claimable, so a provider the
    /// human has since installed is answered on the next ask rather than
    /// pinned to the first failure.
    pub(crate) fn claim(&self, key: &ProbeKey) -> bool {
        let mut answers = self
            .answers
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        match answers.get(key) {
            Some(entry)
                if entry
                    .retry_after
                    .is_none_or(|at| at > std::time::Instant::now()) =>
            {
                false
            }
            _ => {
                if answers.len() >= MAX_CACHED_PROVIDERS {
                    if let Some(evict) = answers.keys().next().cloned() {
                        answers.remove(&evict);
                    }
                }
                answers.insert(
                    key.clone(),
                    ProbeEntry {
                        probe: Probe::Running,
                        retry_after: None,
                    },
                );
                true
            }
        }
    }

    pub(crate) fn finish(&self, key: &ProbeKey, probe: Probe) {
        self.finish_at(key, probe, std::time::Instant::now());
    }

    fn finish_at(&self, key: &ProbeKey, probe: Probe, now: std::time::Instant) {
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
        if answers.contains_key(key) {
            answers.insert(
                key.clone(),
                ProbeEntry {
                    retry_after: matches!(&probe, Probe::Unavailable)
                        .then(|| now + UNAVAILABLE_RETRY_AFTER),
                    probe,
                },
            );
        }
    }

    /// Forget this provider's answer after an update changes its executable.
    pub(crate) fn invalidate(&self, provider: &str) {
        self.answers
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .retain(|key, _| key.provider != provider);
        if let Ok(mut answers) = ACP_PROBE_ANSWERS.lock() {
            answers.retain(|key, _| key.provider != provider);
        }
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

    /// The probe cache answers a claim exactly once per provider, so
    /// ten opens of the form start one provider process, and a failed read is
    /// retryable.
    #[test]
    fn one_claim_per_running_read_and_a_failure_retries() {
        let cache = AcpProbeCache::default();
        let grok = ProbeKey::new(" grok ");
        assert!(cache.claim(&grok), "the first ask claims the slot");
        assert!(!cache.claim(&grok), "and the next ten do not start again");
        cache.finish(&grok, Probe::Answered(vec![feature("engine")]));
        assert!(
            !cache.claim(&grok),
            "an answered read is not re-run by a claim"
        );
        assert!(matches!(cache.peek(&grok), Some(Probe::Answered(_))));
        assert_eq!(grok.provider, "grok", "provider keys are normalized");

        cache.invalidate("grok");
        assert!(
            cache.peek(&grok).is_none(),
            "an invalidation drops every model of that provider"
        );
        assert!(cache.claim(&grok), "a provider update re-opens the read");
        cache.finish(&grok, Probe::Unavailable);
        assert_eq!(cache.peek(&grok), Some(Probe::Unavailable));
        assert!(
            !cache.claim(&grok),
            "a short failure cooldown prevents a spawn storm"
        );
    }

    #[test]
    fn unavailable_answers_expire_and_can_be_retried() {
        let cache = AcpProbeCache::default();
        let key = ProbeKey::new("grok");
        assert!(cache.claim(&key));
        let expired_at = std::time::Instant::now() - UNAVAILABLE_RETRY_AFTER;
        cache.finish_at(&key, Probe::Unavailable, expired_at);
        assert_eq!(cache.peek(&key), None);
        assert!(cache.claim(&key));
    }

    /// A finish that arrives after the cache was cleared underneath the read
    /// puts nothing back: the process it asked is no longer the provider the
    /// entry would describe.
    #[test]
    fn a_finish_after_an_invalidate_writes_nothing() {
        let cache = AcpProbeCache::default();
        let trae = ProbeKey::new("trae");
        assert!(cache.claim(&trae));
        cache.invalidate("trae");
        cache.finish(&trae, Probe::Answered(vec![feature("fast")]));
        assert_eq!(cache.peek(&trae), None, "the stale answer is dropped");
    }
    #[test]
    fn cache_evicts_when_its_provider_bound_is_reached() {
        let cache = AcpProbeCache::default();
        for index in 0..=MAX_CACHED_PROVIDERS {
            let key = ProbeKey::new(&format!("provider-{index}"));
            assert!(cache.claim(&key));
            cache.finish(&key, Probe::Answered(vec![]));
        }
        let entries = cache.answers.lock().unwrap();
        assert!(entries.len() <= MAX_CACHED_PROVIDERS);
    }
}
