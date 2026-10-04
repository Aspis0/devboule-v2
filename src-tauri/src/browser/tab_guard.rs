//! Who may act on a tab's page while someone else is: an agent's acting command
//! and the pane's own gestures (present, park, close) take the same per-tab lock,
//! so a click is never measured against a layout the pane has just replaced.
//!
//! The two sides wait differently, on purpose. An agent's command waits for the
//! tab until its own deadline and then says the tab is busy. The pane is a
//! person's gesture and wins: it waits a moment for an action in flight and then
//! goes ahead without the lock, because a window that stops answering for the
//! length of an agent's command is worse than a command that fails midway.
//! Neither wait blocks a thread: both poll the lock from async code.

use std::sync::Arc;
use std::time::{Duration, Instant};

use devboule_protocol::BrowserError;

use super::cdp_events;
use super::commands::{host_error, tab_not_found};
use super::deadline::Deadline;
use super::registry::{BrowserRegistry, TabGuard};

/// How often a waiter looks again.
const POLL: Duration = Duration::from_millis(25);

/// How long a pane gesture waits for an agent's action on its tab before it
/// goes ahead anyway.
pub const PANE_WAIT: Duration = Duration::from_millis(1_500);

/// Take the tab for one acting command, waiting while another holds it.
pub async fn hold(
    registry: &BrowserRegistry,
    browser_id: &str,
    deadline: Deadline,
) -> Result<impl Send, BrowserError> {
    let guard = registry
        .guard_of(browser_id)
        .ok_or_else(|| tab_not_found(browser_id))?;
    loop {
        if let Some(held) = try_take(&guard) {
            return Ok(held);
        }
        let left = deadline.left();
        if left.is_zero() {
            return Err(host_error(
                "This tab is busy with another action; try again in a moment.",
            ));
        }
        cdp_events::nap(POLL.min(left)).await;
    }
}

/// Take the tab for a pane gesture. None when the tab is not there, and also
/// when an agent's action still holds it after `wait`: the gesture proceeds.
pub async fn hold_for_pane(
    registry: &BrowserRegistry,
    browser_id: &str,
    wait: Duration,
) -> Option<impl Send> {
    let guard = registry.guard_of(browser_id)?;
    let until = Instant::now() + wait;
    loop {
        if let Some(held) = try_take(&guard) {
            return Some(held);
        }
        let left = until.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return None;
        }
        cdp_events::nap(POLL.min(left)).await;
    }
}

fn try_take(guard: &TabGuard) -> Option<impl Send> {
    Arc::clone(guard).try_lock_owned().ok()
}

#[cfg(test)]
#[path = "tab_guard_tests.rs"]
mod tests;
