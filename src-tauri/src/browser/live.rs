//! What is true of a tab's geometry right now, read at the moment it is used.
//!
//! Two facts decide whether a measuring command may touch a page's metrics: is
//! the tab parked, and is a device-metrics override on it. Both change from
//! two sides at once — the pane presenting and parking, an agent's command
//! overriding — so they are atomics behind a handle the command keeps, never a
//! copy taken when it started.

use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Debug)]
pub struct Live {
    parked: AtomicBool,
    overridden: AtomicBool,
}

/// A tab starts parked, as every child webview is created, with no override.
impl Default for Live {
    fn default() -> Self {
        Live {
            parked: AtomicBool::new(true),
            overridden: AtomicBool::new(false),
        }
    }
}

impl Live {
    pub fn parked(&self) -> bool {
        self.parked.load(Ordering::SeqCst)
    }

    pub fn set_parked(&self, parked: bool) {
        self.parked.store(parked, Ordering::SeqCst);
    }

    /// Whether a device-metrics override is on the page.
    #[cfg(test)]
    pub fn overridden(&self) -> bool {
        self.overridden.load(Ordering::SeqCst)
    }

    pub fn set_overridden(&self, on: bool) {
        self.overridden.store(on, Ordering::SeqCst);
    }

    /// Whether an override was on, and from now on it is not: the pane, which
    /// has just presented the page, is the one that clears it.
    pub fn take_overridden(&self) -> bool {
        self.overridden.swap(false, Ordering::SeqCst)
    }
}

#[cfg(test)]
#[path = "live_tests.rs"]
mod tests;
