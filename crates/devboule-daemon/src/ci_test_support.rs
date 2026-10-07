//! Test doubles for the CI watch: a scripted `gh`/`git`, a recording wake
//! target and the fixtures every watch test starts from, so no test needs a
//! network, a login or a session.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use devboule_protocol::{ErrorCode, OwnerId, WireError};
use serde_json::{json, Value};

use crate::ci_gh::{CommandRunner, GhClient, RepoRef};
use crate::ci_wake::WakeSink;
use crate::ci_watch::CiWatches;
use crate::ci_watch_store::CiWatchStore;
use crate::git::{GitOutput, GitRunError};
use crate::session::SendError;

pub(crate) const SHA: &str = "0123456789abcdef0123456789abcdef01234567";
/// The workflow run every scripted check run and attempt job belongs to.
pub(crate) const RUN_ID: u64 = 900;

type Answer = Result<GitOutput, GitRunError>;

/// Answers each command line from the first script entry whose needle it
/// contains; the most recently set entry for a needle wins. A one-shot
/// answer set with [`ScriptedRunner::answer_next`] is spent before the script
/// is consulted, which is how a test makes two reads of one endpoint differ.
#[derive(Default)]
pub(crate) struct ScriptedRunner {
    script: Mutex<Vec<(String, Answer)>>,
    next: Mutex<Vec<(String, Answer)>>,
    calls: Mutex<Vec<String>>,
    last_timeout: Mutex<Option<std::time::Duration>>,
}

impl ScriptedRunner {
    pub(crate) fn set(&self, needle: &str, answer: Answer) {
        let mut script = self.script.lock().expect("script");
        script.retain(|(existing, _)| existing != needle);
        script.insert(0, (needle.to_string(), answer));
    }

    /// The next matching call answers `answer`; the ones after it fall back to
    /// the script.
    pub(crate) fn answer_next(&self, needle: &str, answer: Answer) {
        self.next
            .lock()
            .expect("next")
            .insert(0, (needle.to_string(), answer));
    }

    pub(crate) fn calls(&self) -> Vec<String> {
        self.calls.lock().expect("calls").clone()
    }

    /// The deadline the last call arrived with, if it named one.
    pub(crate) fn last_timeout(&self) -> Option<std::time::Duration> {
        *self.last_timeout.lock().expect("timeout")
    }
}

impl CommandRunner for ScriptedRunner {
    fn run(&self, program: &str, args: &[String]) -> Answer {
        let line = format!("{program} {}", args.join(" "));
        self.calls.lock().expect("calls").push(line.clone());
        {
            let mut next = self.next.lock().expect("next");
            if let Some(index) = next
                .iter()
                .position(|(needle, _)| line.contains(needle.as_str()))
            {
                return next.remove(index).1;
            }
        }
        self.script
            .lock()
            .expect("script")
            .iter()
            .find(|(needle, _)| line.contains(needle.as_str()))
            .map_or(Err(GitRunError::SpawnFailed), |(_, answer)| answer.clone())
    }

    fn run_with_timeout(
        &self,
        program: &str,
        args: &[String],
        timeout: std::time::Duration,
    ) -> Answer {
        *self.last_timeout.lock().expect("timeout") = Some(timeout);
        self.run(program, args)
    }
}

pub(crate) fn ok(stdout: &str) -> Answer {
    Ok(GitOutput {
        success: true,
        code: Some(0),
        stdout: stdout.to_string(),
        stderr: String::new(),
    })
}

pub(crate) fn fail(code: i32, stderr: &str) -> Answer {
    Ok(GitOutput {
        success: false,
        code: Some(code),
        stdout: String::new(),
        stderr: stderr.to_string(),
    })
}

/// One GitHub Actions job as `commits/<sha>/check-runs` lists it.
pub(crate) fn check_run(id: u64, name: &str, status: &str, conclusion: Option<&str>) -> Value {
    json!({
        "id": id,
        "name": name,
        "status": status,
        "conclusion": conclusion,
        "html_url": format!("https://github.com/acme/widgets/actions/runs/{RUN_ID}/job/{id}"),
        "app": {"slug": "github-actions"},
        "output": {"title": null, "summary": null, "text": null},
    })
}

/// One check run of another app on the same commit: it has no workflow run
/// behind it, so nothing about it can be a re-run of one.
pub(crate) fn other_app_check_run(
    id: u64,
    name: &str,
    status: &str,
    conclusion: Option<&str>,
) -> Value {
    json!({
        "id": id,
        "name": name,
        "status": status,
        "conclusion": conclusion,
        "html_url": format!("https://example.test/checks/{id}"),
        "app": {"slug": "other-checker"},
        "output": {"title": null, "summary": null, "text": null},
    })
}

/// What `gh api repos/o/r/actions/runs/<id>` prints: the attempt the run is on
/// now and whether it has finished.
pub(crate) fn run_state(attempt: u64, status: &str) -> String {
    json!({"id": RUN_ID, "run_attempt": attempt, "status": status}).to_string()
}

