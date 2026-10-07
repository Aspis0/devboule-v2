//! Branch mode, supersession and the one approved infra retry, over a
//! scripted GitHub: a branch head that moves ends the watch that was
//! following it, and only a failure that is entirely the platform's is ever
//! re-run, once.

use std::sync::Arc;

use super::CiWatches;
use crate::ci_summary::CiState;
use crate::ci_test_support::{
    branch_head, check_run, checks, github_origin, ok, owner, repo, service, watch_dir,
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

    // The re-run's own attempt: still running, so the watch waits.
    checks(&runner, SHA, &[check_run(21, "test", "in_progress", None)]);
    watches.poll_once(&sink);
    assert_eq!(
        watches.get(&watch.watch_id).expect("kept").state,
        CiState::Running
    );

    // It failed the same way: the one retry is spent, so this is the verdict.
    cancelled(&runner, 22);
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

    // The daemon died after the re-run went out; the same failed checks come
    // back. The retry is spent on disk, so it is never asked for twice.
    let restarted = service(&dir, &runner);
    restarted.poll_once(&sink);
    let kept = restarted.get(&watch_id).expect("kept");
    assert_eq!(kept.state, CiState::Failed);
    assert_eq!(kept.retry_count, 1);
    assert_eq!(
        reruns(&runner),
        1,
        "a retry already issued is never issued again"
    );
    assert_eq!(sink.texts().len(), 1);
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
