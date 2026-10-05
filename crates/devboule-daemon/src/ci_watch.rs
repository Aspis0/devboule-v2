//! The CI watch service: start a watch on an exact commit, poll GitHub until
//! its checks finish, record the verdict, and wake the session that asked.
//!
//! One phrase: follow one commit's CI to a verdict for the session that
//! pushed it. The tool call only validates the commit and reads the first
//! state; everything slow — polling, log reads, the wake — happens on the
//! daemon's poll thread, which a restart resumes from the persistent store.

use std::sync::{Condvar, Mutex};
use std::time::Duration;

use devboule_protocol::OwnerId;

use crate::ci_gh::{CiError, GhClient, RepoRef};
use crate::ci_summary::{self, CiState};
use crate::ci_wake::{wake_text, WakeSink};
use crate::ci_watch_store::{new_watch_id, now_ms, CiWatchRecord, CiWatchStore, Wake};

/// How often the poll thread looks at open watches.
pub(crate) const POLL_INTERVAL: Duration = Duration::from_secs(30);
/// A watch that has seen no verdict by then is closed as failed and says so.
const WATCH_TIMEOUT_MS: u64 = 6 * 60 * 60 * 1000;
const CHECK_RUNS_PAGE: &str = "per_page=100";

pub(crate) struct CiWatches {
    store: CiWatchStore,
    gh: GhClient,
    /// Set by a new watch so the poll thread looks now instead of at its
    /// next interval.
    kicked: Mutex<bool>,
    kick_signal: Condvar,
}

/// Where a watch's wake stands, as the tool reports it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WakeStatus {
    Pending,
    Delivered,
    OwnerSessionEnded,
}

impl WakeStatus {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Delivered => "delivered",
            Self::OwnerSessionEnded => "owner_session_ended",
        }
    }
}

impl CiWatches {
    pub(crate) fn new(store: CiWatchStore, gh: GhClient) -> Self {
        Self {
            store,
            gh,
            kicked: Mutex::new(false),
            kick_signal: Condvar::new(),
        }
    }

    pub(crate) fn gh(&self) -> &GhClient {
        &self.gh
    }

    /// Watch `sha` in `repo` for `session_id`. Asking again for the same
    /// commit from the same session answers the watch that exists.
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
        self.gh
            .get_json(repo, &format!("git/commits/{sha}"))
            .map_err(|error| match error.code {
                "not_found" => CiError::new(
                    "sha_not_found",
                    format!(
                        "GitHub has no commit {} in {}. Push it first, then watch it.",
                        short(sha),
                        repo.slug()
                    ),
                    false,
                ),
                _ => error,
            })?;
        let runs = self.check_runs(repo, sha)?;
        // A watch that begins already finished still gets its verdict through
        // the poll thread, the one place that reads logs and wakes.
        let state = match ci_summary::overall(&runs) {
            CiState::Passed | CiState::Failed => CiState::Running,
            other => other,
        };
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
            state,
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

    pub(crate) fn wake_status(&self, record: &CiWatchRecord, sink: &dyn WakeSink) -> WakeStatus {
        match record.wake {
            Wake::Delivered | Wake::Sending => WakeStatus::Delivered,
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

    /// One pass: advance every open watch, then make every wake that is owed.
    pub(crate) fn poll_once(&self, sink: &dyn WakeSink) {
        for record in self.store.open() {
            self.poll_watch(&record);
        }
        for record in self.store.pending_wakes() {
            self.make_wake(&record, sink);
        }
    }

    fn check_runs(&self, repo: &RepoRef, sha: &str) -> Result<Vec<ci_summary::CheckRun>, CiError> {
        let document = self
            .gh
            .get_json(repo, &format!("commits/{sha}/check-runs?{CHECK_RUNS_PAGE}"))?;
        Ok(ci_summary::parse_check_runs(&document))
    }

    fn poll_watch(&self, record: &CiWatchRecord) {
        let repo = RepoRef {
            host: record.host.clone(),
            owner: record.repo_owner.clone(),
            repo: record.repo.clone(),
        };
        let runs = match self.check_runs(&repo, &record.sha) {
            Ok(runs) => runs,
            // A hiccup is retried next pass; a refusal that will not clear
            // ends the watch with the reason, so the owner is not left waiting
            // on a poll that can never succeed.
            Err(error) if error.retryable => {
                self.close_if_overdue(record);
                return;
            }
            Err(error) => {
                self.finish(record, CiState::Failed, stopped_text(record, &error));
                return;
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
            self.close_if_overdue(record);
            return;
        }
        let verdict = ci_summary::build(&runs, &mut |run| {
            self.gh
                .get_text(&repo, &format!("actions/jobs/{}/logs", run.id))
                .ok()
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
    }

    fn close_if_overdue(&self, record: &CiWatchRecord) {
        if now_ms().saturating_sub(record.created_at_ms) < WATCH_TIMEOUT_MS {
            return;
        }
        let text = format!(
            "CI failed for {} in {} (cause: INFRA: no CI result within 6 hours)\n",
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
        let delivered = sink
            .deliver(&claimed.session_id, &owner, &wake_text(&claimed))
            .is_ok();
        if self
            .store
            .finish_wake(&claimed.watch_id, delivered)
            .is_err()
        {
            eprintln!(
                "ci watch: could not settle the wake of {}",
                claimed.watch_id
            );
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
