use super::*;
use crate::browser::test_support::registry_with;
use devboule_protocol::BrowserCaller;

fn caller() -> BrowserCaller {
    BrowserCaller {
        caller_session_id: "s-1".to_owned(),
        workspace_id: Some("ws-1".to_owned()),
    }
}

#[test]
fn a_minted_id_is_a_uuid_and_never_the_same_one_twice() {
    let one = mint_id().expect("the OS has entropy");
    let two = mint_id().expect("the OS has entropy");

    assert_ne!(one, two, "two tabs in one millisecond must not collide");
    assert_eq!(one.len(), 36, "a v4 UUID as the frontend mints them");
    let groups: Vec<&str> = one.split('-').collect();
    assert_eq!(
        groups.iter().map(|group| group.len()).collect::<Vec<_>>(),
        vec![8, 4, 4, 4, 12]
    );
    assert!(
        groups[2].starts_with('4') && "89ab".contains(&groups[3][0..1]),
        "and it is version 4, not a shape that only looks like one: {one}"
    );
}

#[test]
fn a_caller_with_no_workspace_has_nowhere_to_open_a_tab() {
    let without = BrowserCaller {
        caller_session_id: "s-1".to_owned(),
        workspace_id: None,
    };

    assert!(workspace_of(&caller()).is_ok());
    assert!(
        workspace_of(&without).is_err(),
        "an unowned tab is unlistable"
    );
    assert!(
        workspace_of(&BrowserCaller {
            workspace_id: Some(String::new()),
            ..without
        })
        .is_err(),
        "and an empty workspace is no workspace"
    );
}

#[test]
fn the_tab_event_is_the_shape_the_strip_reads() {
    let opened = serde_json::to_value(TabEvent::Opened {
        browser_id: "tab-1".to_owned(),
        workspace_id: "ws-1".to_owned(),
        url: "https://example.test".to_owned(),
    })
    .expect("the event serialises");
    assert_eq!(
        opened,
        serde_json::json!({
            "kind": "opened",
            "browserId": "tab-1",
            "workspaceId": "ws-1",
            "url": "https://example.test"
        })
    );

    let closed = serde_json::to_value(TabEvent::Closed {
        browser_id: "tab-1".to_owned(),
    })
    .expect("the event serialises");
    assert_eq!(
        closed,
        serde_json::json!({ "kind": "closed", "browserId": "tab-1" })
    );
    let told = serde_json::to_value(TabEvent::State {
        browser_id: "tab-1".to_owned(),
        url: "https://example.test/report".to_owned(),
        title: Some("Quarterly report".to_owned()),
        favicon: None,
    })
    .expect("the event serialises");
    assert_eq!(
        told,
        serde_json::json!({
            "kind": "state",
            "browserId": "tab-1",
            "url": "https://example.test/report",
            "title": "Quarterly report",
            "favicon": null
        }),
        "what a page says for a tab nobody is showing"
    );

    assert_eq!(TAB_EVENT, "browser:tab", "one name, both languages");
}

#[test]
fn a_list_of_tabs_is_the_callers_workspace_and_says_which_is_in_front() {
    let registry = registry_with("tab-1");

    let listed = list_tabs(&registry, &caller()).expect("the workspace has a tab");
    let tabs = listed["tabs"].as_array().expect("tabs are a list");
    assert_eq!(tabs.len(), 1);
    assert_eq!(tabs[0]["browserId"], "tab-1");
    assert_eq!(tabs[0]["url"], "https://example.test/sign-in");
    assert_eq!(tabs[0]["title"], "Sign in");
    assert_eq!(
        tabs[0]["active"], false,
        "a parked page is not the one in front of its pane"
    );

    let other = list_tabs(
        &registry,
        &BrowserCaller {
            caller_session_id: "s-2".to_owned(),
            workspace_id: Some("ws-2".to_owned()),
        },
    )
    .expect("an empty workspace is still a workspace");
    assert_eq!(
        other["tabs"].as_array().map(Vec::len),
        Some(0),
        "another workspace's tab is not in this one's list"
    );
}
