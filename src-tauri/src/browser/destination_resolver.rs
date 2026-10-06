//! The lookups the destination policy waits on: a small fixed pool of resolver
//! threads with in-flight dedupe per host, so that a page navigating in a loop
//! or an agent opening many hosts can neither spawn threads without bound nor
//! hold the page's own thread past `LOOKUP_BUDGET`.

use std::collections::HashMap;
use std::net::{IpAddr, ToSocketAddrs};
use std::sync::mpsc::{self, Receiver, SyncSender, TrySendError};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// How long a hook's lookup may hold the thread it runs on. The navigation
/// waits for the answer, so a lookup that cannot answer inside this is refused
/// rather than allowed: a wrong block is visible and named, a wrong allow is
/// the hole the policy closes.
const LOOKUP_BUDGET: Duration = Duration::from_millis(100);

/// Resolver threads for the whole policy. A lookup the OS never answers holds
/// one of them; the others keep serving.
const WORKERS: usize = 4;

/// Lookups that may wait for a free worker. A full queue refuses at once.
const QUEUE: usize = 64;

type Key = (String, u16);
type Answer = Result<Vec<IpAddr>, String>;
type Waiters = Arc<Mutex<HashMap<Key, Vec<SyncSender<Answer>>>>>;

/// What the policy asks the network for. The real one is the OS resolver; a
/// test hands in its own answers.
pub(super) trait Resolver: Send + Sync {
    fn resolve(&self, host: &str, port: u16) -> Result<Vec<IpAddr>, String>;
}

/// The OS resolver, as `ToSocketAddrs` reaches it.
pub(super) struct SystemResolver;

impl Resolver for SystemResolver {
    fn resolve(&self, host: &str, port: u16) -> Result<Vec<IpAddr>, String> {
        (host, port)
            .to_socket_addrs()
            .map(|addresses| addresses.map(|address| address.ip()).collect())
            .map_err(|error| error.to_string())
    }
}

/// The pool. Calls for one host share one lookup, and no call spawns a thread
/// of its own.
pub(super) struct LookupPool {
    requests: SyncSender<Key>,
    waiting: Waiters,
}

impl LookupPool {
    pub(super) fn start(resolver: Arc<dyn Resolver>) -> Self {
        let (requests, incoming) = mpsc::sync_channel(QUEUE);
        let incoming = Arc::new(Mutex::new(incoming));
        let waiting = Waiters::default();
        for number in 0..WORKERS {
            let worker = Worker {
                resolver: Arc::clone(&resolver),
                incoming: Arc::clone(&incoming),
                waiting: Arc::clone(&waiting),
            };
            let started = std::thread::Builder::new()
                .name(format!("devboule-browser-resolver-{number}"))
                .spawn(move || worker.run());
            if let Err(error) = started {
                eprintln!("devboule: a browser resolver thread did not start: {error}");
            }
        }
        Self { requests, waiting }
    }

    /// One host's addresses, never waited on past `LOOKUP_BUDGET`. A pool that
    /// cannot take the lookup answers with an error, which the policy refuses.
    pub(super) fn lookup(&self, host: &str, port: u16) -> Answer {
        let key = (host.to_string(), port);
        let (sender, receiver) = mpsc::sync_channel(1);
        let first = {
            let mut waiting = lock(&self.waiting);
            let entry = waiting.entry(key.clone()).or_default();
            entry.push(sender);
            entry.len() == 1
        };
        if first {
            if let Err(error) = self.requests.try_send(key.clone()) {
                // Dropping the entry drops every waiter's sender, so any call
                // that joined it meanwhile is answered with the same refusal.
                lock(&self.waiting).remove(&key);
                return Err(match error {
                    TrySendError::Full(_) => "too many lookups are waiting".to_string(),
                    TrySendError::Disconnected(_) => "the resolver is not running".to_string(),
                });
            }
        }
        receiver
            .recv_timeout(LOOKUP_BUDGET)
            .unwrap_or_else(|_| Err(format!("no answer within {} ms", LOOKUP_BUDGET.as_millis())))
    }
}

struct Worker {
    resolver: Arc<dyn Resolver>,
    incoming: Arc<Mutex<Receiver<Key>>>,
    waiting: Waiters,
}

impl Worker {
    fn run(self) {
        loop {
            // The lock is held only while waiting for the next job, so an idle
            // worker is the one that takes it.
            let job = lock(&self.incoming).recv();
            let Ok((host, port)) = job else { return };
            let answer = self.resolver.resolve(&host, port);
            let waiters = lock(&self.waiting).remove(&(host, port));
            for waiter in waiters.into_iter().flatten() {
                let _ = waiter.send(answer.clone());
            }
        }
    }
}

/// A poisoned lock is still a usable table: the policy fails closed on its own
/// answers, so a panic elsewhere must not panic the page's thread here.
fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|error| error.into_inner())
}

#[cfg(test)]
#[path = "destination_resolver_tests.rs"]
mod tests;
