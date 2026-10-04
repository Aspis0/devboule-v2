//! `wait_for`: the one command that watches instead of acting.
//!
//! It is the only command allowed to be slow, and it is still bounded: the
//! daemon's own budget is 15 s and the answer has to travel inside it, so the
//! wait is capped below that and the answer carries the delta either way. A
//! caller that is not met when the budget runs out gets `met: false` and the
//! page as it stood — which is a fact about the page, not a failure.

use std::time::Duration;

use serde::Deserialize;
use serde_json::{json, Value};

use devboule_protocol::BrowserError;

use super::super::cdp::Page;
use super::super::delta;
use super::super::registry::TabInfo;
use super::super::view::Mode;
use super::Deadline;
use super::{args_of, host_error, node_of};

/// The longest a caller may wait. Below the daemon's 15 s budget, with room
/// for the two view reads this command makes around it.
pub const MAX_WAIT_MS: u64 = 12_000;

/// How often the page is looked at.
const POLL: std::time::Duration = std::time::Duration::from_millis(250);

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WaitArgs {
    text: Option<String>,
    url: Option<String>,
    #[serde(rename = "ref")]
    reference: Option<String>,
    state: Option<String>,
    timeout_ms: Option<u64>,
}

pub async fn wait_for(
    tab: &TabInfo,
    page: &dyn Page,
    args: &Value,
    deadline: Deadline,
) -> Result<Value, BrowserError> {
    let asked: WaitArgs = args_of(args)?;
    if asked.text.is_none() && asked.url.is_none() && asked.reference.is_none() {
        return Err(host_error(
            "wait_for needs a text, a url or a ref to wait for.",
        ));
    }
    if let Some(state) = &asked.state {
        if asked.reference.is_none() {
            return Err(host_error("a state to wait for belongs to a ref."));
        }
        if !STATE_WORDS.contains(&state.as_str()) {
            return Err(host_error(format!("{state} is not a state to wait for.")));
        }
    }
    let node = asked.reference.as_deref().map(node_of).transpose()?;
    let start = super::act::read(tab, page).await?;
    super::act::ready(tab, page).await?;
    // The smaller of what the caller asked for, the contract's own cap, and
    // what the command's answer still needs out of its budget.
    let budget = Duration::from_millis(asked.timeout_ms.unwrap_or(5_000).min(MAX_WAIT_MS))
        .min(deadline.wait_for());
    let deadline = std::time::Instant::now() + budget;
    let mut met = false;
    while std::time::Instant::now() < deadline {
        met = is_met(tab, page, &asked, node).await?;
        if met {
            break;
        }
        super::super::cdp_events::nap(POLL).await;
    }
    // A wait that was met has already watched the page until it stopped
    // moving, so it is not settled again; one that ran out has not.
    if !met {
        super::super::cdp_events::settle(&tab.browser_id).await;
    }
    let after = super::view_of(page, Mode::Interactive).await?;
    let delta = delta::between(
        &start.view,
        &after,
        &start.place,
        &super::act::place(tab),
        node,
    );
    Ok(json!({ "met": met, "delta": delta }))
}

/// The words a `state` may be. Checked here once, so the poll below cannot
/// meet on a state it does not understand.
const STATE_WORDS: [&str; 8] = [
    "visible", "present", "hidden", "absent", "enabled", "disabled", "focused", "checked",
];

/// Whether the thing the caller named is true of the page right now.
async fn is_met(
    tab: &TabInfo,
    page: &dyn Page,
    asked: &WaitArgs,
    node: Option<u64>,
) -> Result<bool, BrowserError> {
    if let Some(url) = &asked.url {
        return Ok(super::act::place(tab).url.contains(url.as_str()));
    }
    if let Some(reference) = node {
        // A hidden node is one the runtime no longer reports, so presence and
        // visibility are the same question against the accessible tree.
        let view = super::view_of(page, Mode::Interactive).await?;
        let Some(state) = super::act::node_of_state(&view, reference) else {
            return Ok(matches!(
                asked.state.as_deref(),
                Some("hidden") | Some("absent")
            ));
        };
        return Ok(match asked.state.as_deref().unwrap_or("visible") {
            "visible" | "present" => true,
            "hidden" | "absent" => false,
            "enabled" => !state.disabled,
            "disabled" => state.disabled,
            "focused" => state.focused,
            "checked" => state.checked == Some(true),
            other => return Err(host_error(format!("{other} is not a state to wait for."))),
        });
    }
    let text = asked.text.as_deref().unwrap_or_default().to_lowercase();
    let view = super::view_of(page, Mode::Interactive).await?;
    Ok(view.nodes.iter().any(|node| {
        node.name.to_lowercase().contains(&text)
            || node.value.to_lowercase().contains(&text)
            || node.nearby.to_lowercase().contains(&text)
    }))
}
