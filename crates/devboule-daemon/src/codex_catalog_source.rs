//! The Codex catalog a draft reads: one probe shared by concurrent callers, a
//! live answer kept for a while, and a fresh disk cache answering at once while
//! the probe refreshes in the background.

use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use crate::session::codex_catalog_probe::CodexCatalogRead;

/// What one probe run answers with.
pub(crate) type Probe = Arc<dyn Fn() -> Option<CodexCatalogRead> + Send + Sync>;

/// How long a live answer serves the drafts that ask for it, before a new probe.
const LIVE_TTL: Duration = Duration::from_secs(30 * 60);
/// How long a caller waits on a shared probe. The probe has its own budget; this
/// only bounds a probe thread that never settles.
const WAIT_BUDGET: Duration = Duration::from_secs(20);

#[derive(Default)]
pub(crate) struct CodexCatalogSource {
    shared: Arc<Shared>,
}

#[derive(Default)]
struct Shared {
    slot: Mutex<Slot>,
}

#[derive(Default)]
struct Slot {
    live: Option<(Instant, CodexCatalogRead)>,
    flight: Option<Arc<Flight>>,
}

/// One probe run, shared by every caller that asks while it runs.
struct Flight {
    outcome: Mutex<Option<Option<CodexCatalogRead>>>,
    settled: Condvar,
}

impl CodexCatalogSource {
    /// Whether a probe has answered within the live TTL. A disk answer is not one:
    /// until the background probe lands, the vocabulary query must not cache it.
    pub(crate) fn has_live_answer(&self) -> bool {
        lock(&self.shared.slot)
            .live
            .as_ref()
            .is_some_and(|(at, _)| at.elapsed() < LIVE_TTL)
    }

