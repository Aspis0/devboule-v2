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
    let after = settled(tab, page, deadline).await?;
    let delta = delta::between(&start.view, &after, &start.place, &place(tab), target);
    Ok(json!({ "delta": delta }))
}

/// The same answer for an action that put input into a field: the field first,
/// then what popped up, then a few of the page's other changes.
pub async fn answer_input(
    tab: &TabInfo,
    page: &dyn Page,
    deadline: Deadline,
    start: Start,
    target: Option<u64>,
) -> Result<Value, BrowserError> {
    let after = settled(tab, page, deadline).await?;
    let delta = delta::between_input(&start.view, &after, &start.place, &place(tab), target);
    Ok(json!({ "delta": delta }))
}

async fn settled(tab: &TabInfo, page: &dyn Page, deadline: Deadline) -> Result<View, BrowserError> {
    super::super::cdp_events::settle(&tab.browser_id, deadline).await;
    super::view_of(page, Mode::Interactive).await
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

/// One node as the view last read it.
pub fn node_of_state(view: &View, backend_id: u64) -> Option<&view::ViewNode> {
    view.nodes.iter().find(|node| node.backend_id == backend_id)
}

#[cfg(test)]
#[path = "act_tests.rs"]
mod tests;
