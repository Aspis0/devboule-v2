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
pub struct FakePage {
    answers: HashMap<String, Value>,
    failures: HashMap<String, CdpError>,
    calls: Mutex<Vec<(String, Value)>>,
}

impl FakePage {
    /// A page that answers every call with `null` unless told otherwise.
    pub fn new() -> Self {
        FakePage {
            answers: HashMap::new(),
            failures: HashMap::new(),
            calls: Mutex::new(Vec::new()),
        }
    }

    pub fn answering(mut self, method: &str, value: Value) -> Self {
        self.answers.insert(method.to_owned(), value);
        self
    }

    /// A call that refuses, as a stale ref or a bad method would.
    pub fn refusing(mut self, method: &str, error: CdpError) -> Self {
        self.failures.insert(method.to_owned(), error);
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
            .push((method.to_owned(), params));
        let answer = self.answers.get(method).cloned();
        let failure = self.failures.get(method).cloned();
        Box::pin(async move {
            if let Some(error) = failure {
                return Err(error);
            }
            Ok(answer.unwrap_or(Value::Null))
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
                parked: true,
                cancelled: false,
                workspace: "ws-1".to_owned(),
                state: Arc::new(Mutex::new(BrowserViewState {
                    url: "https://example.test/sign-in".to_owned(),
                    title: Some("Sign in".to_owned()),
                    ..BrowserViewState::default()
                })),
                sink: Arc::new(Mutex::new(tauri::ipc::Channel::new(|_| Ok(())))),
                presented: None,
            },
        )
        .expect("the fixture tab claims");
    registry
}

/// The tree `Accessibility.getFullAXTree` would answer with for a sign-in
/// form: a heading, a form, a field with a value, a checkbox, two buttons and
/// a paragraph, plus the `generic` and `ignored` nodes a real tree is full of.
pub fn ax_fixture() -> Value {
    json!({
        "nodes": [
            {
                "nodeId": "1", "backendDOMNodeId": 10,
                "role": { "value": "RootWebArea" }, "name": { "value": "Test page" },
                "childIds": ["2", "3"]
            },
            {
                "nodeId": "2", "backendDOMNodeId": 11,
                "role": { "value": "heading" }, "name": { "value": "Sign in" },
                "properties": [{ "name": "level", "value": { "value": 2 } }],
                "childIds": []
            },
            {
                "nodeId": "3", "backendDOMNodeId": 19,
                "role": { "value": "form" }, "name": { "value": "Sign in" },
                "childIds": ["4", "5", "6", "7", "8", "9"]
            },
            {
                "nodeId": "4", "backendDOMNodeId": 13,
                "role": { "value": "textbox" }, "name": { "value": "Email" },
                "value": { "value": "person@example.test" },
                "properties": [{ "name": "focused", "value": { "value": "true" } }],
                "childIds": []
            },
            {
                "nodeId": "5", "backendDOMNodeId": 20,
                "role": { "value": "StaticText" },
                "name": { "value": "Keep me signed in" },
                "childIds": []
            },
            {
                "nodeId": "6", "backendDOMNodeId": 14,
                "role": { "value": "checkbox" }, "name": { "value": "Remember me" },
                "properties": [{ "name": "checked", "value": { "value": "false" } }],
                "childIds": []
            },
            {
                "nodeId": "7", "backendDOMNodeId": 15,
                "role": { "value": "button" }, "name": { "value": "Sign in" },
                "properties": [{ "name": "disabled", "value": { "value": "false" } }],
                "childIds": []
            },
            {
                "nodeId": "8", "backendDOMNodeId": 16,
                "role": { "value": "paragraph" }, "name": { "value": "Forgot your password?" },
                "childIds": []
            },
            {
                "nodeId": "9", "backendDOMNodeId": 17,
                "role": { "value": "button" }, "name": { "value": "" },
                "childIds": []
            },
            {
                "nodeId": "10", "backendDOMNodeId": 18, "ignored": true,
                "role": { "value": "generic" }, "name": { "value": "" },
                "childIds": []
            }
        ]
    })
}

/// A tree of `role "name"` children under one root, each `backendDOMNodeId`
/// numbered from 1 in the order it is given. The root's own id is out of the
/// way, so a child's number is its position and nothing else.
pub fn flat_tree(nodes: &[(&str, &str)]) -> AxTree {
    let children: Vec<Value> = (1..=nodes.len())
        .map(|at| Value::String(at.to_string()))
        .collect();
    let mut all = vec![json!({
        "nodeId": "0", "backendDOMNodeId": 900,
        "role": { "value": "RootWebArea" }, "childIds": children
    })];
    all.extend(nodes.iter().enumerate().map(|(at, (role, name))| {
        json!({
            "nodeId": (at + 1).to_string(),
            "backendDOMNodeId": at + 1,
            "role": { "value": role },
            "name": { "value": name }
        })
    }));
    serde_json::from_value(json!({ "nodes": all })).expect("the fixture parses as a tree")
}

/// One checkbox, in the state it is in.
pub fn checkbox_tree(checked: &str) -> AxTree {
    serde_json::from_value(json!({ "nodes": [
        { "nodeId": "0", "backendDOMNodeId": 900, "role": { "value": "RootWebArea" },
          "childIds": ["1"] },
        { "nodeId": "1", "backendDOMNodeId": 1, "role": { "value": "checkbox" },
          "name": { "value": "Remember" },
          "properties": [{ "name": "checked", "value": { "value": checked } }] }
    ]}))
    .expect("the fixture parses as a tree")
}

/// `count` buttons with one name, which is what a page of "Save" looks like.
pub fn buttons(count: u64, name: &str) -> AxTree {
    let nodes: Vec<(&str, &str)> = (0..count).map(|_| ("button", name)).collect();
    flat_tree(&nodes)
}

/// What `DOM.getBoxModel` answers for a control in the middle of the form.
pub fn box_model() -> Value {
    json!({ "model": { "content": [[100, 200], [300, 200], [300, 260], [100, 260]] } })
}

/// What `Runtime.callFunctionOn` answers when a page-side function returns.
pub fn function_answer(value: Value) -> Value {
    json!({ "result": { "type": "string", "value": value } })
}
