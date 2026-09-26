//! Parsing one `devboule_create_agent` call into its validated request.

use serde_json::{json, Value};

use super::labels::parse_labels;

/// One validated `devboule_create_agent` call (`S5` §2, `create-from-profile`).
///
/// The parameters arrive as an MCP `arguments` object; the schema the broker
/// publishes is closed, and this is the enforcement half of it. Every sentence
/// this produces is one of §2's, and the check that a parameter is *known* is
/// read out of the published schema rather than repeated here, so the document
/// an agent sees and the check it hits cannot disagree.
///
/// `profile` is a **name**, and it is the only way this request says what to
/// run: the provider, the model, the mode, the thinking option, the features
/// and the tool overlay all come from the stored profile that name resolves to
/// at the moment of the call. There is no field here that could disagree with
/// it.
#[derive(Debug)]
pub(in crate::mcp_broker) struct AgentCreateRequest {
    pub(in crate::mcp_broker) profile: String,
    pub(in crate::mcp_broker) title: String,
    /// The caller's own labels. The daemon's `devboule.` keys are stamped into
    /// the same map at the creation and are refused here.
    pub(in crate::mcp_broker) labels: std::collections::BTreeMap<String, String>,
    pub(in crate::mcp_broker) workspace_id: Option<String>,
    pub(in crate::mcp_broker) cwd: Option<String>,
    pub(in crate::mcp_broker) initial_prompt: String,
    pub(in crate::mcp_broker) notify: bool,
}

impl AgentCreateRequest {
    /// The fields one creation's identity is made of (`S5-08`).
    ///
    /// The list lives in a function rather than inline in the handler, so the
    /// test drives this very list instead of a copy of it.
    pub(in crate::mcp_broker) fn creation_fingerprint_fields<'a>(
        creator_id: &'a str,
        request: &'a Self,
        notify_field: &'a str,
        labels: &'a str,
    ) -> Vec<&'a str> {
        vec![
            creator_id,
            &request.title,
            &request.profile,
            &request.initial_prompt,
            request.workspace_id.as_deref().unwrap_or(""),
            request.cwd.as_deref().unwrap_or(""),
            notify_field,
            labels,
        ]
    }

    /// The labels as one fingerprint field: their JSON encoding, which is
    /// deterministic for a `BTreeMap`, so two different label sets cannot spell
    /// the same string and the same set always spells the same one.
    pub(in crate::mcp_broker) fn labels_fingerprint(&self) -> String {
        serde_json::to_string(&self.labels).unwrap_or_default()
    }

    pub(in crate::mcp_broker) fn parse(arguments: &Value) -> Result<Self, String> {
        let empty = json!({});
        let arguments = match arguments {
            Value::Null => &empty,
            Value::Object(_) => arguments,
            _ => return Err("arguments must be an object".to_string()),
        };
        let object = arguments
            .as_object()
            .ok_or_else(|| "arguments must be an object".to_string())?;
        let known: Vec<String> = crate::provider_catalog::agent_create_input_schema()["properties"]
            .as_object()
            .map(|properties| properties.keys().cloned().collect())
            .unwrap_or_default();
        for key in object.keys() {
            if !known.iter().any(|known| known == key) {
                // `provider`, `preset` and `mode` land here on purpose: this
                // tool has none of them, because a profile chooses what to run
                // (`create-from-profile`).
                return Err(format!("unknown parameter '{key}'"));
            }
        }
        let text = |key: &str| object.get(key).and_then(Value::as_str);
        let required = |key: &str| {
            text(key)
                .filter(|value| !value.is_empty())
                .map(str::to_string)
                .ok_or_else(|| format!("{key} is required"))
        };
        let title = devboule_protocol::validate_display_name(&required("title")?)?;
        // The profile name is compared against the store's own names, which
        // `agent_profiles.rs` trims on the way in, so a caller that padded its
        // name is naming the same profile rather than a profile that cannot
        // exist. It is deliberately **not** bounded here: a name that matches
        // nothing is refused with the store's sentence, and a name that is
        // longer than the store admits can never match one.
        let profile = required("profile")?;
        let initial_prompt = required("initialPrompt")?;
        // A prompt of spaces is a prompt nobody can act on, and the same rule
        // the display name gets: whitespace is not content.
        let initial_prompt = initial_prompt.trim().to_string();
        if initial_prompt.is_empty() {
            return Err("initialPrompt is required".to_string());
        }
        if initial_prompt.len() > MAX_AGENT_PROMPT_BYTES {
            return Err(format!(
                "initialPrompt is {} bytes; the limit is {MAX_AGENT_PROMPT_BYTES}.",
                initial_prompt.len()
            ));
        }
        let notify = match object.get("notifyOnFinish") {
            None => true,
            Some(Value::Bool(value)) => *value,
            Some(_) => return Err("notifyOnFinish must be a boolean".to_string()),
        };
        Ok(Self {
            profile,
            title,
            labels: parse_labels(object)?,
            workspace_id: optional_text("workspaceId", object)?,
            cwd: optional_text("cwd", object)?,
            initial_prompt,
            notify,
        })
    }
}

/// One optional string parameter, with its type enforced (audit S5-09).
///
/// A `workspaceId` of `null` is absent; a number, a list or an object is an
/// invalid-params error rather than a silently ignored parameter — the caller
/// asked for something and must be told it was not understood, especially when
/// what it asked for was *where* the child would run.
fn optional_text(
    key: &str,
    object: &serde_json::Map<String, Value>,
) -> Result<Option<String>, String> {
    match object.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) => Ok(Some(value.clone())),
        Some(_) => Err(format!("{key} must be a string")),
    }
}

/// Everything one creation's answer depends on, in one string (audit S5-08).
///
/// Every field is length-prefixed, so two different payloads cannot spell the
/// same fingerprint by moving a delimiter: today's fields include the requested
/// workspace, the working directory and whether the creator wants to be told,
/// because a retry that changes any of them is not the same creation.
pub(in crate::mcp_broker) fn creation_fingerprint(fields: &[&str]) -> String {
    let mut fingerprint = String::from("agent-create");
    for field in fields {
        fingerprint.push(':');
        fingerprint.push_str(&field.len().to_string());
        fingerprint.push(':');
        fingerprint.push_str(field);
    }
    fingerprint
}

/// The largest `initialPrompt` one creation may carry (32 KiB).
///
/// The same number as the artifact cap, for the same reason: this text becomes
/// a child's first turn, and a prompt nobody could read is not a prompt.
const MAX_AGENT_PROMPT_BYTES: usize = 32 * 1024;
