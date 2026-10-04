//! One real Chromium for Testing behind this transport, for what a fake
//! endpoint cannot settle: that a page target's own debugger socket answers
//! the same method literals the WebView2 child answers, and that the answers
//! carry a real page's title and real pixels.
//!
//! Ignored, because it needs a browser the machine may not have. Point
//! `DEVBOULE_CFT_CHROME` at one — the win64 Stable of
//! <https://googlechromelabs.github.io/chrome-for-testing/last-known-good-versions-with-downloads.json>
//! — and run it by hand.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use serde_json::{json, Value};

use super::WsPage;
use crate::browser::cdp::Page;

const PAGE: &str = "data:text/html,%3Ctitle%3ECfT%3C%2Ftitle%3E%3Cp%3Ehello%3C%2Fp%3E";

/// The one Chromium this test started, and the only one it ever stops. The
/// handle is dropped on a panic too, so a failing assertion cannot leave a
/// browser behind, and nothing here stops a process by its image name.
struct OwnedChrome(Option<Child>);

impl Drop for OwnedChrome {
    fn drop(&mut self) {
        if let Some(mut child) = self.0.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

/// The port the browser wrote into its profile, which is how port 0 gets to be
/// safe: nothing can be listening on a port nobody chose.
fn debugging_port(profile: &Path) -> std::io::Result<u16> {
    let marker = profile.join("DevToolsActivePort");
    for _ in 0..300 {
        if let Ok(text) = std::fs::read_to_string(&marker) {
            if let Some(port) = text
                .lines()
                .next()
                .and_then(|line| line.trim().parse().ok())
            {
                return Ok(port);
            }
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    Err(std::io::Error::other(
        "no DevToolsActivePort in the profile after 30s",
    ))
}

/// The page targets a Chromium on `port` is holding. Its `/json/list` is the
/// one place a page target's own debugger address is published.
fn page_targets(port: u16) -> std::io::Result<Vec<Value>> {
    let mut socket = std::net::TcpStream::connect(("127.0.0.1", port))?;
    socket.set_read_timeout(Some(Duration::from_secs(10)))?;
    write!(
        socket,
        "GET /json/list HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\r\n"
    )?;
    let mut answer = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        let read = socket.read(&mut chunk)?;
        if read == 0 {
            break;
        }
        answer.extend_from_slice(&chunk[..read]);
        if let Some(body) = body_of(&answer) {
            return serde_json::from_slice(body).map_err(std::io::Error::other);
        }
    }
    Err(std::io::Error::other(
        "the target list ended before its whole body arrived",
    ))
}

/// The answer's body, once all of it has arrived. The server does not close
/// the connection when the request asks it to, so the read stops at the length
/// the headers announce rather than sitting out a read timeout for an EOF.
fn body_of(answer: &[u8]) -> Option<&[u8]> {
    let text = std::str::from_utf8(answer).ok()?;
    let (headers, body) = text.split_once("\r\n\r\n")?;
    let announced = headers
        .lines()
        .find_map(|line| line.strip_prefix("Content-Length:"))
        .and_then(|length| length.trim().parse::<usize>().ok())?;
    (body.len() >= announced).then(|| &body.as_bytes()[..announced])
}

/// The document title, read until the navigated page has one: `Page.navigate`
/// returns when the navigation is answered, and a `data:` document is written
/// by the renderer a moment later.
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
            return title.to_owned();
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("the navigated page had no title after 2s");
}

#[test]
#[ignore = "needs a Chrome for Testing binary: DEVBOULE_CFT_CHROME"]
fn a_page_target_over_a_websocket_navigates_evaluates_and_screenshots() {
    let chrome = std::env::var_os("DEVBOULE_CFT_CHROME")
        .map(PathBuf::from)
        .expect("DEVBOULE_CFT_CHROME names the chrome.exe to drive");
    let profile = tempfile::tempdir().expect("a profile directory of its own");
    let child = Command::new(&chrome)
        .args([
            "--headless=new",
            "--remote-debugging-port=0",
            &format!("--user-data-dir={}", profile.path().display()),
            "--no-first-run",
            "--no-default-browser-check",
            "about:blank",
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("the browser starts");
    let _browser = OwnedChrome(Some(child));

    tauri::async_runtime::block_on(async {
        let port = debugging_port(profile.path()).expect("the browser publishes its port");
        let targets = page_targets(port).expect("the page targets are listed");
        let address = targets
            .iter()
            .find(|target| target["type"] == json!("page"))
            .and_then(|target| target["webSocketDebuggerUrl"].as_str())
            .expect("a page target with its own debugger address")
            .to_owned();

        let (page, mut events) = WsPage::connect(&address)
            .await
            .expect("the target is opened");
        page.call("Page.enable", json!({}))
            .await
            .expect("the page's events are switched on");

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

        let shot = page
            .call("Page.captureScreenshot", json!({ "format": "png" }))
            .await
            .expect("the page is captured");
        let png = shot["data"].as_str().expect("a capture is base64 text");
        assert!(
            png.starts_with("iVBORw0KGgo"),
            "not a PNG: {}",
            &png[..png.len().min(32)]
        );
        assert!(
            png.len() > 200,
            "an empty capture of a rendered page is {} characters",
            png.len()
        );

        let event = tokio::time::timeout(Duration::from_secs(10), events.recv())
            .await
            .expect("a page that has navigated says something within 10s")
            .expect("the reader is still reading while the target is open");
        assert!(
            !event.method.is_empty(),
            "a frame with no method is not one"
        );

        page.close().await;
    });
}
