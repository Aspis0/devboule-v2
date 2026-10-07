//! Branch mode, supersession and the one approved infra retry, over a
//! scripted GitHub: a branch head that moves ends the watch that was
//! following it, and only a failure that is entirely the platform's is ever
//! re-run, once.

use std::sync::Arc;

use super::CiWatches;
use crate::ci_summary::CiState;
use crate::ci_test_support::{
    branch_head, check_run, checks, fail, github_origin, ok, owner, repo, service, watch_dir,
    RecordingSink, ScriptedRunner, SHA,
};
use crate::ci_watch_store::CiWatchRecord;

const BRANCH: &str = "main";
/// A commit the watched branch never led to: what a force-push leaves.
const REWRITTEN: &str = "fedcba9876543210fedcba9876543210fedcba98";
/// The workflow run every scripted check run of this file belongs to.
const RUN: u64 = 900;

fn head(runner: &ScriptedRunner, sha: &str) {
    runner.set("git/ref/heads/main", ok(&branch_head(sha)));
}

fn sha_watch(watches: &CiWatches, retry: bool) -> CiWatchRecord {
    watches
        .start("session-1", &owner(), &repo(), SHA, None, retry)
        .expect("start")
}

fn branch_watch(watches: &CiWatches, sha: &str, retry: bool) -> CiWatchRecord {
    watches
        .start("session-1", &owner(), &repo(), sha, Some(BRANCH), retry)
        .expect("start")
}

fn reruns(runner: &ScriptedRunner) -> usize {
    runner
        .calls()
        .iter()
        .filter(|call| call.contains("run rerun"))
        .count()
}

/// An all-INFRA failure: a job the platform cancelled, with the log its
/// verdict reads.
fn cancelled(runner: &ScriptedRunner, job: u64) {
    checks(
        runner,
        SHA,
        &[check_run(job, "test", "completed", Some("cancelled"))],
    );
    runner.set(&format!("actions/jobs/{job}/logs"), ok(""));
}

#[test]
fn ci_branch_advance_supersedes_old_sha() {
    let runner = Arc::new(github_origin());
    head(&runner, SHA);
    checks(&runner, SHA, &[check_run(11, "build", "in_progress", None)]);
    let watches = service(&watch_dir("branch-advance"), &runner);
    let sink = RecordingSink::live();
    let watch = branch_watch(&watches, SHA, false);

    head(&runner, REWRITTEN);
    watches.poll_once(&sink);

    let kept = watches.get(&watch.watch_id).expect("kept");
    assert_eq!(kept.state, CiState::Superseded);
    let texts = sink.texts();
    assert_eq!(texts.len(), 1, "the move wakes the owner once");
    let text = &texts[0];
    assert!(text.contains("state: superseded"), "{text}");
    assert!(
        text.contains(&format!("eventId: {}:superseded", watch.watch_id)),
        "the wake key names the supersession: {text}"
    );
    assert!(text.contains("main"), "the branch that moved: {text}");
    assert!(
        text.contains("0123456") && text.contains("fedcba9"),
        "the old commit and the new head: {text}"
    );
    assert!(
        !runner
            .calls()
            .iter()
            .any(|call| call.contains(&format!("commits/{SHA}/check-runs"))),
        "the superseded commit's checks are never read"
    );

    let reads = runner.calls().len();
    watches.poll_once(&sink);
    assert_eq!(
        runner.calls().len(),
        reads,
        "a superseded watch is not polled"
    );
    assert_eq!(sink.texts().len(), 1, "and it never wakes twice");
}

#[test]
fn ci_branch_force_push_supersedes() {
    // The head is rewritten to a commit the watched one never led to. The
    // daemon cannot tell a force-push from an advance, and must not keep
    // following a commit that is no longer the branch's head.
    let runner = Arc::new(github_origin());
    head(&runner, SHA);
    checks(&runner, SHA, &[check_run(11, "build", "in_progress", None)]);
    let watches = service(&watch_dir("branch-force"), &runner);
    let sink = RecordingSink::live();
    let watch = branch_watch(&watches, SHA, false);

    watches.poll_once(&sink);
    assert_eq!(
        watches.get(&watch.watch_id).expect("kept").state,
        CiState::Running
    );

    head(&runner, REWRITTEN);
    watches.poll_once(&sink);
    assert_eq!(
        watches.get(&watch.watch_id).expect("kept").state,
        CiState::Superseded
    );
    let texts = sink.texts();
    assert_eq!(texts.len(), 1);
    assert!(texts[0].contains("superseded"), "{}", texts[0]);
    assert!(texts[0].contains("fedcba9"), "{}", texts[0]);
}

