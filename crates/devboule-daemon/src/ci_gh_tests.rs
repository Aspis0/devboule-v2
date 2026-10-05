//! The GitHub client: how remotes become repositories, how `gh` is asked, and
//! how its failures read to an owner.

use std::path::Path;
use std::sync::Arc;

use super::{
    parse_remote, parse_repo_argument, CiError, CommandRunner, GhClient, ProcessRunner, RepoRef,
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
fn a_remote_that_is_not_github_is_not_a_repository() {
    for url in [
        "https://gitlab.com/acme/widgets.git",
        "git@bitbucket.org:acme/widgets.git",
        "https://github.com/acme",
        "https://github.com/../widgets",
        "/some/local/path",
    ] {
        assert_eq!(parse_remote(url), None, "{url}");
    }
}

#[test]
fn the_repo_argument_takes_owner_repo_with_an_optional_host() {
    assert_eq!(parse_repo_argument("acme/widgets"), Some(repo()));
    let enterprise = parse_repo_argument("github.example.com/acme/widgets").expect("enterprise");
    assert_eq!(enterprise.host, "github.example.com");
    assert_eq!(parse_repo_argument("widgets"), None);
    assert_eq!(parse_repo_argument("evil.com/acme/widgets"), None);
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
        ok("https://gitlab.com/acme/widgets.git\n"),
    );
    let refused = client(&runner)
        .origin(Path::new("/work"))
        .expect_err("not github");
    assert_eq!(refused.code, "repo_not_github");
    assert!(
        refused.message.contains("repo"),
        "it says how to name one: {}",
        refused.message
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
        .get_json(&repo(), "commits/abc/check-runs")
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
        .get_json(&repo(), "commits/abc/check-runs")
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
            .get_json(&repo(), "commits/abc/check-runs")
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
fn gh_is_asked_with_an_argument_vector_for_the_repository_on_its_host() {
    let runner = Arc::new(ScriptedRunner::default());
    runner.set("api", ok("{\"check_runs\":[]}"));
    client(&runner)
        .get_json(&repo(), "commits/abc/check-runs?per_page=100")
        .expect("json");
    assert_eq!(
        runner.calls(),
        vec![
            "gh api --hostname github.com -H Accept: application/vnd.github+json \
             repos/acme/widgets/commits/abc/check-runs?per_page=100"
                .to_string()
        ]
    );
}

#[test]
fn a_program_that_does_not_exist_is_reported_as_missing() {
    let outcome = ProcessRunner.run("devboule-no-such-program-for-ci-watch", &[]);
    assert_eq!(outcome, Err(GitRunError::NotFound));
}
