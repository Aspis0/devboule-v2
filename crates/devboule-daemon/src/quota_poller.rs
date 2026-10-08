//! Keeps OpenCode Go's usage reading current while a Pi session is on an
//! OpenCode model. One thread, started by the first demand, asks the source
//! when [`Schedule`] says it may: about once a minute after a reading, and
//! further apart after each kind of failure. A poll that yields no reading
//! leaves the cached one as it was.

use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Condvar, Mutex, Once};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use devboule_protocol::SessionEvent;

use crate::egress_client::OutboundError;
use crate::plan_usage_cache;
use crate::quota_key::{
    key_fingerprint_from_process, opencode_key_from_process, ApiKey, KeySource,
};
use crate::quota_live;
use crate::quota_opencode_go::OpencodeGo;
use crate::quota_source::{fetch, QuotaError};

#[cfg_attr(test, allow(dead_code))]
const POLL_INTERVAL: Duration = Duration::from_secs(60);
/// A demand keeps polling this long after it was last made.
const DEMAND_WINDOW_MS: i64 = 10 * 60 * 1000;
/// The wait after a reading.
const SUCCESS_INTERVAL_MS: i64 = 60_000;
/// The wait after the first transient failure; each further one doubles it.
const FIRST_BACKOFF_MS: i64 = 2 * 60_000;
/// No wait is longer than this, a `Retry-After` included.
const BACKOFF_CAP_MS: i64 = 30 * 60_000;
/// A refused key is not asked again for an hour, unless the key source changes.
const REFUSED_HOLD_MS: i64 = 60 * 60_000;
/// A reply that held no usable reading is not asked again for half an hour.
const MALFORMED_HOLD_MS: i64 = 30 * 60_000;
/// A demand may poll ahead of the schedule, but not more often than this.
const DEMAND_GAP_MS: i64 = 15_000;

/// Until when polling is wanted, in Unix milliseconds.
static DEMAND_UNTIL_MS: AtomicI64 = AtomicI64::new(0);

/// The demand flag and the thread's wake-up. A demand sets the flag and
/// notifies; any number of demands before the thread looks are one flag, so
/// they cost one pass of the loop and at most one request.
struct Wake {
    pending: Mutex<bool>,
    changed: Condvar,
}

static WAKE: Wake = Wake {
    pending: Mutex::new(false),
    changed: Condvar::new(),
};

/// Starts the thread once, on the first demand.
static STARTED: Once = Once::new();

/// A Pi session is on an OpenCode model, or was just attached to one: poll now,
/// and keep polling while the demand is recent.
pub(crate) fn note_demand() {
    DEMAND_UNTIL_MS.store(now_ms() + DEMAND_WINDOW_MS, Ordering::SeqCst);
    if let Ok(mut pending) = WAKE.pending.lock() {
        *pending = true;
    }
    WAKE.changed.notify_all();
    STARTED.call_once(spawn_poller);
}

/// Takes the demand flag, so one demand is one wake.
fn take_demand() -> bool {
    let mut pending = WAKE.pending.lock().unwrap_or_else(|error| error.into_inner());
    std::mem::replace(&mut *pending, false)
}

/// Sleeps until a demand arrives or the timeout passes, whichever is first.
fn wait_for_demand(timeout: Duration) {
    let pending = WAKE.pending.lock().unwrap_or_else(|error| error.into_inner());
    let _ = WAKE.changed.wait_timeout_while(pending, timeout, |pending| !*pending);
}

/// The poll thread. If it cannot start, the flag is set and nothing reads it:
/// no reading is stored, and the cached one stays.
#[cfg(not(test))]
fn spawn_poller() {
    let _ = std::thread::Builder::new()
        .name("opencode-go-quota".to_string())
        .spawn(run);
}

/// Tests set the demand and never start the thread, so no test reads the
/// real environment or the person's auth file.
#[cfg(test)]
fn spawn_poller() {}

