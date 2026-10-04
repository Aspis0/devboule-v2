//! What the command layer is driven by in a test: a page that answers from a
//! table instead of from a WebView2, and a tab that owns one.
//!
//! The fixtures here are written by hand and are about this app's own command
//! surface: a sign-in form with a field, a checkbox, two buttons and a
//! paragraph. Nothing is copied out of a real page, because a fixture that
//! carries a site's content is a fixture that stops being this project's.

#![cfg(test)]

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};

use super::ax::AxTree;
use super::cdp::{Call, CdpError, Page};
use super::registry::{BrowserRegistry, OwnedTab, TabInfo};
use super::tab::BrowserViewState;

/// A page that answers from a table, and remembers what it was asked.
/// Something that happens while a call is in flight.
type Happens = Box<dyn Fn() + Send + Sync>;

/// An answer computed from what was asked, as a renderer computes a capture.
type Answered = Box<dyn Fn(&Value) -> Value + Send + Sync>;

pub struct FakePage {
    answers: HashMap<String, Value>,
    computed: HashMap<String, Answered>,
    failures: HashMap<String, CdpError>,
    late_failures: HashMap<String, (String, CdpError)>,
    hooks: HashMap<String, Happens>,
    calls: Mutex<Vec<(String, Value)>>,
}

impl FakePage {
    /// A page that answers every call with `null` unless told otherwise.
    pub fn new() -> Self {
        FakePage {
            answers: HashMap::new(),
            computed: HashMap::new(),
            failures: HashMap::new(),
            late_failures: HashMap::new(),
            hooks: HashMap::new(),
            calls: Mutex::new(Vec::new()),
        }
    }

    pub fn answering(mut self, method: &str, value: Value) -> Self {
        self.answers.insert(method.to_owned(), value);
        self
    }

    /// An answer that depends on what was asked: a renderer answers a capture
    /// with a picture whose size follows the scale it was asked for, which is
    /// the only way a size ladder can be driven to its end.
    pub fn answering_with(
        mut self,
        method: &str,
        answer: impl Fn(&Value) -> Value + Send + Sync + 'static,
    ) -> Self {
        self.computed.insert(method.to_owned(), Box::new(answer));
        self
    }

    /// A call that refuses, as a stale ref or a bad method would.
    pub fn refusing(mut self, method: &str, error: CdpError) -> Self {
        self.failures.insert(method.to_owned(), error);
        self
    }

    /// A call that answers until `after` has been made and refuses from then
    /// on: a budget that runs out part-way through a command, said in the
    /// order calls happen instead of by a clock a slow machine can outrun.
    pub fn refusing_after(mut self, after: &str, method: &str, error: CdpError) -> Self {
        self.late_failures
            .insert(method.to_owned(), (after.to_owned(), error));
        self
    }

    /// Something that happens elsewhere while this call is in flight: the pane
    /// presenting the page, say, between an override going out and coming back.
    pub fn during(mut self, method: &str, happens: impl Fn() + Send + Sync + 'static) -> Self {
        self.hooks.insert(method.to_owned(), Box::new(happens));
        self
    }

    pub fn calls(&self) -> Vec<(String, Value)> {
        self.calls.lock().expect("fake page poisoned").clone()
    }

    pub fn called(&self, method: &str) -> usize {
        self.calls()
            .iter()
            .filter(|(name, _)| name == method)
            .count()
    }

    pub fn last_params(&self, method: &str) -> Option<Value> {
        self.calls()
            .iter()
            .rev()
            .find(|(name, _)| name == method)
            .map(|(_, params)| params.clone())
    }

    /// The first time this method was called, which is the one that started
    /// something: a key down, a click's press.
    pub fn first_params(&self, method: &str) -> Option<Value> {
        self.calls()
            .iter()
            .find(|(name, _)| name == method)
            .map(|(_, params)| params.clone())
    }
}

impl Page for FakePage {
    fn call<'a>(&'a self, method: &'a str, params: Value) -> Call<'a> {
        self.calls
            .lock()
            .expect("fake page poisoned")
            .push((method.to_owned(), params.clone()));
        if let Some(happens) = self.hooks.get(method) {
            happens();
        }
        let answer = self.answers.get(method).cloned();
        let computed = self.computed.get(method).map(|answer| answer(&params));
        let failure = self.failures.get(method).cloned().or_else(|| {
            let (after, error) = self.late_failures.get(method)?;
            self.called(after).gt(&0).then(|| error.clone())
        });
        Box::pin(async move {
            if let Some(error) = failure {
                return Err(error);
            }
            Ok(answer.or(computed).unwrap_or(Value::Null))
        })
    }
}

/// One tab as the command layer sees it, parked and in `ws-1`.
pub fn parked_tab(id: &str) -> TabInfo {
    let registry = registry_with(id);
    registry.tab_of(id).expect("the fixture tab is claimed")
}

