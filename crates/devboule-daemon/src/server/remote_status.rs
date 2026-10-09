//! One per-peer coalescer for live status and snapshot updates.
//!
//! A remote host can change status faster than a person can read it, and every
//! change crossing the wire is a wake-up on the other machine. The coalescer
//! keeps at most one delivery per window with the latest value winning, and
//! always delivers the trailing value once the window has passed, so the final
//! state is never lost even though the intermediate ones are merged away.
//!
//! The window is a property of the link (`LinkTuning::status_window`); the
//! production value is one second. The core is time-parameterised so the rule
//! is testable without sleeping.

use std::time::{Duration, Instant};

/// Latest-wins coalescer with a trailing delivery.
pub(crate) struct StatusCoalescer<T> {
    pending: Option<T>,
    last_sent: Option<Instant>,
    window: Duration,
}

impl<T> StatusCoalescer<T> {
    pub(crate) fn new(window: Duration) -> Self {
        Self {
            pending: None,
            last_sent: None,
            window,
        }
    }

    /// Offer the latest value. Delivers it now when the window since the last
    /// delivery has passed; otherwise parks it as the trailing value and
    /// returns `None`.
    pub(crate) fn offer(&mut self, value: T, now: Instant) -> Option<T> {
        let due = match self.last_sent {
            Some(sent) => now.duration_since(sent) >= self.window,
            None => true,
        };
        if due {
            self.last_sent = Some(now);
            self.pending = None;
            return Some(value);
        }
        self.pending = Some(value);
        None
    }

    /// Whether a trailing value is parked, so a flusher knows to keep waking.
    pub(crate) fn has_pending(&self) -> bool {
        self.pending.is_some()
    }

    /// The trailing value, if the window has passed since the last delivery.
    pub(crate) fn tick(&mut self, now: Instant) -> Option<T> {
        if let Some(sent) = self.last_sent {
            if now.duration_since(sent) < self.window {
                return None;
            }
        }
        let trailing = self.pending.take();
        if trailing.is_some() {
            self.last_sent = Some(now);
        }
        trailing
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The battery rule: a burst of a hundred changes delivers the first one
    /// immediately and exactly one trailing value — the last — so the receiver
    /// is woken twice, not a hundred times, and the final state is correct.
    #[test]
    fn a_burst_of_a_hundred_statuses_produces_two_deliveries_and_the_last_state() {
        let mut coalescer = StatusCoalescer::new(Duration::from_secs(1));
        let start = Instant::now();
        let mut delivered = Vec::new();
        for value in 0..100 {
            if let Some(due) = coalescer.offer(value, start) {
                delivered.push(due);
            }
        }
        assert_eq!(delivered, vec![0], "one immediate delivery for the burst");
        assert_eq!(
            coalescer.tick(start + Duration::from_millis(999)),
            None,
            "the trailing value waits out the window"
        );
        assert_eq!(
            coalescer.tick(start + Duration::from_millis(1000)),
            Some(99),
            "the trailing value is the last state offered"
        );
        assert_eq!(coalescer.tick(start + Duration::from_millis(2000)), None);
        assert!(delivered.len() < 2, "at most two deliveries for the burst");
    }

    /// A change that arrives inside the window trails the one already
    /// delivered; a change after the window goes out at once.
    #[test]
    fn the_window_delivers_at_most_one_change_per_second() {
        let mut coalescer = StatusCoalescer::new(Duration::from_secs(1));
        let start = Instant::now();
        assert_eq!(coalescer.offer("a", start), Some("a"));
        assert_eq!(
            coalescer.offer("b", start + Duration::from_millis(400)),
            None
        );
        assert_eq!(
            coalescer.tick(start + Duration::from_millis(1000)),
            Some("b")
        );
        assert_eq!(
            coalescer.offer("c", start + Duration::from_millis(1200)),
            None
        );
        assert_eq!(
            coalescer.tick(start + Duration::from_millis(2000)),
            Some("c")
        );
        assert_eq!(
            coalescer.offer("d", start + Duration::from_millis(2500)),
            None,
            "a change inside the window still trails"
        );
        assert_eq!(
            coalescer.tick(start + Duration::from_millis(3000)),
            Some("d")
        );
    }
}