#[test]
fn ci_code_failure_never_retries() {
    let runner = Arc::new(github_origin());
    checks(
        &runner,
        SHA,
        &[check_run(11, "test", "completed", Some("failure"))],
    );
    runner.set("actions/jobs/11/logs", ok("error: assertion failed\n"));
    let watches = service(&watch_dir("retry-code"), &runner);
    let sink = RecordingSink::live();
    let watch = sha_watch(&watches, true);

    watches.poll_once(&sink);

    let kept = watches.get(&watch.watch_id).expect("kept");
    assert_eq!(kept.state, CiState::Failed);
    assert_eq!(kept.retry_count, 0);
    assert_eq!(reruns(&runner), 0, "a code failure is never re-run");
    let texts = sink.texts();
    assert_eq!(texts.len(), 1);
    assert!(texts[0].contains("cause: CODE"), "{}", texts[0]);
    assert!(texts[0].contains("[CODE]"), "{}", texts[0]);
}

#[test]
fn ci_mixed_failure_never_retries() {
    let runner = Arc::new(github_origin());
    checks(
        &runner,
        SHA,
        &[
            check_run(11, "test", "completed", Some("failure")),
            check_run(12, "build", "completed", Some("cancelled")),
        ],
    );
    runner.set("actions/jobs/11/logs", ok("error: assertion failed\n"));
    runner.set("actions/jobs/12/logs", ok(""));
    let watches = service(&watch_dir("retry-mixed"), &runner);
    let sink = RecordingSink::live();
    let watch = sha_watch(&watches, true);

    watches.poll_once(&sink);

    let kept = watches.get(&watch.watch_id).expect("kept");
    assert_eq!(kept.state, CiState::Failed);
    assert_eq!(kept.retry_count, 0);
    assert_eq!(
        reruns(&runner),
        0,
        "one code failure makes the whole failure the code's"
    );
    let texts = sink.texts();
    assert_eq!(texts.len(), 1);
    assert!(texts[0].contains("cause: CODE"), "{}", texts[0]);
    assert!(
        texts[0].contains("[INFRA: the job was cancelled]"),
        "the platform's own job is still labelled: {}",
        texts[0]
    );
}

#[test]
fn ci_infra_retries_once() {
    let runner = Arc::new(github_origin());
    cancelled(&runner, 11);
    runner.set("run rerun", ok("✓ Requested rerun of run 900"));
    let watches = service(&watch_dir("retry-infra"), &runner);
    let sink = RecordingSink::live();
    let watch = sha_watch(&watches, true);

    watches.poll_once(&sink);

    let kept = watches.get(&watch.watch_id).expect("kept");
    assert_eq!(kept.retry_count, 1);
    assert!(kept.retry_issued, "gh took the re-run");
    assert_eq!(kept.retried_runs, vec![RUN]);
    assert!(
        !kept.state.is_terminal(),
        "the watch keeps following the same commit"
    );
    assert!(
        sink.texts().is_empty(),
        "no verdict while the re-run is only asked for"
    );
    assert_eq!(reruns(&runner), 1);
    assert!(
        runner
            .calls()
            .iter()
            .any(|call| call.contains("run rerun --failed 900 --repo github.com/acme/widgets")),
        "the failed run is re-run on the origin's host: {:?}",
        runner.calls()
    );

    // GitHub still answers with the old attempt for a while: that failure is
    // exactly what the re-run was asked to replace, so it is not the verdict.
    watches.poll_once(&sink);
    assert!(
        !watches
            .get(&watch.watch_id)
            .expect("kept")
            .state
            .is_terminal(),
        "the old attempt never closes an accepted re-run"
    );
    assert!(sink.texts().is_empty(), "and never wakes the owner");
    assert_eq!(reruns(&runner), 1);

    // Then the new attempt appears, queued.
    checks(&runner, SHA, &[check_run(21, "test", "queued", None)]);
    watches.poll_once(&sink);
    assert!(!watches
        .get(&watch.watch_id)
        .expect("kept")
        .state
        .is_terminal());
    assert!(sink.texts().is_empty());

    // And it fails the same way: the one retry is spent, so this is the
    // verdict, and it says which re-run it is reading.
    cancelled(&runner, 21);
    watches.poll_once(&sink);

    let kept = watches.get(&watch.watch_id).expect("kept");
    assert_eq!(kept.state, CiState::Failed);
    assert_eq!(reruns(&runner), 1, "never a second retry");
    let texts = sink.texts();
    assert_eq!(texts.len(), 1);
    assert!(
        texts[0].contains("[INFRA: the job was cancelled]"),
        "{}",
        texts[0]
    );
    assert!(
        texts[0].contains("infra retry: one re-run was issued for run(s) 900."),
        "{}",
        texts[0]
    );
}

