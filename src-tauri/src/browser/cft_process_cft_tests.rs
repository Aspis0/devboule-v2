//! The real Chrome for Testing behind the process manager, by hand only.
//!
//! Ignored, because it needs a browser the machine may not have. Point
//! `DEVBOULE_CFT_CHROME` at one — the win64 Stable the manifest pins — and
//! run it by hand. Nothing here stops a process by image name: every browser
//! is the exact handle [`super::CftBrowser`] launched, killed through that
//! handle alone.

use std::path::PathBuf;
use std::time::Duration;

use serde_json::json;

use super::*;
use crate::browser::cdp::Page;
use crate::browser::cdp_ws::WsPage;

const PAGE: &str = "data:text/html,%3Ctitle%3ECfT%3C%2Ftitle%3E%3Cp%3Ehello%3C%2Fp%3E";

fn chrome_exe() -> PathBuf {
    std::env::var_os("DEVBOULE_CFT_CHROME")
        .map(PathBuf::from)
        .expect("DEVBOULE_CFT_CHROME names the chrome.exe to drive")
}

async fn wait_for_title(page: &WsPage) -> String {
    for _ in 0..40 {
        let read = page
            .call(
                "Runtime.evaluate",
                json!({ "expression": "document.title", "returnByValue": true }),
            )
            .await
            .expect("the page evaluates");
        if let Some(title) = read["result"]["value"].as_str() {
            if !title.is_empty() {
                return title.to_owned();
            }
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("the navigated page had no title after 2s");
}

#[test]
#[ignore = "needs a Chrome for Testing binary: DEVBOULE_CFT_CHROME"]
fn launch_opens_a_page_navigates_and_closes_gracefully() {
    let app_data = tempfile::tempdir().expect("an app-data dir");
    tauri::async_runtime::block_on(async {
        let browser = CftBrowser::launch(&chrome_exe(), app_data.path(), true)
            .await
            .expect("the browser starts");
        assert!(browser.port > 0);
        assert!(!browser.browser_ws.is_empty());
        assert_eq!(browser.exe, chrome_exe());
        assert!(browser.nonce > 0);
        assert_eq!(browser.profile, app_data.path().join("browser-profile"));

        // The profile lock is held for the browser's life: a second launch is
        // refused instead of being handed this browser's debugger.
        let second = CftBrowser::launch(&chrome_exe(), app_data.path(), true).await;
        let Err(CftError::Launch(text)) = second else {
            panic!("a second launch must refuse the busy profile");
        };
        assert!(text.contains("another Devboule"), "{text}");

        let (page, mut events) = browser
            .open_page("about:blank")
            .await
            .expect("a target opens");
        page.call("Page.enable", json!({}))
            .await
            .expect("the page's events switch on");
        let navigated = page
            .call("Page.navigate", json!({ "url": PAGE }))
            .await
            .expect("the page navigates");
        assert!(
            navigated["errorText"].is_null(),
            "a data: url is refused as {:?}",
            navigated["errorText"]
        );
        assert_eq!(wait_for_title(&page).await, "CfT");

        let event = tokio::time::timeout(Duration::from_secs(10), events.recv())
            .await
            .expect("a page that navigated says something within 10s")
            .expect("the reader still reads while the target is open");
        assert!(!event.method.is_empty());
        page.close().await;

        let pid = browser.pid();
        assert!(pid > 0);
        browser.shutdown().await;
        assert!(
            app_data
                .path()
                .join("browser-profile")
                .join("DevToolsActivePort")
                .exists(),
            "the owned profile stays on disk for the next launch"
        );
    });
}

#[test]
#[ignore = "needs a Chrome for Testing binary: DEVBOULE_CFT_CHROME"]
fn a_killed_child_is_replaced_never_by_name() {
    let app_data = tempfile::tempdir().expect("an app-data dir");
    tauri::async_runtime::block_on(async {
        let mut browser = CftBrowser::launch(&chrome_exe(), app_data.path(), true)
            .await
            .expect("the browser starts");
        let first = browser.pid();
        assert!(browser.is_alive());

        // Crash it through the owned handle alone: no image name is ever named.
        let child = browser.child.as_mut().expect("the owned child handle");
        child.kill().expect("the owned child is killed");
        let _ = child.wait();
        assert!(!browser.is_alive());

        // A replacement owns the profile only after the dead one let it go:
        // shutdown reaps what is left and releases the lock, which is what
        // slice 4's supervisor does before it relaunches.
        browser.shutdown().await;

        let delay = restart_delay(0);
        assert_eq!(delay, Duration::from_secs(1));
        std::thread::sleep(delay);

        let replacement = CftBrowser::launch(&chrome_exe(), app_data.path(), true)
            .await
            .expect("it restarts");
        assert_ne!(replacement.pid(), first);
        assert!(replacement.port > 0);
        let (page, _) = replacement
            .open_page("about:blank")
            .await
            .expect("it answers");
        page.close().await;
        replacement.shutdown().await;
    });
}
