//! The CI watch service: start a watch on an exact commit, poll GitHub until
//! its checks finish, record the verdict, and wake the session that asked.
//!
//! One phrase: follow one commit's CI to a verdict for the session that
//! pushed it. The tool call only registers the watch; every GitHub read —
//! the commit's existence included — polling, log reads and the wake happen
//! on the daemon's poll thread, which a restart resumes from the persistent
//! store.

use std::sync::{Condvar, Mutex};
use std::time::{Duration, Instant};

use devboule_protocol::OwnerId;

use crate::ci_gh::{CiError, GhClient, RepoRef};
use crate::ci_pages::join_check_run_pages;
use crate::ci_pass::{Budget, Limits, Passes, Turn};
use crate::ci_summary::{self, CiState};
use crate::ci_wake::{wake_text, WakeSink};
use crate::ci_watch_store::{new_watch_id, now_ms, CiWatchRecord, CiWatchStore, Wake};

/// How often the poll thread looks at open watches.
pub(crate) const POLL_INTERVAL: Duration = Duration::from_secs(30);
/// A watch that has seen no verdict by then is closed as failed and says so.
const WATCH_TIMEOUT_MS: u64 = 6 * 60 * 60 * 1000;
/// How far past that a test ages a watch, so the overdue close is the pass
/// under test.
#[cfg(test)]
pub(crate) const OVERDUE_AGE: Duration = Duration::from_millis(WATCH_TIMEOUT_MS + 60_000);
/// Check runs are read a page at a time and every page is joined, so a commit
/// with more than one page of checks is judged whole rather than on its
/// first page.
const CHECK_RUNS_PAGE: &str = "per_page=100";
/// How long one watch may spend reading job logs in a pass; the logs left
/// after it read as unavailable instead of holding the repository's turn.
const LOG_READ_TIME: Duration = Duration::from_secs(60);

pub(crate) struct CiWatches {
    store: CiWatchStore,
    gh: GhClient,
    /// The same login with a shorter fuse, sharing the backoff map: the
    /// tool call resolves its repository fast, the poll thread keeps the
    /// minute.
    gh_tool: GhClient,
    /// A third fuse, for the job logs of a finished pass: a log is a
    /// download that either arrives or never will, and the poll thread still
    /// owes every other open watch its turn.
    gh_logs: GhClient,
    /// Set by a new watch so the poll thread looks now instead of at its
    /// next interval.
    kicked: Mutex<bool>,
    kick_signal: Condvar,
    passes: Passes,
}

/// Where a watch's wake stands, as the tool reports it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WakeStatus {
    Pending,
    Delivered,
    /// The send may already be out, so the wake was never repeated.
    DeliveredUncertain,
    OwnerSessionEnded,
}

impl WakeStatus {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Delivered => "delivered",
            Self::DeliveredUncertain => "delivered_uncertain",
            Self::OwnerSessionEnded => "owner_session_ended",
        }
    }
}

impl CiWatches {
    pub(crate) fn new(store: CiWatchStore, gh: GhClient) -> Self {
        let gh_tool = gh.with_timeout(crate::ci_gh::TOOL_GH_TIMEOUT);
        let gh_logs = gh.with_timeout(crate::ci_gh::LOG_GH_TIMEOUT);
        Self {
            store,
            gh,
            gh_tool,
            gh_logs,
            kicked: Mutex::new(false),
            kick_signal: Condvar::new(),
            passes: Passes::default(),
        }
    }

    /// The short-fuse client for broker-facing calls.
    pub(crate) fn gh_tool(&self) -> &GhClient {
        &self.gh_tool
    }

    /// Watch `sha` in `repo` for `session_id`, without asking GitHub
    /// anything: the poll thread reads the commit's checks and ends the watch
    /// with the reason when GitHub has no such commit. Asking again for the
    /// same commit from the same session answers the watch that exists.
    pub(crate) fn start(
        &self,
        session_id: &str,
        owner: &OwnerId,
        repo: &RepoRef,
        sha: &str,
    ) -> Result<CiWatchRecord, CiError> {
        if let Some(existing) = self.store.find(session_id, &repo.slug(), sha) {
            return Ok(existing);
        }
        let record = CiWatchRecord {
            watch_id: new_watch_id(),
            session_id: session_id.to_string(),
            owner_user: owner.user.clone(),
            owner_client: owner.client.clone(),
            host: repo.host.clone(),
            repo_owner: repo.owner.clone(),
            repo: repo.repo.clone(),
            sha: sha.to_string(),
            created_at_ms: now_ms(),
            state: CiState::Queued,
            summary: None,
            wake_key: None,
            wake: Wake::NotDue,
        };
        self.store.insert(record.clone()).map_err(|error| {
            CiError::new(
                "internal",
                format!("The watch could not be saved: {error}"),
                true,
            )
        })?;
        self.kick();
        Ok(record)
    }