/// One job of a workflow-run attempt, as the attempt's jobs endpoint lists
/// it. The run id is the one every scripted check run belongs to.
pub(crate) fn attempt_job(id: u64, name: &str, status: &str, conclusion: Option<&str>) -> Value {
    json!({
        "id": id,
        "run_id": RUN_ID,
        "name": name,
        "status": status,
        "conclusion": conclusion,
        "html_url": format!("https://github.com/acme/widgets/actions/runs/{RUN_ID}/job/{id}"),
    })
}

/// What `gh api --paginate --slurp repos/o/r/actions/runs/<id>/attempts/<n>/jobs`
/// prints: one page object per array entry, as the check-run read does.
pub(crate) fn attempt_jobs(jobs: &[Value]) -> String {
    json!([{"total_count": jobs.len(), "jobs": jobs}]).to_string()
}

/// What `gh api --paginate --slurp` prints for a single page of check runs.
pub(crate) fn check_runs(runs: &[Value]) -> String {
    check_run_pages(&[runs])
}

/// The same for several pages: an array with one page object per entry, each
/// stating the whole count as GitHub does.
pub(crate) fn check_run_pages(pages: &[&[Value]]) -> String {
    let total: usize = pages.iter().map(|runs| runs.len()).sum();
    let pages: Vec<Value> = pages
        .iter()
        .map(|runs| json!({"total_count": total, "check_runs": runs}))
        .collect();
    Value::Array(pages).to_string()
}

/// The check runs a single page of `items` parses to.
pub(crate) fn parsed_check_runs(items: &[Value]) -> Vec<crate::ci_summary::CheckRun> {
    let pages: Vec<Value> = serde_json::from_str(&check_runs(items)).expect("a page list");
    crate::ci_pages::join_check_run_pages(&pages).expect("a whole list")
}

/// A scripted workspace whose origin is `acme/widgets`; each test scripts the
/// GitHub answers it reads.
pub(crate) fn github_origin() -> ScriptedRunner {
    let runner = ScriptedRunner::default();
    runner.set(
        "git -C",
        ok("https://github.com/acme/widgets.git
"),
    );
    runner
}

/// How the fake sink answers a delivery: accepted, refused before any
/// write (the watch retries), or uncertain after the write began (the
/// watch must not repeat it). One value, never a combination.
#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum SinkOutcome {
    #[default]
    Accept,
    Refuse,
    Uncertain,
}

#[derive(Default)]
pub(crate) struct RecordingSink {
    pub(crate) live: AtomicBool,
    pub(crate) outcome: Mutex<SinkOutcome>,
    pub(crate) delivered: Mutex<Vec<String>>,
}

impl RecordingSink {
    pub(crate) fn live() -> Self {
        let sink = Self::default();
        sink.live.store(true, Ordering::SeqCst);
        sink
    }

    pub(crate) fn refusing() -> Self {
        let sink = Self::live();
        *sink.outcome.lock().expect("outcome") = SinkOutcome::Refuse;
        sink
    }

    pub(crate) fn texts(&self) -> Vec<String> {
        self.delivered.lock().expect("delivered").clone()
    }
}

fn denied() -> WireError {
    WireError::new(ErrorCode::Io, "the session did not take it")
}

impl WakeSink for RecordingSink {
    fn is_live(&self, _session_id: &str, _owner: &OwnerId) -> bool {
        self.live.load(Ordering::SeqCst)
    }

    fn deliver(&self, _session_id: &str, _owner: &OwnerId, text: &str) -> Result<(), SendError> {
        match *self.outcome.lock().expect("outcome") {
            SinkOutcome::Accept => {
                self.delivered
                    .lock()
                    .expect("delivered")
                    .push(text.to_string());
                Ok(())
            }
            SinkOutcome::Refuse => Err(SendError::Refused(denied())),
            SinkOutcome::Uncertain => Err(SendError::Uncertain(denied())),
        }
    }
}

pub(crate) fn owner() -> OwnerId {
    OwnerId::new("user", "client").expect("owner")
}

pub(crate) fn repo() -> RepoRef {
    RepoRef {
        host: "github.com".to_string(),
        owner: "acme".to_string(),
        repo: "widgets".to_string(),
    }
}

/// The watch service over a scripted GitHub, kept in `dir` so a test can
/// build a second one over the same store and mean a daemon restart.
pub(crate) fn service(dir: &std::path::Path, runner: &Arc<ScriptedRunner>) -> CiWatches {
    CiWatches::new(CiWatchStore::load(dir), GhClient::new(runner.clone()))
}

pub(crate) fn watch_dir(tag: &str) -> std::path::PathBuf {
    crate::test_dirs::test_temp_dir(&format!("ci-watch-{tag}"))
}

/// Script one commit's check runs, as `gh api --paginate --slurp` answers
/// them.
pub(crate) fn checks(runner: &ScriptedRunner, sha: &str, runs: &[Value]) {
    runner.set(&format!("commits/{sha}/check-runs"), ok(&check_runs(runs)));
}

/// What `gh api repos/o/r/git/ref/heads/<branch>` prints: one object naming
/// the commit a branch's head points at.
pub(crate) fn branch_head(sha: &str) -> String {
    json!({
        "ref": "refs/heads/main",
        "object": {"sha": sha, "type": "commit"},
    })
    .to_string()
}
