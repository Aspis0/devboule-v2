//! The browser commands an agent is served: the name each answers to, and the
//! arguments it offers.
//!
//! The rows are the contract's 4b-1 wave, and nothing else: a command nobody
//! serves yet has no row, because a tool that always answers "no such tab" is a
//! tool an agent will try. The argument shapes and the check against them are
//! `browser_args`; this file is the table that says which command takes which.
//!
//! A tab travels to the host twice on purpose — inside `args`, because the host
//! needs it, and beside it, because that is what the broker routes on.

use serde_json::{json, Map, Value};

use crate::provider_catalog::{
    MCP_BROWSER_CHECK_TOOL, MCP_BROWSER_CLICK_TOOL, MCP_BROWSER_CLOSE_TAB_TOOL,
    MCP_BROWSER_FILL_TOOL, MCP_BROWSER_FIND_TOOL, MCP_BROWSER_HOVER_TOOL,
    MCP_BROWSER_LIST_TABS_TOOL, MCP_BROWSER_NAVIGATE_TOOL, MCP_BROWSER_NEW_TAB_TOOL,
    MCP_BROWSER_PRESS_TOOL, MCP_BROWSER_SCROLL_TOOL, MCP_BROWSER_SELECT_TOOL,
    MCP_BROWSER_SNAPSHOT_TOOL, MCP_BROWSER_TYPE_TOOL, MCP_BROWSER_WAIT_FOR_TOOL,
};

use super::browser_args::{optional, required, Kind, Spec};

/// Every browser tool, in `tools/list` order, with the bare command it runs.
pub(in crate::mcp_broker) const TOOLS: &[(&str, &str)] = &[
    (MCP_BROWSER_NEW_TAB_TOOL, "new_tab"),
    (MCP_BROWSER_LIST_TABS_TOOL, "list_tabs"),
    (MCP_BROWSER_CLOSE_TAB_TOOL, "close_tab"),
    (MCP_BROWSER_NAVIGATE_TOOL, "navigate"),
    (MCP_BROWSER_SNAPSHOT_TOOL, "snapshot"),
    (MCP_BROWSER_FIND_TOOL, "find"),
    (MCP_BROWSER_CLICK_TOOL, "click"),
    (MCP_BROWSER_FILL_TOOL, "fill"),
    (MCP_BROWSER_TYPE_TOOL, "type"),
    (MCP_BROWSER_PRESS_TOOL, "press"),
    (MCP_BROWSER_SELECT_TOOL, "select"),
    (MCP_BROWSER_CHECK_TOOL, "check"),
    (MCP_BROWSER_HOVER_TOOL, "hover"),
    (MCP_BROWSER_SCROLL_TOOL, "scroll"),
    (MCP_BROWSER_WAIT_FOR_TOOL, "wait_for"),
];

/// The longest wait an agent may ask for. Below the broker's own call deadline so
/// the host still has room to answer a timeout with the page's state rather than
/// with nothing.
const MAX_WAIT_MS: i64 = 12_000;

const SCROLL_DIRECTIONS: &[&str] = &["up", "down"];
const SNAPSHOT_MODES: &[&str] = &["interactive", "full"];
const NAVIGATE_ACTIONS: &[&str] = &["back", "forward", "reload"];
const CLICK_BUTTONS: &[&str] = &["left", "middle", "right"];
const MODIFIERS: &[&str] = &["Alt", "Control", "Meta", "Shift"];

