//! The GitHub client: how remotes become repositories, how `gh` is asked, and
//! how its failures read to an owner.

use std::path::Path;
use std::sync::Arc;

use super::{
    parse_remote, parse_repo_argument, CiError, CommandRunner, GhClient, ProcessRunner, RepoRef,
    TOOL_GH_TIMEOUT,
};
use crate::ci_test_support::{fail, ok, ScriptedRunner};
use crate::git::GitRunError;

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

#[test]
fn every_github_remote_form_names_the_same_repository() {
    for url in [
        "https://github.com/acme/widgets.git",
        "https://github.com/acme/widgets",
        "git@github.com:acme/widgets.git",
        "ssh://git@github.com/acme/widgets.git",
        "https://someone:ghp_secretsecretsecretsecretsecretsecret12@github.com/acme/widgets.git",
    ] {
        assert_eq!(parse_remote(url), Some(repo()), "{url}");
    }
}

#[test]
fn a_remote_that_is_not_a_repository_path_is_refused() {
    for url in [
        "https://github.com/acme",
        "https://github.com/../widgets",
        "/some/local/path",
        "https://127.0.0.1/acme/widgets.git",
        "https://localhost/acme/widgets.git",
        "git@0x7f000001:acme/widgets.git",
    ] {
        assert_eq!(parse_remote(url), None, "{url}");
    }
}

/// A host other than github.com parses, and is then asked about only after the
/// person's own `gh` login vouches for exactly that name.
#[test]
fn github_host_allowlist_exact() {
    for origin in [
        "https://gitlab.com/acme/widgets.git",
        "git@bitbucket.org:acme/widgets.git",
        "https://github.com.evil.test/acme/widgets.git",
        "https://evil-github.example.com/acme/widgets.git",
    ] {
        let runner = origin_runner(origin);
        // `gh` is logged in to one enterprise host, and not to these.
        runner.set(
            "auth status --hostname github.example.com",
            ok("Logged in to github.example.com\n"),
        );
        let refused = client(&runner)
            .resolve_repo(Some(Path::new("/work")), None)
            .expect_err(origin);
        assert_eq!(refused.code, "repo_not_github", "{origin}");
        assert!(
            !runner.calls().iter().any(|call| call.starts_with("gh api")),
            "{origin}: nothing is asked of GitHub for a host nobody vouched for"
        );
    }
    let runner = origin_runner("https://ghe.corp.example.com/acme/widgets.git");
    runner.set(
        "auth status --hostname ghe.corp.example.com",
        ok("Logged in to ghe.corp.example.com\n"),
    );
    let resolved = client(&runner)
        .resolve_repo(Some(Path::new("/work")), None)
        .expect("a host the person is logged in to, by its exact name");
    assert_eq!(resolved.host, "ghe.corp.example.com");
}

#[test]
fn a_stored_watch_cannot_aim_gh_at_a_host_nobody_vouched_for() {
    let runner = Arc::new(ScriptedRunner::default());
    runner.set("api", ok("[]"));
    let stored = RepoRef {
        host: "internal.example.com".to_string(),
        owner: "acme".to_string(),
        repo: "widgets".to_string(),
    };
    let refused = client(&runner)
        .get_json_pages(&stored, "commits/abc/check-runs")
        .expect_err("no login for that host");
    assert_eq!(refused.code, "repo_not_github");
    assert!(
        !runner.calls().iter().any(|call| call.starts_with("gh api")),
        "no `gh api` was spawned: {:?}",
        runner.calls()
    );
    let numeric = RepoRef {
        host: "169.254.169.254".to_string(),
        ..stored
    };
    let fresh = Arc::new(ScriptedRunner::default());
    assert!(client(&fresh)
        .get_json_pages(&numeric, "commits/abc/check-runs")
        .is_err());
    assert!(
        fresh.calls().is_empty(),
        "an address is refused before any spawn"
    );
}

#[test]
fn the_repo_argument_takes_owner_repo_and_never_a_host() {
    assert_eq!(
        parse_repo_argument("acme/widgets"),
        Some(("acme".to_string(), "widgets".to_string()))
    );
    // A host in the argument cannot select where the login is aimed: the
    // host always comes from the workspace origin (see resolve_repo).
    assert_eq!(parse_repo_argument("github.example.com/acme/widgets"), None);
    assert_eq!(parse_repo_argument("evil.com/acme/widgets"), None);
    assert_eq!(parse_repo_argument("widgets"), None);
}