/// A registry holding one tab, in a workspace, at an address.
pub fn registry_with(id: &str) -> BrowserRegistry {
    let registry = BrowserRegistry::new();
    registry
        .claim(
            id,
            OwnedTab {
                label: format!("browser-{id}"),
                rect: super::registry::PARK_RECT,
                live: Arc::default(),
                cancelled: false,
                workspace: "ws-1".to_owned(),
                state: Arc::new(Mutex::new(BrowserViewState {
                    url: "https://example.test/sign-in".to_owned(),
                    title: Some("Sign in".to_owned()),
                    ..BrowserViewState::default()
                })),
                sink: Arc::new(Mutex::new(tauri::ipc::Channel::new(|_| Ok(())))),
                guard: Arc::default(),
            },
        )
        .expect("the fixture tab claims");
    registry
}

/// One `Accessibility.AXValue` as the runtime writes it: always a `type`
/// beside the `value`, which is what a parser that reads only `value` never
/// notices missing.
fn ax_value(kind: &str, value: Value) -> Value {
    json!({ "type": kind, "value": value })
}

/// One `Accessibility.AXProperty`: a name, and a typed value.
pub fn ax_property(name: &str, kind: &str, value: Value) -> Value {
    json!({ "name": name, "value": ax_value(kind, value) })
}

/// A node's own typed value, as a text field's contents arrive.
pub fn ax_text(text: &str) -> Value {
    ax_value("string", json!(text))
}

/// One `Accessibility.AXNode` with every field the protocol requires of it:
/// the ids, `ignored`, the role and the computed name, and the child list.
pub fn ax_node(node_id: &str, backend: u64, role: &str, name: &str, children: &[&str]) -> Value {
    json!({
        "nodeId": node_id,
        "backendDOMNodeId": backend,
        "ignored": false,
        "role": ax_value("role", json!(role)),
        "name": ax_value("computedString", json!(name)),
        "childIds": children,
    })
}

/// The node with one more property, in the order the runtime lists them.
pub fn ax_with_property(mut node: Value, property: Value) -> Value {
    match node.get_mut("properties").and_then(Value::as_array_mut) {
        Some(properties) => properties.push(property),
        None => node["properties"] = json!([property]),
    }
    node
}

/// The node with a field of its own set: a text field's `value`, say.
pub fn ax_with(mut node: Value, field: &str, value: Value) -> Value {
    node[field] = value;
    node
}

/// The tree `Accessibility.getFullAXTree` would answer with for a sign-in
/// form: a heading, a form, a field with a value, a checkbox, two buttons and
/// a paragraph, plus the `generic` and `ignored` nodes a real tree is full of.
pub fn ax_fixture() -> Value {
    let ignored = ax_with(
        ax_node("10", 18, "generic", "", &[]),
        "ignoredReasons",
        json!([{ "name": "uninteresting", "value": ax_value("boolean", json!(true)) }]),
    );
    json!({
        "nodes": [
            ax_node("1", 10, "RootWebArea", "Test page", &["2", "3"]),
            ax_with_property(
                ax_node("2", 11, "heading", "Sign in", &[]),
                ax_property("level", "integer", json!(2)),
            ),
            ax_node("3", 19, "form", "Sign in", &["4", "5", "6", "7", "8", "9"]),
            ax_with_property(
                ax_with(
                    ax_node("4", 13, "textbox", "Email", &[]),
                    "value",
                    ax_text("person@example.test"),
                ),
                ax_property("focused", "booleanOrUndefined", json!(true)),
            ),
            ax_node("5", 20, "StaticText", "Keep me signed in", &[]),
            ax_with_property(
                ax_node("6", 14, "checkbox", "Remember me", &[]),
                ax_property("checked", "tristate", json!("false")),
            ),
            ax_with_property(
                ax_node("7", 15, "button", "Sign in", &[]),
                ax_property("disabled", "boolean", json!(false)),
            ),
            ax_node("8", 16, "paragraph", "Forgot your password?", &[]),
            ax_node("9", 17, "button", "", &[]),
            ax_with(ignored, "ignored", json!(true)),
        ]
    })
}

/// The value this runtime reports for a filled `input[type=password]`: one
/// U+2022 per character. Captured from a real fill on this machine's engine —
/// the run is the runtime's own mask, and its length is the password's
/// length, which is why nothing may print it as it stands.
pub fn masked_by_runtime(characters: usize) -> String {
    "\u{2022}".repeat(characters)
}

/// One element as `DOM.describeNode` writes it: a tag name and the attributes
/// as the runtime sends them, one flat list of name/value pairs.
fn element(tag: &str, attributes: &[(&str, &str)]) -> Value {
    let flat: Vec<Value> = attributes
        .iter()
        .flat_map(|(name, value)| [json!(name), json!(value)])
        .collect();
    json!({ "nodeName": tag, "attributes": flat })
}

