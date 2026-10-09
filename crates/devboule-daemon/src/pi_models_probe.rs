//! The pi model read: one catalog snapshot per daemon run, learned by starting
//! pi once, and the cache that makes "once" mean something.
//!
//! Split from [`crate::provider_vocabulary`] because the two answer different
//! questions: that module asks *what does the query reply*, and is a cache and
//! a TTL; this one asks *how does pi's own answer reach the form without
//! starting a process per keystroke*, and is a cache and a thread. The rules
//! that make a process-spawning read safe to hang off a Settings panel are
//! [`crate::provider_feature_probe`]'s, and this cache follows them:
//!
//! - **It never blocks the ask.** The vocabulary reply is a synchronous
//!   round-trip on the connection's own thread, so the first read claims the
//!   slot, the models axis answers `absent`, the features axis answers
//!   `probing`, and the process starts on a worker. The frontend polls while
//!   the features axis probes, so the pickers arrive without a reopen.
//! - **One read per provider per run, refreshed past ten minutes.** A
//!   failed read expires after a short cooldown, so a later form open can
//!   retry; a success stands until it is ten minutes old, then the next ask
//!   re-probes behind the stale axes it keeps serving.
//! - **The snapshot is the chip's own items.** The worker maps the catalog to
//!   manifest models — the shape the composer chip draws — so the editor and
//!   the chip cannot disagree about what pi offers.

use devboule_protocol::SessionModel;

/// One cache key: the provider whose catalog is being read. Only pi reads
/// through this cache today; the key keeps the door open without pretending
/// another provider already walks it.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct PiModelsKey {
    pub(crate) provider: String,
}

impl PiModelsKey {
    pub(crate) fn new(provider: &str) -> Self {
        Self {
            provider: provider.trim().to_string(),
        }
    }
}

/// The one answer to "has this probe finished, and with what". Kept as a
/// three-way type rather than an `Option<Vec<_>>` because "still running",
/// "answered" and "failed" are three different facts the form renders three
/// different ways.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum PiModelsProbe {
    Running,
    Answered(Vec<SessionModel>),
    Unavailable,
}

const UNAVAILABLE_RETRY_AFTER: std::time::Duration = std::time::Duration::from_secs(30);
/// A snapshot older than this is served stale while a refresh runs: the
/// editor's next open re-probes instead of offering a list that may predate
/// an install or removal.
const STALE_AFTER: std::time::Duration = std::time::Duration::from_secs(10 * 60);

/// Bounded per-provider pi answers. The snapshot has no TTL of its own:
/// failures cool down and retry, successes age past ten minutes into a
/// refresh, and the vocabulary query maps whatever the slot holds.
#[derive(Default)]
pub(crate) struct PiModelsProbeCache {
    answers: std::sync::Mutex<std::collections::HashMap<PiModelsKey, ProbeEntry>>,
    next_generation: std::sync::atomic::AtomicU64,
}

#[derive(Clone)]
struct ProbeEntry {
    probe: PiModelsProbe,
    retry_after: Option<std::time::Instant>,
    generation: u64,
    /// When an `Answered` entry landed. `None` while the read runs or
    /// after it failed — only answers age.
    answered_at: Option<std::time::Instant>,
}

impl PiModelsProbeCache {
    /// The cached answer, and `None` when nothing is known — which the caller
    /// turns into a read through `claim`.
    pub(crate) fn peek(&self, key: &PiModelsKey) -> Option<PiModelsProbe> {
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

    /// Whether the slot holds an answer older than [`STALE_AFTER`]: still
    /// served, but the next ask re-probes behind it.
    pub(crate) fn stale(&self, key: &PiModelsKey) -> bool {
        self.answers
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(key)
            .is_some_and(|entry| {
                matches!(entry.probe, PiModelsProbe::Answered(_))
                    && entry
                        .answered_at
                        .is_some_and(|at| at.elapsed() > STALE_AFTER)
            })
    }

    /// Claim the slot and say whether this caller is the one that must run
    /// the read. A slot already `Running` or freshly `Answered` stays what
    /// it is; a stale answer and an `Unavailable` past its cooldown are
    /// re-claimable, so a later editor open retries.
    pub(crate) fn claim(&self, key: &PiModelsKey) -> Option<u64> {
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
                // A stale answer is re-claimable: the next ask re-probes
                // behind the stale axes it keeps serving. Read inline —
                // the guard is held, so `stale` must not lock again.
                let stale = matches!(entry.probe, PiModelsProbe::Answered(_))
                    && entry
                        .answered_at
                        .is_some_and(|at| at.elapsed() > STALE_AFTER);
                if !stale {
                    return None;
                }
            }
            _ => {}
        }
        let generation = self
            .next_generation
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        answers.insert(
            key.clone(),
            ProbeEntry {
                probe: PiModelsProbe::Running,
                retry_after: None,
                generation,
                answered_at: None,
            },
        );
        Some(generation)
    }

    /// Settle a claimed read. A newer claim owns the slot: a stale worker's
    /// answer is dropped, never adopted.
    pub(crate) fn finish(&self, key: &PiModelsKey, generation: u64, probe: PiModelsProbe) {
        let mut answers = self
            .answers
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let current = answers.get(key).map(|entry| entry.generation);
        if current != Some(generation) {
            return;
        }
        let retry_after = match probe {
            PiModelsProbe::Unavailable => Some(std::time::Instant::now() + UNAVAILABLE_RETRY_AFTER),
            _ => None,
        };
        let answered_at = match probe {
            PiModelsProbe::Answered(_) => Some(std::time::Instant::now()),
            _ => None,
        };
        answers.insert(
            key.clone(),
            ProbeEntry {
                probe,
                retry_after,
                generation,
                answered_at,
            },
        );
    }

    /// Record an answer without running a read. Test-only: the axes tests
    /// place a snapshot at pi's key without starting a process.
    #[cfg(test)]
    pub(crate) fn record_answer_for_test(&self, key: &PiModelsKey, models: Vec<SessionModel>) {
        let generation = self
            .next_generation
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if let Ok(mut answers) = self.answers.lock() {
            answers.insert(
                key.clone(),
                ProbeEntry {
                    probe: PiModelsProbe::Answered(models),
                    retry_after: None,
                    generation,
                    answered_at: Some(std::time::Instant::now()),
                },
            );
        }
    }

    /// Age an answer for the stale-refresh tests.
    #[cfg(test)]
    pub(crate) fn age_for_test(&self, key: &PiModelsKey, by: std::time::Duration) {
        if let Ok(mut answers) = self.answers.lock() {
            if let Some(entry) = answers.get_mut(key) {
                entry.answered_at = entry.answered_at.map(|at| at - by);
            }
        }
    }
}

