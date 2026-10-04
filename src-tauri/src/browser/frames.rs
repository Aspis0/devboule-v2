//! What the page's frame events say about where the tab is.
//!
//! A single-page app moves the address without loading anything: `pushState`,
//! `replaceState` and a hash change change the URL and fire no navigation the
//! webview's own hook ever sees. The runtime reports them as
//! `Page.navigatedWithinDocument`, and reports a new document as
//! `Page.frameNavigated`. Both arrive for every frame, an advert's iframe
//! included, so which frame is the tab's own is part of reading them.

use std::sync::Mutex;

use serde_json::Value;

/// What one event meant for the tab.
#[derive(Debug, PartialEq, Eq)]
pub enum Observed {
    /// The tab's own frame committed a new document.
    NewDocument,
    /// The tab's own frame moved to this address without loading anything.
    SameDocument(String),
    /// Anything else: another frame, another event, a payload that is not one.
    Nothing,
}

/// The tab's own frame, as far as it is known.
#[derive(Default)]
pub struct Frames {
    main: Mutex<Option<String>>,
}

impl Frames {
    /// The frame a `Page.getFrameTree` answer names as the top of the tree:
    /// `{"frameTree":{"frame":{"id":"...", ...}}}`.
    pub fn learn_from_tree(&self, answer: &Value) {
        let id = answer
            .pointer("/frameTree/frame/id")
            .and_then(Value::as_str);
        if let Some(id) = id {
            *self.main.lock().expect("browser frames poisoned") = Some(id.to_owned());
        }
    }

    /// Read one event's parameters, as the runtime sends them in JSON.
    ///
    /// A same-document move of a frame that is not known to be the tab's own is
    /// nothing: until the top frame is known, an iframe's `pushState` would be
    /// shown as the address of the page.
    pub fn observe(&self, event: &str, params: &str) -> Observed {
        let Ok(params) = serde_json::from_str::<Value>(params) else {
            return Observed::Nothing;
        };
        match event {
            "Page.frameNavigated" => {
                let frame = &params["frame"];
                // A frame with a parent is a child frame.
                if frame
                    .get("parentId")
                    .is_some_and(|parent| !parent.is_null())
                {
                    return Observed::Nothing;
                }
                let Some(id) = frame["id"].as_str() else {
                    return Observed::Nothing;
                };
                *self.main.lock().expect("browser frames poisoned") = Some(id.to_owned());
                Observed::NewDocument
            }
            "Page.navigatedWithinDocument" => {
                let main = self.main.lock().expect("browser frames poisoned");
                let ours = params["frameId"].as_str().zip(main.as_deref());
                match (ours, params["url"].as_str()) {
                    (Some((frame, main)), Some(url)) if frame == main && !url.is_empty() => {
                        Observed::SameDocument(url.to_owned())
                    }
                    _ => Observed::Nothing,
                }
            }
            _ => Observed::Nothing,
        }
    }
}

#[cfg(test)]
#[path = "frames_tests.rs"]
mod tests;
