//! What a tab does about the size it is laid out at, and about where its page
//! says it is, from the moment it is created.
//!
//! A tab an agent opens is parked: 1x1, far outside the window, which a page
//! sees as a two-pixel viewport. A responsive page laid out at that size is the
//! page's mobile layout — its navigation collapsed into a menu button, its
//! controls off screen — and an override applied only when the first command
//! runs arrives after the page has already built itself that way. So the
//! override goes on before the page's first navigation, and again whenever the
//! tab's own frame commits a document, which drops it.

use std::sync::Arc;

use tauri::AppHandle;

use super::cdp::{self, Bounded, WebviewPage};
use super::cdp_events::{self, Reports};
use super::deadline::Deadline;
use super::live::Live;

/// Lay a freshly built, still-blank page out at the size a pane would show it,
/// and turn on the events it will speak through. Both are asked for before the
/// page loads anything: the layout because a responsive page builds itself from
/// it, the domains because a console entry written before the first navigation
/// is one of the ones an agent cannot get back later.
///
/// A failure is reported and nothing else: the page loads, at the size it has.
pub async fn before_loading(
    app: &AppHandle,
    id: &str,
    label: &str,
    live: &Live,
    deadline: Deadline,
) {
    let webview = WebviewPage::new(app, label);
    let page = Bounded::new(&webview, deadline);
    if let Err(error) = cdp::present_for(&page, live, live.size()).await {
        eprintln!("devboule: browser page {id} was not given its layout size: {error}");
    }
    cdp_events::listen(&page).await;
}

/// What the tab does when its page speaks: a move of the address within the
/// document is `within_document`'s, and a new document gets the override back
/// while the tab is still parked. The override is applied off the event's
/// thread: the event arrives on the UI thread and a call waits for it.
pub fn reports(
    app: &AppHandle,
    label: &str,
    live: &Arc<Live>,
    within_document: impl Fn(String) + Send + Sync + 'static,
) -> Reports {
    let app = app.clone();
    let label = label.to_owned();
    let live = Arc::clone(live);
    Reports {
        within_document: Arc::new(within_document),
        committed: Arc::new(move || {
            if !live.parked() {
                return;
            }
            let (app, label, live) = (app.clone(), label.clone(), Arc::clone(&live));
            tauri::async_runtime::spawn(async move {
                let page = WebviewPage::new(&app, &label);
                // Whether it took is the page's business: the next command
                // applies it again, and a refusal here changes nothing else.
                let _ = cdp::present_for(&page, &live, live.size()).await;
            });
        }),
    }
}
