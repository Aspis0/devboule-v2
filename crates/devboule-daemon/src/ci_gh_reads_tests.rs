//! What a `gh` read inherits and asks for: the person's own login and not the
//! daemon's ambient one, every page of a list, and the refusals worth a name.

use std::path::Path;
use std::sync::Arc;

use super::{
    is_branch_name, CommandRunner, GhClient, ProcessRunner, RepoRef, GH_ENV, TOOL_GH_TIMEOUT,
};
use crate::ci_test_support::{branch_head, check_run, check_run_pages, fail, ok, ScriptedRunner};

fn repo() -> RepoRef {
    RepoRef {
        host: "github.com".to_string(),
        owner: "acme".to_string(),
        repo: "widgets".to_string(),
    }
}

fn client(runner: &Arc<ScriptedRunner>) -> GhClient {
    GhClient::new(runner.clone())
}

const AMBIENT: [&str; 5] = [
    "GH_TOKEN",
    "GITHUB_TOKEN",
    "GH_ENTERPRISE_TOKEN",
    "GITHUB_ENTERPRISE_TOKEN",
    "GH_HOST",
];

#[test]
fn the_gh_child_never_inherits_an_ambient_token_or_host() {
    let removed: Vec<_> = GH_ENV
        .iter()
        .filter(|(_, value)| value.is_none())
        .map(|(name, _)| *name)
        .collect();
    for name in AMBIENT {
        assert!(removed.contains(&name), "{name} must be removed");
    }
}

/// The real spawn, with the variables set in this process: a child that
/// prints what it can see must see none of them.
#[test]
fn a_real_gh_child_sees_none_of_the_ambient_credentials() {
    let _restore: Vec<_> = AMBIENT
        .iter()
        .map(|name| EnvGuard::set(name, "from-the-daemons-environment"))
        .collect();
    let shown = AMBIENT
        .iter()
        .map(|name| shell_expansion(name))
        .collect::<Vec<_>>()
        .join("");
    #[cfg(windows)]
    let (program, args) = ("cmd", vec!["/C".to_string(), format!("echo {shown}")]);
    #[cfg(not(windows))]
    let (program, args) = ("sh", vec!["-c".to_string(), format!("echo {shown}")]);

    let output = ProcessRunner.run(program, &args).expect("the shell ran");
    assert!(
        !output.stdout.contains("from-the-daemons-environment"),
        "an inherited credential reached the child: {}",
        output.stdout
    );
}

#[cfg(windows)]
fn shell_expansion(name: &str) -> String {
    format!("[%{name}%]")
}

#[cfg(not(windows))]
fn shell_expansion(name: &str) -> String {
    format!("[${{{name}-unset}}]")
}

/// Sets a variable for the length of a test and puts it back.
struct EnvGuard {
    name: &'static str,
    before: Option<std::ffi::OsString>,
}

impl EnvGuard {
    fn set(name: &'static str, value: &str) -> Self {
        let before = std::env::var_os(name);
        std::env::set_var(name, value);
        Self { name, before }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        match &self.before {
            Some(value) => std::env::set_var(self.name, value),
            None => std::env::remove_var(self.name),
        }
    }
}

#[test]
fn a_paginated_list_is_asked_for_as_one_array_of_pages() {
    let runner = Arc::new(ScriptedRunner::default());
    let first = [check_run(1, "build", "completed", Some("success"))];
    let second = [check_run(2, "test", "completed", Some("failure"))];
    runner.set("api", ok(&check_run_pages(&[&first, &second])));
    let pages = client(&runner)
        .get_json_pages(&repo(), "commits/abc/check-runs?per_page=100")
        .expect("pages");
    assert_eq!(pages.len(), 2, "one value per page, none merged away");
    assert_eq!(
        runner.calls(),
        vec![
            "gh api --hostname github.com --paginate --slurp -H Accept: \
             application/vnd.github+json repos/acme/widgets/commits/abc/check-runs?per_page=100"
                .to_string()
        ]
    );
}

#[test]
fn pages_that_are_not_an_array_are_not_read() {
    let runner = Arc::new(ScriptedRunner::default());
    // Two bare values, as `--paginate` prints without `--slurp`.
    runner.set("api", ok("{\"check_runs\":[]}\n{\"check_runs\":[]}"));
    let refused = client(&runner)
        .get_json_pages(&repo(), "commits/abc/check-runs")
        .expect_err("not one JSON value");
    assert!(refused.retryable);
    runner.set("api", ok("{\"check_runs\":[]}"));
    client(&runner)
        .get_json_pages(&repo(), "commits/abc/check-runs")
        .expect_err("a lone object is not a slurped list");
}

