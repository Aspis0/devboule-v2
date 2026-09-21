//! A reservation whose release is owed: taken on one statement, released on
//! another, with a `Drop` for everything in between.
//!
//! Three accounts in the daemon are held that way — the client slot of both
//! accept paths, and an agent message's brake slot — and each one is taken and
//! released on plain statements. A panic in between skips the release: the
//! count stays up, and for `clients` that means `client_disconnected` never
//! sees zero again, so the idle exit never arms for the life of that daemon.

/// One owed release.
///
/// [`ReleaseGuard::armed`] takes the closure that performs the release; the
/// path that knows the outcome calls [`ReleaseGuard::release`] with it, and a
/// `Drop` that never saw that call reports the operation as incomplete.
///
/// The flag the closure receives is the caller's answer to "did the covered
/// operation finish?": the explicit release passes what it knows, `Drop` passes
/// `false`, because an operation that never returned is not a finished one.
#[must_use = "a guard dropped unarmed releases as an incomplete operation"]
pub(crate) struct ReleaseGuard<F: FnOnce(bool)> {
    release: Option<F>,
}

impl<F: FnOnce(bool)> ReleaseGuard<F> {
    pub(crate) fn armed(release: F) -> Self {
        Self {
            release: Some(release),
        }
    }

    /// Run the release now rather than on `Drop`.
    pub(crate) fn release(mut self, completed: bool) {
        self.fire(completed);
    }

    fn fire(&mut self, completed: bool) {
        if let Some(release) = self.release.take() {
            release(completed);
        }
    }
}

impl<F: FnOnce(bool)> Drop for ReleaseGuard<F> {
    fn drop(&mut self) {
        self.fire(false);
    }
}

#[cfg(test)]
#[path = "release_guard_tests.rs"]
mod tests;
