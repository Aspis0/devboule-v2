//! What a `gh` read inherits and asks for: the person's own login and not the
//! daemon's ambient one, every page of a list, and the refusals worth a name.

use std::path::Path;
use std::sync::Arc;

use super::{CommandRunner, GhClient, ProcessRunner, RepoRef, GH_ENV, TOOL_GH_TIMEOUT};
use crate::ci_test_support::{check_run, check_run_pages, fail, ok, ScriptedRunner};

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
