//! The shared range table: every spelling of a private address is read as the
//! address it is, and a public one is left alone.

use super::{address_blocked, is_localhost, literal_address, looks_numeric, normalise_host};

fn blocked(host: &str) -> bool {
    literal_address(host).is_some_and(|address| address_blocked(address).is_some())
}

#[test]
fn every_spelling_of_a_private_address_reads_as_that_address() {
    for host in [
        "127.0.0.1",
        "127.1",
        "2130706433",
        "0x7f000001",
        "0177.0.0.1",
        "0",
        "10.0.0.1",
        "172.16.0.1",
        "192.168.1.1",
        "169.254.169.254",
        "100.64.0.1",
        "[::1]",
        "::ffff:127.0.0.1",
        "::ffff:10.0.0.1",
        "fe80::1",
        "fd12:3456::1",
        "2002:a00:1::",
        "2001::1",
        "64:ff9b::7f00:1",
        "255.255.255.255",
    ] {
        assert!(blocked(host), "{host} must be blocked");
    }
}

#[test]
fn a_public_address_and_the_edges_of_a_range_are_not_blocked() {
    for host in [
        "93.184.216.34",
        "100.63.255.255",
        "100.128.0.0",
        "172.32.0.1",
        "2606:2800:220:1::1",
    ] {
        assert!(!blocked(host), "{host} is public");
    }
}

#[test]
fn a_mapped_address_names_what_it_carries() {
    let loopback = literal_address("::ffff:127.0.0.1").expect("an address");
    assert_eq!(address_blocked(loopback), Some("loopback address"));
}

#[test]
fn hosts_that_are_names_for_this_machine_or_unreadable_numbers() {
    assert!(is_localhost("localhost"));
    assert!(is_localhost("app.LOCALHOST."));
    assert!(!is_localhost("localhost.example.com"));
    assert!(looks_numeric("0x7f.1"));
    assert!(!looks_numeric("example.com"));
    assert_eq!(normalise_host("Example.COM."), "example.com");
}
