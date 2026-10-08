//! Keeps OpenCode Go's usage reading current while a Pi session is on an
//! OpenCode model. One thread, started by the first demand, polls the source
//! about once a minute for as long as demand is recent. A poll that yields no
//! reading leaves the cached one as it was.

use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use devboule_protocol::SessionEvent;

use crate::plan_usage_cache;
use crate::quota_key::{opencode_key_from_process, ApiKey};
use crate::quota_opencode_go::OpencodeGo;
use crate::quota_source::{fetch, QuotaError};

#[cfg_attr(test, allow(dead_code))]
const POLL_INTERVAL: Duration = Duration::from_secs(60);
/// A demand keeps polling this long after it was last made.
const DEMAND_WINDOW_MS: i64 = 10 * 60 * 1000;

/// Until when polling is wanted, in Unix milliseconds.
static DEMAND_UNTIL_MS: AtomicI64 = AtomicI64::new(0);
/// Wakes the thread to poll now; set once, when the thread starts.
static WAKE: OnceLock<Mutex<Sender<()>>> = OnceLock::new();

/// A Pi session is on an OpenCode model, or was just attached to one: poll now,
/// and keep polling while the demand is recent.
pub(crate) fn note_demand() {
    DEMAND_UNTIL_MS.store(now_ms() + DEMAND_WINDOW_MS, Ordering::SeqCst);
    let wake = WAKE.get_or_init(|| {
        let (sender, receiver) = mpsc::channel();
        spawn_poller(receiver);
        Mutex::new(sender)
    });
    if let Ok(sender) = wake.lock() {
        let _ = sender.send(());
    }
}

/// The poll thread. A thread that cannot start leaves the sender with no
/// receiver: the sends above then do nothing and no reading is stored.
#[cfg(not(test))]
fn spawn_poller(receiver: Receiver<()>) {
    let _ = std::thread::Builder::new()
        .name("opencode-go-quota".to_string())
        .spawn(move || run(receiver));
}

/// Tests set the demand and never start the thread, so no test reads the
/// real environment or the person's auth file.
#[cfg(test)]
fn spawn_poller(receiver: Receiver<()>) {
    drop(receiver);
}

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

#[cfg_attr(test, allow(dead_code))]
fn run(wake: Receiver<()>) {
    loop {
        if now_ms() <= DEMAND_UNTIL_MS.load(Ordering::SeqCst) {
            let frame = reading_for(
                opencode_key_from_process().as_ref(),
                now_ms(),
                |key, observed_at_ms| fetch(&OpencodeGo, key, observed_at_ms),
            );
            if let Some(frame) = frame {
                plan_usage_cache::note_live(&frame);
            }
        }
        match wake.recv_timeout(POLL_INTERVAL) {
            Ok(()) | Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return,
        }
    }
}

/// One poll's outcome: the frame when there is a key and the source answers
/// with a reading, otherwise none. No key means no request at all.
pub(crate) fn reading_for<F>(
    key: Option<&ApiKey>,
    observed_at_ms: i64,
    fetch: F,
) -> Option<SessionEvent>
where
    F: FnOnce(&ApiKey, i64) -> Result<SessionEvent, QuotaError>,
{
    let key = key?;
    fetch(key, observed_at_ms).ok()
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
