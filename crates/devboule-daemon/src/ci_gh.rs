//! The GitHub CLI client behind the CI watch: which repository a workspace
//! belongs to, `gh api` reads, and the failures an owner can act on.
//!
//! One phrase: ask GitHub through the daemon user's own `gh` login. Every
//! spawn is an argument vector, never a shell; prompts are off; the token is
//! never requested or printed, and what `gh` says on failure is redacted
//! before it can travel.

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::Value;

use crate::ci_watch_store::now_ms;

use crate::git::{
    run_git_args_with_cap, run_program_args, GitOutput, GitRunError, GIT_COMMAND_TIMEOUT,
    GIT_STDOUT_MAX_BYTES,
};

/// Job logs are the large read; anything past this is cut, and the summary
/// only ever looks at the part it got.
const GH_OUTPUT_MAX_BYTES: usize = 4 * 1024 * 1024;
const ERROR_LINE_CHARS: usize = 300;
/// The environment every `gh` spawn gets: no interactive prompt, and no
/// colour in the output this reads as text.
const GH_ENV: [(&str, &str); 2] = [("GH_PROMPT_DISABLED", "1"), ("NO_COLOR", "1")];
/// The ceiling on a `gh` call that names no deadline of its own: the same
/// minute the git runner allows, stated here so `gh` never rides on git's
/// timeout by accident.
const GH_COMMAND_TIMEOUT: Duration = GIT_COMMAND_TIMEOUT;
/// The broker-facing validation waits seconds, not the full command minute:
/// a hung helper must not hold a tool call, and the slow path already lives
/// on the poll thread with the minute.
pub(crate) const TOOL_GH_TIMEOUT: Duration = Duration::from_secs(10);
/// The job logs of a finished pass: a download that either arrives or never
/// will, read under a fuse short enough that one missing log does not hold
/// the poll thread past the watches queued behind it.
pub(crate) const LOG_GH_TIMEOUT: Duration = Duration::from_secs(20);
/// A rate-limited answer quiets a repository this long when GitHub names no
/// reset time of its own.
const RATE_LIMIT_FALLBACK_MS: u64 = 60_000;

/// A refusal the tool contract names: the code, a sentence that says what to
/// do next, and whether asking again later can help.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CiError {
    pub(crate) code: &'static str,
    pub(crate) message: String,
    pub(crate) retryable: bool,
}

impl CiError {
    pub(crate) fn new(code: &'static str, message: impl Into<String>, retryable: bool) -> Self {
        Self {
            code,
            message: message.into(),
            retryable,
        }
    }
}

/// How the daemon starts `git` and `gh`; tests answer from a script instead.
pub(crate) trait CommandRunner: Send + Sync {
    fn run(&self, program: &str, args: &[String]) -> Result<GitOutput, GitRunError>;
    /// The same spawn with the caller's own deadline; the default keeps the
    /// historical behaviour so fakes only override what they measure.
    fn run_with_timeout(
        &self,
        program: &str,
        args: &[String],
        _timeout: Duration,
    ) -> Result<GitOutput, GitRunError> {
        self.run(program, args)
    }
}

pub(crate) struct ProcessRunner;

impl CommandRunner for ProcessRunner {
    fn run(&self, program: &str, args: &[String]) -> Result<GitOutput, GitRunError> {
        if program == "git" {
            return run_git_args_with_cap(args, GIT_STDOUT_MAX_BYTES);
        }
        run_program_args(
            program,
            &GH_ENV,
            args,
            GH_OUTPUT_MAX_BYTES,
            GH_COMMAND_TIMEOUT,
        )
    }

    fn run_with_timeout(
        &self,
        program: &str,
        args: &[String],
        timeout: Duration,
    ) -> Result<GitOutput, GitRunError> {
        if program == "git" {
            return run_git_args_with_cap(args, GIT_STDOUT_MAX_BYTES);
        }
        run_program_args(program, &GH_ENV, args, GH_OUTPUT_MAX_BYTES, timeout)
    }
}

/// A GitHub repository on a named host.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RepoRef {
    pub(crate) host: String,
    pub(crate) owner: String,
    pub(crate) repo: String,
}

impl RepoRef {
    pub(crate) fn slug(&self) -> String {
        format!("{}/{}", self.owner, self.repo)
    }
}

