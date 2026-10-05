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
    check_run, check_runs, fail, github_with_commit, ok, RecordingSink, ScriptedRunner, SHA,
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
    let runner = Arc::new(github_with_commit());
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
    assert!(text.contains("- build: success"), "{text}");
    assert!(text.contains("- test: failure [CODE]"), "{text}");
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
    let runner = Arc::new(github_with_commit());
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
    let runner = Arc::new(github_with_commit());
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
    let runner = Arc::new(github_with_commit());
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
    let runner = Arc::new(github_with_commit());
    checks(
        &runner,
        &[check_run(11, "build", "completed", Some("success"))],
    );
    let watches = service(&dir, &runner);
    let sink = RecordingSink::live();
    sink.refuse.store(true, Ordering::SeqCst);
    watches
        .start("session-1", &owner(), &repo(), SHA)
        .expect("start");

    watches.poll_once(&sink);
    assert!(sink.texts().is_empty());
    sink.refuse.store(false, Ordering::SeqCst);
    watches.poll_once(&sink);
    watches.poll_once(&sink);
    assert_eq!(sink.texts().len(), 1);
}

#[test]
fn asking_again_for_the_same_commit_returns_the_same_watch() {
    let dir = dir("same");
    let runner = Arc::new(github_with_commit());
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

#[test]
fn a_commit_github_does_not_have_is_refused_with_the_sha() {
    let runner = Arc::new(github_with_commit());
    runner.set(
        &format!("git/commits/{SHA}"),
        fail(1, "gh: Not Found (HTTP 404)"),
    );
    let watches = service(&dir("missing"), &runner);
    let refused = watches
        .start("session-1", &owner(), &repo(), SHA)
        .expect_err("unknown commit");
    assert_eq!(refused.code, "sha_not_found");
    assert!(refused.message.contains("0123456"), "{}", refused.message);
}

#[test]
fn losing_the_login_mid_watch_ends_it_with_the_reason() {
    let dir = dir("lost-login");
    let runner = Arc::new(github_with_commit());
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
    let runner = Arc::new(github_with_commit());
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
        CiState::Running,
        "a retryable failure does not end the watch"
    );
    assert!(sink.texts().is_empty());
}
