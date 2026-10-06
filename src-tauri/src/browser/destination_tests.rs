//! The destination policy's own cases: every blocked range and spelling, the
//! rebinding answer, the exact host:port exception, and who a navigation is
//! checked for.

use std::collections::HashMap;
use std::net::IpAddr;

use super::*;

/// A resolver that answers from a table and refuses to guess.
struct FakeResolver {
    answers: HashMap<&'static str, Vec<IpAddr>>,
}

impl FakeResolver {
    fn new(answers: &[(&'static str, &[&str])]) -> Arc<Self> {
        let answers = answers
            .iter()
            .map(|(host, addresses)| {
                let addresses = addresses
                    .iter()
                    .map(|raw| raw.parse::<IpAddr>().expect("a fixture address"))
                    .collect();
                (*host, addresses)
            })
            .collect();
        Arc::new(Self { answers })
    }
}

impl Resolver for FakeResolver {
    fn resolve(&self, host: &str, _port: u16) -> Result<Vec<IpAddr>, String> {
        self.answers
            .get(host)
            .cloned()
            .ok_or_else(|| format!("{host} is not in this resolver's table"))
    }
}

fn policy(resolver: Arc<dyn Resolver>) -> DestinationPolicy {
    DestinationPolicy::new(Allowlist::default(), resolver)
}

fn admit(policy: &DestinationPolicy, raw: &str) -> Result<(), Blocked> {
    policy.admit(&Url::parse(raw).expect("a fixture URL"), Audience::Agent)
}

const PUBLIC: &str = "93.184.216.34";

#[test]
fn browser_blocks_ipv4_ipv6_special_ranges() {
    let policy = policy(FakeResolver::new(&[("public.test", &[PUBLIC])]));
    for raw in [
        // Loopback, every spelling a resolver would normalise.
        "http://127.0.0.1/",
        "http://127.1/",
        "http://2130706433/",
        "http://0x7f000001/",
        "http://0177.0.0.1/",
        "http://localhost/",
        "http://sub.localhost/",
        "http://localhost./",
        "http://[::1]/",
        "http://[::ffff:127.0.0.1]/",
        // Unspecified and "this network".
        "http://0.0.0.0/",
        "http://0/",
        "http://[::]/",
        // RFC1918.
        "http://10.0.0.1/",
        "http://172.16.0.1/",
        "http://172.31.255.255/",
        "http://192.168.1.1/",
        // Link-local and its IPv6 pair.
        "http://169.254.1.1/",
        "http://[fe80::1]/",
        "http://[fc00::1]/",
        "http://[fd12:3456::1]/",
        // Multicast, broadcast, reserved.
        "http://224.0.0.1/",
        "http://239.255.255.250/",
        "http://255.255.255.255/",
        "http://240.0.0.1/",
        "http://[ff02::1]/",
        // A trailing-dot host is not resolved for agent browsing.
        "http://example.com./",
    ] {
        assert!(admit(&policy, raw).is_err(), "{raw} must be blocked");
    }
    for raw in [
        // A public name through the resolver, and a public literal with none.
        "http://public.test/",
        "http://[2606:2800:220:1::1]/",
    ] {
        assert!(admit(&policy, raw).is_ok(), "{raw} must be allowed");
    }
}

#[test]
fn browser_blocks_tailnet_and_metadata() {
    let policy = policy(FakeResolver::new(&[("public.test", &[PUBLIC])]));
    for raw in [
        "http://100.64.0.1/",
        "http://100.100.100.100/",
        "http://100.127.255.255/",
        "http://169.254.169.254/",
        "http://[fd00:ec2::254]/",
    ] {
        assert!(admit(&policy, raw).is_err(), "{raw} must be blocked");
    }
    for raw in ["http://100.63.255.255/", "http://100.128.0.0/"] {
        assert!(admit(&policy, raw).is_ok(), "{raw} is outside 100.64/10");
    }
}

#[test]
fn redirect_to_private_refused() {
    let policy = policy(FakeResolver::new(&[("public.test", &[PUBLIC])]));
    assert!(admit(&policy, "https://public.test/start").is_ok());
    let refused = admit(&policy, "http://192.168.1.10/after-redirect")
        .expect_err("the redirect target is private");
    assert!(refused.message().contains("192.168.1.10"), "{refused}");
}

#[test]
fn dns_rebind_second_address_refused() {
    let policy = policy(FakeResolver::new(&[
        ("rebind.test", &[PUBLIC, "10.0.0.5"]),
        ("rebind-ok.test", &[PUBLIC]),
    ]));
    let refused = admit(&policy, "http://rebind.test/").expect_err("one private answer is enough");
    assert!(refused.message().contains("10.0.0.5"), "{refused}");
    // The host's answer is cached: the second look is the same verdict.
    assert!(admit(&policy, "http://rebind.test/again").is_err());
    assert!(admit(&policy, "http://rebind-ok.test/").is_ok());
}

#[test]
fn localhost_opt_in_is_host_scoped() {
    let policy = DestinationPolicy::new(
        Allowlist::from_entries(&["localhost:1420"]),
        FakeResolver::new(&[]),
    );
    assert!(admit(&policy, "http://localhost:1420/").is_ok());
    assert!(admit(&policy, "https://localhost:1420/").is_ok());
    for raw in [
        "http://localhost:1421/",
        "http://127.0.0.1:1420/",
        "http://sub.localhost:1420/",
        "http://localhost/",
    ] {
        assert!(
            admit(&policy, raw).is_err(),
            "{raw} is not the allowed host:port"
        );
    }
}

#[test]
fn page_script_navigation_rechecked() {
    let policy = policy(FakeResolver::new(&[("public.test", &[PUBLIC])]));
    let private = Url::parse("http://10.0.0.1/away").expect("a fixture URL");
    // A parked tab is the agent's background page: its own scripts are not a
    // person's clicks.
    assert!(policy.admit_hook(&private, false, true).is_err());
    // An agent command in flight on a presented tab is still the agent.
    assert!(policy.admit_hook(&private, true, false).is_err());
    // The person in front of it, with no agent acting, is the person.
    assert!(policy.admit_hook(&private, false, false).is_ok());
}

#[test]
fn person_driven_tab_unchanged() {
    let policy = policy(FakeResolver::new(&[("public.test", &[PUBLIC])]));
    for raw in [
        "http://127.0.0.1:5173/",
        "http://localhost:1420/",
        "http://192.168.1.1/",
        "http://[::1]:8080/",
    ] {
        let url = Url::parse(raw).expect("a fixture URL");
        assert!(
            policy.admit(&url, Audience::Person).is_ok(),
            "{raw} is the person's to open"
        );
    }
}