#[test]
fn ci_retry_needs_watch_time_approval() {
    let runner = Arc::new(github_origin());
    cancelled(&runner, 11);
    runner.set("run rerun", ok("✓ Requested rerun of run 900"));
    let watches = service(&watch_dir("retry-unapproved"), &runner);
    let sink = RecordingSink::live();
    let watch = sha_watch(&watches, false);

    watches.poll_once(&sink);

    let kept = watches.get(&watch.watch_id).expect("kept");
    assert_eq!(kept.state, CiState::Failed);
    assert_eq!(kept.retry_count, 0);
    assert_eq!(
        reruns(&runner),
        0,
        "without the watch-time approval there is never a retry"
    );
    let texts = sink.texts();
    assert_eq!(texts.len(), 1);
    assert!(texts[0].contains("cause: INFRA"), "{}", texts[0]);
}

#[test]
fn ci_completion_survives_daemon_restart() {
    let dir = watch_dir("restart-branch");
    let runner = Arc::new(github_origin());
    head(&runner, SHA);
    checks(&runner, SHA, &[check_run(11, "build", "in_progress", None)]);
    let sink = RecordingSink::live();
    let watch_id = {
        let watches = service(&dir, &runner);
        let watch = branch_watch(&watches, SHA, false);
        watches.poll_once(&sink);
        assert_eq!(
            watches.get(&watch.watch_id).expect("kept").state,
            CiState::Running
        );
        assert!(sink.texts().is_empty());
        watch.watch_id
    };

    // The daemon comes back: the watch still knows the branch it follows and
    // finishes on the head it was resolved to.
    let restarted = service(&dir, &runner);
    checks(
        &runner,
        SHA,
        &[check_run(11, "build", "completed", Some("success"))],
    );
    restarted.poll_once(&sink);
    restarted.poll_once(&sink);
    let kept = restarted.get(&watch_id).expect("kept");
    assert_eq!(kept.state, CiState::Passed);
    assert_eq!(kept.branch.as_deref(), Some(BRANCH));
    assert_eq!(sink.texts().len(), 1, "the restart wakes its owner once");

    let again = service(&dir, &runner);
    again.poll_once(&sink);
    again.poll_once(&sink);
    assert_eq!(
        sink.texts().len(),
        1,
        "a completed watch wakes nobody again"
    );
}

#[test]
fn ci_retry_not_reissued_after_restart() {
    let dir = watch_dir("retry-restart");
    let runner = Arc::new(github_origin());
    cancelled(&runner, 11);
    runner.set("run rerun", ok("✓ Requested rerun of run 900"));
    let sink = RecordingSink::live();
    let watch_id = {
        let watches = service(&dir, &runner);
        let watch = sha_watch(&watches, true);
        watches.poll_once(&sink);
        assert_eq!(reruns(&runner), 1);
        watch.watch_id
    };

    // The daemon died while the re-run was in flight, and GitHub still
    // answers with the old attempt's failure. The restarted watch resumes
    // the retry instead of closing on the failure it was asked to replace.
    let restarted = service(&dir, &runner);
    restarted.poll_once(&sink);
    assert!(
        !restarted.get(&watch_id).expect("kept").state.is_terminal(),
        "an in-flight retry is resumed, not closed"
    );
    assert!(sink.texts().is_empty());
    assert_eq!(
        reruns(&runner),
        1,
        "a retry already issued is never issued again"
    );

    // The new attempt arrives and finishes green: the resumed watch reads it.
    checks(
        &runner,
        SHA,
        &[check_run(21, "test", "completed", Some("success"))],
    );
    restarted.poll_once(&sink);
    let kept = restarted.get(&watch_id).expect("kept");
    assert_eq!(kept.state, CiState::Passed);
    assert_eq!(reruns(&runner), 1);
    let texts = sink.texts();
    assert_eq!(texts.len(), 1);
    assert!(
        texts[0].contains("infra retry: one re-run was issued for run(s) 900."),
        "{}",
        texts[0]
    );
}

