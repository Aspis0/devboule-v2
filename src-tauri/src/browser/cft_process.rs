//! The app-owned Chrome for Testing process: one per app, never by name.
//!
//! This slice is an unwired library: nothing outside these tests constructs a
//! [`CftBrowser`]. There is no supervisor loop and no app-exit hook here — a
//! caller that wants the browser back constructs a new one, and [`restart_delay`]
//! is policy for the loop slice 4 owns. [`CftBrowser`]'s `Drop` is only the
//! fallback for a child that [`CftBrowser::shutdown`] never reached.
//!
//! UNVERIFIED on macOS beyond compiling: headless launch, port read and the
//! websocket handshake are exercised on Windows here; the headed window and
//! Retina mapping are not.
//!
//! Ownership is the exact [`std::process::Child`] handle plus the pid, exe
//! path and launch nonce recorded at spawn, and the exclusive profile lock
//! held for the browser's life. The debugger endpoint is only used after the
//! browser behind it proved the pid is this child's. Stopping is graceful
//! `Browser.close` over the slice-1 websocket, then killing THAT child after
//! a timeout. Nothing here kills by image name: the owner's own Chrome runs
//! on this machine.

#![cfg_attr(not(test), allow(dead_code))]

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use serde_json::json;

use super::cdp::Page;
use super::cdp_ws::{WsEvents, WsPage};
use super::cft_endpoint::{browser_ws_url, page_ws_url, read_devtools_port};
use super::cft_lock::ResourceLock;

/// Why the browser did not start, answer, or stop.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CftError {
    Launch(String),
    Protocol(String),
}

impl std::fmt::Display for CftError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CftError::Launch(text) => write!(f, "{text}"),
            CftError::Protocol(text) => write!(f, "{text}"),
        }
    }
}

/// How long a fresh launch may take to publish its port.
const LAUNCH_WAIT: Duration = Duration::from_secs(30);

/// How long `shutdown` waits for `Browser.close` to reap the child before it
/// kills THAT handle.
const CLOSE_WAIT: Duration = Duration::from_secs(5);

/// First restart pause and ceiling. Doubling, capped: a browser that dies at
/// once must not spin the app, and a browser that stays up resets the count
/// in the supervisor (slice 4 owns the loop; this owns the policy).
const BACKOFF_FIRST: Duration = Duration::from_secs(1);
const BACKOFF_MAX: Duration = Duration::from_secs(30);

/// One launch nonce per process in this app run. No entropy needed: uniqueness
/// within the run is what tells two launches apart in diagnostics.
static NONCE: AtomicU64 = AtomicU64::new(1);

/// The one browser this app started.
pub(crate) struct CftBrowser {
    child: Option<Child>,
    pid: u32,
    exe: PathBuf,
    nonce: u64,
    port: u16,
    browser_ws: String,
    profile: PathBuf,
    /// Released last, when the struct's fields drop: the profile has exactly
    /// one owner for as long as a browser lives on it.
    _profile_lock: ResourceLock,
}

