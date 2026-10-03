//! The webview one browser tab owns, and the four hooks that make it safe
//! and its chrome truthful. Split from the commands in `browser.rs` because
//! this half runs whatever the frontend asks: the gate below is installed on
//! the webview itself, so it answers navigations no command ever sees.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use tauri::ipc::Channel;
use tauri::{AppHandle, Manager, WebviewBuilder, WebviewUrl, Wry};

use super::page_host;
use super::registry::{profile_dir, BrowserRegistry, OwnedTab, PARK_RECT};
use super::url;

/// The label prefix every browser child webview carries. Tauri needs a
/// distinct label per webview in a window and the frontend needs a stable id
/// per tab that outlives its webview: one is derived from the other, never
/// confused for it.
pub const LABEL_PREFIX: &str = "browser-";

/// What the chrome of one tab reads, and what the frontend keeps for a tab
/// restored after a restart.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserViewState {
    /// The page's current address, after every redirect it survived.
    pub url: String,
    /// The document title, when the page set one. Null leaves the chip on its
    /// hostname fallback.
    pub title: Option<String>,
    pub favicon: Option<String>,
    pub loading: bool,
    pub can_go_back: bool,
    pub can_go_forward: bool,
    /// Why the last navigation was refused, in the words the inline error line
    /// shows.
    pub error: Option<String>,
}

/// What travels down a tab's channel: its page's state changing, or the page
/// asking for a window of its own.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum BrowserUpdate {
    State(BrowserViewState),
    /// A `target=_blank` link or a `window.open`, already gated. The frontend
    /// opens a tab for it in the workspace that asked.
    NewWindow {
        url: String,
    },
}

/// The handles a webview's own hooks need: the channel it reports down and
/// the state it edits. Cloned into each hook, because each takes ownership.
#[derive(Clone)]
struct TabHooks {
    updates: Channel<BrowserUpdate>,
    state: Arc<Mutex<BrowserViewState>>,
}

impl TabHooks {
    fn send(&self, update: BrowserUpdate) {
        let _ = self.updates.send(update);
    }

    /// Edit and publish. The lock is dropped before the send so a slow
    /// frontend cannot hold the state this thread is about to read.
    fn edit(&self, apply: impl FnOnce(&mut BrowserViewState)) {
        let state = {
            let mut state = self.state.lock().expect("browser state poisoned");
            apply(&mut state);
            state.clone()
        };
        self.send(BrowserUpdate::State(state));
    }
}

/// Wire the four hooks that make a page safe and its chrome truthful.
fn builder(
    label: &str,
    target: tauri::Url,
    profile: PathBuf,
    hooks: &TabHooks,
) -> WebviewBuilder<Wry> {
    let on_navigation = hooks.clone();
    let on_title = hooks.clone();
    let on_load = hooks.clone();
    let on_window = hooks.clone();
    let load_label = label.to_owned();

    WebviewBuilder::new(label, WebviewUrl::External(target))
        // Every browser tab shares one profile, so a login in one is a login in
        // the next. It is this app's own folder, and no page input reaches it.
        .data_directory(profile)
        .on_navigation(move |candidate| {
            // The gate page-initiated navigation and every redirect pass through.
            // A refusal becomes the inline error line and the page stays put.
            match url::gate(candidate) {
                Ok(()) => {
                    on_navigation.edit(|state| {
                        state.url = candidate.to_string();
                        state.loading = true;
                        state.error = None;
                    });
                    true
                }
                Err(refusal) => {
                    on_navigation.edit(|state| {
                        state.error = Some(refusal.to_string());
                        state.loading = false;
                    });
                    false
                }
            }
        })
        .on_document_title_changed(move |_, title| {
            on_title.edit(|state| state.title = Some(title));
        })
        .on_page_load(move |app, payload| {
            let finished = payload.event() == tauri::webview::PageLoadEvent::Finished;
            on_load.edit(|state| state.loading = !finished);
            if !finished {
                return;
            }
            // The page is idle: the one moment the history answers and the favicon
            // can be read without racing a load.
            let app = app.app_handle().clone();
            let hooks = on_load.clone();
            let label = load_label.clone();
            tauri::async_runtime::spawn(async move {
                if let Ok(facts) = page_host::facts(&app, &label).await {
                    hooks.edit(|state| {
                        state.can_go_back = facts.can_go_back;
                        state.can_go_forward = facts.can_go_forward;
                        state.favicon = facts.favicon;
                    });
                }
            });
        })
        .on_new_window(move |candidate, _| {
            // Never a native popup: a window this app does not own is a page the
            // user cannot see, cannot close and cannot read a url for.
            match url::gate(&candidate) {
                Ok(()) => on_window.send(BrowserUpdate::NewWindow {
                    url: candidate.to_string(),
                }),
                Err(refusal) => on_window.edit(|state| state.error = Some(refusal.to_string())),
            }
            tauri::webview::NewWindowResponse::Deny
        })
}

/// Open a tab's page and claim its id. The id is claimed BEFORE the child
/// exists so a second create for the same tab cannot slip a second webview in
/// under a label the first one holds; a failed build releases the claim.
pub async fn open(
    app: &AppHandle,
    registry: &BrowserRegistry,
    id: &str,
    raw_url: &str,
    updates: Channel<BrowserUpdate>,
) -> Result<BrowserViewState, String> {
    let target = url::accept(raw_url)?;
    let window = app
        .get_window("main")
        .ok_or_else(|| "The main window is gone.".to_owned())?;
    let local_data = app.path().app_local_data_dir().map_err(|e| e.to_string())?;
    let profile = profile_dir(&local_data);
    std::fs::create_dir_all(&profile).map_err(|e| format!("browser profile: {e}"))?;

    let label = format!("{LABEL_PREFIX}{id}");
    let hooks = TabHooks {
        updates,
        state: Arc::new(Mutex::new(BrowserViewState {
            url: target.to_string(),
            loading: true,
            ..BrowserViewState::default()
        })),
    };
    registry.claim(
        id,
        OwnedTab {
            label: label.clone(),
            rect: PARK_RECT,
            parked: true,
        },
    )?;

    let (position, size) = PARK_RECT.into_tauri();
    let webview = window.add_child(builder(&label, target, profile, &hooks), position, size);
    match webview {
        Ok(webview) => page_host::deny_permissions(&webview),
        Err(error) => {
            registry.release(id);
            return Err(error.to_string());
        }
    }
    let opened = hooks.state.lock().expect("browser state poisoned").clone();
    Ok(opened)
}
