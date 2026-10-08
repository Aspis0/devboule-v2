//! Hands each successful OpenCode Go reading to the attached Pi sessions that
//! are on an OpenCode model, as a transient event: never journaled, never put
//! in a backlog, so a replay cannot hold a reading. The model is read at
//! publish time, so a session that switches off OpenCode stops receiving
//! readings at once.

use std::sync::{Arc, Mutex, Weak};

use devboule_protocol::SessionEvent;

use crate::session::SessionRuntime;

/// Pi's name for the OpenCode model provider: the one the meter follows.
const OPENCODE_MODEL_PROVIDER: &str = "opencode";

/// Every Pi runtime that has stored a manifest, held weakly: a session that
/// ends drops out on the next call.
static PI_RUNTIMES: Mutex<Vec<Weak<SessionRuntime>>> = Mutex::new(Vec::new());

/// Remembers a Pi runtime so a reading can reach it. Called where the runtime's
/// first manifest is stored; a runtime already remembered is not added twice.
pub(crate) fn watch(runtime: &Arc<SessionRuntime>) {
    let Ok(mut runtimes) = PI_RUNTIMES.lock() else {
        return;
    };
    prune(&mut runtimes);
    let known = runtimes
        .iter()
        .any(|weak| std::ptr::eq(weak.as_ptr(), Arc::as_ptr(runtime)));
    if !known {
        runtimes.push(Arc::downgrade(runtime));
    }
}

/// Sends one reading to every watched Pi session that is on an OpenCode model.
pub(crate) fn publish(reading: &SessionEvent) {
    publish_to(&watched_runtimes(), reading);
}

/// Whether a watched Pi session is on an OpenCode model with a client attached:
/// the only condition under which the meter is polled.
pub(crate) fn opencode_attached() -> bool {
    watched_runtimes()
        .iter()
        .any(|runtime| serves_attached_opencode(runtime))
}

/// Whether this session is on an OpenCode model and a client is attached to it.
pub(crate) fn serves_attached_opencode(runtime: &SessionRuntime) -> bool {
    runtime.current_model_provider_id().as_deref() == Some(OPENCODE_MODEL_PROVIDER)
        && runtime.has_observers()
}

/// The live watched sessions, copied out before any session is touched, so no
/// session lock is taken while the watch list is held.
fn watched_runtimes() -> Vec<Arc<SessionRuntime>> {
    let Ok(mut watched) = PI_RUNTIMES.lock() else {
        return Vec::new();
    };
    prune(&mut watched);
    watched.iter().filter_map(Weak::upgrade).collect()
}

fn publish_to(runtimes: &[Arc<SessionRuntime>], reading: &SessionEvent) {
    for runtime in runtimes {
        if runtime.current_model_provider_id().as_deref() == Some(OPENCODE_MODEL_PROVIDER) {
            runtime.publish_plan_usage_live(reading.clone());
        }
    }
}

/// Drops the entries of sessions that have ended. The poll calls it after every
/// attempt, whatever the outcome, so a run of failed polls cannot let the list
/// grow with entries nothing will ever prune.
pub(crate) fn prune_dead() {
    if let Ok(mut runtimes) = PI_RUNTIMES.lock() {
        prune(&mut runtimes);
    }
}

fn prune(runtimes: &mut Vec<Weak<SessionRuntime>>) {
    runtimes.retain(|weak| weak.strong_count() > 0);
}

/// Whether the watch list still holds this entry. Tests only.
#[cfg(test)]
pub(crate) fn holds(runtime: &Weak<SessionRuntime>) -> bool {
    PI_RUNTIMES
        .lock()
        .map(|runtimes| runtimes.iter().any(|weak| Weak::ptr_eq(weak, runtime)))
        .unwrap_or(false)
}

#[cfg(test)]
#[path = "quota_live_tests.rs"]
mod tests;