    #[cfg(test)]
    pub(crate) fn get(&self, watch_id: &str) -> Option<CiWatchRecord> {
        self.store.get(watch_id)
    }

    /// Test-only: age a watch by `by`, so the overdue close is the pass
    /// under test without waiting six hours for it.
    #[cfg(test)]
    pub(crate) fn age(&self, watch_id: &str, by: Duration) {
        self.store
            .age(watch_id, u64::try_from(by.as_millis()).unwrap_or(u64::MAX));
    }

    /// Test-only: leave a wake claimed, the state a daemon that died between
    /// the claim and the send leaves on disk.
    #[cfg(test)]
    pub(crate) fn leave_claim_unsettled(&self, watch_id: &str) {
        self.store.leave_claim_unsettled(watch_id);
    }

    pub(crate) fn wake_status(&self, record: &CiWatchRecord, sink: &dyn WakeSink) -> WakeStatus {
        match record.wake {
            // A claim found after a restart may or may not have gone out, and
            // the daemon cannot know: it says uncertain, never delivered.
            Wake::DeliveredUncertain | Wake::Sending => WakeStatus::DeliveredUncertain,
            Wake::Delivered => WakeStatus::Delivered,
            Wake::NotDue | Wake::Pending => match owner_of(record) {
                Some(owner) if sink.is_live(&record.session_id, &owner) => WakeStatus::Pending,
                _ if record.state.is_terminal() => WakeStatus::OwnerSessionEnded,
                _ => WakeStatus::Pending,
            },
        }
    }

    fn kick(&self) {
        *self
            .kicked
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = true;
        self.kick_signal.notify_all();
    }

    /// Sleep until the next interval, or until a new watch asks for a look.
    pub(crate) fn wait_for_work(&self, interval: Duration) {
        let mut kicked = self
            .kicked
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if !*kicked {
            kicked = self
                .kick_signal
                .wait_timeout(kicked, interval)
                .unwrap_or_else(|error| error.into_inner())
                .0;
        }
        *kicked = false;
    }

