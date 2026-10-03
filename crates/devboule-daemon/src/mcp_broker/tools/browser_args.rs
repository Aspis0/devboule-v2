//! One browser command's arguments: what shape each argument may take, and
//! whether one call matches the set of them.
//!
//! The vocabulary is declared once and read twice — `Kind::schema` is what
//! `tools/list` shows an agent, `Kind::check` is what the daemon refuses
//! against — so a tool cannot offer an argument it then rejects, nor reject one
//! it never offered. The commands that use them are `browser_commands`' rows.
//!
//! The checks here are the shapes an agent can get wrong and a host would only
//! report as its own failure: a ref that is not a ref, a boolean written as a
//! string, a wait longer than the contract allows, two halves of an alternative
//! at once. What a ref *means* and what a click *does* is the host's business.

use std::time::Duration;

use serde_json::{json, Map, Value};

use crate::browser_affinity::MAX_BROWSER_ID_BYTES;

/// One accepted argument's shape.
#[derive(Clone, Copy)]
pub(in crate::mcp_broker) enum Kind {
    /// Free text.
    Text,
    /// An element ref from a snapshot: the contract's `e<backendDOMNodeId>`.
    Ref,
    /// A tab.
    ///
    /// ASCII, so the schema's `maxLength` (characters) and this check's length
    /// (bytes) are one number: a multibyte id the list advertises as within
    /// the bound would be refused here for being longer than it looks.
    Tab,
    /// A whole number, bounded. `None` leaves the upper bound open.
    Integer(i64, Option<i64>),
    /// A boolean.
    Bool,
    /// One word of a closed vocabulary.
    Choices(&'static [&'static str]),
    /// A list drawn from a closed vocabulary.
    Flags(&'static [&'static str]),
}

impl Kind {
    pub(in crate::mcp_broker) fn schema(self) -> Value {
        match self {
            Self::Text => json!({"type": "string"}),
            Self::Ref => json!({"type": "string", "pattern": "^e\\d+$"}),
            Self::Tab => {
                json!({
                    "type": "string",
                    "minLength": 1,
                    "maxLength": MAX_BROWSER_ID_BYTES,
                    "pattern": "^[\\x21-\\x7E]+$"
                })
            }
            Self::Integer(min, max) => {
                let mut schema = Map::from_iter([("type".to_string(), json!("integer"))]);
                schema.insert("minimum".to_string(), json!(min));
                if let Some(max) = max {
                    schema.insert("maximum".to_string(), json!(max));
                }
                Value::Object(schema)
            }
            Self::Bool => json!({"type": "boolean"}),
            Self::Choices(words) => json!({"type": "string", "enum": words}),
            Self::Flags(words) => {
                json!({"type": "array", "items": {"type": "string", "enum": words}})
            }
        }
    }

    fn check(self, tool: &str, name: &str, value: &Value) -> Result<(), String> {
        match self {
            Self::Text => value
                .as_str()
                .map(|_| ())
                .ok_or_else(|| format!("{tool}: '{name}' must be text.")),
            Self::Ref => match value.as_str() {
                Some(text) if is_ref(text) => Ok(()),
                _ => Err(format!(
                    "{tool}: '{name}' must be a ref from a snapshot, like e123."
                )),
            },
            Self::Tab => match value.as_str() {
                Some(text)
                    if !text.is_empty()
                        && text.len() <= MAX_BROWSER_ID_BYTES
                        && text.bytes().all(|byte| byte.is_ascii_graphic()) =>
                {
                    Ok(())
                }
                _ => Err(format!(
                    "{tool}: '{name}' must be a browserId from browser_new_tab or browser_list_tabs."
                )),
            },
            Self::Integer(min, max) => match value.as_i64() {
                Some(number) if number >= min && max.is_none_or(|max| number <= max) => Ok(()),
                _ => {
                    let range = match max {
                        Some(max) => format!("{min} to {max}"),
                        None => format!("{min} or more"),
                    };
                    Err(format!("{tool}: '{name}' must be a whole number from {range}."))
                }
            },
            Self::Bool => value
                .as_bool()
                .map(|_| ())
                .ok_or_else(|| format!("{tool}: '{name}' must be true or false.")),
            Self::Choices(words) => match value.as_str() {
                Some(text) if words.contains(&text) => Ok(()),
                _ => Err(format!("{tool}: '{name}' must be one of {}.", words.join(", "))),
            },
            Self::Flags(words) => match value.as_array() {
                Some(items)
                    if items
                        .iter()
                        .all(|item| item.as_str().is_some_and(|word| words.contains(&word))) =>
                {
                    Ok(())
                }
                _ => Err(format!("{tool}: '{name}' must be a list of {}.", words.join(", "))),
            },
        }
    }
}

