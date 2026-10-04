use super::*;
use crate::browser::test_support::FakePage;
use std::sync::Arc;

fn parked() -> Size {
    Size {
        width: 770.0,
        height: 751.0,
    }
}

#[test]
fn a_parked_page_is_put_on_screen_at_the_size_its_pane_last_used() {
    let page = FakePage::new();
    let live = Live::default();
    tauri::async_runtime::block_on(async {
        present_for(&page, &live, parked())
            .await
            .expect("the page takes the override");
    });
    assert!(live.overridden(), "and the tab knows it carries one");

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
    let live = Live::default();
    live.set_parked(false);
    tauri::async_runtime::block_on(async {
        present_for(&page, &live, parked())
            .await
            .expect("nothing to do is not a failure");
    });
    assert!(!live.overridden());

    assert_eq!(
        page.called("Emulation.setDeviceMetricsOverride"),
        0,
        "overriding a page that is already on screen would re-lay it out for \
         nothing, and the layout is what the user is looking at"
    );
}

/// A page that remembers the longest wait each call was allowed.
struct Recording(std::sync::Mutex<Vec<Duration>>);

impl Page for Recording {
    fn call<'a>(&'a self, _method: &'a str, _params: Value) -> Call<'a> {
        Box::pin(async { Ok(Value::Null) })
    }

    fn call_within<'a>(&'a self, method: &'a str, params: Value, limit: Duration) -> Call<'a> {
        self.0.lock().expect("recording poisoned").push(limit);
        self.call(method, params)
    }
}

#[test]
fn a_command_that_has_run_out_of_time_makes_no_call() {
    let page = FakePage::new();
    let bounded = Bounded::new(&page, Deadline::in_(Duration::from_millis(5)));
    std::thread::sleep(Duration::from_millis(30));

    let error = tauri::async_runtime::block_on(bounded.call("DOM.getBoxModel", json!({})))
        .expect_err("no time is no call");

    assert!(
        error.message().contains("ran out of time"),
        "{}",
        error.message()
    );
    assert_eq!(page.called("DOM.getBoxModel"), 0);
}

#[test]
fn a_call_is_never_allowed_longer_than_what_is_left_of_the_command() {
    let recording = Recording(std::sync::Mutex::new(Vec::new()));
    let bounded = Bounded::new(&recording, Deadline::in_(Duration::from_secs(2)));

    tauri::async_runtime::block_on(async {
        bounded
            .call_within("A", json!({}), Duration::from_secs(10))
            .await
            .expect("answered");
        bounded.call("B", json!({})).await.expect("answered");
        bounded
            .call_within("C", json!({}), Duration::from_millis(50))
            .await
            .expect("answered");
    });

    let limits = recording.0.lock().expect("recording poisoned").clone();
    assert_eq!(limits.len(), 3);
    assert!(
        limits[0] <= Duration::from_secs(2),
        "a fixed 10 s wait is cut to what is left: {:?}",
        limits[0]
    );
    assert!(limits[1] <= Duration::from_secs(2), "{:?}", limits[1]);
    assert!(
        limits[2] <= Duration::from_millis(50),
        "and a shorter ask is kept: {:?}",
        limits[2]
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

#[test]
fn a_page_the_pane_presents_while_the_override_is_in_flight_has_it_cleared() {
    let live = Arc::new(Live::default());
    let presents = Arc::clone(&live);
    let page = FakePage::new().during("Emulation.setDeviceMetricsOverride", move || {
        // What `set_rect` does for a presented page.
        presents.set_parked(false);
        presents.take_overridden();
    });

    tauri::async_runtime::block_on(present_for(&page, &live, parked())).expect("answered");

    assert_eq!(
        page.called("Emulation.clearDeviceMetricsOverride"),
        1,
        "an override that landed after the pane presented is taken off"
    );
    assert!(!live.overridden());
}

#[test]
fn an_override_the_page_refused_leaves_no_flag_behind() {
    let page = FakePage::new().refusing(
        "Emulation.setDeviceMetricsOverride",
        CdpError::Refused("no".to_owned()),
    );
    let live = Live::default();

    let applied = tauri::async_runtime::block_on(present_for(&page, &live, parked()));

    assert!(applied.is_err());
    assert!(!live.overridden());
}
