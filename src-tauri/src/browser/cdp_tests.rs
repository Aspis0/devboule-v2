use super::*;
use crate::browser::test_support::FakePage;

fn parked() -> Size {
    Size {
        width: 770.0,
        height: 751.0,
    }
}

#[test]
fn a_parked_page_is_put_on_screen_at_the_size_its_pane_last_used() {
    let page = FakePage::new();
    tauri::async_runtime::block_on(async {
        present_for(&page, true, parked())
            .await
            .expect("the page takes the override");
    });

    let params = page
        .last_params("Emulation.setDeviceMetricsOverride")
        .expect("the override was sent");
    assert_eq!(params["width"].as_f64(), Some(770.0));
    assert_eq!(params["height"].as_f64(), Some(751.0));
    assert_eq!(params["deviceScaleFactor"], 1);
    assert_eq!(params["mobile"], false);
}

#[test]
fn a_page_in_front_is_left_exactly_as_it_is() {
    let page = FakePage::new();
    tauri::async_runtime::block_on(async {
        present_for(&page, false, parked())
            .await
            .expect("nothing to do is not a failure");
    });

    assert_eq!(
        page.called("Emulation.setDeviceMetricsOverride"),
        0,
        "overriding a page that is already on screen would re-lay it out for \
         nothing, and the layout is what the user is looking at"
    );
}

#[test]
fn a_call_that_addresses_a_node_is_the_one_a_refusal_means_a_dead_ref() {
    // The runtime answers all of these with E_INVALIDARG, so the parameters
    // are the only thing that tells a dead node from a misspelled argument.
    assert!(addresses_node(&json!({ "backendNodeId": 12 })));
    assert!(!addresses_node(&json!({ "objectId": "1" })));
    assert!(!addresses_node(&json!({})));
    assert!(!addresses_node(&json!([])));
}

#[test]
fn a_stale_ref_says_so_in_words_a_caller_can_act_on() {
    let error = CdpError::StaleRef;

    let message = error.message();
    assert!(
        message.starts_with("stale_ref:"),
        "the code is in the message, where a caller reads it: {message}"
    );
    assert!(
        message.contains("take a new snapshot"),
        "and it says what to do: {message}"
    );

    let refused = CdpError::Refused("DOM.getBoxModel: refused".to_owned());
    assert_eq!(
        refused.message(),
        "DOM.getBoxModel: refused",
        "a refused method carries the runtime's own text and no advice"
    );
}
