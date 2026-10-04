//! A password field's value, taken out of the accessible tree before anything
//! reads it.
//!
//! Nothing in the tree marks a filled `<input type=password>` as a secret. What
//! this WebView2 reports for one is role `textbox`, no `protected` property,
//! and a value of one U+2022 per typed character (captured on this machine;
//! the fixture in `test_support.rs` carries those bytes). So the role cannot be
//! the key, and the shape of the value cannot either: that shape is the leak,
//! because its length is the password's length.
//!
//! The one fact that does say it is in the DOM, so it is asked of the page —
//! `DOM.describeNode` on the id the tree carries, whose attributes spell
//! `type="password"` or an `autocomplete` naming a password. The tree is
//! rewritten once here rather than at each of the three places that print a
//! value, which is what makes the rule total: after [`redact`] the value of a
//! password field is a marker, and nothing downstream of it can print what it
//! stood for.

use std::collections::HashSet;
use std::future::Future;
use std::pin::Pin;
use std::task::Poll;

use serde_json::{json, Value};

use super::ax::{AxTree, AxValue};
use super::cdp::{CdpError, Page};

/// What a password field's value reads as instead: one fixed run, so that no
/// answer anywhere carries the length of the password behind it.
pub const SHOWN: &str = "\u{2022}\u{2022}\u{2022}";

/// How many `DOM.describeNode` calls are in flight at once.
///
/// Enough that one round trip covers an ordinary form, few enough that a page
/// with a thousand filled fields does not put a thousand calls at one renderer
/// at the same time.
pub(super) const IN_FLIGHT: usize = 16;

/// Replace every password field's value in `tree` with [`SHOWN`].
///
/// Two readings are possible for a node the page did not answer about — a field
/// that holds a password, or an ordinary one whose markup could not be read —
/// and they are not the same: the first hides a value, the second hides a value
/// that was never secret. This masks, because the cost of guessing wrong is one
/// masked ordinary field against one printed password.
///
/// A command that ran out of time asking is not answered at all: a caller whose
/// budget is gone needs that said, not a page that looks fine because every
/// lookup failed.
pub async fn redact(page: &dyn Page, tree: &mut AxTree) -> Result<(), CdpError> {
    let (asked, unidentified) = valued_ids(tree);
    // Asked a chunk at a time, and each chunk answered before the next goes
    // out: the page is asked about every field, but never about more fields
    // than a renderer should be asked about at once. Answers come back in the
    // order they were asked, so a chunk boundary changes nothing an id is
    // matched by.
    let mut answers: Vec<Result<Option<Value>, CdpError>> = Vec::with_capacity(asked.len());
    for chunk in asked.chunks(IN_FLIGHT) {
        answers.extend(
            concurrently(
                chunk
                    .iter()
                    .map(|id| Box::pin(described(page, *id)))
                    .collect(),
            )
            .await,
        );
    }

    let mut hidden: HashSet<u64> = HashSet::new();
    for (answer, id) in answers.into_iter().zip(asked) {
        match answer {
            Err(CdpError::OutOfTime) => return Err(CdpError::OutOfTime),
            Ok(Some(node)) if holds_a_password(&node) => {
                hidden.insert(id);
            }
            // The page answered, and the markup says this is an ordinary field.
            Ok(Some(_)) => {}
            Ok(None) | Err(_) => {
                hidden.insert(id);
            }
        }
    }
    for (at, node) in tree.nodes.iter_mut().enumerate() {
        let masked = unidentified.contains(&at)
            || node
                .backend_dom_node_id
                .is_some_and(|id| hidden.contains(&id));
        if masked {
            node.value = Some(AxValue {
                value: Some(Value::String(SHOWN.to_owned())),
            });
        }
    }
    Ok(())
}

/// The distinct ids the page is asked about, and the positions of the valued
/// nodes that carry none. Only a node that would print something is worth a
/// call: an empty field prints nothing and has nothing to hide.
fn valued_ids(tree: &AxTree) -> (Vec<u64>, Vec<usize>) {
    let mut asked: Vec<u64> = Vec::new();
    let mut unidentified: Vec<usize> = Vec::new();
    for (at, node) in tree.nodes.iter().enumerate() {
        if node
            .value
            .as_ref()
            .is_none_or(|value| value.text().is_empty())
        {
            continue;
        }
        match node.backend_dom_node_id {
            Some(id) if !asked.contains(&id) => asked.push(id),
            Some(_) => {}
            None => unidentified.push(at),
        }
    }
    (asked, unidentified)
}

/// What the page says of one node, or `None` when it will not say.
async fn described(page: &dyn Page, backend_id: u64) -> Result<Option<Value>, CdpError> {
    Ok(page
        .call(
            "DOM.describeNode",
            json!({ "backendNodeId": backend_id, "depth": 0 }),
        )
        .await?
        .get("node")
        .cloned())
}

/// Whether one node's markup says it holds a password.
///
/// `type=password` is what an input says. `autocomplete` naming a password is
/// what a textarea says — a textarea's value is its own text, so it reaches the
/// reader where an input's value never does — and what a text input marked
/// `current-password` says. These are the two marks the page-side reader in
/// `commands::read` refuses to read as well; neither is a substitute for the
/// other, because a site picks either.
fn holds_a_password(node: &Value) -> bool {
    let tag = node
        .get("nodeName")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_ascii_uppercase();
    if tag != "INPUT" && tag != "TEXTAREA" {
        return false;
    }
    let attributes = node
        .get("attributes")
        .and_then(Value::as_array)
        .map(|list| list.as_chunks::<2>().0)
        .unwrap_or_default();
    let attribute = |wanted: &str| {
        attributes
            .iter()
            .filter_map(|pair| Some((pair[0].as_str()?, pair[1].as_str()?)))
            .find(|(name, _)| *name == wanted)
            .map(|(_, value)| value.to_ascii_lowercase())
            .unwrap_or_default()
    };
    attribute("type") == "password" || attribute("autocomplete").contains("password")
}

/// Every call asked at once and answered in the order it was asked.
///
/// Asked one after another, a page with forty filled fields costs forty round
/// trips to say forty things it could have said at once, and the last of them
/// is the one that runs the command out of time. This polls every call that is
/// still in flight on every wake, so one slow node holds up only itself.
async fn concurrently<F: Future + ?Sized>(calls: Vec<Pin<Box<F>>>) -> Vec<F::Output> {
    let mut answers: Vec<Option<F::Output>> = calls.iter().map(|_| None).collect();
    let mut in_flight: Vec<(usize, Pin<Box<F>>)> = calls.into_iter().enumerate().collect();
    std::future::poll_fn(move |context| {
        in_flight.retain_mut(|(at, call)| match call.as_mut().poll(context) {
            Poll::Ready(answer) => {
                answers[*at] = Some(answer);
                false
            }
            Poll::Pending => true,
        });
        if in_flight.is_empty() {
            Poll::Ready(
                std::mem::take(&mut answers)
                    .into_iter()
                    .map(|answer| answer.expect("every call answered"))
                    .collect(),
            )
        } else {
            Poll::Pending
        }
    })
    .await
}

#[cfg(test)]
#[path = "mask_tests.rs"]
mod tests;
