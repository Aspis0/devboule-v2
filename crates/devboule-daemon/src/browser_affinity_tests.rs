//! The affinity map on its own: scope, first claim, the bound and what leaves
//! first.

use super::*;

#[test]
fn the_same_id_in_two_workspaces_is_two_tabs() {
    let mut map = TabAffinity::default();
    assert_eq!(map.claim(Some("w1"), "tab", "h1"), Claim::Learned);
    assert_eq!(map.claim(Some("w2"), "tab", "h2"), Claim::Learned);
    assert_eq!(map.route(Some("w1"), "tab"), TabRoute::Host("h1"));
    assert_eq!(map.route(Some("w2"), "tab"), TabRoute::Host("h2"));
    assert_eq!(map.route(None, "tab"), TabRoute::Unknown);
}

#[test]
fn the_first_claim_wins_and_a_conflict_moves_nothing() {
    let mut map = TabAffinity::default();
    assert_eq!(map.claim(Some("w1"), "tab", "h1"), Claim::Learned);
    assert_eq!(map.claim(Some("w1"), "tab", "h1"), Claim::Known);
    assert_eq!(map.claim(Some("w1"), "tab", "h2"), Claim::Conflict);
    assert_eq!(map.route(Some("w1"), "tab"), TabRoute::Host("h1"));
    assert_eq!(map.len(), 1);
}

#[test]
fn a_live_entry_is_not_stolen_after_another_host_leaves() {
    let mut map = TabAffinity::default();
    map.claim(Some("w1"), "tab", "h1");
    map.claim(Some("w1"), "other", "h2");
    map.strand_host("h2");
    assert_eq!(map.claim(Some("w1"), "tab", "h3"), Claim::Conflict);
    assert_eq!(map.route(Some("w1"), "tab"), TabRoute::Host("h1"));
}

#[test]
fn a_gone_entry_is_replaced_by_the_next_claim() {
    let mut map = TabAffinity::default();
    map.claim(Some("w1"), "tab", "h1");
    map.strand_host("h1");
    assert_eq!(map.route(Some("w1"), "tab"), TabRoute::Gone);
    assert_eq!(map.claim(Some("w1"), "tab", "h2"), Claim::Learned);
    assert_eq!(map.route(Some("w1"), "tab"), TabRoute::Host("h2"));
    assert_eq!(map.len(), 1);
}

#[test]
fn an_unusable_id_is_not_learned() {
    let mut map = TabAffinity::default();
    assert_eq!(map.claim(None, "", "h1"), Claim::Refused);
    assert_eq!(
        map.claim(None, &"x".repeat(MAX_BROWSER_ID_BYTES + 1), "h1"),
        Claim::Refused
    );
    assert_eq!(map.len(), 0);
}

#[test]
fn forgetting_frees_the_key() {
    let mut map = TabAffinity::default();
    map.claim(Some("w1"), "tab", "h1");
    map.forget(Some("w1"), "tab");
    assert_eq!(map.route(Some("w1"), "tab"), TabRoute::Unknown);
    assert_eq!(map.claim(Some("w1"), "tab", "h2"), Claim::Learned);
}

#[test]
fn at_the_cap_the_oldest_entry_goes_and_gone_entries_go_first() {
    let mut map = TabAffinity::default();
    for n in 0..MAX_TAB_OWNERS {
        let host = if n == 5 { "leaver" } else { "live" };
        assert_eq!(map.claim(None, &format!("tab-{n}"), host), Claim::Learned);
    }
    // One host leaves: its tab is the only `Gone` one, though not the oldest.
    map.strand_host("leaver");
    assert_eq!(map.claim(None, "fresh-a", "h2"), Claim::Learned);
    assert_eq!(map.len(), MAX_TAB_OWNERS, "the map stays at its bound");
    assert_eq!(
        map.route(None, "tab-5"),
        TabRoute::Unknown,
        "the Gone entry was evicted ahead of older live ones"
    );
    assert_eq!(map.route(None, "tab-0"), TabRoute::Host("live"));
    assert_eq!(map.route(None, "fresh-a"), TabRoute::Host("h2"));
}

#[test]
fn with_no_gone_entry_the_oldest_live_one_is_evicted() {
    let mut map = TabAffinity::default();
    for n in 0..MAX_TAB_OWNERS {
        map.claim(None, &format!("tab-{n}"), "h1");
    }
    assert_eq!(map.claim(None, "newer", "h1"), Claim::Learned);
    assert_eq!(map.len(), MAX_TAB_OWNERS);
    assert_eq!(map.route(None, "tab-0"), TabRoute::Unknown);
    assert_eq!(map.route(None, "tab-1"), TabRoute::Host("h1"));
    assert_eq!(
        map.claim(None, "even-newer", "h1"),
        Claim::Learned,
        "a full map still learns"
    );
}

#[test]
fn only_the_declared_commands_create_or_close_tabs() {
    assert!(creates_tab("new_tab"));
    assert!(!creates_tab("navigate"));
    assert!(!creates_tab("list_tabs"));
    assert!(closes_tab("close_tab"));
    assert!(!closes_tab("new_tab"));
}
