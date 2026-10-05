//! The secrets this process has typed into a page, and their removal from
//! everything that page then says.
//!
//! Once a password is in a field it is in the page, and from there it can reach
//! an agent by any of the ways a page can speak: a show-password toggle turns
//! the field into an ordinary text one, a site that logs its own form puts the
//! value on the console, and a text answer built from either reads it. The
//! masking in `mask.rs` covers the fields the DOM still calls password fields;
//! this covers the value itself, wherever it turns up in an answer.
//!
//! One copy per tab and per site, from the fill until the tab's own frame
//! leaves that site — a same-site redirect keeps it, because the page that
//! lands may still print what was typed into the one before — or until the tab
//! closes. Nothing else keeps it: the OS store has its own, and this is the
//! one that would travel.

use std::collections::HashMap;
use std::sync::Mutex;

use serde_json::Value;

/// What a filled password reads as once it has to be taken out. One fixed run,
/// so a replaced password says nothing about its length either.
const HIDDEN: &str = "[hidden]";

static TYPED: Mutex<Option<HashMap<String, Vec<Typed>>>> = Mutex::new(None);

/// One value typed into one tab, on one site.
struct Typed {
    origin: String,
    secret: String,
}

/// A value this process has just typed into `tab` on `origin`.
pub fn remember(tab: &str, origin: &str, secret: &str) {
    let mut typed = TYPED.lock().expect("browser scrub poisoned");
    let held = typed
        .get_or_insert_with(HashMap::new)
        .entry(tab.to_owned())
        .or_default();
    if held
        .iter()
        .any(|one| one.origin == origin && one.secret == secret)
    {
        return;
    }
    held.push(Typed {
        origin: origin.to_owned(),
        secret: secret.to_owned(),
    });
}

/// The tab is gone: nothing of it is held any more.
pub fn forget(tab: &str) {
    if let Some(typed) = TYPED.lock().expect("browser scrub poisoned").as_mut() {
        typed.remove(tab);
    }
}

/// The tab's own frame is now on `origin`, or `None` when its address is one
/// no site can be compared on. A value typed on any other site is of no use to
/// whatever is on screen and is dropped, which is what bounds how long one is
/// held.
///
/// Dropped lazily, on the next command about this tab: a navigation is not an
/// event this module is told about, and holding a value for one command longer
/// only ever scrubs more, never less.
pub fn left(tab: &str, origin: Option<&str>) {
    let mut typed = TYPED.lock().expect("browser scrub poisoned");
    if let Some(held) = typed.as_mut().and_then(|typed| typed.get_mut(tab)) {
        held.retain(|one| Some(one.origin.as_str()) == origin);
    }
}

/// Take every held value out of every string of one answer.
pub fn clean(answer: &mut Value) {
    match answer {
        Value::String(text) => clean_text(text),
        Value::Object(fields) => {
            for value in fields.values_mut() {
                clean(value);
            }
        }
        Value::Array(items) => {
            for item in items {
                clean(item);
            }
        }
        _ => {}
    }
}

/// Take every held value out of one piece of text.
pub fn clean_text(text: &mut String) {
    let typed = TYPED.lock().expect("browser scrub poisoned");
    let Some(held) = typed.as_ref() else {
        return;
    };
    for one in held.values().flatten() {
        if one.secret.is_empty() {
            continue;
        }
        *text = text.replace(one.secret.as_str(), HIDDEN);
    }
}

/// How many values one tab is holding, for a test to read.
#[cfg(test)]
pub fn typed_of(tab: &str) -> usize {
    TYPED
        .lock()
        .expect("browser scrub poisoned")
        .as_ref()
        .and_then(|typed| typed.get(tab))
        .map_or(0, Vec::len)
}

#[cfg(test)]
#[path = "scrub_tests.rs"]
mod tests;
