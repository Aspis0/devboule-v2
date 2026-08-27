//! Per-connection outbound path.
//!
//! RPC replies are a small bounded queue. Session output is **not** copied
//! into that queue: the 256 KiB ring is the live buffer and the writer
//! thread pulls it. A slow client therefore cannot turn the daemon into an
//! unbounded allocator. If it lags past the ring, it misses the same bytes
//! a late attach would miss — honest, and the same cap M2 already had.
//!
//! Coalescing happens *before* seq is assigned (see `session.rs`). Merging
//! already-sequenced frames would punch holes in `seq` and fail the flood
//! correctness test. When this RPC queue fills, the connection thread
//! waits: RPCs are rare and blocking them does not stall the PTY reader.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use devboule_protocol::DaemonMessage;

/// Maximum unread RPC/control frames per connection.
#[allow(dead_code)]
pub const OUTBOUND_RPC_CAP: usize = 64;

#[allow(dead_code)]
pub struct ConnOut {
    inner: Mutex<Inner>,
    cvar: Condvar,
    closed: AtomicBool,
}

#[allow(dead_code)]
struct Inner {
    rpc: VecDeque<DaemonMessage>,
}

#[allow(dead_code)]
impl ConnOut {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            inner: Mutex::new(Inner {
                rpc: VecDeque::new(),
            }),
            cvar: Condvar::new(),
            closed: AtomicBool::new(false),
        })
    }

    /// Enqueue an RPC reply. Blocks if the queue is at cap, unless closed.
    pub fn push_rpc(&self, message: DaemonMessage) {
        if self.closed.load(Ordering::SeqCst) {
            return;
        }
        let mut inner = self.inner.lock().unwrap_or_else(|err| err.into_inner());
        while inner.rpc.len() >= OUTBOUND_RPC_CAP && !self.closed.load(Ordering::SeqCst) {
            inner = self.cvar.wait(inner).unwrap_or_else(|err| err.into_inner());
        }
        if self.closed.load(Ordering::SeqCst) {
            return;
        }
        inner.rpc.push_back(message);
        self.cvar.notify_all();
    }

    pub fn pop_rpc(&self) -> Option<DaemonMessage> {
        let mut inner = self.inner.lock().unwrap_or_else(|err| err.into_inner());
        let message = inner.rpc.pop_front();
        if message.is_some() {
            self.cvar.notify_all();
        }
        message
    }

    /// Wake the writer so it will pull session output even when no RPC is queued.
    pub fn notify(&self) {
        self.cvar.notify_all();
    }

    pub fn close(&self) {
        self.closed.store(true, Ordering::SeqCst);
        self.cvar.notify_all();
    }

    /// Wait for RPC, a notify, or `timeout`. Returns false if closed and idle.
    pub fn wait(&self, timeout: Duration) -> bool {
        let inner = self.inner.lock().unwrap_or_else(|err| err.into_inner());
        if !inner.rpc.is_empty() {
            return true;
        }
        if self.closed.load(Ordering::SeqCst) {
            return false;
        }
        drop(
            self.cvar
                .wait_timeout(inner, timeout)
                .unwrap_or_else(|err| err.into_inner()),
        );
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rpc_queue_wakes_and_preserves_order() {
        let out = ConnOut::new();
        out.push_rpc(DaemonMessage::Ok { id: 1 });
        out.push_rpc(DaemonMessage::Ok { id: 2 });
        assert!(matches!(out.pop_rpc(), Some(DaemonMessage::Ok { id: 1 })));
        assert!(matches!(out.pop_rpc(), Some(DaemonMessage::Ok { id: 2 })));
        assert!(out.pop_rpc().is_none());
    }

    #[test]
    fn close_unblocks_wait() {
        let out = ConnOut::new();
        out.close();
        assert!(!out.wait(Duration::from_millis(50)));
    }
}