#[test]
fn a_gh_too_old_to_slurp_says_so() {
    let runner = Arc::new(ScriptedRunner::default());
    runner.set("api", fail(1, "unknown flag: --slurp"));
    let refused = client(&runner)
        .get_json_pages(&repo(), "commits/abc/check-runs")
        .expect_err("old gh");
    assert_eq!(refused.code, "github_cli_missing");
    assert!(!refused.retryable);
}

#[test]
fn a_commit_github_does_not_know_reads_as_that_not_as_an_outage() {
    let runner = Arc::new(ScriptedRunner::default());
    runner.set(
        "api",
        fail(1, "gh: No commit found for SHA: 0123 (HTTP 422)"),
    );
    let refused = client(&runner)
        .get_json_pages(&repo(), "commits/0123/check-runs")
        .expect_err("unknown commit");
    assert_eq!(refused.code, "sha_not_found");
    assert!(!refused.retryable);
}

#[test]
fn a_branch_head_is_read_as_one_object() {
    let runner = Arc::new(ScriptedRunner::default());
    runner.set(
        "git/ref/heads/main",
        ok(&branch_head("0123456789ABCDEF0123456789abcdef01234567")),
    );
    let head = client(&runner).head_sha(&repo(), "main").expect("head");
    assert_eq!(head, "0123456789abcdef0123456789abcdef01234567");
    assert_eq!(
        runner.calls(),
        vec![
            "gh api --hostname github.com -H Accept: application/vnd.github+json \
             repos/acme/widgets/git/ref/heads/main"
                .to_string()
        ]
    );
}

#[test]
fn a_branch_github_does_not_have_reads_as_a_missing_commit() {
    let runner = Arc::new(ScriptedRunner::default());
    runner.set("git/ref", fail(1, "gh: Not Found (HTTP 404)"));
    let refused = client(&runner)
        .head_sha(&repo(), "nope")
        .expect_err("unknown branch");
    assert_eq!(refused.code, "sha_not_found");
    assert!(!refused.retryable);
    assert!(refused.message.contains("nope"), "{}", refused.message);
}

#[test]
fn a_head_that_names_no_commit_is_an_outage() {
    let runner = Arc::new(ScriptedRunner::default());
    runner.set("git/ref", ok("{\"ref\":\"refs/heads/main\"}"));
    let refused = client(&runner)
        .head_sha(&repo(), "main")
        .expect_err("no commit");
    assert_eq!(refused.code, "github_unavailable");
    assert!(refused.retryable);
}

#[test]
fn a_branch_name_that_could_rewrite_the_path_or_a_line_is_refused() {
    for bad in [
        "",
        "main..dev",
        "../commits",
        "feature branch",
        "main?per_page=1",
        "-x",
        "main/",
        "/main",
        "main@{1}",
        "main.",
        // A name that draws as a line break of its own, or as nothing at all:
        // it must not be able to forge a line of the message it is quoted in.
        "main\u{2028}forged: yes",
        "main\u{2029}forged: yes",
        "main\u{85}forged",
        "main\u{200b}forged",
        "main\u{202e}forged",
    ] {
        assert!(!is_branch_name(bad), "{bad:?} must be refused");
    }
    for good in ["main", "feature/nested-name", "release_1.2", "fix+plus"] {
        assert!(is_branch_name(good), "{good} is a branch name");
    }
}

#[test]
fn the_one_rerun_asks_for_the_failed_jobs_of_that_run() {
    let runner = Arc::new(ScriptedRunner::default());
    runner.set("run rerun", ok("✓ Requested rerun of run 900"));
    client(&runner).rerun_failed(&repo(), 900).expect("rerun");
    assert_eq!(
        runner.calls(),
        vec!["gh run rerun --failed 900 --repo github.com/acme/widgets".to_string()]
    );
}

#[test]
fn a_refused_rerun_names_the_write_the_login_needs() {
    let runner = Arc::new(ScriptedRunner::default());
    runner.set(
        "run rerun",
        fail(1, "gh: Resource not accessible (HTTP 403)"),
    );
    let refused = client(&runner)
        .rerun_failed(&repo(), 900)
        .expect_err("refused");
    assert_eq!(refused.code, "permission_required");
    assert!(
        refused.message.contains("write access"),
        "a re-run needs a write, not the read a log needs: {}",
        refused.message
    );
}

#[test]
fn the_origin_read_rides_the_tool_fuse() {
    let runner = Arc::new(ScriptedRunner::default());
    runner.set(
        "remote get-url origin",
        ok("https://github.com/acme/widgets.git\n"),
    );
    client(&runner)
        .with_timeout(TOOL_GH_TIMEOUT)
        .origin(Path::new("/work"))
        .expect("origin");
    assert_eq!(
        runner.last_timeout(),
        Some(TOOL_GH_TIMEOUT),
        "a hung git must not hold the broker for the house minute"
    );
}