/// Parse a git remote URL (https, ssh or scp form) into a repository, or
/// `None` when it is not a GitHub one. Credentials embedded in the URL are
/// dropped on the way: only host, owner and name survive.
pub(crate) fn parse_remote(url: &str) -> Option<RepoRef> {
    let url = url.trim();
    let (host, path) = if let Some((_, rest)) = url.split_once("://") {
        let (authority, path) = rest.split_once('/')?;
        let host = authority.rsplit('@').next()?;
        (host.split(':').next()?, path)
    } else {
        let (authority, path) = url.split_once(':')?;
        (authority.rsplit('@').next()?, path)
    };
    repo_from_path(host, path)
}

/// Parse the tool's `repo` argument: `owner/repo` only. The host never
/// comes from the argument — it is always the workspace origin's — so a
/// three-part `host/owner/repo` is malformed here, however valid the host.
pub(crate) fn parse_repo_argument(text: &str) -> Option<(String, String)> {
    if text.trim().split('/').count() != 2 {
        return None;
    }
    let repo = repo_from_path("github.com", text)?;
    Some((repo.owner, repo.repo))
}

fn repo_from_path(host: &str, path: &str) -> Option<RepoRef> {
    if !is_github_host(host) {
        return None;
    }
    let path = path.trim_matches('/');
    let path = path.strip_suffix(".git").unwrap_or(path);
    let (owner, repo) = path.split_once('/')?;
    let name_ok = |part: &str| {
        !part.is_empty()
            && part != "."
            && part != ".."
            && part
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    };
    (name_ok(owner) && name_ok(repo)).then(|| RepoRef {
        host: host.to_ascii_lowercase(),
        owner: owner.to_string(),
        repo: repo.to_string(),
    })
}

/// GitHub.com, GitHub-hosted data residency (`*.ghe.com`) and the usual
/// self-hosted `github.<company>` names. Anything else is not asked.
fn is_github_host(host: &str) -> bool {
    let host = host.to_ascii_lowercase();
    host == "github.com" || host.ends_with(".ghe.com") || host.starts_with("github.")
}

#[derive(Clone)]
pub(crate) struct GhClient {
    runner: Arc<dyn CommandRunner>,
    timeout: Duration,
    /// `host/owner/repo` to earliest retry time: a rate-limited answer
    /// quiets the repository instead of spending the next passes on calls
    /// GitHub already refused. Shared by every client over one runner.
    backoff_until_ms: Arc<Mutex<std::collections::HashMap<String, u64>>>,
}

impl GhClient {
    pub(crate) fn new(runner: Arc<dyn CommandRunner>) -> Self {
        Self {
            runner,
            timeout: GIT_COMMAND_TIMEOUT,
            backoff_until_ms: Arc::new(Mutex::new(std::collections::HashMap::new())),
        }
    }

    /// The same login with a shorter fuse, sharing the backoff map: the
    /// tool call validates fast, the poll thread keeps the minute.
    pub(crate) fn with_timeout(&self, timeout: Duration) -> Self {
        Self {
            runner: Arc::clone(&self.runner),
            timeout,
            backoff_until_ms: Arc::clone(&self.backoff_until_ms),
        }
    }

    /// Which repository may be watched: the workspace origin's host is the
    /// only host this machine's login may be aimed at — never the agent's
    /// argument — and a non-github.com host additionally needs the person's
    /// own `gh` login there. The argument may only pick owner and repo on
    /// that same host.
    pub(crate) fn resolve_repo(
        &self,
        workspace: Option<&Path>,
        repo: Option<(String, String)>,
    ) -> Result<RepoRef, CiError> {
        let Some(root) = workspace else {
            return Err(CiError::new(
                "repo_not_github",
                "This session has no workspace, so there is no repository origin to take the host from.",
                false,
            ));
        };
        let origin = self.origin(root)?;
        if origin.host != "github.com" {
            self.check_host_login(&origin.host)?;
        }
        let (owner, repo) = repo.unwrap_or((origin.owner.clone(), origin.repo.clone()));
        Ok(RepoRef {
            host: origin.host,
            owner,
            repo,
        })
    }