/// What `DOM.describeNode` answers for the two fields of
/// [`ax_sign_in_with_password`]: the email field is a text input, and the
/// password field is the one that says `type=password`. That is the fact the
/// accessible tree does not carry, so a fake page has to answer it for the
/// tree's values to be masked at all.
pub fn sign_in_markup(described: &Value) -> Value {
    let node = match described["backendNodeId"].as_u64() {
        Some(14) => element("INPUT", &[("type", "password"), ("name", "pass")]),
        _ => element("INPUT", &[("type", "text"), ("name", "email")]),
    };
    json!({ "node": node })
}

/// A sign-in form whose password field carries `password_value`, in the shape
/// a real fill leaves it in: the field is a `textbox` named by its label, the
/// properties a Chromium text node carries, and no `protected` — the captured
/// node has none, so the shape of the value is all that says "secret".
pub fn ax_sign_in_with_password(password_value: &str) -> Value {
    let password = ax_with_property(
        ax_with(
            ax_node("4", 14, "textbox", "Password", &[]),
            "value",
            ax_text(password_value),
        ),
        ax_property("multiline", "boolean", json!(false)),
    );
    json!({
        "nodes": [
            ax_node("1", 10, "RootWebArea", "Test page", &["2", "3"]),
            ax_node("2", 19, "form", "Sign in", &["4", "5", "6"]),
            password,
            ax_node("5", 15, "button", "Sign in", &[]),
            ax_node("6", 16, "paragraph", "Forgot your password?", &[]),
        ]
    })
}

/// The same field with no accessible name and no `placeholder` of its own,
/// which is what a login form on a site that never labelled its fields looks
/// like. A field like this is the one a `find` has to describe, and describing
/// it falls back to the value — so it is the one that would print a password.
pub fn ax_unnamed_password_field(password_value: &str) -> Value {
    json!({
        "nodes": [
            ax_node("1", 10, "RootWebArea", "Test page", &["2"]),
            ax_with(
                ax_node("2", 14, "textbox", "", &[]),
                "value",
                ax_text(password_value),
            ),
        ]
    })
}

/// A tree of `role "name"` children under one root, each `backendDOMNodeId`
/// numbered from 1 in the order it is given. The root's own id is out of the
/// way, so a child's number is its position and nothing else.
pub fn flat_tree(nodes: &[(&str, &str)]) -> AxTree {
    let ids: Vec<String> = (1..=nodes.len()).map(|at| at.to_string()).collect();
    let children: Vec<&str> = ids.iter().map(String::as_str).collect();
    let mut all = vec![ax_node("0", 900, "RootWebArea", "", &children)];
    all.extend(
        nodes
            .iter()
            .enumerate()
            .map(|(at, (role, name))| ax_node(&ids[at], at as u64 + 1, role, name, &[])),
    );
    serde_json::from_value(json!({ "nodes": all })).expect("the fixture parses as a tree")
}

/// One checkbox, in the state it is in.
pub fn checkbox_tree(checked: &str) -> AxTree {
    serde_json::from_value(json!({ "nodes": [
        ax_node("0", 900, "RootWebArea", "", &["1"]),
        ax_with_property(
            ax_node("1", 1, "checkbox", "Remember", &[]),
            ax_property("checked", "tristate", json!(checked)),
        )
    ]}))
    .expect("the fixture parses as a tree")
}

/// `count` buttons with one name, which is what a page of "Save" looks like.
pub fn buttons(count: u64, name: &str) -> AxTree {
    let nodes: Vec<(&str, &str)> = (0..count).map(|_| ("button", name)).collect();
    flat_tree(&nodes)
}

/// What `DOM.getBoxModel` answers for a node, in the shape this WebView2
/// really answers in: a flat array of eight numbers per box. The numbers are
/// the spike's own recorded answer for a page's root element
/// (`scout/browser-tabs/SPIKE-REPORT-cdp.md`), kept rather than invented
/// because a click is measured against exactly this.
pub fn box_model() -> Value {
    json!({ "model": {
        "border":  [0.0, 0.0, 817.6000366210938, 0.0, 817.6000366210938, 716.0, 0.0, 716.0],
        "content": [30.0, 15.0, 92.0, 15.0, 92.0, 38.0, 30.0, 38.0],
        "height": 23.0,
        "margin":  [0.0, 0.0, 817.6000366210938, 0.0, 817.6000366210938, 716.0, 0.0, 716.0],
        "padding": [30.0, 15.0, 92.0, 15.0, 92.0, 38.0, 30.0, 38.0],
        "width": 62.0
    }, "backendNodeId": 15 })
}

/// What `Runtime.callFunctionOn` answers when a page-side function returns.
pub fn function_answer(value: Value) -> Value {
    json!({ "result": { "type": "string", "value": value } })
}
