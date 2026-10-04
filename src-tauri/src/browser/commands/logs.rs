//! `console_logs`: what the tab's page has said, in the words the caller asked
//! for.
//!
//! One command and no page call at all: the ring was filled by the tab's own
//! event stream as the page ran, so asking what it said is a question about
//! this process and never about the page.

use serde::Deserialize;
use serde_json::{json, Value};

use devboule_protocol::BrowserError;

use super::super::console::{self, Wanted};
use super::{args_of, host_error};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct LogsArgs {
    level: Option<String>,
    since_ms: Option<f64>,
}

pub fn logs(browser_id: &str, args: &Value) -> Result<Value, BrowserError> {
    let asked: LogsArgs = args_of(args)?;
    // A level this host does not know is refused by name: answering with the
    // default would leave the caller believing it asked for something else.
    let wanted = Wanted::parse(asked.level.as_deref()).ok_or_else(|| {
        host_error(format!(
            "{} is not a level; ask for error, warning or all.",
            asked.level.as_deref().unwrap_or_default()
        ))
    })?;
    let (entries, dropped) = console::entries(browser_id, wanted, asked.since_ms);
    Ok(json!({ "entries": entries, "dropped": dropped }))
}

#[cfg(test)]
#[path = "logs_tests.rs"]
mod tests;
