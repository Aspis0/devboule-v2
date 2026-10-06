//! The destination policy's own cases: every blocked range and spelling, the
//! rebinding answer, the exact host:port exception, and what is never checked.

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::Mutex;

use super::*;

/// A resolver that answers from a table and refuses to guess. Mutable and
/// counted, so a test can flip an answer under a policy and see whether the
/// policy looked again.
struct FakeResolver {
    answers: Mutex<HashMap<String, Vec<IpAddr>>>,
    calls: Mutex<HashMap<String, usize>>,
}

impl FakeResolver {
    fn new(answers: &[(&str, &[&str])]) -> Arc<Self> {
        let answers = answers
            .iter()
            .map(|(host, addresses)| (host.to_string(), parse_all(addresses)))
            .collect();
        Arc::new(Self {
            answers: Mutex::new(answers),
            calls: Mutex::new(HashMap::new()),
        })
    }

    fn set(&self, host: &str, addresses: &[&str]) {
        self.answers
            .lock()
            .expect("the fake resolver")
            .insert(host.to_string(), parse_all(addresses));
    }

    fn calls(&self, host: &str) -> usize {
        self.calls
            .lock()
            .expect("the fake resolver")
            .get(host)
            .copied()
            .unwrap_or(0)
    }
}

fn parse_all(addresses: &[&str]) -> Vec<IpAddr> {
    addresses
        .iter()
        .map(|raw| raw.parse::<IpAddr>().expect("a fixture address"))
        .collect()
}

impl Resolver for FakeResolver {
    fn resolve(&self, host: &str, _port: u16) -> Result<Vec<IpAddr>, String> {
        *self
            .calls
            .lock()
            .expect("the fake resolver")
            .entry(host.to_string())
            .or_insert(0) += 1;
        self.answers
            .lock()
            .expect("the fake resolver")
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
        "http://[::ffff:10.0.0.1]/",
        "http://[2002:7f00:1::]/",
        "http://[2002:a00:1::]/",
        "http://[2001::1]/",
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
    // The block is remembered: the second look is the same verdict.
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

#[test]
fn a_mapped_address_keeps_its_category_in_the_reason() {
    let policy = policy(FakeResolver::new(&[]));
    let refused =
        admit(&policy, "http://[::ffff:127.0.0.1]/").expect_err("a mapped loopback is loopback");
    assert!(refused.message().contains("loopback address"), "{refused}");
}

#[test]
fn an_empty_answer_fails_closed() {
    let policy = policy(FakeResolver::new(&[("empty.test", &[])]));
    let refused = admit(&policy, "http://empty.test/").expect_err("no address is not public");
    assert!(refused.message().contains("no address"), "{refused}");
}

#[test]
fn a_public_verdict_is_never_reused() {
    let resolver = FakeResolver::new(&[("flip.test", &[PUBLIC])]);
    let policy = policy(Arc::clone(&resolver) as Arc<dyn Resolver>);
    assert!(admit(&policy, "http://flip.test/").is_ok());
    // The same host:port now answers with a private address inside the block
    // TTL window: the second look must resolve again and refuse.
    resolver.set("flip.test", &["10.0.0.5"]);
    let refused = admit(&policy, "http://flip.test/again").expect_err("a flip is seen");
    assert!(refused.message().contains("10.0.0.5"), "{refused}");
}

#[test]
fn a_blocked_verdict_is_reused_without_a_lookup() {
    let resolver = FakeResolver::new(&[("blocked.test", &["10.0.0.5"])]);
    let policy = policy(Arc::clone(&resolver) as Arc<dyn Resolver>);
    assert!(admit(&policy, "http://blocked.test/").is_err());
    let calls = resolver.calls("blocked.test");
    assert!(admit(&policy, "http://blocked.test/again").is_err());
    assert_eq!(resolver.calls("blocked.test"), calls, "the block is cached");
}

#[test]
fn a_failed_lookup_is_blocked_and_remembered_briefly() {
    let resolver = FakeResolver::new(&[]);
    let policy = policy(Arc::clone(&resolver) as Arc<dyn Resolver>);
    let refused = admit(&policy, "http://unknown.test/").expect_err("no answer is not public");
    assert!(
        refused.message().contains("could not be checked"),
        "{refused}"
    );
    assert_eq!(resolver.calls("unknown.test"), 1);
    assert!(admit(&policy, "http://unknown.test/").is_err());
    assert_eq!(resolver.calls("unknown.test"), 1, "the failure is cached");
}

#[test]
fn the_verdict_table_stays_bounded() {
    let policy = policy(FakeResolver::new(&[]));
    for number in 0..(MAX_VERDICTS + 40) {
        assert!(admit(&policy, &format!("http://host-{number}.test/")).is_err());
    }
    let remembered = policy.verdicts.lock().expect("the verdict table").len();
    assert!(remembered <= MAX_VERDICTS, "{remembered} verdicts kept");
}
