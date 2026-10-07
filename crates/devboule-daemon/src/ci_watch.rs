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

use crate::ci_gh::{is_commit_id, CiError, GhClient, RepoRef};
use crate::ci_pages::join_check_run_pages;
use crate::ci_pass::{Budget, Limits, Passes, Turn};
use crate::ci_summary::{self, CheckRun, CiState, Verdict};
use crate::ci_wake::{wake_text, WakeSink};
use crate::ci_watch_quota::refusal_of;
use crate::ci_watch_store::{new_watch_id, now_ms, Admit, CiWatchRecord, CiWatchStore, Wake};
use crate::diagnostics::redact_secret_tokens;

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
    /// same key — session, repository, commit and branch — answers the watch
    /// that exists, in one store operation, so two calls racing for it cannot
    /// start two.
    ///
    /// `branch` names the branch this commit was resolved as, when the caller
    /// started the watch in branch mode: the poll thread then reads that
    /// branch's head first and supersedes the watch when it moved on. It is
    /// the caller that resolves the head, so a start still reads nothing.
    ///
    /// `retry_approved` is the person's answer at watch time to the one infra
    /// retry; without it no failure is ever re-run.
    pub(crate) fn start(
        &self,
        session_id: &str,
        owner: &OwnerId,
        repo: &RepoRef,
        sha: &str,
        branch: Option<&str>,
        retry_approved: bool,
    ) -> Result<CiWatchRecord, CiError> {
        if !is_commit_id(sha) {
            return Err(CiError::new(
                "invalid_sha",
                "sha must be a full 40-character commit id.",
                false,
            ));
        }
        // GitHub answers a head in lower case, so a stored id must be one too
        // or the first poll would read a move that never happened.
        let sha = sha.to_ascii_lowercase();
        let record = CiWatchRecord {
            watch_id: new_watch_id(),
            session_id: session_id.to_string(),
            owner_user: owner.user.clone(),
            owner_client: owner.client.clone(),
            host: repo.host.clone(),
            repo_owner: repo.owner.clone(),
            repo: repo.repo.clone(),
            sha,
            branch: branch.map(str::to_string),
            created_at_ms: now_ms(),
            state: CiState::Queued,
            summary: None,
            wake_key: None,
            wake: Wake::NotDue,
            retry_approved,
            retry_count: 0,
            retry_issued: false,
            retried_runs: Vec::new(),
            retry_attempts: Vec::new(),
        };
        match self
            .store
            .find_or_insert(record)
            .map_err(|error| refusal_of(error, repo))?
        {
            Admit::Existing(existing) => {
                // A later call may bring the retry approval the first one did
                // not have; it is the same watch, so the yes lands on it. A
                // watch that already finished keeps its verdict and never
                // retries.
                if retry_approved && !existing.retry_approved {
                    return match self.store.approve_retry(&existing.watch_id) {
                        Ok(Some(approved)) => Ok(approved),
                        Ok(None) => Ok(existing),
                        Err(error) => Err(CiError::new(
                            "internal",
                            format!("The retry approval could not be saved: {error}"),
                            true,
                        )),
                    };
                }
                Ok(existing)
            }
            Admit::Inserted(record) => {
                self.kick();
                Ok(record)
            }
        }
    }

    #[cfg(test)]
    pub(crate) fn get(&self, watch_id: &str) -> Option<CiWatchRecord> {
        self.store.get(watch_id)
    }

    /// Test-only: every watch still waiting on CI.
    #[cfg(test)]
    pub(crate) fn open(&self) -> Vec<CiWatchRecord> {
        self.store.open()
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

    /// Test-only: leave a spent retry unconfirmed, the state a daemon that
    /// died between the reservation and `gh`'s answer leaves on disk.
    #[cfg(test)]
    pub(crate) fn leave_retry_unissued(&self, watch_id: &str) {
        self.store.leave_retry_unissued(watch_id);
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
        self.poll_within(sink, Limits::default());
    }

    fn poll_within(&self, sink: &dyn WakeSink, limits: Limits) {
        self.passes
            .run(self.store.open(), limits, &|record, budget| {
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

    /// The commit's check runs, and how many pages GitHub served them in.
    fn check_runs(
        &self,
        repo: &RepoRef,
        sha: &str,
    ) -> Result<(Vec<ci_summary::CheckRun>, usize), CiError> {
        let pages = self
            .gh
            .get_json_pages(repo, &format!("commits/{sha}/check-runs?{CHECK_RUNS_PAGE}"))?;
        Ok((join_check_run_pages(&pages)?, pages.len()))
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
        // A branch watch reads the head before the checks and pays for the
        // pair with its turn: a pass that cannot afford both leaves the watch
        // whole for the next one.
        let requests = if record.branch.is_some() { 2 } else { 1 };
        if !budget.take(requests) {
            return Turn::NoBudget;
        }
        if let Some(turn) = self.supersede_if_moved(record, &repo) {
            return turn;
        }
        let runs = match self.check_runs(&repo, &record.sha) {
            Ok((runs, pages)) => {
                // The first page was paid for with the turn; every page after
                // it was one more request to GitHub.
                budget.spend(pages.saturating_sub(1));
                runs
            }
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
        // A re-run is judged by the run's own newer attempt and by nothing
        // else that appears on the commit: another app's check, a late check
        // of the attempt before, or a green check of another workflow must
        // never turn a red commit green. While that attempt is not finished
        // the watch waits, and a run whose attempt cannot be read keeps the
        // failure that was there.
        let judged = if record.retry_issued && !record.retried_runs.is_empty() {
            match self.retried_runs_state(record, &repo, budget) {
                RetriedRuns::NoBudget => return Turn::NoBudget,
                RetriedRuns::Waiting => {
                    if self
                        .store
                        .set_state(&record.watch_id, CiState::Running)
                        .is_err()
                    {
                        eprintln!(
                            "ci watch: could not record the state of {}",
                            record.watch_id
                        );
                    }
                    self.close_if_overdue(record, runs.is_empty());
                    return Turn::Done;
                }
                RetriedRuns::Attempts(attempts) => replace_retried_run_checks(&runs, &attempts),
            }
        } else {
            runs.clone()
        };
        let state = ci_summary::overall(&judged);
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
        if !budget.take(ci_summary::logs_wanted(&judged)) {
            return Turn::NoBudget;
        }
        let reading_since = Instant::now();
        let verdict = ci_summary::build(&judged, &mut |run| {
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
        } else if verdict.all_failures_infra() {
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
        // The head is read once more before anything terminal is written: a
        // branch that moved while the checks and logs were read is the newer
        // fact, and its verdict belongs to the watch on the new head.
        if record.branch.is_some() {
            if !budget.take(1) {
                return Turn::NoBudget;
            }
            if let Some(turn) = self.supersede_if_moved(record, &repo) {
                return turn;
            }
        }
        // The one retry: only a failure that is entirely the platform's, and
        // only when the person approved it when the watch was started.
        if let Some(runs) = retry_runs(record, &verdict) {
            // The attempt each run is on now is read before the re-run is
            // asked for: the attempt after it is the one this retry will be
            // judged by, and nothing else on the commit can stand in for it.
            let mut targets = Vec::with_capacity(runs.len());
            for run_id in &runs {
                if !budget.take(1) {
                    return Turn::NoBudget;
                }
                match self.gh.run_attempt(&repo, *run_id) {
                    Ok(state) => targets.push((*run_id, state.attempt)),
                    Err(error) if error.retryable => {
                        self.close_if_overdue(record, false);
                        return Turn::RepoFailed;
                    }
                    Err(error) => {
                        // The run cannot be read, so the re-run could not be
                        // told apart from anything else: it is not issued, and
                        // the failure that was there stands.
                        let heading = heading(&header, Some(refused_note(&runs, &error)));
                        self.finish(record, verdict.state, verdict.render(&heading));
                        return Turn::Done;
                    }
                }
            }
            if !budget.take(targets.len()) {
                return Turn::NoBudget;
            }
            // Reserved on disk before GitHub is asked: a daemon that dies here
            // must never issue a second retry, and one that cannot write the
            // reservation must not issue the first.
            if !self.store.note_retry_reserved(&record.watch_id, &targets) {
                return Turn::Done;
            }
            match self.issue_retry(&repo, &runs) {
                Ok(()) => {
                    // The re-run is on its way; the record must say so, or a
                    // restart in the next instant would read it as a retry
                    // nobody can vouch for.
                    if let Err(error) = self.store.mark_retry_issued(&record.watch_id) {
                        eprintln!(
                            "ci watch: could not record the issued retry of {}: {error}",
                            record.watch_id
                        );
                    }
                    if self
                        .store
                        .set_state(&record.watch_id, CiState::Running)
                        .is_err()
                    {
                        eprintln!(
                            "ci watch: could not record the state of {}",
                            record.watch_id
                        );
                    }
                }
                Err(error) => {
                    // The verdict is still the owner's to read: the retry's
                    // own line names what it hit, and the jobs below say what
                    // failed while it was being asked for.
                    let heading = heading(&header, Some(refused_note(&runs, &error)));
                    self.finish(record, verdict.state, verdict.render(&heading));
                }
            }
            return Turn::Done;
        }
        let heading = heading(&header, retry_note(record));
        self.finish(record, verdict.state, verdict.render(&heading));
        Turn::Done
    }

    /// A branch watch whose head no longer points at the watched commit is
    /// over, whatever that commit's checks say: the newer fact is the head.
    /// `None` leaves the watch alone — it follows no branch, or the branch
    /// still points where it did.
    fn supersede_if_moved(&self, record: &CiWatchRecord, repo: &RepoRef) -> Option<Turn> {
        let branch = record.branch.as_deref()?;
        match self.gh.head_sha(repo, branch) {
            // A head that moved, a force-push included, ends this watch: the
            // commit it was watching is not the branch's head any more. The
            // new head is not watched by itself — the agent decides whether
            // to ask for it.
            Ok(head) if head != record.sha => {
                self.finish(record, CiState::Superseded, superseded_text(record, &head));
                Some(Turn::Done)
            }
            Ok(_) => None,
            Err(error) if error.retryable => {
                self.close_if_overdue(record, false);
                Some(Turn::RepoFailed)
            }
            Err(error) => {
                self.finish(record, CiState::Failed, stopped_text(record, &error));
                Some(Turn::Done)
            }
        }
    }

    /// Ask GitHub to re-run the failed jobs of each run. The reservation is
    /// already recorded, so a refusal here cannot lead to a second attempt.
    fn issue_retry(&self, repo: &RepoRef, runs: &[u64]) -> Result<(), CiError> {
        for run in runs {
            self.gh.rerun_failed(repo, *run)?;
        }
        Ok(())
    }

    /// The finished newer attempt of every retried run, read from the run
    /// itself: a re-run keeps the run id and moves its attempt number on, so
    /// that number is what tells this retry apart from anything else on the
    /// commit. A run whose attempt is not there yet, or is still running,
    /// keeps the watch waiting; one whose attempt cannot be read contributes
    /// nothing, so the failure that was there stands.
    fn retried_runs_state(
        &self,
        record: &CiWatchRecord,
        repo: &RepoRef,
        budget: &Budget,
    ) -> RetriedRuns {
        let mut attempts = Vec::new();
        for (run_id, decided_at) in record.retried_run_attempts() {
            if !budget.take(1) {
                return RetriedRuns::NoBudget;
            }
            let state = match self.gh.run_attempt(repo, run_id) {
                Ok(state) => state,
                Err(error) if error.retryable => return RetriedRuns::Waiting,
                Err(_) => continue,
            };
            if state.attempt <= decided_at || state.status != "completed" {
                return RetriedRuns::Waiting;
            }
            if !budget.take(1) {
                return RetriedRuns::NoBudget;
            }
            let jobs = match self.gh.attempt_job_pages(repo, run_id, state.attempt) {
                Ok(pages) => match crate::ci_pages::join_job_pages(&pages) {
                    Ok(jobs) => jobs,
                    Err(_) => continue,
                },
                Err(error) if error.retryable => return RetriedRuns::Waiting,
                Err(_) => continue,
            };
            // An attempt with no jobs is nothing to judge from: the failure
            // that was there stands rather than a run reading green on no
            // evidence at all.
            if !jobs.is_empty() {
                attempts.push((run_id, jobs));
            }
        }
        RetriedRuns::Attempts(attempts)
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

/// The bounded text a superseded branch watch wakes its owner with: which
/// branch moved, from which commit to which. The branch is a name an agent
/// chose, so it passes the same secret redaction a summary does before it
/// rides a message.
fn superseded_text(record: &CiWatchRecord, head: &str) -> String {
    let branch = redact_secret_tokens(record.branch.as_deref().unwrap_or_default());
    format!(
        "CI watch superseded: {} {} moved {} → {}; it no longer polls. Start a watch on the new head if you want its verdict.\n",
        record.slug(),
        branch,
        short(&record.sha),
        short(head)
    )
}

/// What the retried runs say about their newer attempts.
enum RetriedRuns {
    /// A newer attempt is not there yet, or is still running: the watch waits
    /// for it rather than judging the attempt it was asked to replace.
    Waiting,
    /// The pass ran out of requests; the watch keeps its place for the next.
    NoBudget,
    /// One finished attempt's jobs per run the pass could read. A run that is
    /// missing here keeps the failure that was there.
    Attempts(Vec<(u64, Vec<CheckRun>)>),
}

/// The retry this failure earns, or `None` when it earns none: no watch-time
/// approval, a retry already spent, a code failure, a check that was never
/// read (an omitted one could be a code failure), an unread log (unknown is
/// not infra), or a failed job that names no run — a partial retry would
/// leave part of the failure the person approved to re-run.
fn retry_runs(record: &CiWatchRecord, verdict: &Verdict) -> Option<Vec<u64>> {
    if !record.retry_approved || record.retry_count > 0 || verdict.state != CiState::Failed {
        return None;
    }
    if !verdict.all_failures_infra() {
        return None;
    }
    let mut runs = Vec::new();
    for job in verdict.jobs.iter().filter(|job| job.cause.is_some()) {
        let run_id = job.run_id?;
        if !runs.contains(&run_id) {
            runs.push(run_id);
        }
    }
    (!runs.is_empty()).then_some(runs)
}

/// The checks a verdict may be read from once a re-run is out: for every run
/// the retry asked for, its newer attempt's evidence takes the place of the
/// old attempt's checks. Every other check — another workflow, another app, a
/// late check of the attempt before — keeps its own latest verdict, and a run
/// whose attempt could not be read keeps the failure that was there.
fn replace_retried_run_checks(
    runs: &[CheckRun],
    attempts: &[(u64, Vec<CheckRun>)],
) -> Vec<CheckRun> {
    let mut judged = Vec::with_capacity(runs.len());
    let mut placed: Vec<u64> = Vec::new();
    for run in runs {
        let Some(run_id) = run.run_id else {
            judged.push(run.clone());
            continue;
        };
        let Some((_, jobs)) = attempts.iter().find(|(asked, _)| *asked == run_id) else {
            judged.push(run.clone());
            continue;
        };
        if placed.contains(&run_id) {
            continue;
        }
        placed.push(run_id);
        let old: Vec<CheckRun> = runs
            .iter()
            .filter(|check| check.run_id == Some(run_id))
            .cloned()
            .collect();
        judged.extend(ci_summary::newer_attempt_evidence(&old, jobs));
    }
    judged
}

/// The verdict's first lines: the header, and — when a retry was asked for —
/// what became of it, so an owner never reads a re-run's result as the first
/// attempt's.
fn heading(header: &str, retry: Option<String>) -> String {
    match retry {
        Some(note) => format!("{header}\n{note}"),
        None => header.to_string(),
    }
}

/// What became of the watch's one retry, in its verdict's own words. A retry
/// that was only reserved — `gh` was asked and the daemon died before it could
/// record the answer — reads as exactly that, never as a failure no retry was
/// ever allowed for.
fn retry_note(record: &CiWatchRecord) -> Option<String> {
    if record.retry_count == 0 {
        return None;
    }
    let runs = run_list(&record.retried_runs);
    Some(if record.retry_issued {
        format!("infra retry: one re-run was issued for run(s) {runs}.")
    } else {
        format!(
            "infra retry: a re-run was requested for run(s) {runs} but gh never confirmed it; it is not issued again."
        )
    })
}

/// The retry's own line when `gh` refused it: the verdict below it still says
/// what failed.
fn refused_note(runs: &[u64], error: &CiError) -> String {
    format!(
        "infra retry: gh refused the re-run of run(s) {}: {} ({}).",
        run_list(runs),
        error.message,
        error.code
    )
}

fn run_list(runs: &[u64]) -> String {
    runs.iter()
        .map(u64::to_string)
        .collect::<Vec<_>>()
        .join(", ")
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

#[cfg(test)]
#[path = "ci_lifecycle_tests.rs"]
mod lifecycle_tests;
