//! The machinery every action runs through: read the page, put a parked one
//! on screen, act, wait for it to settle, answer with what changed.
//!
//! The order is the whole point and it is not negotiable. A parked page is
//! overridden BEFORE anything measures it, because the override re-lays out a
//! responsive document and any box model taken before it describes a page that
//! no longer exists. The view is read before the action so the delta has
//! something to compare against, and after the settle so it describes the page
//! as it settled rather than mid-flight.

use serde_json::{json, Value};

use devboule_protocol::BrowserError;

use super::super::cdp::{self, Page};
use super::super::deadline::Deadline;
use super::super::delta::{self, Place};
use super::super::registry::TabInfo;
use super::super::view::{self, Mode, View};
use super::{cdp_failure, host_error};

/// One notch of a wheel, in pixels.
pub const NOTCH: f64 = 120.0;

/// The page as it was when a command started.
pub struct Start {
    pub place: Place,
    pub view: View,
}

/// Put a parked page on screen, so the box model an action measures is the
/// page's own and not a 1x1 pixel's.
pub async fn ready(tab: &TabInfo, page: &dyn Page) -> Result<(), BrowserError> {
    cdp::present_for(page, tab.parked, tab.size)
        .await
        .map_err(cdp_failure)
}

pub async fn call(page: &dyn Page, method: &str, params: Value) -> Result<Value, BrowserError> {
    page.call(method, params).await.map_err(cdp_failure)
}

/// The address and title the page's own hooks last reported, read fresh: they
/// are the only record of where a navigation actually landed.
pub fn place(tab: &TabInfo) -> Place {
    let state = tab.state.lock().expect("browser state poisoned");
    Place {
        url: state.url.clone(),
        title: state.title.clone(),
    }
}

/// What the caller acted on, before anything was done to it.
pub async fn read(tab: &TabInfo, page: &dyn Page) -> Result<Start, BrowserError> {
    Ok(Start {
        place: place(tab),
        view: super::view_of(page, Mode::Interactive).await?,
    })
}

/// Wait for the page to stop moving, then answer with what changed.
///
/// The delta is the whole answer: the caller holds the view it acted on, and
/// the one thing it cannot know is what the action did to it. `target` is the
/// node itself, read again, so a checkbox that did not flip is visible as
/// exactly that rather than as a list of changes elsewhere.
pub async fn answer(
    tab: &TabInfo,
    page: &dyn Page,
    deadline: Deadline,
    start: Start,
    target: Option<u64>,
) -> Result<Value, BrowserError> {
    super::super::cdp_events::settle(&tab.browser_id, deadline).await;
    let after = super::view_of(page, Mode::Interactive).await?;
    let delta = delta::between(&start.view, &after, &start.place, &place(tab), target);
    Ok(json!({ "delta": delta }))
}

/// Bring a node into the page's own scroller before acting on it. A node off
/// screen in a nested scroller has a box outside the viewport, and a click at
/// its centre lands on whatever is in front of it.
pub async fn into_view(page: &dyn Page, node: u64) -> Result<(), BrowserError> {
    call(
        page,
        "DOM.scrollIntoViewIfNeeded",
        json!({ "backendNodeId": node }),
    )
    .await?;
    Ok(())
}

/// The middle of a node's own box, which is where a click belongs: an edge is
/// on the border, and a border scrolls the page instead of pressing the
/// control.
pub async fn centre(page: &dyn Page, node: u64) -> Result<(f64, f64), BrowserError> {
    let model = call(page, "DOM.getBoxModel", json!({ "backendNodeId": node })).await?;
    box_centre(&model).ok_or_else(|| host_error("That node has no box on the page right now."))
}

/// The middle of a node's own box, which is where a click belongs.
///
/// `content` is a FLAT array of eight numbers — four corners, x then y, top
/// left, top right, bottom right, bottom left. That is what this WebView2
/// answers with, recorded from a real call in
/// `scout/browser-tabs/SPIKE-REPORT-cdp.md` (`"content":[0,0,802.4,0,802.4,716,
/// 0,716]`), and it is the shape a link's box comes back in whatever the page
/// does with it. A quad this app cannot read is a click that lands nowhere,
/// which is exactly what a live run showed: every click on an ordinary link
/// refused with "that node has no box" while the node itself was fine.
pub fn box_centre(model: &Value) -> Option<(f64, f64)> {
    let quad = model.get("model")?.get("content")?.as_array()?;
    let corners: Vec<f64> = quad.iter().filter_map(|number| number.as_f64()).collect();
    if corners.len() < 8 {
        return None;
    }
    let (x, y) = corners[..8]
        .as_chunks::<2>()
        .0
        .iter()
        .fold((0.0, 0.0), |(x, y), [px, py]| (x + px, y + py));
    Some((x / 4.0, y / 4.0))
}

