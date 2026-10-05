//! Test doubles for the CI watch: a scripted `gh`/`git` and a recording
//! wake target, so no test needs a network, a login or a session.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use devboule_protocol::OwnerId;
use serde_json::{json, Value};

use crate::ci_gh::CommandRunner;
use crate::ci_wake::WakeSink;
use crate::git::{GitOutput, GitRunError};

pub(crate) const SHA: &str = "0123456789abcdef0123456789abcdef01234567";

type Answer = Result<GitOutput, GitRunError>;

/// Answers each command line from the first script entry whose needle it
/// contains; the most recently set entry for a needle wins.
#[derive(Default)]
pub(crate) struct ScriptedRunner {
    script: Mutex<Vec<(String, Answer)>>,
    calls: Mutex<Vec<String>>,
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

#[derive(Default)]
pub(crate) struct RecordingSink {
    pub(crate) live: AtomicBool,
    pub(crate) refuse: AtomicBool,
    pub(crate) delivered: Mutex<Vec<String>>,
}

impl RecordingSink {
    pub(crate) fn live() -> Self {
        let sink = Self::default();
        sink.live.store(true, Ordering::SeqCst);
        sink
    }

    pub(crate) fn texts(&self) -> Vec<String> {
        self.delivered.lock().expect("delivered").clone()
    }
}

impl WakeSink for RecordingSink {
    fn is_live(&self, _session_id: &str, _owner: &OwnerId) -> bool {
        self.live.load(Ordering::SeqCst)
    }

    fn deliver(&self, _session_id: &str, _owner: &OwnerId, text: &str) -> Result<(), String> {
        if self.refuse.load(Ordering::SeqCst) {
            return Err("the session did not take it".to_string());
        }
        self.delivered
            .lock()
            .expect("delivered")
            .push(text.to_string());
        Ok(())
    }
}
