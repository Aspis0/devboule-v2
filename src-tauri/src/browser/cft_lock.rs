//! One exclusive OS file lock per browser resource, held for its lifetime.
//!
//! A second Devboule process — or a concurrent install in this one — gets a
//! clear refusal instead of sharing a profile, a marker, or a staging
//! directory with a browser it did not start. Advisory locking is enough:
//! every opener of these paths goes through this module.

#![cfg_attr(not(test), allow(dead_code))]

use std::fs::{File, OpenOptions};
use std::path::Path;

use fs2::FileExt;

/// The open handle behind an exclusive lock. Dropping closes the handle,
/// which releases the lock on every platform.
pub struct ResourceLock {
    _file: File,
}

impl ResourceLock {
    /// Take the exclusive lock on `path`, creating the file if needed.
    /// Fails at once if another process or thread holds it — never blocks.
    pub fn acquire(path: &Path) -> Result<Self, String> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            // The file is a lock token, not storage: opening it must not
            // modify whatever an earlier owner left there.
            .truncate(false)
            .open(path)
            .map_err(|error| format!("{}: {error}", path.display()))?;
        file.try_lock_exclusive().map_err(|_| {
            format!(
                "{} is held by another Devboule process; refusing to share it",
                path.display()
            )
        })?;
        Ok(ResourceLock { _file: file })
    }
}

#[cfg(test)]
#[path = "cft_lock_tests.rs"]
mod tests;