    /// One pass: advance the open watches the pass can afford, then make
    /// every wake that is owed. A panicking watch must not end CI watching:
    /// each step is caught, logged and skipped. Wakes go out on scoped
    /// threads so one wedged session cannot hold the others behind a
    /// readiness wait.
    pub(crate) fn poll_once(&self, sink: &dyn WakeSink) {
        self.passes
            .run(self.store.open(), Limits::default(), &|record, budget| {
                self.poll_watch(record, budget)
            });
        std::thread::scope(|scope| {
            for record in self.store.pending_wakes() {
                let _wake = scope.spawn(move || {
                    if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        self.make_wake(&record, sink);
                    }))
                    .is_err()
                    {
                        eprintln!(
                            "ci watch: wake for {} panicked; continuing",
                            record.watch_id
                        );
                    }
                });
            }
        });
    }

    fn check_runs(&self, repo: &RepoRef, sha: &str) -> Result<Vec<ci_summary::CheckRun>, CiError> {
        let pages = self
            .gh
            .get_json_pages(repo, &format!("commits/{sha}/check-runs?{CHECK_RUNS_PAGE}"))?;
        join_check_run_pages(&pages)
    }

    fn poll_watch(&self, record: &CiWatchRecord, budget: &Budget) -> Turn {
        let repo = RepoRef {
            host: record.host.clone(),
            owner: record.repo_owner.clone(),
            repo: record.repo.clone(),
        };
        // GitHub asked for quiet: no process is spent, and the repository's
        // other watches would only be told the same.
        if self.gh.is_quiet(&repo) {
            self.close_if_overdue(record, false);
            return Turn::RepoFailed;
        }
        if !budget.take(1) {
            return Turn::NoBudget;
        }
        let runs = match self.check_runs(&repo, &record.sha) {
            Ok(runs) => runs,
            // A hiccup is retried next pass; a refusal that will not clear
            // ends the watch with the reason, so the owner is not left waiting
            // on a poll that can never succeed.
            Err(error) if error.retryable => {
                self.close_if_overdue(record, false);
                return Turn::RepoFailed;
            }
            Err(error) => {
                self.finish(record, CiState::Failed, stopped_text(record, &error));
                return Turn::Done;
            }
        };
        let state = ci_summary::overall(&runs);
        if !state.is_terminal() {
            if self.store.set_state(&record.watch_id, state).is_err() {
                eprintln!(
                    "ci watch: could not record the state of {}",
                    record.watch_id
                );
            }
            self.close_if_overdue(record, runs.is_empty());
            return Turn::Done;
        }
        // The logs are paid for before the first is read: a pass that cannot
        // afford them leaves the watch for the next one rather than judging a
        // failure with half its evidence.
        if !budget.take(ci_summary::logs_wanted(&runs)) {
            return Turn::NoBudget;
        }
        let reading_since = Instant::now();
        let verdict = ci_summary::build(&runs, &mut |run| {
            if reading_since.elapsed() >= LOG_READ_TIME {
                return Err(CiError::new(
                    "github_unavailable",
                    "this pass ran out of time for job logs",
                    true,
                ));
            }
            self.gh_logs
                .get_text(&repo, &format!("actions/jobs/{}/logs", run.id))
        });
        let cause = if verdict.state == CiState::Passed {
            "none"
        } else if verdict.only_infra() {
            "INFRA"
        } else {
            "CODE"
        };
        let header = format!(
            "CI {} for {} in {} (cause: {cause})",
            verdict.state.as_str(),
            short(&record.sha),
            record.slug()
        );
        self.finish(record, verdict.state, verdict.render(&header));
        Turn::Done
    }

    /// A watch that has waited long enough is closed, and says which of the
    /// two waits it was: a commit GitHub has no checks for (no workflow runs
    /// on it) is not a build that failed.
    fn close_if_overdue(&self, record: &CiWatchRecord, no_checks: bool) {
        if now_ms().saturating_sub(record.created_at_ms) < WATCH_TIMEOUT_MS {
            return;
        }
        let reason = if no_checks {
            "INFRA: GitHub has no checks registered for this commit"
        } else {
            "INFRA: no CI result within 6 hours"
        };
        let text = format!(
            "CI failed for {} in {} (cause: {reason})\n",
            short(&record.sha),
            record.slug()
        );
        self.finish(record, CiState::Failed, text);
    }

    fn finish(&self, record: &CiWatchRecord, state: CiState, summary: String) {
        if let Err(error) = self.store.complete(&record.watch_id, state, summary) {
            eprintln!(
                "ci watch: could not record the verdict of {}: {error}",
                record.watch_id
            );
        }
    }

    fn make_wake(&self, record: &CiWatchRecord, sink: &dyn WakeSink) {
        let Some(owner) = owner_of(record) else {
            return;
        };
        if !sink.is_live(&record.session_id, &owner) {
            return;
        }
        let Some(claimed) = self.store.claim_wake(&record.watch_id) else {
            return;
        };
        // Refused never wrote anything, so the claim goes back and a later
        // pass tries again. Uncertain may already be out: the claim settles
        // as delivered-uncertain and is never made again.
        match sink.deliver(&claimed.session_id, &owner, &wake_text(&claimed)) {
            Ok(()) => {
                if self.store.finish_wake(&claimed.watch_id, true).is_err() {
                    eprintln!(
                        "ci watch: could not settle the wake of {}",
                        claimed.watch_id
                    );
                }
            }
            Err(crate::session::SendError::Refused(error)) => {
                eprintln!(
                    "ci watch: wake for {} refused ({}); retrying",
                    claimed.watch_id, error.message
                );
                if self.store.finish_wake(&claimed.watch_id, false).is_err() {
                    eprintln!(
                        "ci watch: could not settle the wake of {}",
                        claimed.watch_id
                    );
                }
            }
            Err(crate::session::SendError::Uncertain(error)) => {
                eprintln!(
                    "ci watch: wake for {} uncertain ({}); not repeating",
                    claimed.watch_id, error.message
                );
                if self.store.settle_wake_uncertain(&claimed.watch_id).is_err() {
                    eprintln!(
                        "ci watch: could not settle the wake of {}",
                        claimed.watch_id
                    );
                }
            }
        }
    }
}

fn owner_of(record: &CiWatchRecord) -> Option<OwnerId> {
    OwnerId::new(record.owner_user.clone(), record.owner_client.clone()).ok()
}

fn stopped_text(record: &CiWatchRecord, error: &CiError) -> String {
    format!(
        "CI watch for {} in {} stopped: {} ({})\n",
        short(&record.sha),
        record.slug(),
        error.message,
        error.code
    )
}

pub(crate) fn short(sha: &str) -> &str {
    sha.get(..7).unwrap_or(sha)
}

#[cfg(test)]
#[path = "ci_watch_tests.rs"]
mod tests;
