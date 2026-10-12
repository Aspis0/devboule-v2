//! `resize`: the viewport a background tab is laid out at, so a layout can be
//! checked at a phone's or a tablet's size.
//!
//! A tab on screen takes its size from the pane it is in, and this never
//! overrides that. A parked tab is measured against the size it was last given,
//! so the size set here is the one the tab's later commands keep.

use serde::Deserialize;
use serde_json::{json, Value};

use devboule_protocol::BrowserError;

use super::super::cdp::{self, Page};
use super::super::registry::{Size, TabInfo};
use super::{act, args_of, cdp_failure, host_error};

#[derive(Deserialize)]
struct ViewportArgs {
    width: u32,
    height: u32,
}

pub async fn resize(tab: &TabInfo, page: &dyn Page, args: &Value) -> Result<Value, BrowserError> {
    let asked: ViewportArgs = args_of(args)?;
    if !tab.live.parked() {
        return Err(host_error(
            "This tab is on screen, and the pane it is in sets its size, so it cannot be resized. A tab in the background can be.",
        ));
    }
    let size = Size {
        width: f64::from(asked.width),
        height: f64::from(asked.height),
    };
    cdp::present_for(page, &tab.live, size)
        .await
        .map_err(cdp_failure)?;
    // The pane can show the tab during the await, and `present_for` then clears
    // the override. The size the pane gave is the one to keep, not this one.
    if !tab.live.parked() {
        return Err(host_error(
            "This tab was shown in its pane while it was being resized, so the pane's size stays. Ask again while it is in the background.",
        ));
    }
    tab.live.set_size(size);
    let (width, height) = act::viewport(page).await?;
    Ok(json!({ "width": width, "height": height }))
}

#[cfg(test)]
#[path = "resize_tests.rs"]
mod tests;
