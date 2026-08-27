//! Per-connection outbound path.
//!
//! Session output is **not** copied into an outbound queue: the 256 KiB ring
//! is the live buffer and the writer thread pulls it. A slow client therefore
//! cannot turn the daemon into an unbounded allocator. If it lags past the
//! ring, it misses the same bytes a late attach would miss — honest, and the
//! same cap M2 already had.
//!
//! Coalescing happens *before* seq is assigned (see `session.rs`). Merging
//! already-sequenced frames would punch holes in `seq` and fail the flood
//! correctness test. The connection writer waits on a generation-aware
//! condition variable when the ring has no output; PTY readers only publish
//! and notify.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

pub struct ConnOut {
    inner: Mutex<Inner>,
    cvar: Condvar,
    closed: AtomicBool,
}

struct Inner {
    wake_generation: u64,
}

impl ConnOut {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            inner: Mutex::new(Inner { wake_generation: 0 }),
            cvar: Condvar::new(),
            closed: AtomicBool::new(false),
        })
    }

    /// Wake the writer so it will pull session output even when no RPC is queued.
    pub fn notify(&self) {
        let mut inner = self.inner.lock().unwrap_or_else(|err| err.into_inner());
        inner.wake_generation = inner.wake_generation.wrapping_add(1);
        self.cvar.notify_all();
    }

    /// Return the notification state observed by the writer before it drains
    /// the other outbound sources. A later wait must compare against this
    /// value, otherwise a notify racing between the drain and the wait can be
    /// mistaken for the new baseline.
    pub fn wake_generation(&self) -> u64 {
        self.inner
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .wake_generation
    }

    pub fn close(&self) {
        self.closed.store(true, Ordering::SeqCst);
        let mut inner = self.inner.lock().unwrap_or_else(|err| err.into_inner());
        inner.wake_generation = inner.wake_generation.wrapping_add(1);
        self.cvar.notify_all();
    }

    /// Wait until the outbound state changes after `generation`, or until the
    /// optional one-shot deadline. The caller captures `generation` before it
    /// drains session events and requests. This lock-protected re-check makes
    /// the state authoritative: a notification that races with the drain is
    /// observed instead of being signalled into a wait that starts afterward.
    pub fn wait_for_notify_since(&self, generation: u64, timeout: Option<Duration>) -> bool {
        let mut inner = self.inner.lock().unwrap_or_else(|err| err.into_inner());
        while !self.closed.load(Ordering::SeqCst) && inner.wake_generation == generation {
            match timeout {
                Some(timeout) => {
                    let (guard, result) = self
                        .cvar
                        .wait_timeout(inner, timeout)
                        .unwrap_or_else(|err| err.into_inner());
                    inner = guard;
                    if result.timed_out() {
                        return true;
                    }
                }
                None => {
                    inner = self.cvar.wait(inner).unwrap_or_else(|err| err.into_inner());
                }
            }
        }
        !self.closed.load(Ordering::SeqCst)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn close_unblocks_notification_wait() {
        let out = ConnOut::new();
        let generation = out.wake_generation();
        out.close();
        assert!(!out.wait_for_notify_since(generation, None));
    }

    #[test]
    fn notification_before_wait_is_not_lost() {
        let out = ConnOut::new();
        let generation = out.wake_generation();
        out.notify();
        let started = std::time::Instant::now();
        assert!(out.wait_for_notify_since(generation, Some(Duration::from_millis(50))));
        assert!(
            started.elapsed() < Duration::from_millis(25),
            "notification was lost; wait took {:?}",
            started.elapsed()
        );
    }
}
