//! The GitHub CLI client behind the CI watch: which repository a workspace
//! belongs to, `gh api` reads, and the failures an owner can act on.
//!
//! One phrase: ask GitHub through the daemon user's own `gh` login. Every
//! spawn is an argument vector, never a shell; prompts are off; the token is
//! never requested or printed, and what `gh` says on failure is redacted
//! before it can travel.

use std::path::Path;
use std::sync::Arc;

use serde_json::Value;

use crate::git::{
    run_git_args_with_cap, run_program_args_with_cap, GitOutput, GitRunError, GIT_STDOUT_MAX_BYTES,
};

/// Job logs are the large read; anything past this is cut, and the summary
/// only ever looks at the part it got.
const GH_OUTPUT_MAX_BYTES: usize = 4 * 1024 * 1024;
const ERROR_LINE_CHARS: usize = 300;

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
}

pub(crate) struct ProcessRunner;

impl CommandRunner for ProcessRunner {
    fn run(&self, program: &str, args: &[String]) -> Result<GitOutput, GitRunError> {
        if program == "git" {
            return run_git_args_with_cap(args, GIT_STDOUT_MAX_BYTES);
        }
        run_program_args_with_cap(
            program,
            &[("GH_PROMPT_DISABLED", "1"), ("NO_COLOR", "1")],
            args,
            GH_OUTPUT_MAX_BYTES,
        )
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

/// Parse the tool's `repo` argument: `owner/repo`, or `host/owner/repo` for
/// a GitHub Enterprise host.
pub(crate) fn parse_repo_argument(text: &str) -> Option<RepoRef> {
    match text.trim().split('/').collect::<Vec<_>>().as_slice() {
        [owner, repo] => repo_from_path("github.com", &format!("{owner}/{repo}")),
        [host, owner, repo] => repo_from_path(host, &format!("{owner}/{repo}")),
        _ => None,
    }
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
}

impl GhClient {
    pub(crate) fn new(runner: Arc<dyn CommandRunner>) -> Self {
        Self { runner }
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
        let output = self.api(repo, endpoint, true)?;
        serde_json::from_str(&output.stdout).map_err(|_| {
            CiError::new(
                "github_unavailable",
                "GitHub answered with something that is not JSON; try again.",
                true,
            )
        })
    }

    /// `gh api` GET of a repository endpoint whose answer is plain text (a
    /// job log). The text may be cut at the output cap.
    pub(crate) fn get_text(&self, repo: &RepoRef, endpoint: &str) -> Result<String, CiError> {
        Ok(self.api(repo, endpoint, false)?.stdout)
    }

    fn api(&self, repo: &RepoRef, endpoint: &str, json: bool) -> Result<GitOutput, CiError> {
        let mut args = vec![
            "api".to_string(),
            "--hostname".to_string(),
            repo.host.clone(),
        ];
        if json {
            args.push("-H".to_string());
            args.push("Accept: application/vnd.github+json".to_string());
        }
        args.push(format!("repos/{}/{}/{endpoint}", repo.owner, repo.repo));
        let output = self.runner.run("gh", &args).map_err(|error| match error {
            GitRunError::NotFound => CiError::new(
                "github_cli_missing",
                "The GitHub CLI (`gh`) was not found on the daemon's PATH. Install it from \
                 https://cli.github.com and make sure it is on the PATH of the user that runs \
                 Devboule; an app started from the Dock or Start menu may not see \
                 Homebrew's directory.",
                false,
            ),
            GitRunError::TimedOut | GitRunError::SpawnFailed => CiError::new(
                "github_unavailable",
                "`gh` did not answer in time; try again.",
                true,
            ),
        })?;
        if output.success {
            Ok(output)
        } else {
            Err(classify_failure(&output))
        }
    }
}

fn not_github(message: &str) -> CiError {
    CiError::new("repo_not_github", message, false)
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
        return CiError::new(
            "github_rate_limited",
            "GitHub's API rate limit is exhausted; the watch retries on its own.",
            true,
        );
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
