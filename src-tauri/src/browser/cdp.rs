//! One DevTools Protocol call on one browser child's own WebView2, and the
//! two traps this runtime sets for anyone who calls it.
//!
//! **A failed call names nothing.** `E_INVALIDARG` (0x80070057) is what an
//! unknown method, a misspelled argument, a dead `backendDOMNodeId` and a bad
//! `objectId` each answer with. The only thing that tells them apart is the
//! call that was sent, so [`call`] classifies it: a refusal of a call whose
//! parameters named a node is a stale ref, and any other refusal is the
//! method or argument problem it is.
//!
//! **A parked page believes it is two pixels wide.** A parked child is 1x1 at
//! (-30000, -30000), so its layout viewport is 0x0, its box models are empty
//! and a dispatched click reaches nothing. [`present_for`] overrides the
//! device metrics with the size the page was last shown at, which is one call
//! and the difference between an action that works and one that silently does
//! nothing.

use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use serde_json::{json, Value};

use super::deadline::Deadline;
use super::registry::Size;

/// A call in flight, so the layer above it never sees a lifetime.
pub type Call<'a> = Pin<Box<dyn Future<Output = Result<Value, CdpError>> + Send + 'a>>;

/// Why a call did not answer. Only [`CdpError::StaleRef`] reaches a caller as
/// its own advice; everything else is a host failure carrying the runtime's
/// own text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CdpError {
    /// The ref named a node this page no longer has. The advice is always the
    /// same: take a new snapshot. A ref is never remapped to another node.
    StaleRef,
    Refused(String),
}

impl std::fmt::Display for CdpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message())
    }
}

impl CdpError {
    pub fn message(&self) -> String {
        match self {
            CdpError::StaleRef => {
                "stale_ref: that node is gone from the page; take a new snapshot".to_owned()
            }
            CdpError::Refused(text) => text.clone(),
        }
    }
}

/// The page one set of commands is about. Written against this, the command
/// layer runs on a WebView2 in the app and on canned answers in a test.
pub trait Page: Send + Sync {
    fn call<'a>(&'a self, method: &'a str, params: Value) -> Call<'a>;

    /// The same call, waiting no longer than `limit` for the answer. A page
    /// that answers at once, as a canned one does, has nothing to cut short.
    fn call_within<'a>(&'a self, method: &'a str, params: Value, _limit: Duration) -> Call<'a> {
        self.call(method, params)
    }
}

/// A page that answers only while its command still has time, and never waits
/// longer than what is left. Every command runs on one of these, so no call it
/// makes can outlive the command's budget.
pub struct Bounded<'p> {
    page: &'p dyn Page,
    deadline: Deadline,
}

impl<'p> Bounded<'p> {
    pub fn new(page: &'p dyn Page, deadline: Deadline) -> Self {
        Bounded { page, deadline }
    }
}

impl Page for Bounded<'_> {
    fn call<'a>(&'a self, method: &'a str, params: Value) -> Call<'a> {
        self.call_within(method, params, self.deadline.left())
    }

    fn call_within<'a>(&'a self, method: &'a str, params: Value, limit: Duration) -> Call<'a> {
        let left = self.deadline.left().min(limit);
        if left.is_zero() {
            return Box::pin(async move {
                Err(CdpError::Refused(format!(
                    "{method}: the command ran out of time"
                )))
            });
        }
        self.page.call_within(method, params, left)
    }
}

/// Give a parked page the layout it would have if it were on screen, at the
/// size its pane last used.
///
/// Override before measuring, never after: a responsive page re-lays out when
/// the metrics move, so a box model taken before the call is measured against
/// a document that no longer exists. Clearing the override does not put the
/// 1x1 layout back — only presenting the tab does, and a navigation drops it
/// on its way — so this never tries to undo it.
pub async fn present_for(page: &dyn Page, parked: bool, size: Size) -> Result<(), CdpError> {
    if !parked {
        return Ok(());
    }
    page.call(
        "Emulation.setDeviceMetricsOverride",
        json!({
            "width": size.width.round(),
            "height": size.height.round(),
            "deviceScaleFactor": 1,
            "mobile": false,
        }),
    )
    .await
    .map(|_| ())
}

/// Whether a call's parameters named a node, which is what makes a refusal of
/// it a stale ref rather than a misspelled argument.
fn addresses_node(params: &Value) -> bool {
    params
        .as_object()
        .is_some_and(|params| params.contains_key("backendNodeId"))
}

#[cfg(windows)]
mod imp {
    use super::{addresses_node, CdpError, Page};
    use std::sync::mpsc;
    use std::time::Duration;

