//! The watch service over a scripted GitHub: an exact commit is followed to a
//! verdict, the owner is woken once, and nothing wakes twice across a retry
//! or a restart.

use std::sync::atomic::Ordering;
use std::sync::Arc;

use devboule_protocol::OwnerId;

use super::{CiWatches, WakeStatus};
use crate::ci_gh::{GhClient, RepoRef};
use crate::ci_summary::CiState;
use crate::ci_test_support::{
    check_run, check_run_pages, check_runs, fail, github_origin, ok, RecordingSink, ScriptedRunner,
    SinkOutcome, SHA,
};
use crate::ci_watch_store::CiWatchStore;

fn owner() -> OwnerId {
    OwnerId::new("user", "client").expect("owner")
}

fn repo() -> RepoRef {
    RepoRef {
        host: "github.com".to_string(),
        owner: "acme".to_string(),
        repo: "widgets".to_string(),
    }
}

fn service(dir: &std::path::Path, runner: &Arc<ScriptedRunner>) -> CiWatches {
    CiWatches::new(CiWatchStore::load(dir), GhClient::new(runner.clone()))
}

fn checks(runner: &ScriptedRunner, runs: &[serde_json::Value]) {
    runner.set(&format!("commits/{SHA}/check-runs"), ok(&check_runs(runs)));
}

fn dir(tag: &str) -> std::path::PathBuf {
    crate::test_dirs::test_temp_dir(&format!("ci-watch-{tag}"))
}

#[test]
fn ci_watch_exact_sha_wakes_once() {
    let dir = dir("once");
    let runner = Arc::new(github_origin());
    checks(&runner, &[check_run(11, "build", "queued", None)]);
    runner.set(
        "actions/jobs/12/logs",
        ok("2025-01-01T00:00:00.0000000Z error[E0599]: no method named frobnicate\n"),
    );
    let watches = service(&dir, &runner);
    let sink = RecordingSink::live();

    let watch = watches
        .start("session-1", &owner(), &repo(), SHA)
        .expect("start");
    assert_eq!(watch.state, CiState::Queued);
    assert_eq!(
        watch.sha, SHA,
        "the exact commit asked for is the one watched"
    );

    checks(&runner, &[check_run(11, "build", "in_progress", None)]);
    watches.poll_once(&sink);
    assert_eq!(
        watches.get(&watch.watch_id).expect("kept").state,
        CiState::Running
    );
    assert!(sink.texts().is_empty(), "nothing is sent while CI runs");

    checks(
        &runner,
        &[
            check_run(11, "build", "completed", Some("success")),
            check_run(12, "test", "completed", Some("failure")),
        ],
    );
    watches.poll_once(&sink);
    watches.poll_once(&sink);
    watches.poll_once(&sink);

    let texts = sink.texts();
    assert_eq!(texts.len(), 1, "the verdict wakes its owner exactly once");
    let text = &texts[0];
    assert!(text.contains("role: daemon"), "{text}");
    assert!(text.contains("kind: ci_verdict"), "{text}");
    assert!(
        text.contains(&format!("eventId: {}:failed", watch.watch_id)),
        "the idempotency key is watch id and verdict: {text}"
    );
    assert!(text.contains("- job 11: success"), "{text}");
    assert!(text.contains("- job 12: failure [CODE]"), "{text}");
    assert!(text.contains("name: test"), "{text}");
    assert!(text.contains("no method named frobnicate"), "{text}");

    let asked_other_commit = runner
        .calls()
        .iter()
        .any(|call| call.contains("check-runs") && !call.contains(SHA));
    assert!(!asked_other_commit, "only the exact commit is read");
    assert_eq!(
        watches.wake_status(&watches.get(&watch.watch_id).expect("kept"), &sink),
        WakeStatus::Delivered
    );
}