pub async fn mouse(
    page: &dyn Page,
    kind: &str,
    at: (f64, f64),
    button: &str,
    count: u32,
    modifiers: u32,
) -> Result<(), BrowserError> {
    call(
        page,
        "Input.dispatchMouseEvent",
        json!({
            "type": kind,
            "x": at.0,
            "y": at.1,
            "button": button,
            "clickCount": count,
            "modifiers": modifiers,
        }),
    )
    .await?;
    Ok(())
}

pub async fn wheel(page: &dyn Page, at: (f64, f64), delta_y: f64) -> Result<(), BrowserError> {
    call(
        page,
        "Input.dispatchMouseEvent",
        json!({
            "type": "mouseWheel",
            "x": at.0,
            "y": at.1,
            "deltaX": 0,
            "deltaY": delta_y,
            "button": "none",
            "clickCount": 0,
        }),
    )
    .await?;
    Ok(())
}

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

/// Run one function on the page's own node, and hand back what it returned.
///
/// `Runtime.callFunctionOn` needs a live `objectId` — the one from
/// `DOM.resolveNode` — which is why the page's script is reached per call and
/// never held: an object id belongs to the node that produced it.
pub async fn on_node(
    page: &dyn Page,
    node: u64,
    function: &str,
    args: Value,
) -> Result<Value, BrowserError> {
    let resolved = call(page, "DOM.resolveNode", json!({ "backendNodeId": node })).await?;
    let object = resolved
        .get("object")
        .and_then(|object| object.get("objectId"))
        .and_then(Value::as_str)
        .ok_or_else(|| host_error("That node could not be resolved on the page."))?
        .to_owned();
    let answered = call(
        page,
        "Runtime.callFunctionOn",
        json!({
            "objectId": object,
            "functionDeclaration": function,
            "arguments": args,
            "returnByValue": true,
        }),
    )
    .await?;
    // A function that threw is still a successful protocol answer, with the
    // failure beside an absent value: read as `null`, it would let `fill` type
    // into a field the clear never emptied.
    if let Some(thrown) = answered.get("exceptionDetails") {
        return Err(host_error(format!(
            "The page's script failed on that node: {}",
            thrown_text(thrown)
        )));
    }
    Ok(answered
        .get("result")
        .and_then(|result| result.get("value"))
        .cloned()
        .unwrap_or(Value::Null))
}

/// What a thrown script said, short enough to read: the exception's own
/// description when the runtime sent one, else the `text` beside it.
fn thrown_text(thrown: &Value) -> String {
    let said = thrown
        .get("exception")
        .and_then(|exception| exception.get("description"))
        .or_else(|| thrown.get("text"))
        .and_then(Value::as_str)
        .unwrap_or("it threw");
    said.chars().take(200).collect()
}

/// Empty a field the way a person would: through the native setter a
/// framework's value tracker wraps, then the events it listens for. A plain
/// `.value = ""` is invisible to that tracker, which is most of them.
pub const CLEAR: &str = r#"function () {
  if (this.isContentEditable) {
    this.textContent = "";
  } else if ("value" in this) {
    const setter = Object.getOwnPropertyDescriptor(
      Object.getPrototypeOf(this), "value"
    ).set;
    setter.call(this, "");
  } else {
    return false;
  }
  this.dispatchEvent(new Event("input", { bubbles: true }));
  this.dispatchEvent(new Event("change", { bubbles: true }));
  return true;
}"#;

/// Pick an option by value or by the label the page shows, and fire what a
/// person's choice fires.
pub const CHOOSE: &str = r#"function (wanted) {
  const options = this.options ? Array.from(this.options) : [];
  const wantedLabel = (wanted.label || "").trim();
  const hit = options.find((option) => option.value === wanted.value)
    || options.find((option) => (option.label || option.textContent || "").trim() === wantedLabel);
  if (!hit) return null;
  const setter = Object.getOwnPropertyDescriptor(
    Object.getPrototypeOf(this), "value"
  ).set;
  setter.call(this, hit.value);
  this.dispatchEvent(new Event("input", { bubbles: true }));
  this.dispatchEvent(new Event("change", { bubbles: true }));
  return hit.value;
}"#;

/// One node as the view last read it.
pub fn node_of_state(view: &View, backend_id: u64) -> Option<&view::ViewNode> {
    view.nodes.iter().find(|node| node.backend_id == backend_id)
}

#[cfg(test)]
#[path = "act_tests.rs"]
mod tests;