impl CftBrowser {
    /// Start the installed executable with the shared profile. Headless only
    /// behind the test flag: the visible pane runs headed (slice 4 proves the
    /// compositor frames there).
    pub(crate) async fn launch(
        exe: &Path,
        app_data: &Path,
        headless: bool,
    ) -> Result<Self, CftError> {
        let profile = app_data.join("browser-profile");
        std::fs::create_dir_all(&profile)
            .map_err(|error| CftError::Launch(format!("{}: {error}", profile.display())))?;
        // Taken before the marker is touched or a child is spawned: a second
        // instance is refused here instead of reading the first one's port.
        let profile_lock = ResourceLock::acquire(&profile.join("devboule-browser.lock"))
            .map_err(CftError::Launch)?;
        // A previous launch's marker would hand back a dead port: the new
        // browser is the only one allowed to write it.
        let _ = std::fs::remove_file(profile.join("DevToolsActivePort"));
        let args = chrome_args(&profile, headless);
        let mut child = Command::new(exe)
            .args(&args)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|error| CftError::Launch(format!("{}: {error}", exe.display())))?;
        let pid = child.id();
        // The wait is a sleep-and-poll loop of up to `LAUNCH_WAIT`; it runs on
        // a blocking worker so a launch inside the app's runtime does not
        // hold a runtime thread for half a minute.
        let waited = profile.clone();
        let port = match tauri::async_runtime::spawn_blocking(move || {
            read_devtools_port(&waited, Instant::now() + LAUNCH_WAIT)
        })
        .await
        {
            Ok(Ok(port)) => port,
            Ok(Err(error)) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(CftError::Launch(format!(
                    "the browser did not publish its port: {error}"
                )));
            }
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(CftError::Launch(format!("the launch wait ended: {error}")));
            }
        };
        let browser_ws = browser_ws_url(port).map_err(|error| {
            let _ = child.kill();
            let _ = child.wait();
            CftError::Launch(format!("the browser did not answer /json/version: {error}"))
        })?;
        // `/json/version` says what the port answers, not whose process is
        // behind it. The launch counts only if the browser names THIS child.
        if let Err(error) = verify_owner(&browser_ws, pid).await {
            let _ = child.kill();
            let _ = child.wait();
            return Err(CftError::Launch(format!(
                "the debugger endpoint is not this app's browser: {error}"
            )));
        }
        Ok(CftBrowser {
            child: Some(child),
            pid,
            exe: exe.to_owned(),
            nonce: NONCE.fetch_add(1, Ordering::Relaxed),
            port,
            browser_ws,
            profile,
            _profile_lock: profile_lock,
        })
    }

    /// Open a page target at `url` and connect the slice-1 transport to it.
    /// This is the one handoff slice 4 needs.
    pub(crate) async fn open_page(&self, url: &str) -> Result<(WsPage, WsEvents), CftError> {
        let (browser, _) = self.connect_owned().await?;
        let created = match browser
            .call("Target.createTarget", json!({ "url": url }))
            .await
        {
            Ok(created) => created,
            Err(error) => {
                let text = error.to_string();
                browser.close().await;
                return Err(CftError::Protocol(text));
            }
        };
        let Some(target_id) = created["targetId"].as_str() else {
            browser.close().await;
            return Err(CftError::Protocol(format!(
                "Target.createTarget answered {created}"
            )));
        };
        let target_id = target_id.to_owned();
        browser.close().await;
        // The target list may trail the creation by a moment; poll briefly.
        let started = Instant::now();
        loop {
            match page_ws_url(self.port, &target_id) {
                Ok(address) => {
                    return WsPage::connect(&address)
                        .await
                        .map_err(|error| CftError::Protocol(error.to_string()));
                }
                Err(_) if started.elapsed() < Duration::from_secs(5) => {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
                Err(error) => return Err(CftError::Protocol(error.to_string())),
            }
        }
    }

    /// Connect to the browser's debugger, refusing it unless it names this
    /// child as its `browser` process. A refusal sends no other command.
    async fn connect_owned(&self) -> Result<(WsPage, WsEvents), CftError> {
        let (browser, events) = WsPage::connect(&self.browser_ws)
            .await
            .map_err(|error| CftError::Protocol(error.to_string()))?;
        if let Err(text) = check_process_owner(&browser, self.pid).await {
            browser.close().await;
            return Err(CftError::Protocol(format!(
                "refusing a debugger that is not this app's browser: {text}"
            )));
        }
        Ok((browser, events))
    }

    /// Graceful `Browser.close`, then kill THAT child after a timeout. Never
    /// by name: the handle recorded at launch is the only process touched.
    /// One-shot by construction — it takes `self` — and the handle it reaps
    /// is gone before `Drop` runs, so a pid that was later reused is never
    /// signalled again.
    pub(crate) async fn shutdown(mut self) {
        if let Ok((browser, _)) = self.connect_owned().await {
            let _ = browser.call("Browser.close", json!({})).await;
            browser.close().await;
        }
        let deadline = Instant::now() + CLOSE_WAIT;
        while Instant::now() < deadline
            && self
                .child
                .as_mut()
                .is_some_and(|child| child.try_wait().is_ok_and(|status| status.is_none()))
        {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        kill_and_reap(&mut self.child);
    }

    /// Whether the owned child is still running.
    fn is_alive(&mut self) -> bool {
        self.child
            .as_mut()
            .is_some_and(|child| matches!(child.try_wait(), Ok(None)))
    }

    /// The pid recorded at launch, for diagnostics only. Never used to stop.
    fn pid(&self) -> u32 {
        self.pid
    }
}

impl Drop for CftBrowser {
    fn drop(&mut self) {
        kill_and_reap(&mut self.child);
    }
}

/// Kill and reap the exact child behind the handle and forget it. `Drop`
/// calls this when `shutdown` did not complete, so no browser outlives the
/// value that owns it.
fn kill_and_reap(child: &mut Option<Child>) {
    if let Some(child) = child.as_mut() {
        let _ = child.kill();
        let _ = child.wait();
    }
    *child = None;
}

/// Pause before restart attempt `attempt` (0-based): doubling, capped.
pub(crate) fn restart_delay(attempt: u32) -> Duration {
    let shift = attempt.min(5);
    let secs = BACKOFF_FIRST.as_secs().saturating_mul(1 << shift);
    Duration::from_secs(secs.min(BACKOFF_MAX.as_secs()))
}

/// The exact arguments a launch uses. `--no-sandbox` is never among them.
fn chrome_args(profile: &Path, headless: bool) -> Vec<String> {
    let mut args = Vec::new();
    if headless {
        args.push("--headless=new".to_owned());
    }
    args.push("--remote-debugging-port=0".to_owned());
    args.push(format!("--user-data-dir={}", profile.display()));
    args.push("--no-first-run".to_owned());
    args.push("--no-default-browser-check".to_owned());
    args.push("about:blank".to_owned());
    args
}

/// Prove the debugger at `address` is `expected_pid`'s browser before it is
/// used: connect, ask `SystemInfo.getProcessInfo`, close. Nothing else is
/// sent on a refusal.
async fn verify_owner(address: &str, expected_pid: u32) -> Result<(), String> {
    let (browser, _) = WsPage::connect(address)
        .await
        .map_err(|error| error.to_string())?;
    let verdict = check_process_owner(&browser, expected_pid).await;
    browser.close().await;
    verdict
}

/// Whether the browser behind an open debugger socket names `expected_pid`
/// as its own `browser` process.
async fn check_process_owner(browser: &WsPage, expected_pid: u32) -> Result<(), String> {
    let answer = browser
        .call("SystemInfo.getProcessInfo", json!({}))
        .await
        .map_err(|error| error.to_string())?;
    let found = answer["processInfo"]
        .as_array()
        .and_then(|entries| {
            entries
                .iter()
                .find(|entry| entry["type"] == json!("browser"))
        })
        .and_then(|entry| entry["id"].as_u64());
    match found {
        Some(pid) if pid == u64::from(expected_pid) => Ok(()),
        Some(pid) => Err(format!(
            "the debugger belongs to process {pid}, not this app's {expected_pid}"
        )),
        None => Err("the debugger named no browser process".to_owned()),
    }
}

#[cfg(test)]
impl CftBrowser {
    /// The real type, with a real child and a real profile lock, but no
    /// debugger behind `browser_ws`: the lifecycle tests need `Drop` and
    /// `shutdown` without an installed browser.
    fn for_test(child: Child, profile_lock: ResourceLock) -> Self {
        CftBrowser {
            pid: child.id(),
            child: Some(child),
            exe: PathBuf::new(),
            nonce: 0,
            port: 1,
            browser_ws: "ws://192.0.2.1:9/x".to_owned(),
            profile: PathBuf::new(),
            _profile_lock: profile_lock,
        }
    }
}

#[cfg(test)]
#[path = "cft_process_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "cft_process_cft_tests.rs"]
mod cft;