#[test]
fn a_restart_after_completion_does_not_wake_again() {
    let dir = dir("restart");
    let runner = Arc::new(github_origin());
    checks(
        &runner,
        &[check_run(11, "build", "completed", Some("success"))],
    );
    let sink = RecordingSink::live();
    let watch_id = {
        let watches = service(&dir, &runner);
        let watch = watches
            .start("session-1", &owner(), &repo(), SHA)
            .expect("start");
        watches.poll_once(&sink);
        assert_eq!(sink.texts().len(), 1);
        watch.watch_id
    };

    let restarted = service(&dir, &runner);
    restarted.poll_once(&sink);
    restarted.poll_once(&sink);
    assert_eq!(sink.texts().len(), 1, "the restart finds nothing owed");
    assert_eq!(
        restarted.get(&watch_id).expect("kept").state,
        CiState::Passed
    );
}

#[test]
fn a_verdict_recorded_before_a_crash_is_delivered_after_the_restart() {
    let dir = dir("crash");
    let runner = Arc::new(github_origin());
    checks(
        &runner,
        &[check_run(11, "build", "completed", Some("failure"))],
    );
    runner.set("actions/jobs/11/logs", ok("error: boom\n"));
    let gone = RecordingSink::default();
    {
        let watches = service(&dir, &runner);
        watches
            .start("session-1", &owner(), &repo(), SHA)
            .expect("start");
        watches.poll_once(&gone);
        assert!(gone.texts().is_empty(), "the owner is not there to hear it");
    }

    let restarted = service(&dir, &runner);
    let live = RecordingSink::live();
    restarted.poll_once(&live);
    restarted.poll_once(&live);
    assert_eq!(
        live.texts().len(),
        1,
        "recorded, then woken once after the restart"
    );
}

#[test]
fn a_wake_is_kept_while_the_owner_is_gone_and_reported() {
    let dir = dir("ended");
    let runner = Arc::new(github_origin());
    checks(
        &runner,
        &[check_run(11, "build", "completed", Some("success"))],
    );
    let watches = service(&dir, &runner);
    let sink = RecordingSink::default();
    let watch = watches
        .start("session-1", &owner(), &repo(), SHA)
        .expect("start");
    watches.poll_once(&sink);

    let kept = watches.get(&watch.watch_id).expect("kept");
    assert_eq!(kept.state, CiState::Passed, "the verdict is kept");
    assert_eq!(
        watches.wake_status(&kept, &sink),
        WakeStatus::OwnerSessionEnded
    );

    sink.live.store(true, Ordering::SeqCst);
    watches.poll_once(&sink);
    assert_eq!(sink.texts().len(), 1, "a resumed owner still gets it");
}

#[test]
fn a_delivery_that_did_not_happen_is_retried_not_lost() {
    let dir = dir("refused");
    let runner = Arc::new(github_origin());
    checks(
        &runner,
        &[check_run(11, "build", "completed", Some("success"))],
    );
    let watches = service(&dir, &runner);
    let sink = RecordingSink::refusing();
    watches
        .start("session-1", &owner(), &repo(), SHA)
        .expect("start");

    watches.poll_once(&sink);
    assert!(sink.texts().is_empty());
    *sink.outcome.lock().expect("outcome") = SinkOutcome::Accept;
    watches.poll_once(&sink);
    watches.poll_once(&sink);
    assert_eq!(sink.texts().len(), 1);
}

#[test]
fn asking_again_for_the_same_commit_returns_the_same_watch() {
    let dir = dir("same");
    let runner = Arc::new(github_origin());
    checks(&runner, &[check_run(11, "build", "queued", None)]);
    let watches = service(&dir, &runner);
    let first = watches
        .start("session-1", &owner(), &repo(), SHA)
        .expect("first");
    let second = watches
        .start("session-1", &owner(), &repo(), SHA)
        .expect("second");
    assert_eq!(first.watch_id, second.watch_id);
}

