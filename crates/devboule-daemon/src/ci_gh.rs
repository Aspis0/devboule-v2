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
use crate::egress_policy::{classify_gh_host, GhHost};

use crate::git::{
    run_git_args_with_cap, run_git_args_with_cap_and_timeout, run_program_args, GitOutput,
    GitRunError, GIT_COMMAND_TIMEOUT, GIT_STDOUT_MAX_BYTES,
};

/// Job logs are the large read; anything past this is cut, and the summary
/// only ever looks at the part it got.
const GH_OUTPUT_MAX_BYTES: usize = 4 * 1024 * 1024;
const ERROR_LINE_CHARS: usize = 300;
/// The environment every `gh` spawn gets: no interactive prompt, no colour in
/// the output this reads as text, and none of the variables that would let the
/// daemon's own environment replace the person's `gh` login or point it at
/// another host. `gh` then uses the stored login for the origin's host.
const GH_ENV: [(&str, Option<&str>); 7] = [
    ("GH_PROMPT_DISABLED", Some("1")),
    ("NO_COLOR", Some("1")),
    ("GH_TOKEN", None),
    ("GITHUB_TOKEN", None),
    ("GH_ENTERPRISE_TOKEN", None),
    ("GITHUB_ENTERPRISE_TOKEN", None),
    ("GH_HOST", None),
];
/// The ceiling on a `gh` call that names no deadline of its own: the same
/// minute the git runner allows, stated here so `gh` never rides on git's
/// timeout by accident.
const GH_COMMAND_TIMEOUT: Duration = GIT_COMMAND_TIMEOUT;
/// The broker-facing resolution (a local `git` read, a local `gh` login check)
/// waits seconds, not the full command minute: a hung helper must not hold a
/// tool call, and every GitHub read lives on the poll thread.
pub(crate) const TOOL_GH_TIMEOUT: Duration = Duration::from_secs(5);
/// The job logs of a finished pass: a download that either arrives or never
/// will, read under a fuse short enough that one missing log does not hold
/// the poll thread past the watches queued behind it.
pub(crate) const LOG_GH_TIMEOUT: Duration = Duration::from_secs(20);
/// A rate-limited answer quiets a repository this long when GitHub names no
/// reset time of its own.
const RATE_LIMIT_FALLBACK_MS: u64 = 60_000;
/// One attempt's jobs are read a page at a time and every page is joined, so
/// an attempt with more than one page of jobs is judged whole.
const JOBS_PAGE: &str = "per_page=100";

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
            return run_git_args_with_cap_and_timeout(args, GIT_STDOUT_MAX_BYTES, timeout);
        }
        run_program_args(program, &GH_ENV, args, GH_OUTPUT_MAX_BYTES, timeout)
    }
}

