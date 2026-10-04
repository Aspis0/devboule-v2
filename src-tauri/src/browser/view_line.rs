//! One node as a line of a view: the indent, the marker, the role, the name, the
//! states and the ref. Written once, so a delta's lines are the view's lines.

use super::ax::{AxNode, AxValue};

/// The longest value a state carries.
pub(super) const VALUE_MAX: usize = 80;

/// The states a line carries, in a fixed order so two reads of one node are
/// byte-identical.
fn states(node: &AxNode) -> Vec<String> {
    let mut states = Vec::new();
    match node.property("checked").as_str() {
        "true" => states.push("checked".to_owned()),
        "false" => states.push("unchecked".to_owned()),
        _ => {}
    }
    match node.property("expanded").as_str() {
        "true" => states.push("expanded".to_owned()),
        "false" => states.push("collapsed".to_owned()),
        _ => {}
    }
    if node.property("selected") == "true" {
        states.push("selected".to_owned());
    }
    if node.property("disabled") == "true" {
        states.push("disabled".to_owned());
    }
    let value = node.value.as_ref().map(AxValue::text).unwrap_or_default();
    if !value.is_empty() {
        states.push(format!("value=\"{}\"", clip(&value, VALUE_MAX)));
    }
    let level = node.property("level");
    if !level.is_empty() {
        states.push(format!("level={level}"));
    }
    states
}

/// One node's line, as the view prints it and as a delta repeats it: the
/// indent, the marker, the role, the name, the states and the ref. Written
/// once, so a delta's lines are the view's lines.
pub(super) fn line_for(node: &AxNode, backend_id: u64, depth: usize) -> String {
    let name = node.name();
    let mut line = format!("{}- {}", "  ".repeat(depth), node.role());
    if !name.is_empty() {
        line.push_str(&format!(" \"{name}\""));
    }
    for state in states(node) {
        line.push_str(&format!(" [{state}]"));
    }
    line.push_str(&format!(" [ref=e{backend_id}]"));
    line
}

/// Shorten to `max` characters, at a boundary that does not split one.
pub(super) fn clip(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_owned();
    }
    let mut out: String = text.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}