    /// The person behind this daemon logged into `host` with `gh`: a local
    /// config read, never a credential, and the gate that keeps an agent
    /// from aiming this machine's login at a host it never chose.
    fn check_host_login(&self, host: &str) -> Result<(), CiError> {
        let args = vec![
            "auth".to_string(),
            "status".to_string(),
            "--hostname".to_string(),
            host.to_string(),
        ];
        match self.runner.run_with_timeout("gh", &args, self.timeout) {
            Ok(output) if output.success => Ok(()),
            Ok(_) => Err(CiError::new(
                "repo_not_github",
                format!(
                    "The workspace origin host `{host}` is not github.com, and this machine has no `gh` login for it (`gh auth login --hostname {host}`)."
                ),
                false,
            )),
            Err(GitRunError::NotFound) => Err(cli_missing_error()),
            Err(_) => Err(CiError::new(
                "repo_not_github",
                format!(
                    "The workspace origin host `{host}` is not github.com, and its `gh` login could not be checked."
                ),
                false,
            )),
        }
    }

    /// The repository a workspace's `origin` remote names.
    pub(crate) fn origin(&self, workspace: &Path) -> Result<RepoRef, CiError> {
        let args = [
            "-C".to_string(),
            workspace.to_string_lossy().into_owned(),
            "remote".to_string(),
            "get-url".to_string(),
            "origin".to_string(),
        ];
        let output = self.runner.run("git", &args).map_err(|error| match error {
            GitRunError::NotFound => CiError::new(
                "git_missing",
                "git was not found on the daemon's PATH, so the workspace's origin remote cannot be read.",
                false,
            ),
            GitRunError::TimedOut | GitRunError::SpawnFailed => {
                CiError::new("github_unavailable", "git did not answer; try again.", true)
            }
        })?;
        if !output.success {
            return Err(not_github(
                "This workspace has no `origin` remote, so there is no GitHub repository to watch. Pass `repo` as owner/repo.",
            ));
        }
        parse_remote(output.stdout.trim()).ok_or_else(|| {
            not_github(
                "This workspace's `origin` remote is not a GitHub repository. Pass `repo` as owner/repo to name one.",
            )
        })
    }

    /// `gh api` GET of a repository endpoint, parsed as JSON.
    pub(crate) fn get_json(&self, repo: &RepoRef, endpoint: &str) -> Result<Value, CiError> {
        let output = self.api(repo, endpoint, Api::Json)?;
        serde_json::from_str(&output.stdout).map_err(|_| {
            CiError::new(
                "github_unavailable",
                "GitHub answered with something that is not JSON; try again.",
                true,
            )
        })
    }

    /// The same read with `gh`'s own paging: a list longer than one page is
    /// merged, so a verdict is never read off the first hundred entries.
    pub(crate) fn get_json_merged(&self, repo: &RepoRef, endpoint: &str) -> Result<Value, CiError> {
        let output = self.api(repo, endpoint, Api::MergedJson)?;
        serde_json::from_str(&output.stdout).map_err(|_| {
            CiError::new(
                "github_unavailable",
                "GitHub answered with something that is not JSON; try again.",
                true,
            )
        })
    }

    /// `gh api` GET of a repository endpoint whose answer is plain text (a
    /// job log). Each job's log is its own call, capped at
    /// [`GH_OUTPUT_MAX_BYTES`]: the per-job bound, kept at the fetch so a
    /// long tail of matches is never silently cut for economy.
    pub(crate) fn get_text(&self, repo: &RepoRef, endpoint: &str) -> Result<String, CiError> {
        Ok(self.api(repo, endpoint, Api::Text)?.stdout)
    }

    fn api(&self, repo: &RepoRef, endpoint: &str, shape: Api) -> Result<GitOutput, CiError> {
        let key = format!("{}/{}", repo.host, repo.slug());
        if self.backed_off(&key) {
            return Err(rate_limited_error());
        }
        let mut args = vec![
            "api".to_string(),
            "--hostname".to_string(),
            repo.host.clone(),
        ];
        if matches!(shape, Api::MergedJson) {
            args.push("--paginate".to_string());
        }
        if shape.needs_accept() {
            args.push("-H".to_string());
            args.push("Accept: application/vnd.github+json".to_string());
        }
        args.push(format!("repos/{}/{}/{endpoint}", repo.owner, repo.repo));
        let output = self
            .runner
            .run_with_timeout("gh", &args, self.timeout)
            .map_err(|error| match error {
                GitRunError::NotFound => cli_missing_error(),
                GitRunError::TimedOut | GitRunError::SpawnFailed => CiError::new(
                    "github_unavailable",
                    "`gh` did not answer in time; try again.",
                    true,
                ),
            })?;
        if output.success {
            Ok(output)
        } else {
            let failure = classify_failure(&output);
            if failure.code == "github_rate_limited" {
                self.note_rate_limit(repo, &key);
            }
            Err(failure)
        }
    }

