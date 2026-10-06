//! The webview one browser tab owns, and the four hooks that make it safe
//! and its chrome truthful. Split from the commands in `browser.rs` because
//! this half runs whatever the frontend asks: the gate below is installed on
//! the webview itself, so it answers navigations no command ever sees.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use tauri::ipc::Channel;
use tauri::Url;
use tauri::{AppHandle, Emitter, Manager, WebviewBuilder, WebviewUrl, Wry};

use super::commands::tabs::{TabEvent, TAB_EVENT};
use super::deadline::Deadline;
use super::destination;
use super::live::Live;
use super::page_host;
use super::registry::{profile_dir, AgentDrive, BrowserRegistry, OwnedTab, PARK_RECT};
use super::tab_reports;
use super::tab_watch::TabWatch;
use super::url;

/// The empty document a child is created on, before it is allowed to load
/// anything. Its only job is to be there for the handlers to be installed on:
/// the real address is navigated to once they are.
pub const BOOTSTRAP_URL: &str = "about:blank";

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

/// What travels down a tab's channel: its page's state changing, the page
/// asking for a window of its own, or a chord pressed while the page itself
/// held the focus and the app's own keymap could not hear it.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum BrowserUpdate {
    State(BrowserViewState),
    /// A `target=_blank` link or a `window.open`, already gated. The frontend
    /// opens a tab for it in the workspace that asked.
    NewWindow {
        url: String,
    },
    Chord {
        chord: BrowserChord,
    },
}

/// The two keys a browser tab answers wherever the focus is. Read down the
/// channel rather than answered here: the app's keymap owns what a chord
/// means, and this half only reports that one was pressed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum BrowserChord {
    FocusAddress,
    Reload,
}

/// The handles a webview's own hooks need: the channel it reports down, the
/// state it edits, and enough of the app to tell the strip about a page no
/// pane is showing. Cloned into each hook, because each takes ownership.
///
/// The channel sits behind a lock because the page outlives any one watcher:
/// a page an agent opened has no pane to report to until the user opens its
/// chip, and adopting that page hands the channel over without rebuilding it.
#[derive(Clone)]
struct TabHooks {
    sink: Arc<Mutex<Channel<BrowserUpdate>>>,
    state: Arc<Mutex<BrowserViewState>>,
    app: AppHandle,
    /// This tab's own id, which the strip's record is keyed by.
    id: String,
}

impl TabHooks {
    fn send(&self, update: BrowserUpdate) {
        let _ = self
            .sink
            .lock()
            .expect("browser sink poisoned")
            .send(update);
    }

    /// Put a note on the pane's own line, the way a refused navigation does.
    /// The next navigation clears it.
    fn send_note(&self, note: &str) {
        self.edit(|state| state.error = Some(note.to_owned()));
    }

    /// Edit and publish. The lock is dropped before the send so a slow
    /// frontend cannot hold the state this thread is about to read.
    fn edit(&self, apply: impl FnOnce(&mut BrowserViewState)) {
        let state = {
            let mut state = self.state.lock().expect("browser state poisoned");
            apply(&mut state);
            state.clone()
        };
        self.send(BrowserUpdate::State(state.clone()));
        self.tell_the_strip(&state);
    }

    /// Tell the strip what this page now says, for a tab no pane is showing.
    ///
    /// A tab an agent opened reports into a channel nothing has adopted yet, so
    /// its chip kept the hostname until the user opened it and the page's title
    /// arrived. The strip is app-lifetime state and the strip is what persists
    /// the record, so this is where a watched-less page's title has to go. It
    /// fires a handful of times per page load — navigation, title, load, icon —
    /// and never carries anything a page's own text.
    fn tell_the_strip(&self, state: &BrowserViewState) {
        let event = TabEvent::State {
            browser_id: self.id.clone(),
            url: state.url.clone(),
            title: state.title.clone(),
            favicon: state.favicon.clone(),
        };
        if let Err(error) = self.app.emit(TAB_EVENT, &event) {
            eprintln!(
                "devboule: the strip was not told about {}: {error}",
                self.id
            );
        }
    }
}