#[test]
fn origin_reads_the_workspace_remote_and_refuses_a_foreign_one() {
    let runner = Arc::new(ScriptedRunner::default());
    runner.set(
        "remote get-url origin",
        ok("git@github.com:acme/widgets.git\n"),
    );
    assert_eq!(
        client(&runner).origin(Path::new("/work")).expect("origin"),
        repo()
    );

    runner.set(
        "remote get-url origin",
        fail(2, "error: No such remote 'origin'"),
    );
    let refused = client(&runner)
        .origin(Path::new("/work"))
        .expect_err("no origin");
    assert_eq!(refused.code, "repo_not_github");
}

#[test]
fn ci_auth_missing_is_actionable() {
    let runner = Arc::new(ScriptedRunner::default());
    runner.set(
        "api",
        fail(
            4,
            "To get started with GitHub CLI, please run:  gh auth login",
        ),
    );
    let refused = client(&runner)
        .get_json_pages(&repo(), "commits/abc/check-runs")
        .expect_err("not logged in");
    assert_eq!(refused.code, "github_auth_required");
    assert!(!refused.retryable);
    assert!(
        refused.message.contains("gh auth login"),
        "the hint is the command to run: {}",
        refused.message
    );

    let missing = Arc::new(ScriptedRunner::default());
    missing.set("api", Err(GitRunError::NotFound));
    let refused = client(&missing)
        .get_json_pages(&repo(), "commits/abc/check-runs")
        .expect_err("no gh");
    assert_eq!(refused.code, "github_cli_missing");
    assert!(
        refused.message.contains("https://cli.github.com"),
        "the hint says where to get it: {}",
        refused.message
    );

    // Neither failure asked for, or printed, a credential.
    for line in runner.calls().into_iter().chain(missing.calls()) {
        assert!(
            !line.contains("auth token"),
            "no token is ever requested: {line}"
        );
    }
}

#[test]
fn other_failures_say_what_to_do_and_never_quote_a_secret() {
    let runner = Arc::new(ScriptedRunner::default());
    let read = |stderr: &str| -> CiError {
        runner.set("api", fail(1, stderr));
        client(&runner)
            .get_json_pages(&repo(), "commits/abc/check-runs")
            .expect_err("a failure")
    };
    assert_eq!(read("gh: Not Found (HTTP 404)").code, "not_found");
    let limited = read("gh: API rate limit exceeded (HTTP 403)");
    assert_eq!(
        (limited.code, limited.retryable),
        ("github_rate_limited", true)
    );
    assert_eq!(
        read("gh: Resource not accessible (HTTP 403)").code,
        "permission_required"
    );
    let unknown = read("gh: connection reset ghp_abcdefghijklmnopqrstuvwxyz0123456789 by peer");
    assert_eq!(unknown.code, "github_unavailable");
    assert!(unknown.retryable);
    assert!(
        !unknown
            .message
            .contains("ghp_abcdefghijklmnopqrstuvwxyz0123456789"),
        "gh's own text is redacted before it is quoted: {}",
        unknown.message
    );
}

#[test]
fn a_program_that_does_not_exist_is_reported_as_missing() {
    let outcome = ProcessRunner.run("devboule-no-such-program-for-ci-watch", &[]);
    assert_eq!(outcome, Err(GitRunError::NotFound));
}

fn epoch_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0)
}

fn limited_runner() -> Arc<ScriptedRunner> {
    let runner = Arc::new(ScriptedRunner::default());
    runner.set(
        "check-runs",
        fail(1, "gh: API rate limit exceeded (HTTP 403)"),
    );
    runner
}

fn limited_err(client: &GhClient) -> CiError {
    client
        .get_json_pages(&repo(), "commits/abc/check-runs")
        .expect_err("rate limited")
}

#[test]
fn a_rate_limited_repo_makes_no_further_calls() {
    let runner = limited_runner();
    // GitHub names no reset here, so the fallback quiets the repo.
    runner.set("rate_limit", fail(1, "gh: Not Found (HTTP 404)"));
    // One client: the backoff lives on it, as the watch service holds one.
    let client = client(&runner);
    let limited = limited_err(&client);
    assert_eq!(limited.code, "github_rate_limited");
    assert!(limited.retryable);
    let spawns = runner.calls().len();
    assert_eq!(limited_err(&client).code, "github_rate_limited");
    assert_eq!(
        runner.calls().len(),
        spawns,
        "a backed-off repo spawns nothing"
    );
}

