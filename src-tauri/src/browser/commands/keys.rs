//! Keys, as the runtime wants them: the name an agent writes (`Enter`,
//! `Control+A`) turned into the key, code, key code and characters of one key
//! press, and the two events that press it.

use serde_json::json;

use devboule_protocol::BrowserError;

use super::super::cdp::Page;
use super::act::call;
use super::host_error;

/// One named key, as the runtime needs it spelled. `text` is empty for
/// everything a key press does not type, which is how the runtime tells a
/// keydown from a character.
struct Key {
    key: &'static str,
    code: &'static str,
    virtual_key: u32,
    text: &'static str,
}

const KEYS: [Key; 13] = [
    Key {
        key: "Enter",
        code: "Enter",
        virtual_key: 13,
        text: "\r",
    },
    Key {
        key: "Tab",
        code: "Tab",
        virtual_key: 9,
        text: "",
    },
    Key {
        key: "Escape",
        code: "Escape",
        virtual_key: 27,
        text: "",
    },
    Key {
        key: "Backspace",
        code: "Backspace",
        virtual_key: 8,
        text: "",
    },
    Key {
        key: "Delete",
        code: "Delete",
        virtual_key: 46,
        text: "",
    },
    Key {
        key: "ArrowUp",
        code: "ArrowUp",
        virtual_key: 38,
        text: "",
    },
    Key {
        key: "ArrowDown",
        code: "ArrowDown",
        virtual_key: 40,
        text: "",
    },
    Key {
        key: "ArrowLeft",
        code: "ArrowLeft",
        virtual_key: 37,
        text: "",
    },
    Key {
        key: "ArrowRight",
        code: "ArrowRight",
        virtual_key: 39,
        text: "",
    },
    Key {
        key: "Home",
        code: "Home",
        virtual_key: 36,
        text: "",
    },
    Key {
        key: "End",
        code: "End",
        virtual_key: 35,
        text: "",
    },
    Key {
        key: "PageUp",
        code: "PageUp",
        virtual_key: 33,
        text: "",
    },
    Key {
        key: "PageDown",
        code: "PageDown",
        virtual_key: 34,
        text: "",
    },
];

/// What one key press is: its name, its code, its key code and the characters
/// it types. A chord is the modifiers plus the same key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chord {
    modifiers: u32,
    key: String,
    code: String,
    virtual_key: u32,
    text: String,
}

/// Parse `Enter`, `ArrowDown`, `a` or `Control+Shift+S`. An unknown name is a
/// refusal rather than a silent no-op: a key that did nothing is the one
/// failure an agent cannot see.
pub fn chord(written: &str) -> Result<Chord, BrowserError> {
    let parts: Vec<&str> = written.split('+').collect();
    let (modifiers, key) = parts.split_at(parts.len().saturating_sub(1));
    let name = key[0];
    let mut mask = 0;
    for modifier in modifiers {
        let written = modifier.trim();
        mask |= match written.to_lowercase().as_str() {
            "alt" => 1,
            "control" | "ctrl" => 2,
            "meta" | "cmd" | "command" | "super" => 4,
            "shift" => 8,
            _ => return Err(host_error(format!("{written} is not a modifier."))),
        };
    }
    if let Some(named) = KEYS
        .iter()
        .find(|named| named.key.eq_ignore_ascii_case(name))
    {
        return Ok(Chord {
            modifiers: mask,
            key: named.key.to_owned(),
            code: named.code.to_owned(),
            virtual_key: named.virtual_key,
            text: named.text.to_owned(),
        });
    }
    let mut characters = name.chars();
    let (Some(one), None) = (characters.next(), characters.next()) else {
        return Err(host_error(format!(
            "{written} is not a key this app can press."
        )));
    };
    Ok(Chord {
        modifiers: mask,
        key: one.to_string(),
        code: String::new(),
        virtual_key: one.to_ascii_uppercase() as u32,
        text: if mask & 2 == 0 {
            one.to_string()
        } else {
            String::new()
        },
    })
}

/// Press a key. `rawKeyDown` then `keyUp` for anything that types nothing,
/// and a `keyDown` carrying the characters for anything that does.
pub async fn press_key(page: &dyn Page, written: &str) -> Result<(), BrowserError> {
    let chord = chord(written)?;
    let kind = if chord.text.is_empty() {
        "rawKeyDown"
    } else {
        "keyDown"
    };
    let mut event = json!({
        "type": kind,
        "key": chord.key,
        "code": chord.code,
        "windowsVirtualKeyCode": chord.virtual_key,
        "nativeVirtualKeyCode": chord.virtual_key,
        "modifiers": chord.modifiers,
    });
    if !chord.text.is_empty() {
        event["text"] = json!(chord.text);
    }
    call(page, "Input.dispatchKeyEvent", event).await?;
    call(
        page,
        "Input.dispatchKeyEvent",
        json!({
            "type": "keyUp",
            "key": chord.key,
            "code": chord.code,
            "windowsVirtualKeyCode": chord.virtual_key,
            "nativeVirtualKeyCode": chord.virtual_key,
            "modifiers": chord.modifiers,
        }),
    )
    .await?;
    Ok(())
}

#[cfg(test)]
#[path = "keys_tests.rs"]
mod tests;