    /// Whether this repository is quiet until its backoff lifts.
    fn backed_off(&self, key: &str) -> bool {
        let until = self
            .backoff_until_ms
            .lock()
            .map(|map| map.get(key).copied())
            .unwrap_or(None);
        matches!(until, Some(until) if now_ms() < until)
    }

    /// Quiet a rate-limited repository: until the reset time GitHub names,
    /// or a minute when it names none. One `rate_limit` read, never a loop:
    /// its own failure just takes the fallback.
    fn note_rate_limit(&self, repo: &RepoRef, key: &str) {
        let until = self
            .rate_limit_reset_ms(repo)
            .unwrap_or_else(|| now_ms().saturating_add(RATE_LIMIT_FALLBACK_MS));
        if let Ok(mut map) = self.backoff_until_ms.lock() {
            let _ = map.insert(key.to_string(), until);
        }
    }

    fn rate_limit_reset_ms(&self, repo: &RepoRef) -> Option<u64> {
        let args = vec![
            "api".to_string(),
            "--hostname".to_string(),
            repo.host.clone(),
            "rate_limit".to_string(),
        ];
        let output = self
            .runner
            .run_with_timeout("gh", &args, self.timeout)
            .ok()?;
        if !output.success {
            return None;
        }
        let reset = serde_json::from_str::<Value>(&output.stdout)
            .ok()?
            .pointer("/resources/core/reset")?
            .as_u64()?;
        reset.checked_mul(1000)?.checked_add(1000)
    }
}

/// What one `gh api` read is for: a JSON document, a JSON list read whole,
/// or the plain text of a job log.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Api {
    Json,
    MergedJson,
    Text,
}

impl Api {
    fn needs_accept(self) -> bool {
        !matches!(self, Self::Text)
    }
}

fn not_github(message: &str) -> CiError {
    CiError::new("repo_not_github", message, false)
}

fn cli_missing_error() -> CiError {
    CiError::new(
        "github_cli_missing",
        "The GitHub CLI (`gh`) was not found on the daemon's PATH. Install it from \
         https://cli.github.com and make sure it is on the PATH of the user that runs \
         Devboule; an app started from the Dock or Start menu may not see \
         Homebrew's directory.",
        false,
    )
}

fn rate_limited_error() -> CiError {
    CiError::new(
        "github_rate_limited",
        "GitHub's API rate limit is exhausted; the watch retries on its own.",
        true,
    )
}

/// Read `gh`'s failure into a code the owner can act on. The text comes from
/// `gh` itself and passes the secret redactor before it is quoted.
fn classify_failure(output: &GitOutput) -> CiError {
    let text = format!("{}\n{}", output.stderr, output.stdout);
    let lower = text.to_ascii_lowercase();
    if output.code == Some(4)
        || lower.contains("gh auth login")
        || lower.contains("http 401")
        || lower.contains("bad credentials")
    {
        return CiError::new(
            "github_auth_required",
            "The GitHub CLI is not logged in on this machine. Run `gh auth login` as the user \
             that runs Devboule, then call this tool again.",
            false,
        );
    }
    if lower.contains("http 404") {
        return CiError::new(
            "not_found",
            "GitHub does not know that repository, commit or job, or this login cannot see it.",
            false,
        );
    }
    if lower.contains("http 403") && lower.contains("rate limit") {
        return rate_limited_error();
    }
    if lower.contains("http 403") {
        return CiError::new(
            "permission_required",
            "This GitHub login cannot read checks, Actions runs or workflow logs for the \
             repository. Run `gh auth refresh -s repo` (or use a token with Actions read access).",
            false,
        );
    }
    let first_line = text
        .lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("");
    let cleaned = crate::diagnostics::redact_secret_tokens(first_line.trim());
    let cleaned: String = cleaned.chars().take(ERROR_LINE_CHARS).collect();
    CiError::new(
        "github_unavailable",
        format!("GitHub could not be reached through `gh`: {cleaned}"),
        true,
    )
}

#[cfg(test)]
#[path = "ci_gh_tests.rs"]
mod tests;
