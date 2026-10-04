//! `screenshot`: the page as a picture, for what its own words do not carry.
//!
//! A canvas, a map, an image of a chart, a layout only a person can see: the
//! view describes the controls and says nothing about the picture above them.
//! This is where that picture comes from, as JPEG, small enough to travel
//! inside an answer.
//!
//! **The picture is in CSS pixels times the zoom, on every display.** A display
//! that scales renders at one and a half times the CSS size, so the scale sent
//! to the renderer divides that out and the picture comes back at the size the
//! caller asked for; a point read off it and divided by the zoom is a point
//! `click_at` takes. The size the answer reports is the picture's own, read out
//! of its header, because a renderer that rounds is the only thing that knows
//! how big it made one.
//!
//! The clip is in the CSS pixels of the viewport, the same space a `click_at`
//! point is in. A zoom with no clip zooms the viewport, never nothing.
//!
//! What is capped is the **encoded answer** — the base64 and the JSON around
//! it — under what the daemon and the wire allow, because a picture measured in
//! decoded bytes becomes a third bigger on the wire and is refused on arrival.
//! Quality is stepped down first and then the picture is taken smaller, and a
//! page that will not fit at any size this tool sends is refused rather than
//! sent.

use serde::Deserialize;
use serde_json::{json, Value};

use devboule_protocol::BrowserError;

use super::super::cdp::Page;
use super::super::registry::TabInfo;
use super::{act, args_of, host_error};

/// The largest encoded answer this host sends: 820 KiB, under the 900 KiB the
/// daemon and the wire cap a result at, so the JSON around the base64 always
/// fits inside what the caller's frame allows.
pub const MAX_ANSWER_BYTES: usize = 820 * 1024;

/// The qualities it is taken at, best first. A page of flat colour is well
/// under the cap at the first rung and a photograph of a page of charts needs
/// the last.
const QUALITIES: [u32; 6] = [80, 65, 50, 40, 30, 20];

/// The smallest scale this tool sends: one picture pixel per CSS pixel, so a
/// `click_at` point still reads off it at one to one. Below that a point would
/// not land where it was read from, which is worse than no picture.
const MIN_ZOOM: f64 = 1.0;

/// How far the base64 alphabet is read for the header. A JPEG says its size in
/// its first twenty-odd bytes; this is four times that.
const HEADER_CHARS: usize = 64;

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
    let zoom = asked.zoom.unwrap_or(MIN_ZOOM);
    if !(MIN_ZOOM..=3.0).contains(&zoom) {
        return Err(host_error(format!(
            "zoom is a number from 1 to 3; this one is {zoom}."
        )));
    }
    // A parked page photographs as two blank pixels, so it is put at the size
    // a pane would show it at before anything is asked of the renderer.
    act::ready(tab, page).await?;
    let (css_width, css_height) = act::viewport(page).await?;
    let display = act::display_scale(page).await?;
    // A zoom with no clip zooms the whole viewport: a picture labelled twice the
    // size must be twice the size, or a point read off it lands elsewhere.
    let clip = match asked
        .clip
        .filter(|clip| clip.width > 0.0 && clip.height > 0.0)
    {
        Some(clip) => clip,
        None => Clip {
            x: 0.0,
            y: 0.0,
            width: css_width,
            height: css_height,
        },
    };
    let mut picture_zoom = zoom;
    loop {
        // The scale the renderer is asked for is the zoom over the display's
        // own, so the picture comes back in CSS pixels times the zoom whatever
        // the display does.
        let scale = picture_zoom / display;
        for quality in QUALITIES {
            let picture = capture(page, &clip, quality, scale).await?;
            let answered = answer(&picture, &clip, css_width, css_height, picture_zoom);
            if encoded(&answered) <= MAX_ANSWER_BYTES {
                return Ok(answered);
            }
        }
        if picture_zoom <= MIN_ZOOM {
            return Err(host_error(
                "This page's picture is too large to send even at one pixel per point; \
                 photograph part of it with a clip.",
            ));
        }
        picture_zoom = (picture_zoom / 2.0).max(MIN_ZOOM);
    }
}

