//! The supervisor's crash-loop brake: back off when the daemon dies young
//! repeatedly, stay fast when it dies old.
//!
//! A reconnect and a crash loop are not the same event and must not get the
//! same answer. A connection lost after the daemon served for a while is the
//! normal handoff back to the reconnect path and keeps today's speed; a
//! connection lost (or refused) soon after each spawn, over and over, is a
//! crash loop and gets an exponential delay with a ceiling. A connected
//! phase that lasted at least [`HEALTHY_CONNECTED`] is the definition of
//! *healthy*: it resets the count, so an outage that has ended cannot keep
//! slowing the app down hours later.

use std::time::Duration;

/// Fast failures tolerated before the brake delays anything: a daemon that
/// comes up on the third try is not made slower than it is today.
pub(crate) const FAST_FAILURE_TOLERANCE: u32 = 3;

/// The first backed-off delay; it doubles with every further fast failure.
pub(crate) const BACKOFF_BASE: Duration = Duration::from_secs(2);

/// The ceiling: however long the crash loop, the supervisor never waits
/// longer than this between attempts.
pub(crate) const MAX_BACKOFF: Duration = Duration::from_secs(60);

/// A connected phase that lasted at least this long counts as a genuinely
/// healthy connection and resets the brake.
pub(crate) const HEALTHY_CONNECTED: Duration = Duration::from_secs(10);

/// Counts consecutive fast failures and turns them into a delay.
#[derive(Default)]
pub(crate) struct CrashLoopBrake {
    consecutive_fast_failures: u32,
}

impl CrashLoopBrake {
    pub(crate) fn observe_fast_failure(&mut self) {
        self.consecutive_fast_failures = self.consecutive_fast_failures.saturating_add(1);
    }

    pub(crate) fn reset(&mut self) {
        self.consecutive_fast_failures = 0;
    }

    /// The delay this fast failure should wait, or `None` while the failures
    /// are still inside the tolerance and the caller keeps today's speed.
    pub(crate) fn backoff_delay(&self) -> Option<Duration> {
        let past_tolerance = self
            .consecutive_fast_failures
            .checked_sub(FAST_FAILURE_TOLERANCE)?;
        let doublings = past_tolerance.checked_sub(1)?.min(31);
        let delay = BACKOFF_BASE
            .checked_mul(1u32.checked_shl(doublings)?)
            .unwrap_or(MAX_BACKOFF);
        Some(delay.min(MAX_BACKOFF))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn delays_after(count: u32) -> Option<Duration> {
        let mut brake = CrashLoopBrake::default();
        (0..count).for_each(|_| brake.observe_fast_failure());
        brake.backoff_delay()
    }

    #[test]
    fn the_first_failures_keep_todays_fast_path() {
        assert_eq!(delays_after(1), None);
        assert_eq!(delays_after(2), None);
        assert_eq!(delays_after(3), None, "the tolerance is not made slower");
    }

    /// M-a's unit anchor: past the tolerance the delay is exponential.
    #[test]
    fn the_delay_grows_across_repeated_fast_failures() {
        assert_eq!(delays_after(4), Some(Duration::from_secs(2)));
        assert_eq!(delays_after(5), Some(Duration::from_secs(4)));
        assert_eq!(delays_after(6), Some(Duration::from_secs(8)));
        assert_eq!(delays_after(7), Some(Duration::from_secs(16)));
        assert_eq!(delays_after(8), Some(Duration::from_secs(32)));
    }

    /// M-d: however long the crash loop, the delay never passes the ceiling.
    #[test]
    fn the_delay_is_capped() {
        assert_eq!(delays_after(9), Some(MAX_BACKOFF));
        assert_eq!(delays_after(10), Some(MAX_BACKOFF));
        assert_eq!(delays_after(100), Some(MAX_BACKOFF));
        assert_eq!(delays_after(u32::MAX), Some(MAX_BACKOFF));
    }

    /// M-b's unit anchor: a reset restores the fast path.
    #[test]
    fn a_reset_restores_the_fast_path() {
        let mut brake = CrashLoopBrake::default();
        (0..9).for_each(|_| brake.observe_fast_failure());
        assert_eq!(brake.backoff_delay(), Some(MAX_BACKOFF));
        brake.reset();
        assert_eq!(brake.backoff_delay(), None);
        brake.observe_fast_failure();
        brake.observe_fast_failure();
        brake.observe_fast_failure();
        assert_eq!(
            brake.backoff_delay(),
            None,
            "the first failures after a reset are inside the tolerance again"
        );
        brake.observe_fast_failure();
        assert_eq!(
            brake.backoff_delay(),
            Some(BACKOFF_BASE),
            "the schedule restarts at the base, not where it left off"
        );
    }
}
