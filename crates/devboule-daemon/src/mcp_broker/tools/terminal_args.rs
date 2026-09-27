//! The closed argument sets of the three terminal writes: what each
//! `tools/call` accepts, and nothing else.

use serde_json::Value;

use devboule_protocol::validate_display_name;

/// The one argument `devboule_create_terminal` takes, trimmed and judged
/// here: absent and null and empty-after-trimming are Paseo's absent name, a
/// name the wire refuses keeps the wire's sentence, and a name that carries a
/// control, invisible or line-break character is refused outright — it is
/// stored as the terminal's title and printed back on the roster, where one
/// escape sequence or one forged line is a rendering of something the daemon
/// never judged.
pub(super) fn parse_name(arguments: &Value) -> Result<Option<String>, String> {
    let object = argument_map(arguments)?;
    for key in object.keys() {
        if key != "name" {
            return Err(format!("unknown parameter '{key}'"));
        }
    }
    match object.get("name") {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) => {
            let trimmed = value.trim();
            if trimmed.is_empty() {
                return Ok(None);
            }
            let name = validate_display_name(trimmed)?;
            // The tool-visible sentence is the tool's own contract: the
            // protocol validator above enforces the same rule, but a model
            // reading this tool must see this tool's words.
            match devboule_protocol::unsafe_character(&name) {
                Some(category) => Err(format!("name must be plain text: it contains {category}")),
                None => Ok(Some(name)),
            }
        }
        Some(_) => Err("name must be a string".to_string()),
    }
}

/// What one `send_terminal_keys` call carries: Paseo's three fields, closed.
pub(super) fn parse_keys(arguments: &Value) -> Result<KeysRequest, String> {
    let object = argument_map(arguments)?;
    for key in object.keys() {
        if !matches!(key.as_str(), "terminalId" | "keys" | "literal") {
            return Err(format!("unknown parameter '{key}'"));
        }
    }
    let terminal = object
        .get("terminalId")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "terminalId is required".to_string())?
        .to_string();
    // An empty payload types nothing while still spending the consent card
    // for it, so it is refused here — with the call, before anything asks.
    let keys = match object.get("keys") {
        None | Some(Value::Null) => return Err("keys is required".to_string()),
        Some(Value::String(value)) if value.is_empty() => {
            return Err("keys must not be empty".to_string());
        }
        Some(Value::String(value)) => value.clone(),
        Some(_) => return Err("keys must be a string".to_string()),
    };
    let literal = match object.get("literal") {
        None | Some(Value::Null) => false,
        Some(Value::Bool(value)) => *value,
        Some(_) => return Err("literal must be a boolean".to_string()),
    };
    Ok(KeysRequest {
        terminal,
        keys,
        literal,
    })
}

/// The closed kill shape: one terminal, named by the id the roster answered.
pub(super) fn parse_terminal(arguments: &Value) -> Result<String, String> {
    let object = argument_map(arguments)?;
    for key in object.keys() {
        if key != "terminalId" {
            return Err(format!("unknown parameter '{key}'"));
        }
    }
    object
        .get("terminalId")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .ok_or_else(|| "terminalId is required".to_string())
}

/// The argument object of a `tools/call`: an absent `arguments` is the empty
/// document, anything that is not an object is a malformed request.
fn argument_map(arguments: &Value) -> Result<serde_json::Map<String, Value>, String> {
    match arguments {
        Value::Null => Ok(serde_json::Map::new()),
        Value::Object(object) => Ok(object.clone()),
        _ => Err("arguments must be an object".to_string()),
    }
}

pub(super) struct KeysRequest {
    pub(super) terminal: String,
    pub(super) keys: String,
    pub(super) literal: bool,
}
