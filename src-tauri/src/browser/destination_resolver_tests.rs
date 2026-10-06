//! The lookup pool's bounds: a hung resolver costs the caller its budget and
//! nothing more, and a flood of hosts is refused rather than queued without
//! end.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Condvar;
use std::time::Instant;

use super::*;

/// A resolver that answers only once the test opens its gate, standing in for
/// a lookup the OS never finishes.
struct Gated {
    open: Mutex<bool>,
    opened: Condvar,
    started: AtomicUsize,
}

impl Gated {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            open: Mutex::new(false),
            opened: Condvar::new(),
            started: AtomicUsize::new(0),
        })
    }

    fn release(&self) {
        *lock(&self.open) = true;
        self.opened.notify_all();
    }
}

impl Resolver for Gated {
    fn resolve(&self, _host: &str, _port: u16) -> Result<Vec<IpAddr>, String> {
        self.started.fetch_add(1, Ordering::SeqCst);
        let mut open = lock(&self.open);
        while !*open {
            open = self
                .opened
                .wait(open)
                .unwrap_or_else(|error| error.into_inner());
        }
        Ok(vec!["93.184.216.34".parse().expect("a fixture address")])
    }
}

#[test]
fn a_lookup_the_os_never_answers_is_given_up_on_at_the_budget() {
    let gate = Gated::new();
    let pool = LookupPool::start(Arc::clone(&gate) as Arc<dyn Resolver>);
    let began = Instant::now();
    let answer = pool.lookup("hung.test", 80);
    assert!(answer.is_err());
    assert!(
        began.elapsed() < Duration::from_secs(2),
        "the wait is bounded"
    );
    gate.release();
}

#[test]
fn more_lookups_than_the_pool_can_hold_are_refused_at_once() {
    let gate = Gated::new();
    let pool = Arc::new(LookupPool::start(Arc::clone(&gate) as Arc<dyn Resolver>));
    let refused_full = Arc::new(AtomicUsize::new(0));
    let callers: Vec<_> = (0..WORKERS + QUEUE + 16)
        .map(|number| {
            let pool = Arc::clone(&pool);
            let refused_full = Arc::clone(&refused_full);
            std::thread::spawn(move || {
                let answer = pool.lookup(&format!("flood-{number}.test"), 80);
                if answer.is_err_and(|reason| reason.contains("too many lookups")) {
                    refused_full.fetch_add(1, Ordering::SeqCst);
                }
            })
        })
        .collect();
    for caller in callers {
        caller.join().expect("a lookup caller");
    }
    assert!(
        refused_full.load(Ordering::SeqCst) >= 16,
        "the pool and its queue hold {} lookups",
        WORKERS + QUEUE
    );
    assert!(
        gate.started.load(Ordering::SeqCst) <= WORKERS,
        "no more resolver calls run at once than the pool has threads"
    );
    gate.release();
}
