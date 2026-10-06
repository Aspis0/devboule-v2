//! One poll pass over the open watches, bounded three ways: an API request
//! budget for the whole pass, one worker per repository (a few side by side),
//! and a time limit per repository.
//!
//! One phrase: decide which watches get a turn. A watch that GitHub's limits
//! or a slow repository cost its turn is served first on the next pass; a
//! repository that fails ends its turn for the pass, its other watches go first
//! next time, and the watch that failed goes behind them, so one watch that
//! cannot be read never keeps its siblings waiting.

use std::collections::{HashMap, HashSet, VecDeque};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

use crate::ci_watch_store::CiWatchRecord;

/// What one watch's turn left behind.
pub(crate) enum Turn {
    Done,
    /// The pass had no requests left for it.
    NoBudget,
    /// The read failed in a way that may clear: the watch's turn ends, and so
    /// does the repository's for this pass.
    RepoFailed,
}

/// The GitHub API requests one pass may still make: one for each plain read,
/// and one for each page a paginated read turns out to have fetched.
pub(crate) struct Budget(AtomicUsize);

impl Budget {
    /// Pay for `requests` up front, or refuse when they are not there.
    pub(crate) fn take(&self, requests: usize) -> bool {
        self.0
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |left| {
                left.checked_sub(requests)
            })
            .is_ok()
    }

    /// Pay for requests already made, such as the pages after the first: the
    /// budget runs down as far as it goes and the next turn finds it spent.
    pub(crate) fn spend(&self, requests: usize) {
        let _ = self
            .0
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |left| {
                Some(left.saturating_sub(requests))
            });
    }
}

#[derive(Clone, Copy)]
pub(crate) struct Limits {
    pub(crate) requests: usize,
    pub(crate) workers: usize,
    pub(crate) repo_time: Duration,
}

impl Default for Limits {
    /// Forty requests per half-minute pass stays well under GitHub's hourly
    /// budget, and four repositories at once with a minute and a half each
    /// keeps one stuck repository from holding the rest for long.
    fn default() -> Self {
        Self {
            requests: 40,
            workers: 4,
            repo_time: Duration::from_secs(90),
        }
    }
}

#[derive(Default)]
pub(crate) struct Passes {
    passed_over: Mutex<HashSet<String>>,
}

type Group = Vec<CiWatchRecord>;

impl Passes {
    /// Give every open watch a turn the limits allow, each repository's
    /// watches in order on one worker.
    pub(crate) fn run(
        &self,
        open: Vec<CiWatchRecord>,
        limits: Limits,
        turn: &(dyn Fn(&CiWatchRecord, &Budget) -> Turn + Sync),
    ) {
        let groups = self.grouped(open);
        let workers = limits.workers.min(groups.len());
        let queue = Mutex::new(groups);
        let budget = Budget(AtomicUsize::new(limits.requests));
        std::thread::scope(|scope| {
            for _ in 0..workers {
                let _worker = scope.spawn(|| loop {
                    let next = lock(&queue).pop_front();
                    let Some(group) = next else { return };
                    self.serve(group, limits.repo_time, &budget, turn);
                });
            }
        });
    }

    /// The open watches by repository, passed-over ones first.
    fn grouped(&self, open: Vec<CiWatchRecord>) -> VecDeque<Group> {
        let first = {
            let mut passed_over = lock(&self.passed_over);
            passed_over.retain(|id| open.iter().any(|record| &record.watch_id == id));
            passed_over.clone()
        };
        let (mut ordered, rest): (Vec<_>, Vec<_>) = open
            .into_iter()
            .partition(|record| first.contains(&record.watch_id));
        ordered.extend(rest);
        let mut groups: Vec<Group> = Vec::new();
        let mut at: HashMap<String, usize> = HashMap::new();
        for record in ordered {
            let key = format!("{}/{}/{}", record.host, record.repo_owner, record.repo);
            let index = *at.entry(key).or_insert_with(|| {
                groups.push(Vec::new());
                groups.len() - 1
            });
            groups[index].push(record);
        }
        groups.into()
    }

    fn serve(
        &self,
        group: Group,
        repo_time: Duration,
        budget: &Budget,
        turn: &(dyn Fn(&CiWatchRecord, &Budget) -> Turn + Sync),
    ) {
        let started = Instant::now();
        let mut failed = false;
        let mut waiting = 0usize;
        for record in group {
            if failed {
                self.pass_over(&record);
                waiting += 1;
                continue;
            }
            if started.elapsed() >= repo_time {
                self.pass_over(&record);
                continue;
            }
            match catch_unwind(AssertUnwindSafe(|| turn(&record, budget))) {
                Ok(Turn::Done) => self.served(&record),
                Ok(Turn::NoBudget) => self.pass_over(&record),
                Ok(Turn::RepoFailed) => {
                    self.served(&record);
                    failed = true;
                }
                Err(_) => eprintln!(
                    "ci watch: poll for {} panicked; continuing",
                    record.watch_id
                ),
            }
        }
        if waiting > 0 {
            eprintln!(
                "ci watch: a read failed; {waiting} more watch(es) of that repository go first next pass"
            );
        }
    }

    fn pass_over(&self, record: &CiWatchRecord) {
        lock(&self.passed_over).insert(record.watch_id.clone());
    }

    fn served(&self, record: &CiWatchRecord) {
        lock(&self.passed_over).remove(&record.watch_id);
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|error| error.into_inner())
}

#[cfg(test)]
#[path = "ci_pass_tests.rs"]
mod tests;