#[test]
fn a_named_reset_time_decides_the_backoff() {
    let runner = limited_runner();
    // A reset already past proves the named value (not the fallback) decides:
    // the next call spawns again instead of staying quiet.
    runner.set(
        "rate_limit",
        ok(&format!(
            "{{\"resources\":{{\"core\":{{\"reset\":{}}}}}}}",
            epoch_now().saturating_sub(120)
        )),
    );
    let client = client(&runner);
    assert_eq!(limited_err(&client).code, "github_rate_limited");
    let spawns = runner.calls().len();
    assert_eq!(limited_err(&client).code, "github_rate_limited");
    assert!(
        runner.calls().len() > spawns,
        "a past reset lifts the backoff"
    );
}

#[test]
fn a_future_reset_holds_until_then() {
    let runner = limited_runner();
    runner.set(
        "rate_limit",
        ok(&format!(
            "{{\"resources\":{{\"core\":{{\"reset\":{}}}}}}}",
            epoch_now() + 300
        )),
    );
    let client = client(&runner);
    assert_eq!(limited_err(&client).code, "github_rate_limited");
    let spawns = runner.calls().len();
    assert_eq!(limited_err(&client).code, "github_rate_limited");
    assert_eq!(
        runner.calls().len(),
        spawns,
        "a future reset holds the backoff"
    );
}

#[test]
fn backoff_is_per_repository() {
    let runner = limited_runner();
    runner.set("rate_limit", fail(1, "gh: Not Found (HTTP 404)"));
    let client = client(&runner);
    assert_eq!(limited_err(&client).code, "github_rate_limited");
    let other = RepoRef {
        host: "github.com".to_string(),
        owner: "other".to_string(),
        repo: "widgets".to_string(),
    };
    runner.set("other/widgets", ok("[]"));
    assert!(
        client
            .get_json_pages(&other, "commits/abc/check-runs")
            .is_ok(),
        "a quiet repo never quiets its neighbours"
    );
}

#[test]
fn the_deadline_rides_with_the_client() {
    let runner = Arc::new(ScriptedRunner::default());
    runner.set("api", ok("[]"));
    client(&runner)
        .with_timeout(TOOL_GH_TIMEOUT)
        .get_json_pages(&repo(), "commits/abc/check-runs")
        .expect("answer");
    assert_eq!(runner.last_timeout(), Some(TOOL_GH_TIMEOUT));
}

fn origin_runner(url: &str) -> Arc<ScriptedRunner> {
    let runner = Arc::new(ScriptedRunner::default());
    runner.set("remote get-url origin", ok(&format!("{url}\n")));
    runner
}

#[test]
fn resolve_repo_without_a_workspace_has_no_host_to_take() {
    let client = client(&Arc::new(ScriptedRunner::default()));
    let refused = client
        .resolve_repo(None, Some(("acme".to_string(), "widgets".to_string())))
        .expect_err("no workspace");
    assert_eq!(refused.code, "repo_not_github");
    let refused = client.resolve_repo(None, None).expect_err("no workspace");
    assert_eq!(refused.code, "repo_not_github");
}

#[test]
fn resolve_repo_takes_the_host_from_the_origin_never_the_argument() {
    let runner = origin_runner("https://github.com/acme/widgets.git");
    let resolved = client(&runner)
        .resolve_repo(
            Some(Path::new("/work")),
            Some(("attacker".to_string(), "gadget".to_string())),
        )
        .expect("resolved");
    assert_eq!(resolved.host, "github.com");
    assert_eq!(resolved.owner, "attacker");
    assert_eq!(resolved.repo, "gadget");
    // github.com needs no login proof: only the origin read ran.
    assert!(
        !runner
            .calls()
            .iter()
            .any(|call| call.contains("auth status")),
        "no login check for github.com"
    );
}

#[test]
fn resolve_repo_keeps_an_enterprise_host_with_a_login() {
    let runner = origin_runner("https://github.example.com/acme/widgets.git");
    runner.set("auth status", ok("Logged in to github.example.com\n"));
    let resolved = client(&runner)
        .resolve_repo(Some(Path::new("/work")), None)
        .expect("resolved");
    assert_eq!(resolved.host, "github.example.com");
    assert!(
        runner
            .calls()
            .iter()
            .any(|call| call.contains("auth status") && call.contains("github.example.com")),
        "the login is checked on that host"
    );
}

#[test]
fn resolve_repo_refuses_an_enterprise_host_without_a_login() {
    let runner = origin_runner("https://github.example.com/acme/widgets.git");
    runner.set("auth status", fail(1, "You are not logged in"));
    let refused = client(&runner)
        .resolve_repo(Some(Path::new("/work")), None)
        .expect_err("no login");
    assert_eq!(refused.code, "repo_not_github");
    assert!(refused.message.contains("github.example.com"));
}
