//! `screenshot`: the page as a picture, for what its own words do not carry.
//!
//! A canvas, a map, an image of a chart, a layout only a person can see: the
//! view describes the controls and says nothing about the picture above them.
//! This is where that picture comes from, as JPEG, small enough to travel
//! inside an answer.
//!
//! The size is stepped down until the picture fits, because the daemon's answer
//! limit is a hard one and a screenshot that exceeds it is no answer at all.
//! The clip is in the CSS pixels of the viewport, the same space a `click_at`
//! point is in, so a point read off this picture is one that can be pressed.

use serde::Deserialize;
use serde_json::{json, Value};

use devboule_protocol::BrowserError;

use super::super::cdp::Page;
use super::super::registry::TabInfo;
use super::{act, args_of, host_error};

/// The largest a picture may be before it is not sent. Below the daemon's own
/// answer limit, so the answer that carries it still fits.
pub const MAX_BYTES: usize = 700 * 1024;

/// The qualities it is taken at, best first. A page of flat colour is well
/// under the cap at the first rung and a photograph of a page of charts needs
/// the last.
const QUALITIES: [u32; 6] = [80, 65, 50, 40, 30, 20];

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ShotArgs {
    clip: Option<Clip>,
    zoom: Option<f64>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Clip {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}

pub async fn screenshot(
    tab: &TabInfo,
    page: &dyn Page,
    args: &Value,
) -> Result<Value, BrowserError> {
    let asked: ShotArgs = args_of(args)?;
    let zoom = asked.zoom.unwrap_or(1.0);
    if !(1.0..=3.0).contains(&zoom) {
        return Err(host_error(format!(
            "zoom is a number from 1 to 3; this one is {zoom}."
        )));
    }
    let clip = asked
        .clip
        .filter(|clip| clip.width > 0.0 && clip.height > 0.0);
    // A parked page photographs as two blank pixels, so it is put at the size
    // a pane would show it at before anything is asked of the renderer.
    act::ready(tab, page).await?;
    let (css_width, css_height) = act::viewport(page).await?;
    let mut picture = capture(page, clip.as_ref(), QUALITIES[0], zoom).await?;
    for quality in &QUALITIES[1..] {
        if bytes_of(&picture) <= MAX_BYTES {
            break;
        }
        picture = capture(page, clip.as_ref(), *quality, zoom).await?;
    }
    // The picture is as many pixels as the space asked for at the zoom asked
    // for; the runtime reports no size of its own, and these are the two
    // numbers the request was made of.
    let (width, height) = match &clip {
        Some(clip) => (clip.width * zoom, clip.height * zoom),
        None => (css_width * zoom, css_height * zoom),
    };
    let mut answered = json!({
        "mimeType": "image/jpeg",
        "data": picture,
        "width": width.round(),
        "height": height.round(),
        "cssWidth": css_width,
        "cssHeight": css_height,
    });
    if let Some(clip) = clip {
        answered["clip"] = json!({
            "x": clip.x, "y": clip.y, "width": clip.width, "height": clip.height,
        });
    }
    Ok(answered)
}

/// One picture, as base64.
///
/// No `data` at all is a refusal rather than a picture: answering with an empty
/// image would read as "the page is blank", which is the one thing the caller
/// would then believe.
async fn capture(
    page: &dyn Page,
    clip: Option<&Clip>,
    quality: u32,
    zoom: f64,
) -> Result<String, BrowserError> {
    let mut params = json!({
        "format": "jpeg",
        "quality": quality,
        // The viewport is what a screenshot means here: a point read off it is
        // pressed with `click_at`, and a point on the page below is not on it.
        "captureBeyondViewport": false,
    });
    if let Some(clip) = clip {
        params["clip"] = json!({
            "x": clip.x,
            "y": clip.y,
            "width": clip.width,
            "height": clip.height,
            // The one number that turns a viewport measurement into a picture
            // of it: a device pixel is a CSS pixel times this.
            "scale": zoom,
        });
    }
    let answered = act::call(page, "Page.captureScreenshot", params).await?;
    answered
        .get("data")
        .and_then(Value::as_str)
        .filter(|data| !data.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| host_error("This page returned no picture."))
}

/// How many bytes of picture a run of base64 holds. Base64 is four characters
/// to every three bytes, padding apart, so this is the size within two bytes.
fn bytes_of(base64: &str) -> usize {
    base64.len() / 4 * 3
}

#[cfg(test)]
#[path = "shot_tests.rs"]
mod tests;
