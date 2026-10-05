//! What the fill's tests are driven by: a vault over an in-memory store, a tab,
//! and a sign-in page that answers from a table and models which field holds
//! the focus.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use serde_json::{json, Value};

use super::super::secrets::fake::InMemory;
use super::{page_script, TabInfo, Vault};
use crate::browser::test_support::{registry_with, FakePage};

/// A value that is obviously a test's.
pub(super) const SECRET: &str = "SENTINEL-PW-7f3a";
pub(super) const SITE: &str = "https://shop.example.test";
pub(super) const OTHER: &str = "https://ads.example.test";
pub(super) const USER: &str = "person@example.test";

/// One username field and one password field in the page's own frame, and a
/// third field in a child frame of another site.
pub(super) const USERNAME: &str = "e13";
pub(super) const PASSWORD: &str = "e14";
pub(super) const FOREIGN: &str = "e21";

/// The `<iframe>` element that owns the child frame, whose document holds
/// `FOREIGN` (node 21).
const CHILD_OWNER: u64 = 900;
/// A control the page can move the focus to.
const ELSEWHERE: u64 = 99;

pub(super) struct Over {
    _dir: tempfile::TempDir,
    pub(super) vault: Arc<Vault>,
    pub(super) store: Arc<InMemory>,
}

impl Over {
    pub(super) fn new() -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = Arc::new(InMemory::empty());
        let vault = Arc::new(Vault::new(
            dir.path().to_path_buf(),
            Box::new(Arc::clone(&store)),
        ));
        Over {
            _dir: dir,
            vault,
            store,
        }
    }

    /// One saved login for `origin`, and the id its password is filed under.
    pub(super) fn save(&self, label: &str, origin: &str) -> String {
        self.vault
            .create(label, &[origin.to_owned()], USER, SECRET)
            .expect("the login was saved")
            .id
    }
}

/// The tab the commands run on: parked, owned, at the fixture's address.
pub(super) fn tab() -> TabInfo {
    tab_named("fill-login-tab")
}

/// A tab of its own, for a test that counts what the scrub holds for it: the
/// scrub is keyed by tab id and shared by every test running at once.
pub(super) fn tab_named(id: &str) -> TabInfo {
    registry_with(id)
        .tab_of(id)
        .expect("the fixture tab is claimed")
}

/// What the browser says of a node: an `<iframe>` carries the document it
/// owns, and nothing names the frame a plain input is in.
pub(super) fn described(asked: &Value) -> Value {
    let node = &asked["backendNodeId"];
    if *node == json!(CHILD_OWNER) {
        return json!({"node": {"backendNodeId": CHILD_OWNER, "nodeName": "IFRAME",
            "contentDocument": {"backendNodeId": 901, "children": [
                {"backendNodeId": 21, "nodeName": "INPUT"}]}}});
    }
    let kind = match node.as_u64() {
        Some(13) => "email",
        Some(14 | 21) => "password",
        _ => "text",
    };
    json!({"node": {"backendNodeId": node, "nodeName": "INPUT", "attributes": ["type", kind]}})
}

/// A page whose own frame is `SITE`, with one child frame of `OTHER`, and the
/// three fields above in one frame or the other. `DOM.focus` gives a field the
/// focus, and a script that asks who holds it is answered from that.
pub(super) fn sign_in_page() -> FakePage {
    focus_model(FakePage::new(), false)
        .answering_with("Page.getFrameTree", |_| {
            json!({"frameTree": {
                "frame": {"id": "main", "url": format!("{SITE}/sign-in")},
                "childFrames": [{"frame": {"id": "child", "url": format!("{OTHER}/widget")}}],
            }})
        })
        .answering_with(
            "DOM.getFrameOwner",
            |_| json!({"backendNodeId": CHILD_OWNER}),
        )
        .answering_with("DOM.describeNode", described)
        .answering_with(
            "DOM.getBoxModel",
            |_| json!({"model": {"width": 180, "height": 24}}),
        )
        .answering_with(
            "DOM.resolveNode",
            |asked| json!({"object": {"objectId": format!("node-{}", asked["backendNodeId"])}}),
        )
        .answering_with(
            "Runtime.evaluate",
            |_| json!({ "result": { "type": "string", "value": "complete" } }),
        )
}

/// [`sign_in_page`], on which emptying a field is a script that hands the
/// focus to another control, as a page's own `input` handler can.
pub(super) fn page_that_takes_the_focus_when_a_field_is_emptied() -> FakePage {
    focus_model(sign_in_page(), true)
}

/// Who holds the focus, as `DOM.focus` and the page's own scripts move it.
fn focus_model(page: FakePage, steals: bool) -> FakePage {
    let held = Arc::new(AtomicU64::new(0));
    let given = Arc::clone(&held);
    page.answering_with("DOM.focus", move |asked| {
        given.store(
            asked["backendNodeId"].as_u64().unwrap_or_default(),
            Ordering::SeqCst,
        );
        json!({})
    })
    .answering_with("Runtime.callFunctionOn", move |asked| {
        let node = asked["objectId"]
            .as_str()
            .and_then(|id| id.strip_prefix("node-"))
            .and_then(|id| id.parse::<u64>().ok())
            .unwrap_or_default();
        let value = if asked["functionDeclaration"] == json!(page_script::CLEAR) {
            if steals {
                held.store(ELSEWHERE, Ordering::SeqCst);
            }
            true
        } else {
            held.load(Ordering::SeqCst) == node
        };
        json!({"result": {"value": value}})
    })
}

/// [`sign_in_page`] with `node` described as `element` instead.
pub(super) fn with_node(node: u64, element: Value) -> FakePage {
    sign_in_page().answering_with("DOM.describeNode", move |asked| {
        if asked["backendNodeId"] == json!(node) {
            json!({ "node": element.clone() })
        } else {
            described(asked)
        }
    })
}

/// What the page was asked to type, in the order it was asked.
pub(super) fn typed(page: &FakePage) -> Vec<Value> {
    page.calls()
        .into_iter()
        .filter(|(method, _)| method == "Input.insertText")
        .map(|(_, params)| params["text"].clone())
        .collect()
}

pub(super) fn asking(args: Value) -> Value {
    let mut args = args;
    args["browserId"] = json!("fill-login-tab");
    args
}