/// One argument of one command.
pub(in crate::mcp_broker) struct Field {
    pub(in crate::mcp_broker) name: &'static str,
    pub(in crate::mcp_broker) kind: Kind,
    pub(in crate::mcp_broker) required: bool,
    /// What an agent cannot read off the type alone.
    pub(in crate::mcp_broker) hint: &'static str,
}

pub(in crate::mcp_broker) const fn required(name: &'static str, kind: Kind) -> Field {
    Field {
        name,
        kind,
        required: true,
        hint: "",
    }
}

pub(in crate::mcp_broker) const fn optional(name: &'static str, kind: Kind) -> Field {
    Field {
        name,
        kind,
        required: false,
        hint: "",
    }
}

impl Field {
    pub(in crate::mcp_broker) const fn described(mut self, hint: &'static str) -> Field {
        self.hint = hint;
        self
    }

    pub(in crate::mcp_broker) fn schema(&self) -> Value {
        let mut schema = self.kind.schema();
        if !self.hint.is_empty() {
            schema["description"] = json!(self.hint);
        }
        schema
    }
}

/// One command's arguments: the closed set, and which one of its alternatives it
/// wants.
///
/// The alternatives are the contract's `a | b` arguments (`url` or `action`,
/// `value` or `label`, one of the three waits). Exactly one group must be
/// complete, which also refuses two at once: a navigate carrying both a `url`
/// and an `action` has no single meaning to run.
pub(in crate::mcp_broker) struct Spec {
    pub(in crate::mcp_broker) command: &'static str,
    pub(in crate::mcp_broker) fields: &'static [Field],
    pub(in crate::mcp_broker) alternatives: &'static [&'static [&'static str]],
    /// Whether the command acts on a named tab, which the broker routes on.
    pub(in crate::mcp_broker) tab: bool,
}

/// A call the daemon checked and can hand to the host.
pub(in crate::mcp_broker) struct Call {
    /// The arguments as the host receives them, camelCase, unchanged.
    pub(in crate::mcp_broker) args: Value,
    /// The tab this call is about, for the broker's routing.
    pub(in crate::mcp_broker) browser_id: Option<String>,
    /// The agent's own deadline, when it named one.
    pub(in crate::mcp_broker) timeout: Option<Duration>,
}

/// Check one call against its command's set of arguments.
///
/// The closed set is the point of this pass: an unknown key is refused by name
/// rather than travelling to a host that would have to guess what it meant, and
/// a value of the wrong shape is refused here rather than reaching an agent as a
/// host failure that reads like the page's fault.
pub(in crate::mcp_broker) fn parse(
    spec: &Spec,
    tool: &str,
    arguments: &Value,
) -> Result<Call, String> {
    // A tool with no arguments sends nothing, and `null` is how that arrives.
    let map = match arguments {
        Value::Object(map) => map,
        Value::Null if spec.fields.is_empty() => return Ok(routing(spec, Map::new())),
        _ => return Err(format!("{tool}: the arguments must be a JSON object.")),
    };
    for (name, value) in map {
        let field = spec
            .fields
            .iter()
            .find(|field| field.name == name)
            .ok_or_else(|| {
                let offered = spec
                    .fields
                    .iter()
                    .map(|field| field.name)
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("{tool}: '{name}' is not an argument of this tool. It accepts: {offered}.")
            })?;
        field.kind.check(tool, name, value)?;
    }
    for field in spec.fields.iter().filter(|field| field.required) {
        if !map.contains_key(field.name) {
            return Err(format!(
                "{tool}: '{field_name}' is required.",
                field_name = field.name
            ));
        }
    }
    if !spec.alternatives.is_empty() {
        let complete = spec
            .alternatives
            .iter()
            .filter(|group| group.iter().all(|name| map.contains_key(*name)))
            .count();
        if complete != 1 {
            let wanted = spec
                .alternatives
                .iter()
                .map(|group| group.join(" and "))
                .collect::<Vec<_>>()
                .join(", or ");
            return Err(format!("{tool}: give exactly one of {wanted}."));
        }
    }
    Ok(routing(spec, map.clone()))
}

/// The routing facts the checked argument object already holds.
fn routing(spec: &Spec, map: Map<String, Value>) -> Call {
    Call {
        args: Value::Object(map.clone()),
        browser_id: map
            .get("browserId")
            .and_then(Value::as_str)
            .map(str::to_string)
            .filter(|_| spec.tab),
        timeout: map
            .get("timeoutMs")
            .and_then(Value::as_u64)
            .map(Duration::from_millis),
    }
}

/// A ref is the contract's `e<backendDOMNodeId>`: the letter, then digits, and
/// nothing else, because a ref of any other shape names no node.
fn is_ref(text: &str) -> bool {
    match text.strip_prefix('e') {
        Some(digits) => !digits.is_empty() && digits.bytes().all(|byte| byte.is_ascii_digit()),
        None => false,
    }
}
