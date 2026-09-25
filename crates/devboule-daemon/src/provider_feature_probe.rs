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

/// One cache key: a provider, and the model the read was made against.
///
/// The model is in the key because Paseo's own answer is: its `listFeatures`
/// opens `session/new` with **no** model (`acp-agent.ts:1130`, `{cwd,
/// mcpServers: []}`), while its profile form re-asks on every model change
/// (`use-profile-form-inputs.ts` keys the fetch on provider, cwd, mode, model and
/// thinking option). The two are not a contradiction — the request does not name
/// a model, the *answer may differ per model anyway*, because an agent derives
/// its own `configOptions` from the model it started that session on. Copying it
/// means one spawn per model a human actually chooses, which is what Paseo costs
/// its users, and never a prune of a B-valid value against A's list.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct ProbeKey {
    pub(crate) provider: String,
    pub(crate) model: Option<String>,
}

impl ProbeKey {
    pub(crate) fn new(provider: &str, model: Option<&str>) -> Self {
        Self {
            provider: provider.to_string(),
            model: model.map(|model| model.to_string()),
        }
    }
}

/// The cached ACP declarations for one provider **and model**, if that read has
/// answered. `Some(vec![])` is a real answer — the agent declared nothing — while
/// `None` is "nobody has asked *for this model* yet", and the difference decides
/// whether the store may prune at all
/// ([`crate::provider_features::prune_for`]).
///
/// The model is part of the lookup and not a detail: pruning a profile against a
/// list read while the agent ran a different model would delete a choice this
/// agent does offer. `None` model is its own key, the same one the form uses
/// before a model is chosen.
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

/// The feature axis of one named ACP provider at one model: the cached answer
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
/// - **One read per provider *and model*, never one per keystroke.** The key is
///   [`ProbeKey`]: a distinct read per model a human actually chooses, which is
///   Paseo's own cost, while typing in a model field costs nothing because the
///   form asks only when the field settles.
/// - **A failed read is not an answer.** A provider that is not installed, or
///   died, or refused the handshake leaves no cached failure: the next ask
///   claims the slot again, so installing it is picked up without anyone
///   reaching for a refresh button.
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

/// The ACP probe answers, one per provider and model, for the life of the daemon.
///
/// Keyed on [`ProbeKey`] - provider *and* model - because a list read while the agent
/// ran one model must never justify pruning a profile that names another: the
/// `session/new` the read sends carries no model, but the agent derives its own
/// `configOptions` from the model that session started on, and Paseo's profile form
/// re-asks per model for exactly that reason. There is no TTL, because the value
/// costs a process to obtain, and no entry per keystroke, because the form re-reads
/// only when a model field settles.
///
/// `Running` holds the slot open so ten fast asks start ten probes; a failed
/// read is not cached, because a provider that was not installed a moment ago
/// may be installed now, and `Unavailable` must never outlive the read that
/// produced it.
#[derive(Default)]
pub(crate) struct AcpProbeCache {
    answers: std::sync::Mutex<std::collections::HashMap<ProbeKey, Probe>>,
}

impl AcpProbeCache {
    /// The cached answer, and `None` when nothing is known — which the caller
    /// turns into a read through `start`.
    pub(crate) fn peek(&self, key: &ProbeKey) -> Option<Probe> {
        self.answers
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(key)
            .cloned()
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
            Some(Probe::Running) | Some(Probe::Answered(_)) => false,
            Some(Probe::Unavailable) | None => {
                answers.insert(key.clone(), Probe::Running);
                true
            }
        }
    }

    pub(crate) fn finish(&self, key: &ProbeKey, probe: Probe) {
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
            answers.insert(key.clone(), probe);
        }
    }

    /// Forget every answer. The provider update's callback: a new version of
    /// an agent may declare a different surface, and the next ask re-reads it.
    /// Forget every answer for one provider, whatever model it was read for: a
    /// provider update changes the executable, and a list read from the old one
    /// describes a surface that may no longer exist.
    pub(crate) fn invalidate(&self, provider: &str) {
        self.answers
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .retain(|key, _| key.provider != provider);
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

    /// The probe cache answers a claim exactly once per provider *and model*, so
    /// ten opens of the form start one provider process, and a failed read is
    /// retryable.
    #[test]
    fn one_claim_per_running_read_and_a_failure_retries() {
        let cache = AcpProbeCache::default();
        let grok = ProbeKey::new("grok", None);
        let other_model = ProbeKey::new("grok", Some("glm-4.6"));
        assert!(cache.claim(&grok), "the first ask claims the slot");
        assert!(!cache.claim(&grok), "and the next ten do not start again");
        cache.finish(&grok, Probe::Answered(vec![feature("engine")]));
        assert!(
            !cache.claim(&grok),
            "an answered read is not re-run by a claim"
        );
        assert!(matches!(cache.peek(&grok), Some(Probe::Answered(_))));
        // The model is in the key, so a list read on one model is never the
        // answer for another. This is the half that protects a value rather than
        // the cost of a spawn: `prune_for` asks for the profile's own pair, and
        // an entry for a different model must not answer it.
        assert!(
            cache.peek(&other_model).is_none(),
            "a second model has no answer from the first's read"
        );
        assert!(
            cache.claim(&other_model),
            "and it is claimed on its own, so its read runs"
        );

        cache.invalidate("grok");
        assert!(
            cache.peek(&grok).is_none(),
            "an invalidation drops every model of that provider"
        );
        assert!(cache.claim(&grok), "a provider update re-opens the read");
        cache.finish(&grok, Probe::Unavailable);
        assert!(
            cache.claim(&grok),
            "a failure is not cached: a provider installed since then is answered now"
        );
        assert_eq!(cache.peek(&grok), Some(Probe::Running));
    }

    /// A finish that arrives after the cache was cleared underneath the read
    /// puts nothing back: the process it asked is no longer the provider the
    /// entry would describe.
    #[test]
    fn a_finish_after_an_invalidate_writes_nothing() {
        let cache = AcpProbeCache::default();
        let trae = ProbeKey::new("trae", None);
        assert!(cache.claim(&trae));
        cache.invalidate("trae");
        cache.finish(&trae, Probe::Answered(vec![feature("fast")]));
        assert_eq!(cache.peek(&trae), None, "the stale answer is dropped");
    }
    /// The store's prune asks for the profile's own pair. A list read while the
    /// agent ran another model is not an answer about this profile, and answering
    /// the lookup with it is how a valid stored choice gets deleted: the same
    /// `Some(list)` that licenses a prune for one model must not license one for
    /// another.
    #[test]
    fn an_answer_for_one_model_never_answers_the_lookup_for_another() {
        let mut answers = std::collections::HashMap::new();
        answers.insert(
            ProbeKey::new("grok", Some("glm-4.6")),
            vec![feature("fast")],
        );
        let read = |key: &ProbeKey| answers.get(key).cloned();
        assert!(
            read(&ProbeKey::new("grok", Some("glm-4.6"))).is_some(),
            "the pair that was asked has an answer"
        );
        assert!(
            read(&ProbeKey::new("grok", Some("glm-4.7"))).is_none(),
            "a neighbour model's list is not this one's answer"
        );
        assert!(
            read(&ProbeKey::new("grok", None)).is_none(),
            "and no model at all is not the same key as some model"
        );
    }
}
