//! The chain's contract: what it names, how it stays bounded, and that data
//! hops outlive an agent hop, a cut and a later delivery.

use super::{hop, Chain, MAX_CHAIN_HOPS};

fn from_sender(inbound: &Chain, id: &str) -> Chain {
    inbound.extend(hop("local", id))
}

#[test]
fn a_chain_names_every_hop_sender_last() {
    let far = Chain::default().extend(hop("peer", "dev-phone/s.far.1"));
    let relayed = far.extend(hop("local", "s.local.2"));
    assert_eq!(
        relayed.hops(),
        vec!["peer:dev-phone/s.far.1", "local:s.local.2"]
    );
    assert!(!relayed.is_tainted());
}

#[test]
fn a_chain_is_bounded_and_the_cut_is_marked() {
    let mut chain = Chain::default();
    for index in 0..8 {
        chain = from_sender(&chain, &format!("s.{index}"));
    }
    let hops = chain.hops();
    assert_eq!(hops.len(), MAX_CHAIN_HOPS);
    assert_eq!(hops.first().map(String::as_str), Some("…"));
    assert_eq!(hops.last().map(String::as_str), Some("local:s.7"));
    let long = hop("peer", &"d".repeat(500));
    assert!(long.chars().count() <= "peer:".len() + 96);
}

#[test]
fn a_page_read_taints_and_the_hop_rides_ahead_of_the_agents() {
    let read = Chain::default().tainted_by(hop("browser", "shop.example.test"));
    let relayed = read.extend(hop("local", "s.a"));
    assert!(relayed.is_tainted());
    assert_eq!(
        relayed.hops(),
        vec!["browser:shop.example.test", "local:s.a"]
    );
    let again = read.tainted_by(hop("browser", "shop.example.test"));
    assert_eq!(again.hops().len(), 1, "one hop per source");
}

/// A later harmless delivery replaces the agent hops and keeps the data hops: an
/// agent that read a page cannot be cleaned by hearing from another agent.
#[test]
fn a_later_delivery_does_not_launder_an_earlier_taint() {
    let tainted = Chain::default().tainted_by("terminal".to_string());
    let harmless = Chain::default().extend(hop("local", "s.c"));
    let after = tainted.receive(&harmless);
    assert!(after.is_tainted());
    assert_eq!(after.hops(), vec!["terminal", "local:s.c"]);
}

#[test]
fn a_taint_survives_the_bound_as_a_flag() {
    let mut chain = Chain::default().tainted_by("ci".to_string());
    for index in 0..6 {
        chain = from_sender(&chain, &format!("s.{index}"));
    }
    assert!(chain.is_tainted(), "the flag outlives the dropped hop");
    assert!(!chain.hops().contains(&"ci".to_string()));
    assert_eq!(chain.hops().first().map(String::as_str), Some("…"));
}

#[test]
fn a_taint_travels_with_the_relay_to_the_receiver() {
    let reader = Chain::default().tainted_by(hop("browser", "evil.example.test"));
    let frame = reader.extend(hop("local", "s.a"));
    let receiver = Chain::default().receive(&frame);
    assert!(receiver.is_tainted());
    assert!(receiver
        .hops()
        .contains(&"browser:evil.example.test".to_string()));
}