/// A daemon that died between the reservation and `gh`'s answer: the re-run
/// may or may not be out, so the watch says that instead of reading as a
/// failure no retry was ever allowed for.
#[test]
fn ci_retry_not_confirmed_after_restart() {
    let dir = watch_dir("retry-unconfirmed");
    let runner = Arc::new(github_origin());
    cancelled(&runner, 11);
    runner.set("run rerun", ok("✓ Requested rerun of run 900"));
    let sink = RecordingSink::live();
    let watch_id = {
        let watches = service(&dir, &runner);
        let watch = sha_watch(&watches, true);
        watches.poll_once(&sink);
        watches.leave_retry_unissued(&watch.watch_id);
        watch.watch_id
    };

    let restarted = service(&dir, &runner);
    restarted.poll_once(&sink);

    let kept = restarted.get(&watch_id).expect("kept");
    assert_eq!(kept.state, CiState::Failed);
    assert_eq!(kept.retry_count, 1);
    assert!(!kept.retry_issued);
    assert_eq!(
        reruns(&runner),
        1,
        "an unconfirmed retry is never asked for again"
    );
    let texts = sink.texts();
    assert_eq!(texts.len(), 1);
    assert!(texts[0].contains("never confirmed"), "{}", texts[0]);
    assert!(
        texts[0].contains("[INFRA: the job was cancelled]"),
        "the verdict that was there still rides: {}",
        texts[0]
    );
}

#[test]
fn a_later_call_brings_the_retry_approval_to_the_watch_it_already_has() {
    let runner = Arc::new(github_origin());
    let watches = service(&watch_dir("retry-later"), &runner);
    let first = sha_watch(&watches, false);
    assert!(!first.retry_approved);

    let second = sha_watch(&watches, true);
    assert_eq!(second.watch_id, first.watch_id, "it is the same watch");
    assert!(second.retry_approved, "the yes lands on it");
    assert!(watches.get(&first.watch_id).expect("kept").retry_approved);
}

/// The log cannot be read, so why the job failed is unknown — and unknown is
/// never the platform's doing, so it is never a reason to re-run.
#[test]
fn ci_log_read_error_never_retries() {
    let runner = Arc::new(github_origin());
    checks(
        &runner,
        SHA,
        &[check_run(11, "test", "completed", Some("failure"))],
    );
    runner.set(
        "actions/jobs/11/logs",
        fail(1, "gh: Resource not accessible (HTTP 403)"),
    );
    runner.set("run rerun", ok("✓ Requested rerun of run 900"));
    let watches = service(&watch_dir("retry-unknown"), &runner);
    let sink = RecordingSink::live();
    let watch = sha_watch(&watches, true);

    watches.poll_once(&sink);

    let kept = watches.get(&watch.watch_id).expect("kept");
    assert_eq!(kept.state, CiState::Failed);
    assert_eq!(kept.retry_count, 0);
    assert_eq!(reruns(&runner), 0, "an unread log proves nothing");
    let texts = sink.texts();
    assert_eq!(texts.len(), 1);
    assert!(texts[0].contains("cause: CODE"), "{}", texts[0]);
    assert!(
        texts[0].contains("[UNKNOWN: the log cannot be read with this login]"),
        "the reason still travels: {}",
        texts[0]
    );
}

/// The same commit asked for as a sha and as a branch is two watches: only
/// the second one follows a head, and asking again the same way answers it.
#[test]
fn ci_dedup_keeps_branch_mode() {
    let runner = Arc::new(github_origin());
    let watches = service(&watch_dir("dedup-branch"), &runner);

    let as_sha = watches
        .start("session-1", &owner(), &repo(), SHA, None, false)
        .expect("sha watch");
    let as_branch = watches
        .start("session-1", &owner(), &repo(), SHA, Some(BRANCH), false)
        .expect("branch watch");
    assert_ne!(
        as_sha.watch_id, as_branch.watch_id,
        "a commit watched as a head is its own watch"
    );
    assert!(as_sha.branch.is_none());
    assert_eq!(as_branch.branch.as_deref(), Some(BRANCH));

    let again = watches
        .start("session-1", &owner(), &repo(), SHA, Some(BRANCH), false)
        .expect("again");
    assert_eq!(again.watch_id, as_branch.watch_id);
    let other = watches
        .start("session-1", &owner(), &repo(), SHA, Some("release"), false)
        .expect("another branch");
    assert_ne!(other.watch_id, as_branch.watch_id);
    assert_eq!(watches.open().len(), 3);
}

