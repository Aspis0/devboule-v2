//! One real Chromium for Testing behind this transport, for what a fake
//! endpoint cannot settle: that a page target's own debugger socket answers
//! the same method literals the WebView2 child answers, that the answers
//! carry a real page's title and real pixels, and that a real page's own
//! events reach the shared ingestion.
//!
//! Ignored, because it needs a browser the machine may not have. Point
//! `DEVBOULE_CFT_CHROME` at one — the win64 Stable of
//! <https://googlechromelabs.github.io/chrome-for-testing/last-known-good-versions-with-downloads.json>
//! — and run it by hand.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};

use super::WsPage;
use crate::browser::cdp::Page;
use crate::browser::cdp_events;
use crate::browser::console::{self, Wanted};
use crate::browser::deadline::Deadline;

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

/// One started browser, its profile, and the debugger address of its page
/// target. The fields drop in order, so the browser goes before the profile
/// directory it has been writing into.
struct Launched {
    _browser: OwnedChrome,
    _profile: tempfile::TempDir,
    address: String,
}

/// Start the Chrome the environment names on one page target of its own.
fn launch() -> Launched {
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
    let browser = OwnedChrome(Some(child));

    let port = debugging_port(profile.path()).expect("the browser publishes its port");
    let address = page_address(port);
    Launched {
        _browser: browser,
        _profile: profile,
        address,
    }
}

/// The debugger address of the browser's page target. The port marker is
/// written before the first page target exists, so this waits for one instead
/// of believing the first list it reads.
fn page_address(port: u16) -> String {
    for _ in 0..300 {
        if let Ok(targets) = page_targets(port) {
            if let Some(address) = targets
                .iter()
                .find(|target| target["type"] == json!("page"))
                .and_then(|target| target["webSocketDebuggerUrl"].as_str())
            {
                return address.to_owned();
            }
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    panic!("no page target with its own debugger address after 30s");
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
    let launched = launch();

    tauri::async_runtime::block_on(async {
        let (page, mut events) = WsPage::connect(&launched.address)
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

        let event = tokio::time::timeout(Duration::from_secs(10), events.state.recv())
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

/// What the page below says on its console and throws: the voice the ring
/// keeps, and nothing else.
const SCRIPT: &str = "console.log('over the socket'); \
                    setTimeout(function () { throw new Error('thrown over the socket'); }, 0)";

/// A page watched for its voice and nothing else: there is no pane here to be
/// told where the tab moved.
fn no_reports() -> cdp_events::Reports {
    cdp_events::Reports {
        within_document: Arc::new(|_| {}),
        committed: Arc::new(|| {}),
    }
}

#[test]
#[ignore = "needs a Chrome for Testing binary: DEVBOULE_CFT_CHROME"]
fn a_real_pages_voice_reaches_the_ring_over_the_socket() {
    let launched = launch();
    let id = "cft-voice";

    tauri::async_runtime::block_on(async {
        let (page, events) = WsPage::connect(&launched.address)
            .await
            .expect("the target is opened");
        let _watch = cdp_events::watch_ws(
            &page,
            id,
            events,
            Deadline::in_(Duration::from_secs(10)),
            no_reports(),
        )
        .await;

        page.call("Runtime.evaluate", json!({ "expression": SCRIPT }))
            .await
            .expect("the page runs the expression");

        let mut entries = Vec::new();
        for _ in 0..40 {
            entries = console::entries(id, Wanted::All, None).0;
            if entries
                .iter()
                .any(|entry| entry.source.as_deref() == Some("exception"))
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert!(
            entries.iter().any(|entry| entry.text == "over the socket"),
            "the console call did not reach the ring: {entries:?}"
        );
        assert!(
            entries.iter().any(|entry| entry
                .source
                .as_deref()
                .is_some_and(|source| source == "exception")
                && entry.text.contains("thrown over the socket")),
            "the throw did not reach the ring: {entries:?}"
        );

        page.close().await;
    });
    cdp_events::forget(id);
}