const SPECS: &[Spec] = &[
    Spec {
        command: "new_tab",
        fields: &[required("url", Kind::Text).described("The address to open.")],
        alternatives: &[],
        tab: false,
    },
    Spec {
        command: "list_tabs",
        fields: &[],
        alternatives: &[],
        tab: false,
    },
    Spec {
        command: "close_tab",
        fields: &[required("browserId", Kind::Tab)],
        alternatives: &[],
        tab: true,
    },
    Spec {
        command: "navigate",
        fields: &[
            required("browserId", Kind::Tab),
            optional("url", Kind::Text).described("Where to go."),
            optional("action", Kind::Choices(NAVIGATE_ACTIONS)),
        ],
        alternatives: &[&["url"], &["action"]],
        tab: true,
    },
    Spec {
        command: "snapshot",
        fields: &[
            required("browserId", Kind::Tab),
            optional("scope", Kind::Ref).described("Read one element's subtree instead of the page."),
            optional("mode", Kind::Choices(SNAPSHOT_MODES)),
            optional("cursor", Kind::Text)
                .described("The cursor a truncated view answered, to continue from."),
        ],
        alternatives: &[],
        tab: true,
    },
    Spec {
        command: "find",
        fields: &[
            required("browserId", Kind::Tab),
            required("query", Kind::Text).described("What the control is called or does."),
        ],
        alternatives: &[],
        tab: true,
    },
    Spec {
        command: "click",
        fields: &[
            required("browserId", Kind::Tab),
            required("ref", Kind::Ref),
            optional("button", Kind::Choices(CLICK_BUTTONS)),
            optional("clickCount", Kind::Integer(1, Some(3))).described("2 is a double click."),
            optional("modifiers", Kind::Flags(MODIFIERS)),
        ],
        alternatives: &[],
        tab: true,
    },
    Spec {
        command: "fill",
        fields: &[
            required("browserId", Kind::Tab),
            required("ref", Kind::Ref),
            required("text", Kind::Text).described("The value to put in, replacing what is there."),
        ],
        alternatives: &[],
        tab: true,
    },
    Spec {
        command: "type",
        fields: &[
            required("browserId", Kind::Tab),
            required("text", Kind::Text).described("The text to type as keystrokes."),
            optional("ref", Kind::Ref).described("Type into this element instead of the focused one."),
        ],
        alternatives: &[],
        tab: true,
    },
    Spec {
        command: "press",
        fields: &[
            required("browserId", Kind::Tab),
            required("key", Kind::Text).described("A key or a chord, like Enter or Control+A."),
            optional("ref", Kind::Ref).described("Press in this element first."),
        ],
        alternatives: &[],
        tab: true,
    },
    Spec {
        command: "select",
        fields: &[
            required("browserId", Kind::Tab),
            required("ref", Kind::Ref),
            optional("value", Kind::Text).described("The option's value."),
            optional("label", Kind::Text).described("The option's visible text."),
        ],
        alternatives: &[&["value"], &["label"]],
        tab: true,
    },
    Spec {
        command: "check",
        fields: &[
            required("browserId", Kind::Tab),
            required("ref", Kind::Ref),
            required("checked", Kind::Bool),
        ],
        alternatives: &[],
        tab: true,
    },
    Spec {
        command: "hover",
        fields: &[required("browserId", Kind::Tab), required("ref", Kind::Ref)],
        alternatives: &[],
        tab: true,
    },
    Spec {
        command: "scroll",
        fields: &[
            required("browserId", Kind::Tab),
            optional("ref", Kind::Ref).described("Scroll this element into view."),
            optional("direction", Kind::Choices(SCROLL_DIRECTIONS)),
            optional("amount", Kind::Integer(1, None)).described("Pixels to scroll, with a direction."),
        ],
        // Scrolling an element into view and scrolling the page are two
        // different acts, so one of them has to be named: neither named means
        // no instruction at all, and both named means no single one to run.
        alternatives: &[&["ref"], &["direction"]],
        tab: true,
    },
    Spec {
        command: "wait_for",
        fields: &[
            required("browserId", Kind::Tab),
            optional("text", Kind::Text).described("Text the page must show."),
            optional("url", Kind::Text).described("A url the page must have reached."),
            optional("ref", Kind::Ref),
            optional("state", Kind::Text).described(
                "One of the states a snapshot prints: visible, hidden, enabled, disabled, checked, unchecked, expanded, collapsed.",
            ),
            optional("timeoutMs", Kind::Integer(1, Some(MAX_WAIT_MS))),
        ],
        alternatives: &[&["text"], &["url"], &["ref", "state"]],
        tab: true,
    },
];

/// Whether the broker serves `tool`.
pub(in crate::mcp_broker) fn serves(tool: &str) -> bool {
    command_for(tool).is_some()
}

/// The bare command a tool name runs, or `None` for a name the broker does not
/// serve — which is how the peer door tells a browser tool from any other name.
pub(in crate::mcp_broker) fn command_for(tool: &str) -> Option<&'static str> {
    TOOLS
        .iter()
        .find(|(name, _)| *name == tool)
        .map(|(_, command)| *command)
}

/// The row of one tool, once a caller has named it. `None` for a name the
/// broker does not serve.
pub(in crate::mcp_broker) fn spec_for(tool: &str) -> Option<&'static Spec> {
    spec_of(command_for(tool)?)
}

fn spec_of(command: &str) -> Option<&'static Spec> {
    SPECS.iter().find(|spec| spec.command == command)
}

/// The `tools/list` input schema for one tool, read out of its row.
pub(in crate::mcp_broker) fn schema_for(tool: &str) -> Option<Value> {
    let spec = spec_of(command_for(tool)?)?;
    Some(json!({
        "type": "object",
        "properties": spec.fields.iter().map(|field| (field.name.to_string(), field.schema())).collect::<Map<_, _>>(),
        "required": spec.fields.iter().filter(|field| field.required).map(|field| field.name).collect::<Vec<_>>(),
        "additionalProperties": false,
    }))
}
