//! Test doubles for the CI watch: a scripted `gh`/`git` and a recording
//! wake target, so no test needs a network, a login or a session.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use devboule_protocol::{ErrorCode, OwnerId, WireError};
use serde_json::{json, Value};

use crate::ci_gh::CommandRunner;
use crate::ci_wake::WakeSink;
use crate::git::{GitOutput, GitRunError};
use crate::session::SendError;

pub(crate) const SHA: &str = "0123456789abcdef0123456789abcdef01234567";

type Answer = Result<GitOutput, GitRunError>;

/// Answers each command line from the first script entry whose needle it
/// contains; the most recently set entry for a needle wins.
#[derive(Default)]
pub(crate) struct ScriptedRunner {
    script: Mutex<Vec<(String, Answer)>>,
    calls: Mutex<Vec<String>>,
    last_timeout: Mutex<Option<std::time::Duration>>,
}

impl ScriptedRunner {
    pub(crate) fn set(&self, needle: &str, answer: Answer) {
        let mut script = self.script.lock().expect("script");
        script.retain(|(existing, _)| existing != needle);
        script.insert(0, (needle.to_string(), answer));
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
        "html_url": format!("https://github.com/acme/widgets/actions/runs/900/job/{id}"),
        "app": {"slug": "github-actions"},
        "output": {"title": null, "summary": null, "text": null},
    })
}

pub(crate) fn check_runs(runs: &[Value]) -> String {
    json!({"total_count": runs.len(), "check_runs": runs}).to_string()
}

/// A scripted GitHub where the commit exists and the origin is `acme/widgets`.
pub(crate) fn github_with_commit() -> ScriptedRunner {
    let runner = ScriptedRunner::default();
    runner.set("git -C", ok("https://github.com/acme/widgets.git\n"));
    runner.set(
        &format!("git/commits/{SHA}"),
        ok(&format!("{{\"sha\":\"{SHA}\"}}")),
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