/// A workflow run's own answer about its attempts: the number the re-run
/// moves on, and whether the run is finished.
pub(crate) struct RunAttempt {
    pub(crate) attempt: u64,
    /// The run's own status: `completed` is the one that ends an attempt.
    pub(crate) status: String,
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

/// A full commit id: 40 hex digits. Anything else is refused before it is
/// stored, so only well-formed ids reach GitHub.
pub(crate) fn is_commit_id(sha: &str) -> bool {
    sha.len() == 40 && sha.bytes().all(|byte| byte.is_ascii_hexdigit())
}

/// A branch name that may be put into a `gh api` path and into a wake text:
/// git's own refname rules where they matter here — no controls, no space,
/// no `..`, no `@{`, no leading dash or slash, bounded — plus every scalar
/// that draws as a line break or as nothing (U+2028 and U+2029 included, the
/// shared protocol tables), so the argument can only ever name a ref under
/// `heads/` and can never forge a line of the message it is quoted in.
pub(crate) fn is_branch_name(branch: &str) -> bool {
    const MAX_CHARS: usize = 255;
    const BANNED: [char; 16] = [
        ' ', '~', '^', ':', '?', '*', '[', '\\', '"', '\'', '`', '#', '%', '<', '>', '|',
    ];
    !branch.is_empty()
        && branch.chars().count() <= MAX_CHARS
        && !branch.starts_with('-')
        && !branch.starts_with('/')
        && !branch.ends_with('/')
        && !branch.ends_with('.')
        && !branch.contains("..")
        && !branch.contains("@{")
        && !branch.chars().any(|character| {
            character.is_control()
                || devboule_protocol::is_mandatory_line_break(character)
                || devboule_protocol::is_invisible_format(character)
                || BANNED.contains(&character)
        })
}

fn repo_from_path(host: &str, path: &str) -> Option<RepoRef> {
    classify_gh_host(host).ok()?;
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

#[derive(Clone)]
pub(crate) struct GhClient {
    runner: Arc<dyn CommandRunner>,
    timeout: Duration,
    /// `host/owner/repo` to earliest retry time: a rate-limited answer
    /// quiets the repository instead of spending the next passes on calls
    /// GitHub already refused. Shared by every client over one runner.
    backoff_until_ms: Arc<Mutex<std::collections::HashMap<String, u64>>>,
    /// Enterprise hosts this machine's `gh` has been seen logged in to, by
    /// exact name: a host is asked about only after one of these proofs.
    vouched_hosts: Arc<Mutex<std::collections::HashSet<String>>>,
}

impl GhClient {
    pub(crate) fn new(runner: Arc<dyn CommandRunner>) -> Self {
        Self {
            runner,
            timeout: GIT_COMMAND_TIMEOUT,
            backoff_until_ms: Arc::new(Mutex::new(std::collections::HashMap::new())),
            vouched_hosts: Arc::new(Mutex::new(std::collections::HashSet::new())),
        }
    }

    /// The same login with a shorter fuse, sharing the backoff map: the
    /// tool call validates fast, the poll thread keeps the minute.
    pub(crate) fn with_timeout(&self, timeout: Duration) -> Self {
        Self {
            runner: Arc::clone(&self.runner),
            timeout,
            backoff_until_ms: Arc::clone(&self.backoff_until_ms),
            vouched_hosts: Arc::clone(&self.vouched_hosts),
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
        let host = self.vouched_host(&origin.host)?;
        let (owner, repo) = repo.unwrap_or((origin.owner.clone(), origin.repo.clone()));
        Ok(RepoRef { host, owner, repo })
    }

    /// The host `gh` may be asked about, by its exact name: `github.com`, or
    /// an enterprise host the person's own `gh` is logged in to. No pattern
    /// admits a host, and no `gh` is spawned for one this has not passed.
    /// `gh` makes its own connections, so their addresses are not checked
    /// here, only the name.
    fn vouched_host(&self, host: &str) -> Result<String, CiError> {
        match classify_gh_host(host) {
            Ok(GhHost::Dotcom) => Ok("github.com".to_string()),
            Ok(GhHost::Enterprise(host)) => {
                let known = self
                    .vouched_hosts
                    .lock()
                    .map(|hosts| hosts.contains(&host))
                    .unwrap_or(false);
                if !known {
                    self.check_host_login(&host)?;
                    if let Ok(mut hosts) = self.vouched_hosts.lock() {
                        hosts.insert(host.clone());
                    }
                }
                Ok(host)
            }
            Err(refusal) => Err(not_github(&refusal.0)),
        }
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
        let output = self
            .runner
            .run_with_timeout("git", &args, self.timeout)
            .map_err(|error| match error {
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

    /// `gh api` GET of a list endpoint with `gh`'s own paging, one JSON value
    /// per page. `gh` prints the pages as separate values unless they are
    /// slurped into one array, and a list read off the first page alone is how
    /// a verdict goes falsely green.
    pub(crate) fn get_json_pages(
        &self,
        repo: &RepoRef,
        endpoint: &str,
    ) -> Result<Vec<Value>, CiError> {
        let output = self.api(repo, endpoint, Api::Pages)?;
        match parse_json(&output.stdout)? {
            Value::Array(pages) => Ok(pages),
            _ => Err(not_json()),
        }
    }

    /// `gh api` GET of a single JSON object: a ref, not a list.
    pub(crate) fn get_json(&self, repo: &RepoRef, endpoint: &str) -> Result<Value, CiError> {
        let output = self.api(repo, endpoint, Api::Json)?;
        parse_json(&output.stdout)
    }

    /// The commit a branch's remote head points at now. A branch GitHub does
    /// not have is a missing commit to watch, not an outage: the watch is
    /// refused at once instead of waiting on a poll that can never succeed.
    pub(crate) fn head_sha(&self, repo: &RepoRef, branch: &str) -> Result<String, CiError> {
        let document = self
            .get_json(repo, &format!("git/ref/heads/{branch}"))
            .map_err(|error| match error.code {
                "not_found" => CiError::new(
                    "sha_not_found",
                    format!(
                        "GitHub has no branch named `{branch}` in {}, or this login cannot see it. Push it first, then watch it.",
                        repo.slug()
                    ),
                    false,
                ),
                _ => error,
            })?;
        let sha = document
            .pointer("/object/sha")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_ascii_lowercase();
        if !is_commit_id(&sha) {
            return Err(CiError::new(
                "github_unavailable",
                "GitHub's answer for that branch named no commit; try again.",
                true,
            ));
        }
        Ok(sha)
    }

    /// `gh api` GET of a repository endpoint whose answer is plain text (a
    /// job log). Each job's log is its own call, capped at
    /// [`GH_OUTPUT_MAX_BYTES`]: the per-job bound, kept at the fetch so a
    /// long tail of matches is never silently cut for economy.
    pub(crate) fn get_text(&self, repo: &RepoRef, endpoint: &str) -> Result<String, CiError> {
        Ok(self.api(repo, endpoint, Api::Text)?.stdout)
    }

    fn api(&self, repo: &RepoRef, endpoint: &str, shape: Api) -> Result<GitOutput, CiError> {
        // A stored watch names its host too: every spawn passes the same gate.
        self.vouched_host(&repo.host)?;
        let key = format!("{}/{}", repo.host, repo.slug());
        if self.backed_off(&key) {
            return Err(rate_limited_error());
        }
        let mut args = vec![
            "api".to_string(),
            "--hostname".to_string(),
            repo.host.clone(),
        ];
        if matches!(shape, Api::Pages) {
            args.push("--paginate".to_string());
            args.push("--slurp".to_string());
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

    /// Whether GitHub asked this repository to be left alone for now.
    pub(crate) fn is_quiet(&self, repo: &RepoRef) -> bool {
        self.backed_off(&format!("{}/{}", repo.host, repo.slug()))
    }

    /// The attempt a workflow run is on now, and whether it has finished. A
    /// re-run keeps the run's id and moves this number on, which is the one
    /// fact on a commit that says *this* re-run ran; anything else that
    /// appears on the commit is another check, not this retry.
    pub(crate) fn run_attempt(&self, repo: &RepoRef, run_id: u64) -> Result<RunAttempt, CiError> {
        let document = self.get_json(repo, &format!("actions/runs/{run_id}"))?;
        let attempt = document
            .get("run_attempt")
            .and_then(Value::as_u64)
            .filter(|attempt| *attempt > 0);
        let Some(attempt) = attempt else {
            return Err(CiError::new(
                "github_unavailable",
                "GitHub's answer for that workflow run named no attempt; try again.",
                true,
            ));
        };
        Ok(RunAttempt {
            attempt,
            status: document
                .get("status")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
        })
    }

    /// The pages of one attempt's jobs, as `gh api --paginate --slurp` hands
    /// them over. A `--failed` re-run runs the failed jobs, so this is the
    /// attempt's own work and never the attempt before it.
    pub(crate) fn attempt_job_pages(
        &self,
        repo: &RepoRef,
        run_id: u64,
        attempt: u64,
    ) -> Result<Vec<Value>, CiError> {
        self.get_json_pages(
            repo,
            &format!("actions/runs/{run_id}/attempts/{attempt}/jobs?{JOBS_PAGE}"),
        )
    }

    /// `gh run rerun --failed <run-id>`: the one retry a watch may hold, and
    /// only with the person's approval recorded at watch time. `--repo`
    /// carries host, owner and repository, so a run id can never be re-run on
    /// a host the workspace origin did not name.
    pub(crate) fn rerun_failed(&self, repo: &RepoRef, run_id: u64) -> Result<(), CiError> {
        self.vouched_host(&repo.host)?;
        let args = vec![
            "run".to_string(),
            "rerun".to_string(),
            "--failed".to_string(),
            run_id.to_string(),
            "--repo".to_string(),
            format!("{}/{}/{}", repo.host, repo.owner, repo.repo),
        ];
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
            Ok(())
        } else {
            Err(rerun_error(&output))
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

/// What one `gh api` read is for: every page of a JSON list, one JSON
/// object, or the plain text of a job log.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Api {
    Pages,
    Json,
    Text,
}

impl Api {
    fn needs_accept(self) -> bool {
        !matches!(self, Self::Text)
    }
}

fn parse_json(text: &str) -> Result<Value, CiError> {
    serde_json::from_str(text).map_err(|_| not_json())
}

fn not_json() -> CiError {
    CiError::new(
        "github_unavailable",
        "GitHub answered with something that is not JSON; try again.",
        true,
    )
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

/// What a refused re-run means: the same login and host sentences a read
/// gives, with the write the person must grant named, and GitHub's own line
/// for anything else.
fn rerun_error(output: &GitOutput) -> CiError {
    let classified = classify_failure(output);
    match classified.code {
        "permission_required" => CiError::new(
            "permission_required",
            "This GitHub login cannot re-run workflow runs for the repository. Run `gh auth refresh -s repo` (or use a token with Actions write access).",
            false,
        ),
        "not_found" => CiError::new(
            "not_found",
            "GitHub does not know that workflow run, or this login cannot see it.",
            false,
        ),
        _ => classified,
    }
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
    if lower.contains("unknown flag") && lower.contains("slurp") {
        return CiError::new(
            "github_cli_missing",
            "The installed GitHub CLI (`gh`) is too old to read every page of a list (`--slurp`).              Update it from https://cli.github.com.",
            false,
        );
    }
    if lower.contains("http 422") && lower.contains("no commit found") {
        return CiError::new(
            "sha_not_found",
            "GitHub has no commit with that id in this repository. Push it first, then watch it.",
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

#[cfg(test)]
#[path = "ci_gh_reads_tests.rs"]
mod reads_tests;
