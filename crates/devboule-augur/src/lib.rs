//! Review findings as a plain library. The Polis plugin backend links this;
//! the app does not.
//!
//! A scan is work on someone else's repository and runs because a caller
//! asked, not because a process started. Adding a kind of review is adding
//! one detector file and registering it. Nothing here talks to Tauri, and
//! nothing here assumes a detector runs in process or is cheap.

mod detector;
mod detectors;
mod error;
mod exchange;
mod finding;
mod ledger;
mod ruleset;
#[cfg(test)]
mod tokens;

pub use detector::{Context, Cost, Detector, FailedDetector, Registry, Review};
pub use error::Error;
pub use exchange::to_sarif;
pub use finding::{Finding, FindingId, Location, Severity};
pub use ledger::Ledger;
