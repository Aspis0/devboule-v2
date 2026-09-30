//! The per-repository-root write mutexes: stage, unstage, discard, commit
//! and the rename's `git mv` — this daemon's index-touching acts — each take
//! their root's lock, so two of this daemon's writers never cross and meet
//! `index.lock`. Keyed by the resolved root rather than a workspace id, so
//! two ids naming one checkout share one lock. The guard retires its entry
//! on drop, so a root written long ago does not outlive the daemon in the
//! map.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// The registry `ServerState` owns: one entry per root some act is writing
/// or has written since the last retirement.
pub(super) type GitWriteLocks = Mutex<HashMap<PathBuf, Arc<Mutex<()>>>>;

/// Take this root's write mutex: fetched (created on first use) under the
/// map lock, then handed back as a guard — the map lock itself is never held
/// across an act.
pub(super) fn acquire<'a>(locks: &'a GitWriteLocks, root: &Path) -> GitWriteLock<'a> {
    let mut map = locks.lock().unwrap_or_else(|error| error.into_inner());
    let lock = Arc::clone(map.entry(root.to_path_buf()).or_default());
    drop(map);
    GitWriteLock {
        locks,
        root: root.to_path_buf(),
        lock,
    }
}

pub(crate) struct GitWriteLock<'a> {
    locks: &'a GitWriteLocks,
    root: PathBuf,
    lock: Arc<Mutex<()>>,
}

impl GitWriteLock<'_> {
    /// The root's mutex, held across the act. A poisoned guard is unwrapped:
    /// a panicking act must not stop the next write for the life of the
    /// process.
    pub(crate) fn lock(&self) -> std::sync::MutexGuard<'_, ()> {
        self.lock.lock().unwrap_or_else(|error| error.into_inner())
    }
}

impl Drop for GitWriteLock<'_> {
    fn drop(&mut self) {
        // Under the map lock, so no concurrent acquire can slip between the
        // count and the removal. The guard's own clone is still alive in a
        // dropping struct: two means the map's entry plus this guard, and
        // every other holder or cloner makes it larger — such an entry is
        // never removed.
        let mut map = self.locks.lock().unwrap_or_else(|error| error.into_inner());
        if Arc::strong_count(&self.lock) == 2 {
            map.remove(&self.root);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    use super::super::ServerState;

    fn held_roots(state: &ServerState) -> usize {
        state.git_write_lock_roots()
    }

    #[test]
    fn locking_two_roots_and_dropping_both_leaves_an_empty_map() {
        let state = ServerState::new("git-write-lock-prune".to_string());
        {
            let _first = state.git_write_lock(Path::new("/r/first"));
            let _second = state.git_write_lock(Path::new("/r/second"));
            assert_eq!(
                held_roots(&state),
                2,
                "two roots hold two entries while both guards live"
            );
        }
        assert_eq!(
            held_roots(&state),
            0,
            "the last guard out retires its entry"
        );
    }

    #[test]
    fn a_held_entry_survives_another_roots_prune() {
        let state = ServerState::new("git-write-lock-held".to_string());
        let held = state.git_write_lock(Path::new("/r/held"));
        {
            let _transient = state.git_write_lock(Path::new("/r/transient"));
        }
        assert_eq!(
            held_roots(&state),
            1,
            "the dropped root's entry is gone, the held one stays"
        );
        drop(held);
        assert_eq!(held_roots(&state), 0, "its own drop retires it");
    }

    #[test]
    fn two_concurrent_holders_of_the_same_root_still_serialize() {
        let state = Arc::new(ServerState::new("git-write-lock-serial".to_string()));
        let root = Path::new("/r/shared");
        let inside = Arc::new(AtomicUsize::new(0));
        let max_inside = Arc::new(AtomicUsize::new(0));
        let holders: Vec<_> = (0..2)
            .map(|_| {
                let state = Arc::clone(&state);
                let inside = Arc::clone(&inside);
                let max_inside = Arc::clone(&max_inside);
                std::thread::spawn(move || {
                    let lock = state.git_write_lock(root);
                    let _guard = lock.lock();
                    let now = inside.fetch_add(1, Ordering::SeqCst) + 1;
                    max_inside.fetch_max(now, Ordering::SeqCst);
                    std::thread::sleep(std::time::Duration::from_millis(50));
                    inside.fetch_sub(1, Ordering::SeqCst);
                })
            })
            .collect();
        for holder in holders {
            holder.join().expect("lock holder");
        }
        assert_eq!(
            max_inside.load(Ordering::SeqCst),
            1,
            "one writer at a time on the shared root"
        );
        assert_eq!(
            held_roots(&state),
            0,
            "the shared entry is retired by the last holder out"
        );
    }
}
