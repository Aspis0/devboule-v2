//! `screenshot` on a real Chromium that has been scrolled, by hand only.
//!
//! Ignored, because it needs a browser the machine may not have. Point
//! `DEVBOULE_CFT_CHROME` at one and run it:
//!
//! ```text
//! DEVBOULE_CFT_CHROME="C:/Program Files/Google/Chrome/Application/chrome.exe" \
//!   cargo test -p devboule --lib browser::commands::shot::live_tests -- --ignored
//! ```
//!
//! The first capture is the viewport's own band left at the document's origin
//! after the page scrolled away from it: the runtime answers that with a blank
//! picture, which is the fault this guards against. The second is the command's
//! own answer, and it must not be blank.

use std::path::PathBuf;

use serde_json::json;

use super::*;
use crate::browser::cft_process::CftBrowser;
use crate::browser::test_support::parked_tab;

fn chrome_exe() -> PathBuf {
    std::env::var_os("DEVBOULE_CFT_CHROME")
        .map(PathBuf::from)
        .expect("DEVBOULE_CFT_CHROME names the browser to drive")
}

/// A page taller than its window many times over, with rows that photograph as
/// unlike each other.
fn tall_page() -> String {
    let rows = (0..200)
        .map(|at| {
            let colour = if at % 2 == 0 { "#111" } else { "#eee" };
            format!("<p style=\"margin:0;color:{colour};font:16px sans-serif\">row {at} of a scrolled page</p>")
        })
        .collect::<String>();
    format!(
        "data:text/html,{}",
        urlencoding(&format!("<body style=\"margin:0\">{rows}</body>"))
    )
}

/// `encodeURIComponent`, for the characters this page is built from.
fn urlencoding(text: &str) -> String {
    text.bytes()
        .map(|byte| match byte {
            b'A'..=b'Z'
            | b'a'..=b'z'
            | b'0'..=b'9'
            | b'-'
            | b'_'
            | b'.'
            | b'!'
            | b'~'
            | b'*'
            | b'\''
            | b'('
            | b')' => (byte as char).to_string(),
            b' ' => "%20".to_owned(),
            other => format!("%{other:02X}"),
        })
        .collect()
}

#[test]
#[ignore = "needs a Chromium binary: DEVBOULE_CFT_CHROME"]
fn a_scrolled_page_is_photographed_not_answered_blank() {
    let app_data = tempfile::tempdir().expect("an app-data dir");
    tauri::async_runtime::block_on(async {
        let browser = CftBrowser::launch(&chrome_exe(), app_data.path(), true)
            .await
            .expect("the browser starts");
        let (page, _events) = browser
            .open_page("about:blank")
            .await
            .expect("a target opens");
        page.call("Page.enable", json!({}))
            .await
            .expect("the page's events switch on");
        page.call("Page.navigate", json!({ "url": tall_page() }))
            .await
            .expect("the page navigates");
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;

        let tab = parked_tab("tab-1");
        // The command puts a parked page at the size its pane last used before
        // it measures anything, so the size is read the way it will be.
        act::ready(&tab, &page)
            .await
            .expect("the parked page is put at a pane size");
        let (width, height) = act::viewport(&page).await.expect("the viewport reads");
        page.call(
            "Runtime.evaluate",
            json!({ "expression": "window.scrollTo({ top: 2000, behavior: 'smooth' })" }),
        )
        .await
        .expect("the page scrolls");
        tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
        let (_, page_y) = act::viewport_origin(&page).await.expect("its origin reads");
        assert_eq!(
            page_y, 2000.0,
            "the page is scrolled away from its first band"
        );

        let params = json!({
            "format": "jpeg",
            "quality": 80,
            "captureBeyondViewport": false,
            "clip": { "x": 0.0, "y": 0.0, "width": width, "height": height, "scale": 1.0 },
        });
        let blank = page
            .call("Page.captureScreenshot", params)
            .await
            .expect("the runtime answers")["data"]
            .as_str()
            .expect("a picture")
            .to_owned();

        let answered = screenshot(&tab, &page, &json!({ "browserId": "tab-1" }))
            .await
            .expect("the command answers");
        let picture = answered["data"].as_str().expect("a picture");

        assert_eq!(
            answered["clip"],
            json!({ "x": 0.0, "y": 0.0, "width": width, "height": height }),
            "the band is the viewport's own, whatever the scroll"
        );
        let (_, after) = act::viewport_origin(&page).await.expect("its origin reads");
        assert_eq!(
            after, 2000.0,
            "and the page was scrolled while it was taken"
        );
        // A blank picture of this size is a few kilobytes however many colours
        // it has; a page of rows costs an order of magnitude more.
        assert!(
            picture.len() > blank.len() * 5,
            "the scrolled band is a picture of the page: {} bytes against the blank {}",
            picture.len(),
            blank.len()
        );

        browser.shutdown().await;
    });
}