/// The commit's existence is a GitHub read like any other, so it is the poll
/// thread's to find out: the watch ends and the owner is told which commit.
#[test]
fn a_commit_github_does_not_have_ends_the_watch_with_the_sha() {
    let dir = dir("missing");
    let runner = Arc::new(github_origin());
    runner.set(
        &format!("commits/{SHA}/check-runs"),
        fail(1, "gh: No commit found for SHA: 0123 (HTTP 422)"),
    );
    let watches = service(&dir, &runner);
    let sink = RecordingSink::live();
    let watch = watches
        .start("session-1", &owner(), &repo(), SHA)
        .expect("a watch is registered without asking GitHub");

    watches.poll_once(&sink);
    assert_eq!(
        watches.get(&watch.watch_id).expect("kept").state,
        CiState::Failed
    );
    let texts = sink.texts();
    assert_eq!(texts.len(), 1);
    assert!(texts[0].contains("sha_not_found"), "{}", texts[0]);
    assert!(texts[0].contains("0123456"), "{}", texts[0]);
}

#[test]
fn losing_the_login_mid_watch_ends_it_with_the_reason() {
    let dir = dir("lost-login");
    let runner = Arc::new(github_origin());
    checks(&runner, &[check_run(11, "build", "in_progress", None)]);
    let watches = service(&dir, &runner);
    let sink = RecordingSink::live();
    let watch = watches
        .start("session-1", &owner(), &repo(), SHA)
        .expect("start");

    runner.set(
        &format!("commits/{SHA}/check-runs"),
        fail(
            4,
            "To get started with GitHub CLI, please run:  gh auth login",
        ),
    );
    watches.poll_once(&sink);
    let kept = watches.get(&watch.watch_id).expect("kept");
    assert_eq!(kept.state, CiState::Failed);
    let texts = sink.texts();
    assert_eq!(texts.len(), 1);
    assert!(
        texts[0].contains("github_auth_required"),
        "the owner learns why the watch stopped: {}",
        texts[0]
    );
}

#[test]
fn a_hiccup_is_waited_out() {
    let dir = dir("hiccup");
    let runner = Arc::new(github_origin());
    checks(&runner, &[check_run(11, "build", "in_progress", None)]);
    let watches = service(&dir, &runner);
    let sink = RecordingSink::live();
    let watch = watches
        .start("session-1", &owner(), &repo(), SHA)
        .expect("start");
    runner.set(
        &format!("commits/{SHA}/check-runs"),
        fail(1, "gh: connection reset by peer"),
    );
    watches.poll_once(&sink);
    assert_eq!(
        watches.get(&watch.watch_id).expect("kept").state,
        CiState::Queued,
        "a retryable failure does not end the watch"
    );
    assert!(sink.texts().is_empty());
}

#[test]
fn an_uncertain_send_is_settled_never_repeated() {
    let dir = dir("uncertain");
    let runner = Arc::new(github_origin());
    checks(
        &runner,
        &[check_run(11, "build", "completed", Some("failure"))],
    );
    runner.set("actions/jobs/11/logs", ok("error: boom\n"));
    let watches = service(&dir, &runner);
    let sink = RecordingSink::live();
    *sink.outcome.lock().expect("outcome") = SinkOutcome::Uncertain;
    let watch = watches
        .start("session-1", &owner(), &repo(), SHA)
        .expect("start");

    watches.poll_once(&sink);
    assert!(
        sink.texts().is_empty(),
        "an uncertain send is never confirmed"
    );
    let kept = watches.get(&watch.watch_id).expect("kept");
    assert_eq!(
        kept.wake,
        crate::ci_watch_store::Wake::DeliveredUncertain,
        "the claim settles instead of going back"
    );
    assert_eq!(
        watches.wake_status(&kept, &sink),
        WakeStatus::DeliveredUncertain
    );
    assert_eq!(
        watches.wake_status(&kept, &sink).as_str(),
        "delivered_uncertain"
    );

    watches.poll_once(&sink);
    watches.poll_once(&sink);
    assert!(
        sink.texts().is_empty(),
        "no later pass serves the settled wake"
    );
}