/// The answer: the picture, the space it is in, and the factor between them.
fn answer(picture: &str, clip: &Clip, css_width: f64, css_height: f64, zoom: f64) -> Value {
    // What the picture really is, when it says: the size it was made at is the
    // only one a point read off it can be divided into.
    let (width, height) = jpeg_size(picture)
        .unwrap_or_else(|| ((clip.width * zoom).round(), (clip.height * zoom).round()));
    let zoom = if clip.width > 0.0 {
        width / clip.width
    } else {
        zoom
    };
    json!({
        "mimeType": "image/jpeg",
        "data": picture,
        "width": width,
        "height": height,
        "cssWidth": css_width,
        "cssHeight": css_height,
        // The factor a picture pixel is worth in the points click_at takes, so
        // an agent reads a point off the picture and divides by this.
        "zoom": zoom,
        "clip": { "x": clip.x, "y": clip.y, "width": clip.width, "height": clip.height },
    })
}

/// The answer as it goes on the wire, which is what the cap is about.
fn encoded(answered: &Value) -> usize {
    serde_json::to_vec(answered).map_or(usize::MAX, |frame| frame.len())
}

/// One picture, as base64.
///
/// No `data` at all is a refusal rather than a picture: answering with an empty
/// image would read as "the page is blank", which is the one thing the caller
/// would then believe.
async fn capture(
    page: &dyn Page,
    clip: &Clip,
    quality: u32,
    scale: f64,
) -> Result<String, BrowserError> {
    let params = json!({
        "format": "jpeg",
        "quality": quality,
        // The viewport is what a screenshot means here: a point read off it is
        // pressed with `click_at`, and a point on the page below is not on it.
        "captureBeyondViewport": false,
        "clip": {
            "x": clip.x,
            "y": clip.y,
            "width": clip.width,
            "height": clip.height,
            // The one number that turns a viewport measurement into a picture
            // of it.
            "scale": scale,
        },
    });
    let answered = act::call(page, "Page.captureScreenshot", params).await?;
    answered
        .get("data")
        .and_then(Value::as_str)
        .filter(|data| !data.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| host_error("This page returned no picture."))
}

/// The picture's own size, read out of its own header.
///
/// The renderer is the only thing that knows how big it made a picture: a
/// viewport is asked for, not a size, and a renderer rounds. A header this
/// cannot read is not an answer's problem — the caller falls back to what it
/// asked for.
fn jpeg_size(picture: &str) -> Option<(f64, f64)> {
    let bytes = header_bytes(picture)?;
    if bytes.first() != Some(&0xFF) || bytes.get(1) != Some(&0xD8) {
        return None;
    }
    let mut at = 2;
    while at + 4 <= bytes.len() {
        if bytes[at] != 0xFF {
            return None;
        }
        let marker = bytes[at + 1];
        at += 2;
        // A start-of-frame marker: its length, the sample precision, then the
        // height and the width, big-endian. The three markers in this range that
        // are not one — a Huffman table, a restart interval, arithmetic coding —
        // are read over, like every other segment.
        if (0xC0..=0xCF).contains(&marker) && !matches!(marker, 0xC4 | 0xC8 | 0xCC) {
            if at + 7 > bytes.len() {
                return None;
            }
            let height = u16::from_be_bytes([bytes[at + 3], bytes[at + 4]]) as f64;
            let width = u16::from_be_bytes([bytes[at + 5], bytes[at + 6]]) as f64;
            return (width > 0.0 && height > 0.0).then_some((width, height));
        }
        // Anything else on its own carries no length: start of image, end of
        // image, a restart marker.
        if matches!(marker, 0xD8 | 0xD9 | 0x01) {
            continue;
        }
        let length = u16::from_be_bytes([bytes[at], bytes[at + 1]]) as usize;
        if length < 2 {
            return None;
        }
        at += length;
    }
    None
}

/// The first bytes of a base64 run. The alphabet is walked once and the bytes
/// it makes are taken, so a 700 KiB picture is not decoded to read its header.
fn header_bytes(picture: &str) -> Option<Vec<u8>> {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut bytes = Vec::new();
    let mut packed = 0u32;
    let mut bits = 0u32;
    for character in picture.chars().take(HEADER_CHARS) {
        let value = ALPHABET
            .iter()
            .position(|letter| *letter as char == character)? as u32;
        packed = (packed << 6) | value;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            bytes.push((packed >> bits) as u8);
        }
    }
    Some(bytes)
}

#[cfg(test)]
#[path = "shot_tests.rs"]
mod tests;