/// A manifest that names a Pi session on an OpenCode model is a demand: a
/// stored manifest is what the session declares, so every handshake and model
/// switch passes here.
pub(crate) fn note_manifest(manifest: &SessionEvent) {
    if let SessionEvent::SessionManifest {
        provider_id: Some(provider_id),
        current_model_provider_id: Some(model_provider_id),
        ..
    } = manifest
    {
        if provider_id == "pi" && model_provider_id == "opencode" {
            note_demand();
        }
    }
}

/// What one poll attempt came to. The schedule decides the next attempt from it,
/// and nothing here carries the key or the reply.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Attempt {
    Reading,
    NoKey,
    /// The key was refused (401 or 403).
    Refused,
    /// The provider asked for less traffic (429), with its delay when it named one.
    Throttled { retry_after_secs: Option<u64> },
    /// The network, a timeout, or a server error: the usual kind of failure.
    Transient,
    /// The reply held no usable reading, or was refused as a whole.
    Malformed,
}

impl From<QuotaError> for Attempt {
    fn from(error: QuotaError) -> Self {
        match error {
            QuotaError::Rejected => Self::Refused,
            QuotaError::Throttled { retry_after_secs } => Self::Throttled { retry_after_secs },
            QuotaError::Status(_)
            | QuotaError::Outbound(
                OutboundError::Timeout | OutboundError::Cut | OutboundError::Transport,
            ) => Self::Transient,
            QuotaError::Malformed
            | QuotaError::Outbound(OutboundError::TooLarge | OutboundError::Refused(_)) => {
                Self::Malformed
            }
        }
    }
}

/// When the next poll may run, decided from what the last one came to. The
/// caller passes the clock, the key-source fingerprint and whether a demand
/// arrived, so every rule is tested without a thread or a network.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Schedule {
    /// The earliest moment the next poll may run, in Unix milliseconds.
    next_ms: i64,
    /// Set by any outcome that is not a reading or a missing key: a demand may
    /// not poll early while held.
    held: bool,
    last_attempt_ms: Option<i64>,
    /// Consecutive transient failures, which set the backoff.
    failures: u32,
    /// The key-source fingerprint at the last refusal: a changed source is
    /// tried at once.
    refused_fingerprint: Option<u64>,
}

impl Schedule {
    pub(crate) fn new() -> Self {
        Self {
            next_ms: 0,
            held: false,
            last_attempt_ms: None,
            failures: 0,
            refused_fingerprint: None,
        }
    }

    /// Whether a poll may run now.
    pub(crate) fn due(&self, now_ms: i64, fingerprint: u64, demand: bool) -> bool {
        if let Some(refused) = self.refused_fingerprint {
            return fingerprint != refused || now_ms >= self.next_ms;
        }
        if now_ms >= self.next_ms {
            return true;
        }
        demand
            && !self.held
            && self
                .last_attempt_ms
                .is_some_and(|last| now_ms >= last + DEMAND_GAP_MS)
    }

    /// Records one attempt and sets the wait that follows it.
    pub(crate) fn record(&mut self, attempt: Attempt, now_ms: i64, fingerprint: u64) {
        self.last_attempt_ms = Some(now_ms);
        self.held = !matches!(attempt, Attempt::Reading | Attempt::NoKey);
        self.refused_fingerprint = None;
        match attempt {
            Attempt::Reading => {
                self.failures = 0;
                self.wait(now_ms, SUCCESS_INTERVAL_MS);
            }
            Attempt::NoKey => self.wait(now_ms, SUCCESS_INTERVAL_MS),
            Attempt::Refused => {
                self.refused_fingerprint = Some(fingerprint);
                self.wait(now_ms, REFUSED_HOLD_MS);
            }
            Attempt::Throttled {
                retry_after_secs: Some(seconds),
            } => {
                self.failures = self.failures.saturating_add(1);
                let named = i64::try_from(seconds.saturating_mul(1000)).unwrap_or(i64::MAX);
                self.wait(now_ms, named.clamp(SUCCESS_INTERVAL_MS, BACKOFF_CAP_MS));
            }
            Attempt::Throttled {
                retry_after_secs: None,
            }
            | Attempt::Transient => {
                self.failures = self.failures.saturating_add(1);
                self.wait(now_ms, self.backoff());
            }
            Attempt::Malformed => self.wait(now_ms, MALFORMED_HOLD_MS),
        }
    }

