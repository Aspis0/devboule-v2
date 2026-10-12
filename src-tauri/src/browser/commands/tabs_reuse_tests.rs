//! `new_tab` when the workspace already has the address open: the tab it finds,
//! the reload it sends, and the list of tabs the answer carries.

use serde_json::json;

use crate::browser::deadline::Deadline;
use crate::browser::registry::BrowserRegistry;
use crate::browser::test_support::{ax_fixture, registry_with, FakePage};

use super::{open_at, reload_tab, tab_list};

const ADDRESS: &str = "https://example.test/sign-in";

/// A registry whose one tab an agent has acted on, which is what makes the tab
/// the agent's. The taint is kept after the command, so the guard is dropped.
fn agents_tab() -> BrowserRegistry {
    let registry = registry_with("tab-1");
    drop(registry.drive_of("tab-1").expect("the fixture tab").begin());
    registry
}

#[test]
fn an_agent_tab_of_this_workspace_at_the_address_is_found_and_another_workspace_is_not() {
    let registry = agents_tab();

    let found = open_at(&registry, "ws-1", ADDRESS).expect("the tab is open there");
    assert_eq!(found.browser_id, "tab-1");
    assert!(open_at(&registry, "ws-2", ADDRESS).is_none());
    assert!(open_at(&registry, "ws-1", "https://example.test/elsewhere").is_none());
}

#[test]
fn a_tab_the_person_opened_is_never_reloaded_for_an_agent() {
    let registry = registry_with("tab-1");

    assert!(open_at(&registry, "ws-1", ADDRESS).is_none());
}

#[test]
fn an_address_the_app_would_refuse_finds_no_tab() {
    let registry = agents_tab();

    assert!(open_at(&registry, "ws-1", "example.test/sign-in").is_none());
    assert!(open_at(&registry, "ws-1", "not an address").is_none());
}

#[test]
fn a_trailing_slash_is_another_page_and_padding_around_the_address_is_not() {
    let registry = agents_tab();
    assert!(open_at(&registry, "ws-1", "https://example.test/sign-in/").is_none());

    let found = open_at(&registry, "ws-1", "  https://example.test/sign-in  ");
    assert_eq!(found.map(|tab| tab.browser_id), Some("tab-1".to_owned()));
}

#[test]
fn a_reused_tab_is_reloaded_and_the_reload_is_waited_for() {
    let registry = agents_tab();
    let tab = open_at(&registry, "ws-1", ADDRESS).expect("the tab is open there");
    let page = FakePage::new()
        .answering("Page.reload", json!({}))
        .answering("Accessibility.getFullAXTree", ax_fixture());

    tauri::async_runtime::block_on(reload_tab(
        &tab,
        &page,
        Deadline::in_(std::time::Duration::from_secs(10)),
    ))
    .expect("reloaded");

    assert_eq!(page.called("Page.reload"), 1);
}

#[test]
fn the_tabs_the_answer_lists_are_the_workspaces_own() {
    let registry = registry_with("tab-1");

    let listed = tab_list(&registry, "ws-1");

    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0]["browserId"], "tab-1");
    assert_eq!(listed[0]["url"], ADDRESS);
    assert!(tab_list(&registry, "ws-2").is_empty());
}
