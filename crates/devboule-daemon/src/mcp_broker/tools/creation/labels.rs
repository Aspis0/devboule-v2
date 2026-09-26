//! Creation label rules: what a caller may write, what the daemon stamps.

use serde_json::Value;

use super::profile::ResolvedProfile;

/// The prefix the daemon reserves for its own label facts.
const RESERVED_LABEL_PREFIX: &str = "devboule.";

/// The most labels one creation may carry.
///
/// Bounded because the map is written into the session row and travels on every
/// roster push: an unbounded map is a payload every attached client pays for on
/// every push, for an annotation nothing decides anything from.
const MAX_AGENT_LABELS: usize = 32;

/// The longest label key and value, in bytes.
const MAX_LABEL_KEY_BYTES: usize = 64;
const MAX_LABEL_VALUE_BYTES: usize = 256;

/// True when a label's text would break out of the line it is written on.
///
/// Label keys and values are interpolated inline in the creation card the
/// human approves (`Labels: key=value, …`), with no fence of their own, so
/// every line terminator the spawn-prompt fence splits on — LF, CR, U+2028,
/// U+2029, U+0085, VT, FF — and every other C0/C1 control character would
/// put a line of the caller's own writing among the daemon's metadata. The
/// prompt gets a marked block; a label gets refused instead.
fn label_carries_a_break(text: &str) -> bool {
    text.chars()
        .any(|character| character.is_control() || matches!(character, '\u{2028}' | '\u{2029}'))
}

/// The caller's labels, checked (`create-from-profile`).
///
/// A free map of string to string, with one reserved prefix: the daemon stamps
/// `devboule.created-by`, `devboule.depth`, `devboule.origin` and
/// `devboule.profile` itself, so a caller that sets or overwrites any of them is
/// refused rather than silently overridden. A caller cannot even arrive at one
/// by accident, because a label nothing can be decided from still has to be
/// honest about who wrote it.
pub(in crate::mcp_broker) fn parse_labels(
    object: &serde_json::Map<String, Value>,
) -> Result<std::collections::BTreeMap<String, String>, String> {
    let Some(value) = object.get("labels") else {
        return Ok(std::collections::BTreeMap::new());
    };
    let Value::Object(entries) = value else {
        return Err("labels must be an object of strings".to_string());
    };
    if entries.len() > MAX_AGENT_LABELS {
        return Err(format!(
            "labels carries {} entries; the limit is {MAX_AGENT_LABELS}.",
            entries.len()
        ));
    }
    let mut labels = std::collections::BTreeMap::new();
    for (key, value) in entries {
        if key.starts_with(RESERVED_LABEL_PREFIX) {
            return Err("reserved label prefix".to_string());
        }
        if key.trim().is_empty() {
            return Err("a label has an empty key".to_string());
        }
        if label_carries_a_break(key) {
            return Err(format!(
                "the label key '{key}' carries a line terminator or control character"
            ));
        }
        if key.len() > MAX_LABEL_KEY_BYTES {
            return Err(format!(
                "the label key '{key}' is {} bytes; the limit is {MAX_LABEL_KEY_BYTES}.",
                key.len()
            ));
        }
        let Value::String(value) = value else {
            return Err(format!("the label '{key}' must be a string"));
        };
        if value.len() > MAX_LABEL_VALUE_BYTES {
            return Err(format!(
                "the label '{key}' is {} bytes; the limit is {MAX_LABEL_VALUE_BYTES}.",
                value.len()
            ));
        }
        if label_carries_a_break(value) {
            return Err(format!(
                "the label '{key}' carries a line terminator or control character"
            ));
        }
        labels.insert(key.clone(), value.clone());
    }
    Ok(labels)
}

/// The labels the daemon stamps into every child it creates.
///
/// The four keys are the daemon's own facts about the child, and they are
/// stamped here, where all four are known, rather than by the session path: the
/// creation is what measured them. `devboule.profile` is the profile's **stable
/// id**, like the session's own field, so the label and the row cannot disagree
/// about which profile made this child, and both survive a rename.
pub(in crate::mcp_broker) fn stamped_labels(
    caller: &std::collections::BTreeMap<String, String>,
    creator_session_id: &str,
    profile: &ResolvedProfile,
    depth: u32,
    origin: &devboule_protocol::SessionOrigin,
) -> std::collections::BTreeMap<String, String> {
    let mut labels = caller.clone();
    labels.insert(
        "devboule.created-by".to_string(),
        creator_session_id.to_string(),
    );
    labels.insert("devboule.depth".to_string(), depth.to_string());
    labels.insert("devboule.origin".to_string(), origin_label(origin));
    labels.insert("devboule.profile".to_string(), profile.id.clone());
    labels
}

/// `devboule.origin`'s value: the same word the wire uses for the kind, plus the
/// device for a peer's child.
///
/// A label is text a human reads, so a peer's device id is spelled into it
/// rather than left to a second lookup the label has no way to make.
fn origin_label(origin: &devboule_protocol::SessionOrigin) -> String {
    use devboule_protocol::SessionOriginKind;
    match origin.kind {
        SessionOriginKind::Peer => match origin.device_id.as_deref() {
            Some(device) => format!("peer:{device}"),
            None => "peer".to_string(),
        },
        SessionOriginKind::Local => "local".to_string(),
        SessionOriginKind::Unknown => "unknown".to_string(),
    }
}
