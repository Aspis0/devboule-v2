//! The time one agent command has, from being taken off the queue to being
//! answered. Every wait a command makes — a CDP call, the settle, a poll, the
//! wait for a tab's lock — is cut to what is left of it, so the answer reaches
//! the daemon before the daemon gives up on it.

use std::time::{Duration, Instant};

/// How long one command may take. Under the daemon's 15 s, so the answer has
/// time to travel, and over the longest wait a caller may ask for.
pub const COMMAND_BUDGET: Duration = Duration::from_secs(13);

/// What a command must leave for its own answer: the settle (3 s, the cap in
/// `cdp_events`) and the two tree reads a delta is built from, plus slack.
/// Taken off a wait, so a `wait_for` that uses all the time it is given still
/// answers inside the budget instead of being cut off by the daemon's.
const ANSWER_RESERVE: Duration = Duration::from_secs(6);

#[derive(Debug, Clone, Copy)]
pub struct Deadline(Instant);

impl Deadline {
    pub fn from_now() -> Self {
        Deadline(Instant::now() + COMMAND_BUDGET)
    }

    /// A deadline of the caller's choosing, for a test that is not about time.
    #[cfg(test)]
    pub fn in_(budget: Duration) -> Self {
        Deadline(Instant::now() + budget)
    }

    /// What is left of the whole budget. Zero once it is spent, never negative.
    pub fn left(&self) -> Duration {
        self.0.saturating_duration_since(Instant::now())
    }

    /// How long a command may still wait: what is left of the budget, less what
    /// its answer needs.
    pub fn wait_for(&self) -> Duration {
        self.left().saturating_sub(ANSWER_RESERVE)
    }
}

#[cfg(test)]
#[path = "deadline_tests.rs"]
mod tests;
