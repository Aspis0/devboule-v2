//! The markup table: what one node has to say before its value is read as a
//! password's, and the promise the calls that find out are made together.

use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::task::Poll;

use serde_json::json;
use serde_json::Value;

use crate::browser::cdp::{Call, CdpError, Page};

use super::{concurrently, holds_a_password, valued_ids, AxTree};

/// A page whose answer arrives on the second poll of every call, counting how
/// many calls were outstanding at once.
#[derive(Default)]
struct SecondPoll {
    outstanding: Mutex<u32>,
    peak: Mutex<u32>,
    asked: Mutex<u32>,
}

impl SecondPoll {
    fn peak(&self) -> u32 {
        *self.peak.lock().expect("the page count is poisoned")
    }

    fn asked(&self) -> u32 {
        *self.asked.lock().expect("the page count is poisoned")
    }
}

impl Page for SecondPoll {
    fn call<'a>(&'a self, _method: &'a str, _params: serde_json::Value) -> Call<'a> {
        {
            let mut asked = self.asked.lock().expect("the page count is poisoned");
            *asked += 1;
            let mut outstanding = self.outstanding.lock().expect("the page count is poisoned");
            *outstanding += 1;
            let mut peak = self.peak.lock().expect("the page count is poisoned");
            *peak = (*peak).max(*outstanding);
        }
        let mut polls = 0;
        Box::pin(std::future::poll_fn(move |context| {
            polls += 1;
            if polls == 1 {
                // Asked again: an answer that arrived without a second look
                // would never show two calls in flight at all.
                context.waker().wake_by_ref();
                return Poll::Pending;
            }
            *self.outstanding.lock().expect("the page count is poisoned") -= 1;
            Poll::Ready(Ok(json!({
                "node": { "nodeName": "INPUT", "attributes": ["type", "text"] }
            })))
        }))
    }
}

/// One node as `DOM.describeNode` writes it: a tag name and the attributes as
/// the runtime sends them, one flat list of name/value pairs.
fn node(tag: &str, attributes: &[(&str, &str)]) -> serde_json::Value {
    let flat: Vec<serde_json::Value> = attributes
        .iter()
        .flat_map(|(name, value)| [json!(name), json!(value)])
        .collect();
    json!({ "nodeName": tag, "attributes": flat })
}

#[test]
fn an_input_that_says_it_is_a_password_is_one() {
    assert!(holds_a_password(&node("INPUT", &[("type", "password")])));
    // A site that misspells the casing of its own markup is still a site that
    // meant it: the attribute value is read case-insensitively.
    assert!(holds_a_password(&node("INPUT", &[("type", "Password")])));
    assert!(holds_a_password(&node(
        "input",
        &[("id", "passwd"), ("type", "password"), ("name", "pass")]
    )));
}

#[test]
fn a_marked_control_is_one_whatever_its_type_says() {
    // A text input carrying `autocomplete="current-password"` is a password
    // field the runtime renders as ordinary text.
    assert!(holds_a_password(&node(
        "INPUT",
        &[("type", "text"), ("autocomplete", "current-password")]
    )));
    // A textarea says it with `autocomplete`, since it has no `type`.
    assert!(holds_a_password(&node(
        "TEXTAREA",
        &[("autocomplete", "new-password"), ("name", "passphrase")]
    )));
}

#[test]
fn an_ordinary_field_is_not_one() {
    for attributes in [
        vec![("type", "text")],
        vec![("type", "search")],
        vec![("type", "email")],
        vec![("type", "passwordless")],
        // The word alone is not the mark: it must be a whole token.
        vec![("autocomplete", "name email")],
        vec![("type", "hidden")],
    ] {
        assert!(
            !holds_a_password(&node("INPUT", &attributes)),
            "{attributes:?} is an ordinary field"
        );
    }
    // A div that merely mentions a password in a class name is not a control.
    assert!(!holds_a_password(&node(
        "DIV",
        &[("class", "password-help"), ("type", "password")]
    )));
}