    /// The wait the transient failures so far call for: two minutes, then four,
    /// eight and so on, up to the cap.
    fn backoff(&self) -> i64 {
        let doublings = self.failures.saturating_sub(1).min(10);
        (FIRST_BACKOFF_MS << doublings).min(BACKOFF_CAP_MS)
    }

    fn wait(&mut self, now_ms: i64, delay_ms: i64) {
        self.next_ms = now_ms.saturating_add(delay_ms);
    }

    /// How long the thread may sleep before it looks again: until the next
    /// poll is due, and never longer than [`POLL_INTERVAL`], so a changed key
    /// source is noticed within a minute.
    pub(crate) fn sleep_for(&self, now_ms: i64) -> Duration {
        let until = Duration::from_millis(u64::try_from((self.next_ms - now_ms).max(0)).unwrap_or(0));
        until.clamp(Duration::from_secs(1), POLL_INTERVAL)
    }
}

#[cfg_attr(test, allow(dead_code))]
fn run() {
    let mut schedule = Schedule::new();
    let mut last_source = None;
    loop {
        let demand = take_demand();
        let now = now_ms();
        if now <= DEMAND_UNTIL_MS.load(Ordering::SeqCst) {
            pass(
                &mut schedule,
                &mut last_source,
                now,
                demand,
                key_fingerprint_from_process(),
                opencode_key_from_process,
                |key, observed_at_ms| fetch(&OpencodeGo, key, observed_at_ms),
            );
        }
        wait_for_demand(schedule.sleep_for(now_ms()));
    }
}

/// One pass of the loop: asks the source when the schedule says it is due and
/// records what came of it. The key is looked up only when a request is due,
/// and the log names the key source once per change, never per poll. Returns
/// whether a request was made.
fn pass<S, F>(
    schedule: &mut Schedule,
    last_source: &mut Option<KeySource>,
    now: i64,
    demand: bool,
    fingerprint: u64,
    source: S,
    fetch: F,
) -> bool
where
    S: FnOnce() -> (Option<ApiKey>, KeySource),
    F: FnOnce(&ApiKey, i64) -> Result<SessionEvent, QuotaError>,
{
    if !schedule.due(now, fingerprint, demand) {
        return false;
    }
    let (key, key_source) = source();
    if *last_source != Some(key_source) {
        eprintln!("{}", key_source.log_line());
        *last_source = Some(key_source);
    }
    let attempt = poll_once(key.as_ref(), now, fetch);
    schedule.record(attempt, now, fingerprint);
    true
}

/// What one fetch came to and, when it gave one, the reading. No key means no
/// request at all.
pub(crate) fn attempt_for<F>(
    key: Option<&ApiKey>,
    observed_at_ms: i64,
    fetch: F,
) -> (Attempt, Option<SessionEvent>)
where
    F: FnOnce(&ApiKey, i64) -> Result<SessionEvent, QuotaError>,
{
    let Some(key) = key else {
        return (Attempt::NoKey, None);
    };
    match fetch(key, observed_at_ms) {
        Ok(frame) => (Attempt::Reading, Some(frame)),
        Err(error) => (Attempt::from(error), None),
    }
}

/// One poll attempt, whatever its outcome: the watch list is pruned first, so a
/// run of failures cannot leave ended sessions in it. A reading is stored for
/// the cache and handed to the attached sessions; a failed poll stores nothing.
pub(crate) fn poll_once<F>(key: Option<&ApiKey>, observed_at_ms: i64, fetch: F) -> Attempt
where
    F: FnOnce(&ApiKey, i64) -> Result<SessionEvent, QuotaError>,
{
    let (attempt, frame) = attempt_for(key, observed_at_ms, fetch);
    quota_live::prune_dead();
    if let Some(frame) = &frame {
        plan_usage_cache::note_live(frame);
        quota_live::publish(frame);
    }
    attempt
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

#[cfg(test)]
#[path = "quota_poller_tests.rs"]
mod tests;