/// A repository whose commit exists but has no checks registered at all: the
/// overdue close must say that, not report a build that failed.
#[test]
fn a_commit_with_no_checks_is_not_reported_as_a_failed_build() {
    let dir = dir("no-checks");
    let runner = Arc::new(github_origin());
    checks(&runner, &[]);
    let watches = service(&dir, &runner);
    let sink = RecordingSink::live();
    let watch = watches
        .start("session-1", &owner(), &repo(), SHA)
        .expect("start");
    watches.age(&watch.watch_id, crate::ci_watch::OVERDUE_AGE);

    watches.poll_once(&sink);
    let kept = watches.get(&watch.watch_id).expect("kept");
    let summary = kept.summary.expect("a closed watch says why");
    assert!(summary.contains("no checks registered"), "{summary}");
    assert!(
        !summary.contains("no CI result within 6 hours"),
        "a commit with no workflow is not a build that never finished: {summary}"
    );
}

/// A watch past its deadline with checks that simply never finished keeps the
/// plain overdue sentence.
#[test]
fn a_watch_that_timed_out_says_the_result_never_arrived() {
    let dir = dir("overdue");
    let runner = Arc::new(github_origin());
    checks(&runner, &[check_run(11, "build", "in_progress", None)]);
    let watches = service(&dir, &runner);
    let sink = RecordingSink::live();
    let watch = watches
        .start("session-1", &owner(), &repo(), SHA)
        .expect("start");
    watches.age(&watch.watch_id, crate::ci_watch::OVERDUE_AGE);

    watches.poll_once(&sink);
    let kept = watches.get(&watch.watch_id).expect("kept");
    let summary = kept.summary.expect("a closed watch says why");
    assert!(summary.contains("no CI result within 6 hours"), "{summary}");
}

/// A claim found after a restart may or may not have gone out, so the tool
/// reports the uncertainty rather than a delivery it cannot vouch for.
#[test]
fn an_unsettled_claim_reads_as_uncertain_not_delivered() {
    let dir = dir("unsettled");
    let runner = Arc::new(github_origin());
    checks(
        &runner,
        &[check_run(11, "build", "completed", Some("failure"))],
    );
    runner.set("actions/jobs/11/logs", ok("error: boom\n"));
    let watches = service(&dir, &runner);
    let sink = RecordingSink::live();
    let watch = watches
        .start("session-1", &owner(), &repo(), SHA)
        .expect("start");
    watches.poll_once(&sink);

    // A daemon that died between the claim and the send leaves `sending` on
    // disk; a fresh read of that record must not claim a delivery.
    let settled = watches.get(&watch.watch_id).expect("kept");
    watches.leave_claim_unsettled(&watch.watch_id);
    let after_restart = watches.get(&watch.watch_id).expect("kept");
    assert_eq!(settled.wake, crate::ci_watch_store::Wake::Delivered);
    assert_eq!(
        watches.wake_status(&after_restart, &sink),
        WakeStatus::DeliveredUncertain,
        "a crash mid-send cannot be reported as a delivery"
    );
}

/// The verdict reads every check of the commit, not the first hundred: a
/// truncated list can call a broken commit green.
#[test]
fn the_check_run_list_is_asked_for_whole() {
    let dir = dir("paginate");
    let runner = Arc::new(github_origin());
    checks(
        &runner,
        &[check_run(11, "build", "completed", Some("failure"))],
    );
    let watches = service(&dir, &runner);
    let sink = RecordingSink::live();
    watches
        .start("session-1", &owner(), &repo(), SHA)
        .expect("start");
    watches.poll_once(&sink);

    let read = runner
        .calls()
        .into_iter()
        .find(|call| call.contains("check-runs"))
        .expect("the check-run read is recorded");
    assert!(
        read.contains("--paginate --slurp"),
        "gh prints one JSON value per page unless the pages are slurped: {read}"
    );
}