#[test]
fn a_node_the_page_answers_with_nothing_is_not_one() {
    // The caller treats a node it could not describe as a password field
    // without consulting this; what this answers is the other direction, that
    // an answer carrying no markup at all names nothing.
    assert!(!holds_a_password(&json!({})));
    assert!(!holds_a_password(&json!({ "nodeName": "INPUT" })));
}

#[test]
fn every_field_a_tree_carries_is_asked_about_once() {
    let tree: AxTree = serde_json::from_value(json!({ "nodes": [
        { "nodeId": "1", "backendDOMNodeId": 7, "value": { "value": "a" } },
        { "nodeId": "2", "backendDOMNodeId": 8, "value": { "value": "b" } },
        // The same node reached twice: one call covers both.
        { "nodeId": "3", "backendDOMNodeId": 7, "value": { "value": "c" } },
        // A field with nothing in it prints nothing, so it is not asked about.
        { "nodeId": "4", "backendDOMNodeId": 9, "value": { "value": "" } },
        { "nodeId": "5", "backendDOMNodeId": 10, "name": { "value": "Sign in" } },
        // A value with no node to ask about is a different case, and the
        // walker's own table answers it.
        { "nodeId": "6", "value": { "value": "d" } },
    ] }))
    .expect("the fixture parses as a tree");

    let (asked, unidentified) = valued_ids(&tree);

    assert_eq!(asked, vec![7, 8], "each distinct valued id, once");
    assert_eq!(unidentified, vec![5], "the one no id could be asked about");
}

#[test]
fn no_more_than_one_chunk_of_asks_is_in_flight_at_a_time() {
    // A page that answers on the second poll of every call is what makes "how
    // many asks were outstanding at once" a fact about this code and not about
    // the page. Unbounded, forty fields would put forty calls in flight against
    // one renderer at once.
    let tree: AxTree = serde_json::from_value(json!({ "nodes":
        (1..=40)
            .map(|at| json!({
                "nodeId": at.to_string(),
                "backendDOMNodeId": at,
                "value": { "value": format!("value {at}") },
            }))
            .collect::<Vec<Value>>(),
    }))
    .expect("the fixture parses as a tree");
    let page = SecondPoll::default();
    let mut tree = tree;

    tauri::async_runtime::block_on(super::redact(&page, &mut tree)).expect("redacted");

    assert_eq!(
        page.peak(),
        super::IN_FLIGHT as u32,
        "no more asks at once than the bound allows"
    );
    assert_eq!(
        page.asked(),
        40,
        "every field is asked about, whatever chunk it fell in"
    );
    assert!(
        tree.nodes.iter().all(|node| node
            .value
            .as_ref()
            .is_some_and(|value| value.text().starts_with("value "))),
        "and an ordinary field keeps its value whichever chunk it fell in"
    );
}

#[test]
fn every_call_is_in_flight_before_any_of_them_is_answered() {
    // A call that only answers once every other call has been asked is the
    // shape of a page that does not answer one node at a time. Asked one after
    // another, the first call would wait for round trips that never start: it
    // gives up, and the test says so.
    let asked = Arc::new(AtomicU32::new(0));
    let waited = Arc::new(AtomicU32::new(0));
    let calls = (0..4)
        .map(|at| {
            let asked = Arc::clone(&asked);
            let waited = Arc::clone(&waited);
            let mut polls = 0;
            Box::pin(std::future::poll_fn(move |context| {
                asked.fetch_add(1, Ordering::SeqCst);
                if asked.load(Ordering::SeqCst) >= 4 {
                    return Poll::Ready(Ok(at));
                }
                polls += 1;
                if polls > 8 {
                    waited.fetch_add(1, Ordering::SeqCst);
                    return Poll::Ready(Err(CdpError::Refused("asked alone".to_owned())));
                }
                context.waker().wake_by_ref();
                Poll::Pending
            })) as Pin<Box<dyn Future<Output = Result<i32, CdpError>> + Send>>
        })
        .collect();

    let answered = tauri::async_runtime::block_on(concurrently(calls));

    assert_eq!(waited.load(Ordering::SeqCst), 0, "no call waited alone");
    assert_eq!(
        answered,
        vec![Ok(0), Ok(1), Ok(2), Ok(3)],
        "every call answered, in the order it was asked"
    );
}
