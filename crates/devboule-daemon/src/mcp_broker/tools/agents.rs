//! Agent roster, activity, stop/close and profile-move tools.

use serde_json::{json, Value};

use devboule_protocol::SessionEvent;

pub(in crate::mcp_broker) fn agent_value(
    session: &devboule_protocol::Session,
    runtime: &crate::session::SessionRuntime,
    depth: u32,
) -> Value {
    let manifest = runtime.session_manifest();
    let manifest_provider = manifest.as_ref().and_then(|event| match event {
        SessionEvent::SessionManifest { provider_id, .. } => provider_id.clone(),
        _ => None,
    });
    let model = manifest.as_ref().and_then(|event| match event {
        SessionEvent::SessionManifest {
            current_model_id, ..
        } => current_model_id.clone(),
        _ => None,
    });
    // `name` is the display name a created agent was given; a session a person
    // started has none, and the row still needs a name a caller can address, so
    // it falls back to the same title the app renders (`S5` §1). `state` is the
    // A2A word, with a pending card outranking "working": a child parked on a
    // human's answer is the one fact a creator most needs to see.
    let name = session
        .display_name
        .clone()
        .unwrap_or_else(|| session.title.clone());
    let state = crate::session::roster_task_state(session, runtime);
    // S2: every roster entry carries the S1 word, read off the runtime the
    // broker stored at spawn and flipped on verification. S9 lists all agent
    // kinds; the card and result carry the promise for children too young to
    // have verified.
    json!({
        "id": session.id,
        "provider": session.provider.clone().or(manifest_provider),
        "model": model,
        "state": state,
        "name": name,
        "title": session.title,
        "createdBy": session.created_by,
        "depth": depth,
        "tools": runtime.tools_state().as_str(),
    })
}

/// One validated `devboule_agent_activity` call: the child to read plus the
/// bounded recent-lines limit. The known-parameter check is read out of the
/// published schema, so the document and the check cannot disagree.
pub(in crate::mcp_broker) fn parse_activity_arguments(
    arguments: &Value,
) -> Result<(String, usize), String> {
    let empty = json!({});
    let arguments = match arguments {
        Value::Null => &empty,
        Value::Object(_) => arguments,
        _ => return Err("arguments must be an object".to_string()),
    };
    let object = arguments
        .as_object()
        .ok_or_else(|| "arguments must be an object".to_string())?;
    let known: Vec<String> = crate::provider_catalog::agent_activity_input_schema()["properties"]
        .as_object()
        .map(|properties| properties.keys().cloned().collect())
        .unwrap_or_default();
    for key in object.keys() {
        if !known.iter().any(|known| known == key) {
            return Err(format!("unknown parameter '{key}'"));
        }
    }
    let session = object
        .get("session")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "session is required".to_string())?;
    let limit = match object.get("limit") {
        None | Some(Value::Null) => crate::agent_activity::clamp_limit(None),
        Some(Value::Number(n)) => {
            let n = n
                .as_u64()
                .ok_or_else(|| "limit must be an integer 0..50".to_string())?;
            crate::agent_activity::clamp_limit(Some(n))
        }
        Some(_) => return Err("limit must be an integer 0..50".to_string()),
    };
    Ok((session.to_string(), limit))
}