    /// The catalog to answer with. A live answer within its TTL is reused; a disk
    /// answer, when given, is returned at once and refreshed by a background probe;
    /// otherwise the caller waits on the one probe that is running, or starts it.
    pub(crate) fn read(
        &self,
        probe: Probe,
        disk: Option<CodexCatalogRead>,
    ) -> Option<CodexCatalogRead> {
        let mut slot = lock(&self.shared.slot);
        if let Some((at, answer)) = &slot.live {
            if at.elapsed() < LIVE_TTL {
                return Some(answer.clone());
            }
        }
        if let Some(disk) = disk {
            if slot.flight.is_none() {
                let flight = start_flight(&mut slot);
                drop(slot);
                let shared = self.shared.clone();
                std::thread::spawn(move || {
                    run_flight(&shared, &flight, &probe);
                });
            }
            return Some(disk);
        }
        if let Some(flight) = slot.flight.clone() {
            drop(slot);
            return wait(&flight);
        }
        let flight = start_flight(&mut slot);
        drop(slot);
        run_flight(&self.shared, &flight, &probe)
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

fn start_flight(slot: &mut Slot) -> Arc<Flight> {
    let flight = Arc::new(Flight {
        outcome: Mutex::new(None),
        settled: Condvar::new(),
    });
    slot.flight = Some(flight.clone());
    flight
}

/// Runs the probe for `flight` and settles it, so every waiter gets the same answer.
fn run_flight(shared: &Shared, flight: &Arc<Flight>, probe: &Probe) -> Option<CodexCatalogRead> {
    let outcome = probe();
    let mut slot = lock(&shared.slot);
    if let Some(read) = &outcome {
        slot.live = Some((Instant::now(), read.clone()));
    }
    if slot
        .flight
        .as_ref()
        .is_some_and(|current| Arc::ptr_eq(current, flight))
    {
        slot.flight = None;
    }
    drop(slot);
    *lock(&flight.outcome) = Some(outcome.clone());
    flight.settled.notify_all();
    outcome
}

fn wait(flight: &Flight) -> Option<CodexCatalogRead> {
    let deadline = Instant::now() + WAIT_BUDGET;
    let mut outcome = lock(&flight.outcome);
    while outcome.is_none() {
        let remaining = deadline.checked_duration_since(Instant::now())?;
        let (guard, _) = flight
            .settled
            .wait_timeout(outcome, remaining)
            .unwrap_or_else(PoisonError::into_inner);
        outcome = guard;
    }
    outcome.clone().flatten()
}

#[cfg(test)]
mod tests {
    use super::{CodexCatalogSource, Probe};
    use crate::session::codex_catalog_probe::CodexCatalogRead;
    use serde_json::{json, Value};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{mpsc, Arc, Mutex};
    use std::time::{Duration, Instant};

    fn answer(model: &str) -> CodexCatalogRead {
        CodexCatalogRead {
            rows: vec![json!({ "model": model })],
            config: Value::Null,
        }
    }

    fn counting(calls: Arc<AtomicUsize>, delay: Duration, model: &'static str) -> Probe {
        Arc::new(move || {
            calls.fetch_add(1, Ordering::SeqCst);
            std::thread::sleep(delay);
            Some(answer(model))
        })
    }

    #[test]
    fn two_concurrent_cold_reads_share_one_probe() {
        let source = Arc::new(CodexCatalogSource::default());
        let calls = Arc::new(AtomicUsize::new(0));
        let probe = counting(calls.clone(), Duration::from_millis(300), "gpt-a");

        let readers: Vec<_> = (0..2)
            .map(|_| {
                let source = source.clone();
                let probe = probe.clone();
                std::thread::spawn(move || source.read(probe, None))
            })
            .collect();
        let answers: Vec<_> = readers
            .into_iter()
            .map(|reader| reader.join().expect("reader"))
            .collect();

        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "one probe serves both callers"
        );
        assert!(
            answers.iter().all(Option::is_some),
            "both callers get the answer"
        );
    }

    #[test]
    fn a_live_answer_is_reused_without_another_probe() {
        let source = CodexCatalogSource::default();
        let calls = Arc::new(AtomicUsize::new(0));
        let probe = counting(calls.clone(), Duration::ZERO, "gpt-a");

        source.read(probe.clone(), None);
        source.read(probe, None);

        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn a_fresh_disk_answer_comes_back_without_waiting_for_the_probe() {
        let source = CodexCatalogSource::default();
        let (release, gate) = mpsc::channel::<()>();
        let gate = Mutex::new(gate);
        let probe: Probe = Arc::new(move || {
            let _ = gate
                .lock()
                .expect("gate")
                .recv_timeout(Duration::from_secs(5));
            Some(answer("live"))
        });

        let started = Instant::now();
        let read = source.read(probe, Some(answer("disk")));
        let waited = started.elapsed();
        drop(release);

        assert!(
            waited < Duration::from_secs(2),
            "the disk answer did not wait: {waited:?}"
        );
        assert_eq!(read.expect("the disk answer").rows[0]["model"], "disk");
    }

    #[test]
    fn a_disk_answer_is_not_live_until_the_background_probe_lands() {
        let source = CodexCatalogSource::default();
        let calls = Arc::new(AtomicUsize::new(0));
        let probe = counting(calls.clone(), Duration::from_millis(100), "live");

        source.read(probe, Some(answer("disk")));
        assert!(!source.has_live_answer(), "a disk answer is not a live one");
        let deadline = Instant::now() + Duration::from_secs(2);
        while !source.has_live_answer() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }

        assert!(source.has_live_answer(), "the probe answer is live");
    }

    #[test]
    fn a_fresh_disk_answer_is_refreshed_by_a_background_probe() {
        let source = CodexCatalogSource::default();
        let calls = Arc::new(AtomicUsize::new(0));
        let probe = counting(calls.clone(), Duration::ZERO, "live");

        source.read(probe, Some(answer("disk")));
        let deadline = Instant::now() + Duration::from_secs(2);
        while calls.load(Ordering::SeqCst) == 0 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }

        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "the probe refreshed the answer"
        );
    }
}