/// Wire the four hooks that make a page safe and its chrome truthful.
fn builder(
    label: &str,
    bootstrap: tauri::Url,
    profile: PathBuf,
    hooks: &TabHooks,
    watch: &TabWatch,
) -> WebviewBuilder<Wry> {
    let on_navigation = hooks.clone();
    let on_title = hooks.clone();
    let on_load = hooks.clone();
    let on_window = hooks.clone();
    let navigation_watch = watch.clone();
    let window_watch = watch.clone();
    let load_label = label.to_owned();

    WebviewBuilder::new(label, WebviewUrl::External(bootstrap))
        // Every browser tab shares one profile, so a login in one is a login in
        // the next. It is this app's own folder, and no page input reaches it.
        .data_directory(profile)
        .on_navigation(move |candidate| {
            // The empty document the child starts on, so that nothing is on
            // the network until its handlers exist. Nothing can ask for
            // anything from it, and refusing it would be refusing our own
            // bootstrap rather than the page's.
            if candidate.as_str() == BOOTSTRAP_URL {
                return true;
            }
            // The gate page-initiated navigation and every redirect pass
            // through, and so does the destination policy whenever the tab is
            // agent-driven. A refusal becomes the inline error line and the
            // page stays put.
            let refusal = match url::gate(candidate) {
                Ok(()) => navigation_watch.blocked(candidate),
                Err(refusal) => Some(refusal.to_string()),
            };
            match refusal {
                None => {
                    on_navigation.edit(|state| {
                        state.url = candidate.to_string();
                        state.loading = true;
                        state.error = None;
                    });
                    true
                }
                Some(refusal) => {
                    on_navigation.edit(|state| {
                        state.error = Some(refusal);
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
            // user cannot see, cannot close and cannot read a url for. The
            // destination is checked exactly as a navigation's would be.
            match url::gate(&candidate) {
                Ok(()) => match window_watch.blocked(&candidate) {
                    None => on_window.send(BrowserUpdate::NewWindow {
                        url: candidate.to_string(),
                    }),
                    Some(refusal) => on_window.edit(|state| state.error = Some(refusal)),
                },
                Err(refusal) => on_window.edit(|state| state.error = Some(refusal.to_string())),
            }
            tauri::webview::NewWindowResponse::Deny
        })
}

/// Open a tab's page and claim its id. The id is claimed BEFORE the child
/// exists so a second create for the same tab cannot slip a second webview in
/// under a label the first one holds; a failed build releases the claim, and a
/// claim a close cancelled while the child was being built closes that child
/// instead of handing back a page nobody asked for.
///
/// The caller hands in the tab's drive: whose tab this is must be decided
/// before its first byte loads, and cannot be said once it exists.
#[allow(clippy::too_many_arguments)]
pub async fn open(
    app: &AppHandle,
    registry: &BrowserRegistry,
    id: &str,
    raw_url: &str,
    workspace: &str,
    updates: Channel<BrowserUpdate>,
    deadline: Deadline,
    drive: Arc<AgentDrive>,
) -> Result<BrowserViewState, String> {
    let target = url::accept(raw_url)?;
    let window = app
        .get_window("main")
        .ok_or_else(|| "The main window is gone.".to_owned())?;
    let local_data = app.path().app_local_data_dir().map_err(|e| e.to_string())?;
    let profile = profile_dir(&local_data);
    std::fs::create_dir_all(&profile).map_err(|e| format!("browser profile: {e}"))?;

    let label = format!("{LABEL_PREFIX}{id}");
    let sink = Arc::new(Mutex::new(updates));
    let state = Arc::new(Mutex::new(BrowserViewState {
        url: target.to_string(),
        loading: true,
        ..BrowserViewState::default()
    }));
    let hooks = TabHooks {
        sink: Arc::clone(&sink),
        state: Arc::clone(&state),
        app: app.clone(),
        id: id.to_owned(),
    };
    let live: Arc<Live> = Arc::default();
    registry.claim(
        id,
        OwnedTab {
            label: label.clone(),
            rect: PARK_RECT,
            live: Arc::clone(&live),
            cancelled: false,
            workspace: workspace.to_owned(),
            state: Arc::clone(&state),
            sink,
            guard: Arc::default(),
            drive: Arc::clone(&drive),
        },
    )?;

    let policy = app
        .try_state::<Arc<destination::DestinationPolicy>>()
        .map(|state| Arc::clone(state.inner()))
        .unwrap_or_else(|| Arc::new(destination::DestinationPolicy::load(None)));
    let watch = TabWatch::new(policy, drive);
    let bootstrap = Url::parse(BOOTSTRAP_URL).expect("the bootstrap URL is a constant");
    let (position, size) = PARK_RECT.into_tauri();
    let webview = match window.add_child(
        builder(&label, bootstrap, profile, &hooks, &watch),
        position,
        size,
    ) {
        Ok(webview) => webview,
        Err(error) => {
            registry.release(id);
            return Err(error.to_string());
        }
    };
    // A close that arrived while this child was being built is the one thing
    // that can leave the id claimed with no page behind it: nobody else is
    // holding the webview this create just got.
    if !registry.claim_is_live(id) {
        let _ = webview.close();
        registry.release(id);
        return Err("This browser tab was closed before it opened.".to_owned());
    }
    #[cfg(windows)]
    super::frame_watch::install(&webview, watch.clone());
    // Every handler this app answers with goes on before the first navigation.
    // A page that asks for the camera in the time between being created and
    // being restricted asked a question this app had not installed a "no" for
    // yet, so an empty document is what it is created on.
    let on_download = hooks.clone();
    let on_chord = hooks.clone();
    if let Err(error) = page_host::restrict(
        &webview,
        move || on_download.send_note("Downloads are not supported yet."),
        move |chord| on_chord.send(BrowserUpdate::Chord { chord }),
    ) {
        let _ = webview.close();
        registry.release(id);
        return Err(error);
    }
    // A parked page lays itself out at two pixels, so it is given the size a
    // pane would show it at while it is still the blank bootstrap, before it
    // has a first layout to get wrong — and told to speak, for the same reason.
    tab_reports::before_loading(app, id, &label, &live, deadline).await;
    if let Err(error) = webview.navigate(target) {
        let _ = webview.close();
        registry.release(id);
        return Err(error.to_string());
    }
    // The page is on the network now; its own event stream is what tells an
    // agent whether the document is still moving, so the subscription is
    // installed here rather than on the first command that needs it.
    let on_move = hooks.clone();
    let reports = tab_reports::reports(app, &label, &live, move |url| {
        // A page that moved its own address — `pushState`, a hash — has no
        // navigation for the webview's hook to see, and the pane, the strip's
        // record and the agent's answers all read this one state.
        on_move.edit(|state| state.url = url);
    });
    super::cdp_events::watch(app, id, &label, deadline, reports).await;
    let opened = state.lock().expect("browser state poisoned").clone();
    Ok(opened)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_bootstrap_is_something_the_gate_would_refuse() {
        // The empty document a child starts on exists so that nothing is on
        // the network before its handlers are installed. That is worth exactly
        // as long as it is not an address a page could reach on its own: an
        // https bootstrap would be loaded before the restriction exists, and
        // this test is what says so.
        let bootstrap = Url::parse(BOOTSTRAP_URL).expect("the bootstrap URL parses");

        assert!(url::gate(&bootstrap).is_err());
    }
}
