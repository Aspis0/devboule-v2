//! The accessible tree as the runtime reports it: `getFullAXTree` fetched over
//! CDP, parsed, and nothing else. The compaction that turns it into lines an
//! agent reads is in `view.rs`; the tree itself never leaves this process.

use serde::Deserialize;
use serde_json::{json, Value};

use super::cdp::{CdpError, Page};

/// The `{type, value, relatedNodes}` shape every AX field arrives in.
#[derive(Debug, Clone, Deserialize, Default)]
pub struct AxValue {
    #[serde(default)]
    pub value: Option<Value>,
}

impl AxValue {
    /// The field as text. A number, a boolean and a string all arrive in the
    /// same slot (`"true"`, `"mixed"`, `"2"`), and a view that printed them as
    /// JSON would be useless to read.
    pub fn text(&self) -> String {
        match &self.value {
            None | Some(Value::Null) => String::new(),
            Some(Value::String(text)) => text.clone(),
            Some(other) => other.to_string(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct AxProperty {
    pub name: String,
    #[serde(default)]
    pub value: Option<AxValue>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct AxNode {
    /// Spelled as the protocol spells it, because a child list names its
    /// parent this way and nothing else links the tree together.
    #[serde(rename = "nodeId", default)]
    pub node_id: String,
    /// Spelled as the protocol spells it, because this is the protocol's own
    /// shape: the ref an agent holds is this number.
    #[serde(rename = "backendDOMNodeId", default)]
    pub backend_dom_node_id: Option<u64>,
    #[serde(default)]
    pub ignored: bool,
    #[serde(default)]
    pub role: Option<AxValue>,
    #[serde(default)]
    pub name: Option<AxValue>,
    #[serde(default)]
    pub description: Option<AxValue>,
    #[serde(default)]
    pub value: Option<AxValue>,
    #[serde(default)]
    pub properties: Vec<AxProperty>,
    #[serde(rename = "childIds", default)]
    pub child_ids: Vec<String>,
}

impl AxNode {
    pub fn role(&self) -> String {
        self.role.as_ref().map(AxValue::text).unwrap_or_default()
    }

    pub fn name(&self) -> String {
        self.name.as_ref().map(AxValue::text).unwrap_or_default()
    }

    pub fn text_of(&self, field: &str) -> String {
        match field {
            "description" => self.description.as_ref().map(AxValue::text),
            "value" => self.value.as_ref().map(AxValue::text),
            _ => None,
        }
        .unwrap_or_default()
    }

    /// A `<placeholder>` reaches the AX tree as text in the node's own value
    /// only when nothing else names it, and as a property when the runtime
    /// computed one. Either is worth reading; neither is guaranteed.
    pub fn placeholder(&self) -> String {
        let property = self.property("placeholder");
        if !property.is_empty() {
            return property;
        }
        match self.role().as_str() {
            "textbox" | "searchbox" => self.value.as_ref().map(AxValue::text).unwrap_or_default(),
            _ => String::new(),
        }
    }

    /// One property's text, or empty. `checked` arrives as `"true"`, `"false"`
    /// or `"mixed"` and `level` as a number, so all of them are read as text.
    pub fn property(&self, name: &str) -> String {
        self.properties
            .iter()
            .find(|property| property.name == name)
            .and_then(|property| property.value.as_ref())
            .map(AxValue::text)
            .unwrap_or_default()
    }
}

/// A tree as the runtime sent it.
#[derive(Debug, Clone, Deserialize, Default)]
pub struct AxTree {
    #[serde(default)]
    pub nodes: Vec<AxNode>,
}

/// Fetch one page's whole accessible tree.
///
/// One call, no parameters: this is the runtime's own computed roles and
/// names, including the cases a reimplementation drifts on.
pub async fn tree(page: &dyn Page) -> Result<AxTree, CdpError> {
    let answered = page.call("Accessibility.getFullAXTree", json!({})).await?;
    serde_json::from_value(answered)
        .map_err(|error| CdpError::Refused(format!("the page's accessibility tree: {error}")))
}

#[cfg(test)]
#[path = "ax_tests.rs"]
mod tests;
