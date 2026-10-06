//! The destination policy: which URLs a call site's declaration admits, and
//! which addresses a name may resolve to.

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use reqwest::Url;

use super::{admit, classify_gh_host, GhHost, Resolver, Rule};

pub(crate) const PUBLIC: &str = "93.184.216.34";
const BUDGET: Duration = Duration::from_secs(2);

/// A resolver that answers from a table and counts what it was asked.
pub(crate) struct FakeResolver {
    answers: Mutex<HashMap<String, Vec<IpAddr>>>,
    asked: Mutex<Vec<String>>,
}

impl FakeResolver {
    pub(crate) fn new(answers: &[(&str, &[&str])]) -> Arc<Self> {
        Arc::new(Self {
            answers: Mutex::new(
                answers
                    .iter()
                    .map(|(host, addresses)| {
                        (
                            host.to_string(),
                            addresses
                                .iter()
                                .map(|raw| raw.parse().expect("a fixture address"))
                                .collect(),
                        )
                    })
                    .collect(),
            ),
            asked: Mutex::new(Vec::new()),
        })
    }

    pub(crate) fn asked(&self) -> Vec<String> {
        self.asked.lock().expect("asked").clone()
    }
}

impl Resolver for FakeResolver {
    fn resolve(&self, host: &str, _port: u16) -> Result<Vec<IpAddr>, String> {
        self.asked.lock().expect("asked").push(host.to_string());
        self.answers
            .lock()
            .expect("answers")
            .get(host)
            .cloned()
            .ok_or_else(|| format!("{host} is not in this resolver's table"))
    }
}

pub(crate) const CDN: Rule = Rule {
    hosts: &["cdn.example.test"],
    loopback_path: None,
};

const ORACLE: Rule = Rule {
    hosts: &[],
    loopback_path: Some("/oracle/v1/query"),
};

fn verdict(rule: &Rule, raw: &str, resolver: &Arc<FakeResolver>) -> Result<(), String> {
    let resolver: Arc<dyn Resolver> = resolver.clone();
    admit(
        rule,
        &Url::parse(raw).expect("a fixture URL"),
        &resolver,
        BUDGET,
    )
    .map(|_| ())
    .map_err(|refusal| refusal.0)
}

#[test]
fn egress_client_denies_private_targets() {
    let resolver = FakeResolver::new(&[("cdn.example.test", &[PUBLIC])]);
    for raw in [
        "https://127.0.0.1/",
        "https://[::1]/",
        "https://169.254.169.254/latest/meta-data/",
        "https://10.0.0.5/",
        "https://2130706433/",
        "https://0x7f000001/",
        "https://localhost/",
        "https://sub.localhost/",
        "https://user:secret@cdn.example.test/",
        "https://cdn.example.test:8443/",
        "http://cdn.example.test/",
        "ftp://cdn.example.test/",
        "https://other.example.test/",
        "https://cdn.example.test.evil.test/",
        "https://evil-cdn.example.test/",
    ] {
        assert!(
            verdict(&CDN, raw, &resolver).is_err(),
            "{raw} must be refused"
        );
    }
    assert_eq!(
        resolver.asked(),
        Vec::<String>::new(),
        "a host the site never declared is never looked up"
    );
    assert!(verdict(&CDN, "https://cdn.example.test/registry.json", &resolver).is_ok());
    assert!(
        verdict(&CDN, "https://CDN.Example.Test./registry.json", &resolver).is_ok(),
        "case and a trailing dot are one name"
    );
}

#[test]
fn dns_rebind_mixed_answers_refused() {
    let resolver = FakeResolver::new(&[
        ("cdn.example.test", &[PUBLIC, "10.0.0.5"]),
        ("empty.example.test", &[]),
    ]);
    let refused = verdict(&CDN, "https://cdn.example.test/", &resolver).expect_err("one private");
    assert!(refused.contains("private address"), "{refused}");

    let other = Rule {
        hosts: &["empty.example.test", "unknown.example.test"],
        loopback_path: None,
    };
    assert!(verdict(&other, "https://empty.example.test/", &resolver).is_err());
    assert!(
        verdict(&other, "https://unknown.example.test/", &resolver).is_err(),
        "a lookup that fails is a refusal"
    );
}

#[test]
fn oracle_loopback_exception_only_for_its_endpoint() {
    let resolver = FakeResolver::new(&[]);
    assert!(verdict(&ORACLE, "http://127.0.0.1:41234/oracle/v1/query", &resolver).is_ok());
    for raw in [
        "http://127.0.0.1:41234/other",
        "http://127.0.0.1:41234/oracle/v1/query/extra",
        "http://127.0.0.1/oracle/v1/query",
        "http://localhost:41234/oracle/v1/query",
        "http://127.0.0.2:41234/oracle/v1/query",
        "http://10.0.0.5:41234/oracle/v1/query",
        "https://127.0.0.1:41234/oracle/v1/query",
        "http://169.254.169.254/oracle/v1/query",
    ] {
        assert!(verdict(&ORACLE, raw, &resolver).is_err(), "{raw}");
    }
    // A site with no loopback endpoint of its own has no exception at all.
    assert!(verdict(&CDN, "http://127.0.0.1:41234/oracle/v1/query", &resolver).is_err());
}

#[test]
fn github_host_allowlist_exact() {
    assert_eq!(classify_gh_host("github.com"), Ok(GhHost::Dotcom));
    assert_eq!(classify_gh_host("GitHub.com."), Ok(GhHost::Dotcom));
    // Nothing is admitted by a pattern: these are only names the person's own
    // `gh` login must then vouch for, exactly.
    for host in ["github.example.com", "foo.ghe.com", "github.com.evil.test"] {
        assert_eq!(
            classify_gh_host(host),
            Ok(GhHost::Enterprise(host.to_string())),
            "{host}"
        );
    }
    for host in [
        "",
        "127.0.0.1",
        "[::1]",
        "0x7f000001",
        "localhost",
        "github.com@evil.test",
        "github.com/evil",
        "github .com",
        "-github.com",
        "a..b",
    ] {
        assert!(
            classify_gh_host(host).is_err(),
            "{host:?} is not a host name"
        );
    }
}
