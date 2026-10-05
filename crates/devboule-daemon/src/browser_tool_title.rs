//! The one line a browser tool call shows in the chat transcript: the command,
//! and the single argument a reader needs to recognise the call.
//!
//! `browser_fill` on a ref with a value in it reads as `fill e3 "WebView2"`, and
//! nothing else: the whole argument set and the result are in the transcript
//! body. A value is cut so a long text or a query cannot turn the row into a
//! paragraph, and a url is shown as its host — the full address is in the body,
//! and a cut one would name a page that does not exist.
//!
//! A name outside the lane answers `None`, and its row keeps whatever title the
//! provider's own view gave it.

use serde_json::Value;

use crate::wire_json::broker_tool_name;

/// How much of a value the row shows before it is cut.
const MAX_VALUE_CHARS: usize = 40;
const CUT: char = '\u{2026}';

/// The row's line for one browser tool call, or `None` for any other tool. A
/// provider may have qualified the name with this daemon's MCP server
/// (`mcp__devboule__browser_click`), which is the same call.
pub(crate) fn browser_tool_title(name: &str, input: &Value) -> Option<String> {
    let row = match broker_tool_name(name) {
        "browser_new_tab" => row("new tab", url(input)),
        "browser_list_tabs" => Some("list tabs".to_string()),
        "browser_close_tab" => Some("close tab".to_string()),
        "browser_navigate" => row("navigate", url(input).or_else(|| field(input, "action"))),
        "browser_snapshot" => row("snapshot", field(input, "scope")),
        "browser_find" => row("find", quoted(field(input, "query"))),
        "browser_click" => row("click", field(input, "ref")),
        "browser_fill" => row("fill", join(field(input, "ref"), length(input, "text"))),
        "browser_type" => row("type", join(field(input, "ref"), length(input, "text"))),
        "browser_press" => row("press", field(input, "key")),
        "browser_select" => row(
            "select",
            join(
                field(input, "ref"),
                length_of(
                    input
                        .get("value")
                        .or_else(|| input.get("label"))
                        .and_then(Value::as_str),
                ),
            ),
        ),
        "browser_check" => row("check", join(field(input, "ref"), flag(input, "checked"))),
        "browser_hover" => row("hover", field(input, "ref")),
        "browser_scroll" => row(
            "scroll",
            field(input, "ref").or_else(|| join(field(input, "direction"), field(input, "amount"))),
        ),
        "browser_wait_for" => row(
            "wait for",
            quoted(field(input, "text"))
                .or_else(|| url(input))
                .or_else(|| join(field(input, "ref"), field(input, "state"))),
        ),
        "browser_act" => input
            .get("steps")
            .and_then(Value::as_array)
            .map(|steps| format!("act {} steps", steps.len())),
        "browser_screenshot" => row(
            "screenshot",
            field(input, "zoom").map(|zoom| format!("zoom {zoom}")),
        ),
        "browser_click_at" => point(input),
        "browser_read_text" => row("read text", field(input, "scope")),
        "browser_console_logs" => row("console logs", field(input, "level")),
        "browser_fill_login" => row(
            "fill login",
            join(field(input, "usernameRef"), field(input, "passwordRef")),
        ),
        _ => return None,
    }?;
    Some(row)
}

/// `verb detail`, or the verb alone when the call named no detail.
fn row(verb: &str, detail: Option<String>) -> Option<String> {
    Some(match detail.filter(|detail| !detail.is_empty()) {
        Some(detail) => format!("{verb} {detail}"),
        None => verb.to_string(),
    })
}

/// Both halves of a row's detail, the second one only when the first is there.
fn join(first: Option<String>, second: Option<String>) -> Option<String> {
    Some(
        [first, second]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(" "),
    )
}

/// One argument as the row shows it: text on one line, or a number.
fn field(input: &Value, key: &str) -> Option<String> {
    let value = input.get(key)?;
    value
        .as_str()
        .map(one_line)
        .or_else(|| value.as_f64().map(number))
        .filter(|shown| !shown.is_empty())
}

/// A number as a row reads it: a whole one without its decimals, because `12`
/// is what the caller wrote and `12.0` would be this file's noise.
fn number(value: f64) -> String {
    if value.fract() == 0.0 {
        format!("{}", value as i64)
    } else {
        format!("{value}")
    }
}

/// One boolean argument, spelled as the row reads it.
fn flag(input: &Value, key: &str) -> Option<String> {
    input.get(key)?.as_bool().map(|on| on.to_string())
}

/// One argument in quotes, so a value with a space in it stays one value.
fn quoted(value: Option<String>) -> Option<String> {
    value.map(|value| format!("\"{value}\""))
}

/// How much text a call put into a page, and never the text itself: a row is
/// journaled, and a filled password would outlive the call in the clear.
fn length(input: &Value, key: &str) -> Option<String> {
    length_of(input.get(key)?.as_str())
}

fn length_of(value: Option<&str>) -> Option<String> {
    let chars = value?.chars().count();
    Some(format!(
        "({chars} {})",
        if chars == 1 { "char" } else { "chars" }
    ))
}

/// A url as its host and port, so the row names the page and nothing of the
/// credentials or the path. An address with no host to name shows no detail.
fn url(input: &Value) -> Option<String> {
    let address = input.get("url")?.as_str()?;
    let authority = address.split_once("://")?.1;
    let host = authority.split(['/', '?', '#']).next()?;
    let host = host.rsplit_once('@').map_or(host, |(_, host)| host);
    (!host.is_empty()).then(|| host.to_ascii_lowercase())
}

/// The pixel a click named. Cut never applies: a rounded pixel is another pixel.
fn point(input: &Value) -> Option<String> {
    let x = input.get("x")?.as_i64()?;
    let y = input.get("y")?.as_i64()?;
    Some(format!("click at {x},{y}"))
}

/// One line, at most [`MAX_VALUE_CHARS`], so no argument can make the row a
/// paragraph.
fn one_line(text: &str) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= MAX_VALUE_CHARS {
        return flat;
    }
    let cut = flat.chars().take(MAX_VALUE_CHARS).collect::<String>();
    format!("{cut}{CUT}")
}

#[cfg(test)]
#[path = "browser_tool_title_tests.rs"]
mod tests;