    use serde_json::Value;
    use tauri::{AppHandle, Manager};
    use webview2_com::CallDevToolsProtocolMethodCompletedHandler;
    use windows::core::HSTRING;

    /// Every WebView2 completion arrives through the UI thread's message
    /// pump, so waiting past this is waiting on a thread that is already gone.
    const CALL_TIMEOUT: Duration = Duration::from_secs(10);

    /// `E_INVALIDARG`: the one HRESULT a bad method, a bad argument and a dead
    /// node all answer with on this runtime.
    const INVALID_ARG: i64 = 0x8007_0057u32 as i64;

    fn classify(error: windows::core::Error, method: &str, named_node: bool) -> CdpError {
        if named_node && error.code().0 as i64 == INVALID_ARG {
            return CdpError::StaleRef;
        }
        CdpError::Refused(format!("{method}: {error}"))
    }

    pub(super) async fn call_on(
        app: &AppHandle,
        label: &str,
        method: &str,
        params: Value,
        limit: Duration,
    ) -> Result<Value, CdpError> {
        let webview = app
            .get_webview(label)
            .ok_or_else(|| CdpError::Refused("This browser tab is no longer open.".to_owned()))?;
        let named_node = addresses_node(&params);
        let method = method.to_owned();
        // The closure owns the name; this copy is what the messages below are
        // written with, once the closure has taken it.
        let called = method.clone();
        let (tx, rx) = mpsc::channel();
        let err_tx = tx.clone();
        webview
            .with_webview(move |pw| {
                let outcome = (|| {
                    let core = unsafe { pw.controller().CoreWebView2() }
                        .map_err(|error| classify(error, &method, named_node))?;
                    let body = HSTRING::from(params.to_string());
                    let named = method.clone();
                    let handler = CallDevToolsProtocolMethodCompletedHandler::create(Box::new(
                        move |hr, result| {
                            let _ = tx.send(
                                hr.map(|()| result)
                                    .map_err(|error| classify(error, &named, named_node)),
                            );
                            Ok(())
                        },
                    ));
                    let called = HSTRING::from(method.as_str());
                    let outcome =
                        unsafe { core.CallDevToolsProtocolMethod(&called, &body, &handler) };
                    outcome.map_err(|error| classify(error, &method, named_node))
                })();
                if let Err(error) = outcome {
                    let _ = err_tx.send(Err(error));
                }
            })
            .map_err(|error| CdpError::Refused(format!("{called}: {error}")))?;
        match rx.recv_timeout(limit.min(CALL_TIMEOUT)) {
            Ok(Ok(body)) => Ok(serde_json::from_str(&body).unwrap_or(Value::Null)),
            Ok(Err(error)) => Err(error),
            Err(_) => Err(CdpError::Refused(format!(
                "{called}: the page did not answer"
            ))),
        }
    }

    /// The real page: one child webview, addressed by the label the registry
    /// holds for its tab.
    pub struct WebviewPage {
        app: AppHandle,
        label: String,
    }

    impl WebviewPage {
        pub fn new(app: &AppHandle, label: &str) -> Self {
            WebviewPage {
                app: app.clone(),
                label: label.to_owned(),
            }
        }
    }

    impl Page for WebviewPage {
        fn call<'a>(&'a self, method: &'a str, params: Value) -> super::Call<'a> {
            self.call_within(method, params, CALL_TIMEOUT)
        }

        fn call_within<'a>(
            &'a self,
            method: &'a str,
            params: Value,
            limit: Duration,
        ) -> super::Call<'a> {
            let app = self.app.clone();
            let label = self.label.clone();
            let method = method.to_owned();
            Box::pin(async move { call_on(&app, &label, &method, params, limit).await })
        }
    }
}

#[cfg(not(windows))]
mod imp {
    use super::{CdpError, Page};
    use serde_json::Value;
    use std::sync::Arc;
    use std::sync::Mutex;
    use tauri::AppHandle;

    /// The address of a page on a target that has no browser page: kept so a
    /// command layer compiles and refuses by name everywhere.
    pub struct WebviewPage {
        _app: AppHandle,
        _label: Arc<Mutex<String>>,
    }

    impl WebviewPage {
        pub fn new(app: &AppHandle, label: &str) -> Self {
            WebviewPage {
                _app: app.clone(),
                _label: Arc::new(Mutex::new(label.to_owned())),
            }
        }
    }

    impl Page for WebviewPage {
        fn call<'a>(&'a self, method: &'a str, _params: Value) -> super::Call<'a> {
            Box::pin(async move {
                Err(CdpError::Refused(format!(
                    "{method} is not available on this platform."
                )))
            })
        }
    }
}

pub use imp::WebviewPage;

#[cfg(test)]
#[path = "cdp_tests.rs"]
mod tests;