/// Two calls racing for the same key cannot both insert: one watch, one retry
/// counter, one re-run.
#[test]
fn ci_concurrent_starts_issue_one_rerun() {
    let runner = Arc::new(github_origin());
    cancelled(&runner, 11);
    runner.set("run rerun", ok("✓ Requested rerun of run 900"));
    let watches = Arc::new(service(&watch_dir("retry-concurrent"), &runner));
    let sink = RecordingSink::live();

    // A barrier, so the calls really do land together rather than one after
    // the last one has already stored its watch.
    let calls = 8;
    let start_together = std::sync::Barrier::new(calls);
    std::thread::scope(|scope| {
        for _ in 0..calls {
            let watches = Arc::clone(&watches);
            let start_together = &start_together;
            scope.spawn(move || {
                start_together.wait();
                watches
                    .start("session-1", &owner(), &repo(), SHA, None, true)
                    .expect("start");
            });
        }
    });

    assert_eq!(watches.open().len(), 1, "one watch, not one per call");
    watches.poll_once(&sink);
    assert_eq!(reruns(&runner), 1, "one retry counter, one re-run");
}

/// The head moves while the checks and the log are being read: the newer fact
/// is the head, so the old commit's failure is never written as the verdict.
#[test]
fn ci_head_moves_during_reads_supersedes() {
    let runner = Arc::new(github_origin());
    // The pass starts on the watched commit...
    runner.answer_next("git/ref/heads/main", ok(&branch_head(SHA)));
    runner.set("git/ref/heads/main", ok(&branch_head(REWRITTEN)));
    checks(
        &runner,
        SHA,
        &[check_run(11, "test", "completed", Some("failure"))],
    );
    runner.set("actions/jobs/11/logs", ok("error: boom\n"));
    let watches = service(&watch_dir("branch-race"), &runner);
    let sink = RecordingSink::live();
    let watch = branch_watch(&watches, SHA, false);

    // ...and by the time the checks and the log are in, it moved on.
    watches.poll_once(&sink);

    assert_eq!(
        watches.get(&watch.watch_id).expect("kept").state,
        CiState::Superseded
    );
    let texts = sink.texts();
    assert_eq!(texts.len(), 1);
    assert!(texts[0].contains("superseded"), "{}", texts[0]);
    assert!(texts[0].contains("fedcba9"), "{}", texts[0]);
    assert!(
        !texts[0].contains("cause:"),
        "no verdict is written for a commit that is not the head: {}",
        texts[0]
    );
}

/// A refused re-run does not cost the owner the verdict: the refusal is one
/// more line, and the jobs that failed are still listed.
#[test]
fn a_refused_retry_keeps_the_verdict() {
    let runner = Arc::new(github_origin());
    cancelled(&runner, 11);
    runner.set(
        "run rerun",
        fail(1, "gh: Resource not accessible (HTTP 403)"),
    );
    let watches = service(&watch_dir("retry-refused"), &runner);
    let sink = RecordingSink::live();
    let watch = sha_watch(&watches, true);

    watches.poll_once(&sink);

    let kept = watches.get(&watch.watch_id).expect("kept");
    assert_eq!(kept.state, CiState::Failed);
    assert_eq!(kept.retry_count, 1);
    assert!(!kept.retry_issued, "gh did not take it");
    assert_eq!(reruns(&runner), 1, "and it is never asked for again");
    let texts = sink.texts();
    assert_eq!(texts.len(), 1);
    let text = &texts[0];
    assert!(
        text.contains("gh refused the re-run of run(s) 900"),
        "{text}"
    );
    assert!(text.contains("permission_required"), "{text}");
    assert!(text.contains("- job 11: cancelled"), "{text}");
    assert!(
        text.contains("[INFRA: the job was cancelled]"),
        "the per-job verdict still rides: {text}"
    );
}

/// A branch name is an agent's text like any other: it is redacted before it
/// rides a wake.
#[test]
fn a_superseded_wake_redacts_a_token_shaped_branch() {
    const SECRET: &str = "abcdefghijklmnop";
    let branch = format!("token={SECRET}");
    let runner = Arc::new(github_origin());
    runner.set(&format!("git/ref/heads/{branch}"), ok(&branch_head(SHA)));
    checks(&runner, SHA, &[check_run(11, "build", "in_progress", None)]);
    let watches = service(&watch_dir("branch-secret"), &runner);
    let sink = RecordingSink::live();
    watches
        .start("session-1", &owner(), &repo(), SHA, Some(&branch), false)
        .expect("start");

    runner.set(
        &format!("git/ref/heads/{branch}"),
        ok(&branch_head(REWRITTEN)),
    );
    watches.poll_once(&sink);

    let texts = sink.texts();
    assert_eq!(texts.len(), 1);
    assert!(
        !texts[0].contains(SECRET),
        "a branch name is redacted like a summary: {}",
        texts[0]
    );
    assert!(texts[0].contains("superseded"), "{}", texts[0]);
}
