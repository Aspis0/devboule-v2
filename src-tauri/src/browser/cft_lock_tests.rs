//! What the lock promises: one owner, a clear refusal, release on drop.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use super::*;

#[test]
fn a_second_acquire_is_refused_while_the_first_is_held() {
    let dir = tempfile::tempdir().expect("a scratch dir");
    let path = dir.path().join("owner.lock");
    let first = ResourceLock::acquire(&path).expect("the first owner takes it");
    let refused = ResourceLock::acquire(&path);
    assert!(
        refused.is_err(),
        "a second browser on one profile must be refused"
    );
    drop(first);
    ResourceLock::acquire(&path).expect("release on drop hands it over");
}

#[test]
fn concurrent_installers_never_overlap_on_one_lock() {
    let dir = tempfile::tempdir().expect("a scratch dir");
    let path = dir.path().join("install.lock");
    let inside = std::sync::Arc::new(AtomicUsize::new(0));
    let total = std::sync::Arc::new(AtomicUsize::new(0));
    let mut handles = Vec::new();
    for _ in 0..8 {
        let path = path.clone();
        let inside = std::sync::Arc::clone(&inside);
        let total = std::sync::Arc::clone(&total);
        handles.push(std::thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(30);
            while Instant::now() < deadline {
                if let Ok(_held) = ResourceLock::acquire(&path) {
                    let previous = inside.fetch_add(1, Ordering::SeqCst);
                    assert_eq!(previous, 0, "two installers held one lock");
                    std::thread::sleep(Duration::from_millis(2));
                    inside.fetch_sub(1, Ordering::SeqCst);
                    total.fetch_add(1, Ordering::SeqCst);
                    return;
                }
                std::thread::sleep(Duration::from_millis(1));
            }
            panic!("an installer waited 30s for a 2ms critical section");
        }));
    }
    for handle in handles {
        handle.join().expect("no installer thread panics");
    }
    assert_eq!(total.load(Ordering::SeqCst), 8);
}