/// The models axis of one answered snapshot: pi's own answer, provider
/// origin, or `none` when pi answered with no models at all.
pub(crate) fn pi_models_axis(
    models: Vec<devboule_protocol::SessionModel>,
) -> devboule_protocol::VocabularyModels {
    use devboule_protocol::{VocabularyModels, VocabularyOrigin, VocabularyState};
    if models.is_empty() {
        return VocabularyModels::new(VocabularyState::None, None, Vec::new())
            .expect("a none models axis carries no origin");
    }
    VocabularyModels::new(
        VocabularyState::Present,
        Some(VocabularyOrigin::Provider),
        models,
    )
    .expect("a present models axis always carries its origin and items")
}

/// Run one pi read on the calling thread and settle the claim with it. The
/// vocabulary query calls this on a worker, so a slow pi (or a missing one)
/// never blocks the connection the ask arrived on.
pub(crate) fn run_pi_models_probe(
    cache: &std::sync::Arc<PiModelsProbeCache>,
    key: PiModelsKey,
    generation: u64,
) {
    let probe = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        crate::session::probe_models_snapshot()
    })) {
        Ok(Ok(models)) => PiModelsProbe::Answered(models),
        Ok(Err(error)) => {
            eprintln!(
                "pi model read: {} learned nothing (provider error of {} bytes)",
                key.provider,
                error.message.len()
            );
            PiModelsProbe::Unavailable
        }
        Err(_) => {
            eprintln!("pi model read: {} worker panicked", key.provider);
            PiModelsProbe::Unavailable
        }
    };
    cache.finish(&key, generation, probe);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key() -> PiModelsKey {
        PiModelsKey::new("pi")
    }

    fn model(id: &str, provider: &str) -> SessionModel {
        SessionModel {
            model_id: id.to_string(),
            name: id.to_string(),
            provider_id: Some(provider.to_string()),
            description: None,
            context_tokens: None,
            current_effort: None,
            efforts: None,
        }
    }

    #[test]
    fn claim_runs_once_and_finish_settles_the_slot() {
        let cache = PiModelsProbeCache::default();
        assert!(cache.peek(&key()).is_none());
        let generation = cache.claim(&key()).expect("first claim runs");
        // A second claim while the read runs loses: one process per question.
        assert!(cache.claim(&key()).is_none());
        assert_eq!(cache.peek(&key()), Some(PiModelsProbe::Running));
        cache.finish(
            &key(),
            generation,
            PiModelsProbe::Answered(vec![model("mimo", "opencode-go")]),
        );
        let answered = cache.peek(&key()).expect("answered");
        assert!(matches!(answered, PiModelsProbe::Answered(_)));
        // Answered sticks: no second read for the run.
        assert!(cache.claim(&key()).is_none());
    }

    #[test]
    fn stale_worker_answer_is_dropped_never_adopted() {
        let cache = PiModelsProbeCache::default();
        let first = cache.claim(&key()).expect("claim");
        cache.finish(&key(), first, PiModelsProbe::Unavailable);
        // A stale generation cannot overwrite the slot.
        cache.finish(&key(), first + 99, PiModelsProbe::Answered(vec![]));
        assert_eq!(cache.peek(&key()), Some(PiModelsProbe::Unavailable));
    }

    #[test]
    fn stale_snapshot_is_reclaimable_for_a_refresh() {
        let cache = PiModelsProbeCache::default();
        let generation = cache.claim(&key()).expect("claim");
        cache.finish(
            &key(),
            generation,
            PiModelsProbe::Answered(vec![model("mimo", "opencode-go")]),
        );
        // Fresh: no second read for the run.
        assert!(cache.claim(&key()).is_none());
        assert!(!cache.stale(&key()));
        // Older than ten minutes: the editor's next open re-probes while
        // the stale axes keep serving.
        cache.age_for_test(&key(), std::time::Duration::from_secs(11 * 60));
        assert!(cache.stale(&key()));
        assert!(cache.claim(&key()).is_some());
        assert_eq!(cache.peek(&key()), Some(PiModelsProbe::Running));
    }

    #[test]
    fn answered_models_are_provider_origin_none_is_none() {
        let present = pi_models_axis(vec![model("mimo", "opencode-go")]);
        assert_eq!(present.state, devboule_protocol::VocabularyState::Present);
        assert_eq!(
            present.origin,
            Some(devboule_protocol::VocabularyOrigin::Provider)
        );
        assert_eq!(present.items.len(), 1);
        let none = pi_models_axis(Vec::new());
        assert_eq!(none.state, devboule_protocol::VocabularyState::None);
        assert_eq!(none.origin, None);
        assert!(none.items.is_empty());
    }
}
