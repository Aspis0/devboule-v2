//! `new_tab` when the calling session already has the address open in the
//! background: the one tab it may take back, and the tabs that stay untouched.

use std::sync::Arc;

use crate::browser::registry::{AgentDrive, BrowserRegistry};
use crate::browser::test_support::{registry_with, registry_with_drive};

use super::{reusable_at, tab_list};

const ADDRESS: &str = "https://example.test/sign-in";

/// A registry whose one tab was opened by `session` and is parked in the background.
fn opened_by(session: &str) -> BrowserRegistry {
    registry_with_drive("tab-1", Arc::new(AgentDrive::opened_by_agent(&[], session)))
}

#[test]
fn a_parked_tab_the_same_session_opened_is_taken_back() {
    let registry = opened_by("s-1");

    let found = reusable_at(&registry, "ws-1", ADDRESS, "s-1").expect("the session's tab");
    assert_eq!(found.browser_id, "tab-1");
}

#[test]
fn another_sessions_tab_is_never_taken_back() {
    let registry = opened_by("s-2");

    assert!(reusable_at(&registry, "ws-1", ADDRESS, "s-1").is_none());
}

#[test]
fn a_tab_on_screen_is_never_taken_back_even_from_its_own_session() {
    let registry = opened_by("s-1");
    registry
        .tab_of("tab-1")
        .expect("the fixture tab")
        .live
        .set_parked(false);

    assert!(reusable_at(&registry, "ws-1", ADDRESS, "s-1").is_none());
}

#[test]
fn a_tab_the_person_opened_is_never_taken_back() {
    let registry = registry_with("tab-1");

    assert!(reusable_at(&registry, "ws-1", ADDRESS, "s-1").is_none());
}

#[test]
fn a_tab_an_agent_only_acted_on_is_not_one_it_opened() {
    let registry = registry_with("tab-1");
    // Acting on a tab marks it the agent's to police, not one this session made.
    drop(registry.drive_of("tab-1").expect("the fixture tab").begin());

    assert!(reusable_at(&registry, "ws-1", ADDRESS, "s-1").is_none());
}

#[test]
fn the_tab_is_taken_only_at_its_own_workspace_and_address() {
    let registry = opened_by("s-1");

    assert!(reusable_at(&registry, "ws-2", ADDRESS, "s-1").is_none());
    assert!(reusable_at(&registry, "ws-1", "https://example.test/elsewhere", "s-1").is_none());
}

#[test]
fn an_address_the_app_would_refuse_finds_no_tab() {
    let registry = opened_by("s-1");

    assert!(reusable_at(&registry, "ws-1", "example.test/sign-in", "s-1").is_none());
    assert!(reusable_at(&registry, "ws-1", "not an address", "s-1").is_none());
}

#[test]
fn a_trailing_slash_is_another_page_and_padding_around_the_address_is_not() {
    let registry = opened_by("s-1");
    assert!(reusable_at(&registry, "ws-1", "https://example.test/sign-in/", "s-1").is_none());

    let found = reusable_at(&registry, "ws-1", "  https://example.test/sign-in  ", "s-1");
    assert_eq!(found.map(|tab| tab.browser_id), Some("tab-1".to_owned()));
}

#[test]
fn the_tabs_the_answer_lists_are_the_workspaces_own() {
    let registry = opened_by("s-1");

    let listed = tab_list(&registry, "ws-1");

    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0]["browserId"], "tab-1");
    assert_eq!(listed[0]["url"], ADDRESS);
    assert!(tab_list(&registry, "ws-2").is_empty());
}