/// Two pages, the failure on the second: the verdict is failed, which the
/// first page alone would have read as passed.
#[test]
fn a_failure_on_the_second_page_is_the_verdict() {
    let dir = dir("two-pages");
    let runner = Arc::new(github_origin());
    let first: Vec<_> = (1..=100)
        .map(|id| check_run(id, "build", "completed", Some("success")))
        .collect();
    let second = [check_run(101, "deploy", "completed", Some("failure"))];
    runner.set(
        &format!("commits/{SHA}/check-runs"),
        ok(&check_run_pages(&[&first, &second])),
    );
    runner.set("actions/jobs/101/logs", ok("error: deploy refused\n"));
    let watches = service(&dir, &runner);
    let sink = RecordingSink::live();
    let watch = watches
        .start("session-1", &owner(), &repo(), SHA)
        .expect("start");

    watches.poll_once(&sink);
    assert_eq!(
        watches.get(&watch.watch_id).expect("kept").state,
        CiState::Failed
    );
    let texts = sink.texts();
    assert_eq!(texts.len(), 1);
    assert!(texts[0].contains("CI failed for"), "{}", texts[0]);
}

/// A page that did not arrive is not a green: the watch waits and asks again.
#[test]
fn a_partial_check_run_list_gives_no_verdict() {
    let dir = dir("partial");
    let runner = Arc::new(github_origin());
    let only = [check_run(1, "build", "completed", Some("success"))];
    runner.set(
        &format!("commits/{SHA}/check-runs"),
        ok(&serde_json::json!([{"total_count": 2, "check_runs": only}]).to_string()),
    );
    let watches = service(&dir, &runner);
    let sink = RecordingSink::live();
    let watch = watches
        .start("session-1", &owner(), &repo(), SHA)
        .expect("start");

    watches.poll_once(&sink);
    assert!(!watches
        .get(&watch.watch_id)
        .expect("kept")
        .state
        .is_terminal());
    assert!(sink.texts().is_empty(), "no verdict off half a list");
}

/// A finished pass reads its job logs under their own fuse: one missing log
/// must not hold the poll thread past the watches queued behind it.
#[test]
fn the_job_logs_read_under_their_own_fuse() {
    use crate::ci_gh::LOG_GH_TIMEOUT;

    let dir = dir("log-fuse");
    let runner = Arc::new(github_origin());
    checks(
        &runner,
        &[check_run(11, "build", "completed", Some("failure"))],
    );
    runner.set("actions/jobs/11/logs", ok("error: boom\n"));
    let watches = service(&dir, &runner);
    let sink = RecordingSink::live();
    watches
        .start("session-1", &owner(), &repo(), SHA)
        .expect("start");
    watches.poll_once(&sink);
    assert_eq!(runner.last_timeout(), Some(LOG_GH_TIMEOUT));
}

/// The tool call only registers the watch: no GitHub read happens on the
/// broker's request path, so it answers at once whatever `gh` is doing.
#[test]
fn the_tool_call_asks_github_nothing_and_the_poll_keeps_the_minute() {
    use crate::git::GIT_COMMAND_TIMEOUT;

    let dir = dir("timeouts");
    let runner = Arc::new(github_origin());
    checks(&runner, &[check_run(11, "build", "in_progress", None)]);
    let watches = service(&dir, &runner);
    let sink = RecordingSink::live();
    watches
        .start("session-1", &owner(), &repo(), SHA)
        .expect("start");
    assert!(
        runner.calls().is_empty(),
        "registering a watch reads nothing from GitHub"
    );
    watches.poll_once(&sink);
    assert_eq!(
        runner.last_timeout(),
        Some(GIT_COMMAND_TIMEOUT),
        "the poll side keeps the full minute"
    );
}
