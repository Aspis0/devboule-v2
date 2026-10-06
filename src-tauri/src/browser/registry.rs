//! Which child webviews this app owns, and the one profile directory they
//! share. The registry is the only place that knows a browser id maps to a
//! webview label, so every command that touches a page resolves through it
//! and a disposed tab resolves to nothing at all.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use tauri::ipc::Channel;
use tauri::{LogicalPosition, LogicalSize};

use super::live::Live;
use super::tab::{BrowserUpdate, BrowserViewState};

/// The folder name under the app's local data directory that holds every
/// browser page's cookies, cache and storage. It is named here and nowhere
/// else, and no page input ever reaches it.
const PROFILE_DIR: &str = "browser-profile";

/// The one browser profile: a folder of this app's own, apart from the data
/// the app UI writes, so a page can never read what the app stores and every
/// browser tab sees the same logins.
pub(super) fn profile_dir(app_local_data_dir: &Path) -> PathBuf {
    app_local_data_dir.join(PROFILE_DIR)
}

/// A parked page: 1x1, far outside the window. Not `hide()` — a hidden
/// WebView2 throttles the page's timers to about 1 Hz, and a parked tab is
/// supposed to keep running (its agent tools read it live).
pub const PARK_RECT: LogicalRect = LogicalRect {
    x: -30_000.0,
    y: -30_000.0,
    width: 1.0,
    height: 1.0,
};

/// A rectangle in Tauri logical pixels. The main webview's CSS pixels ARE
/// this unit (both are physical pixels over 96 dpi), so `BrowserTab.tsx`
/// passes the `getBoundingClientRect()` values straight through; the type
/// exists so the unit is named where it is defined and nowhere guessed.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LogicalRect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl LogicalRect {
    pub(super) fn into_tauri(self) -> (LogicalPosition<f64>, LogicalSize<f64>) {
        (
            tauri::LogicalPosition::new(self.x, self.y),
            tauri::LogicalSize::new(self.width, self.height),
        )
    }
}

/// One owned page: the webview label it lives under, where it was last put,
/// and the three things an agent command needs about it — which workspace it
/// belongs to, what the page last reported, and where to report next.
pub struct OwnedTab {
    pub label: String,
    pub rect: LogicalRect,
    /// Whether the page is parked and whether an override is on it, shared
    /// with every command that addresses the page.
    pub live: Arc<Live>,
    /// A close that arrived before the child existed. A claim is made before
    /// `add_child` runs, so this is the one thing a close can leave behind
    /// that the create itself has to act on.
    pub cancelled: bool,
    /// The workspace whose agent may address this page. A tab of another
    /// workspace is not addressable, exactly as an unknown id is not.
    pub workspace: String,
    /// The page's last reported state, shared with the hooks that own it: what
    /// `list_tabs`, `snapshot` and every delta read without asking the page.
    pub state: Arc<Mutex<BrowserViewState>>,
    /// Where the page's reports go. A page an agent opened has none until a
    /// pane adopts it, which is what hands the channel over.
    pub sink: Arc<Mutex<Channel<BrowserUpdate>>>,
    /// Held by whatever changes this page or where it is shown: an agent's
    /// acting command, and the pane's present, park and close. Two of those at
    /// once is a click measured against a layout the pane has just replaced.
    pub guard: TabGuard,
    /// Who is driving this page, and the last navigation the destination
    /// policy refused on it.
    pub drive: Arc<AgentDrive>,
}

/// One tab's lock, owned so a command can hold it across its own awaits.
pub type TabGuard = Arc<tauri::async_runtime::Mutex<()>>;

/// One page's driving state: how many agent commands are on it, and the last
/// navigation the policy refused. The webview's own hook runs on the thread
/// that is showing the page, so it reads both without waiting for anything.
#[derive(Default)]
pub struct AgentDrive {
    in_flight: AtomicUsize,
    refusal: Mutex<Option<String>>,
}

impl AgentDrive {
    /// Count an agent command for as long as the guard lives.
    pub fn begin(self: &Arc<Self>) -> DriveGuard {
        self.in_flight.fetch_add(1, Ordering::SeqCst);
        DriveGuard(Arc::clone(self))
    }

    pub fn in_flight(&self) -> bool {
        self.in_flight.load(Ordering::SeqCst) > 0
    }

    pub fn note_refusal(&self, reason: String) {
        *self.refusal.lock().expect("browser drive poisoned") = Some(reason);
    }

    /// The refusal waiting for the next tool result, if one is waiting.
    pub fn take_refusal(&self) -> Option<String> {
        self.refusal.lock().expect("browser drive poisoned").take()
    }
}

/// One agent command's hold on a tab's driving state.
pub struct DriveGuard(Arc<AgentDrive>);

impl Drop for DriveGuard {
    fn drop(&mut self) {
        self.0.in_flight.fetch_sub(1, Ordering::SeqCst);
    }
}

/// A width and a height, in the pane's logical pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Size {
    pub width: f64,
    pub height: f64,
}

/// The last presented size for a page that has never been presented: what an
/// agent measures a parked page against, so a responsive document lays out
/// for something a person could read.
pub const DEFAULT_PRESENTED: Size = Size {
    width: 1280.0,
    height: 800.0,
};

/// What one tab is, read once by whoever is about to address its page.
#[derive(Debug, Clone)]
pub struct TabInfo {
    pub browser_id: String,
    pub label: String,
    pub workspace: String,
    pub url: String,
    pub title: Option<String>,
    /// Whether the page is parked, and therefore whether measuring it needs a
    /// device-metrics override first. A handle and not a copy: it is read again
    /// when the override is about to be applied, because the pane may have
    /// presented the page since this was taken.
    pub live: Arc<Live>,
    /// The size to measure against: the last presented one, or the default.
    pub size: Size,
    pub state: Arc<Mutex<BrowserViewState>>,
}

/// Every browser page this process owns, keyed by the browser id the frontend
/// minted. The id is the frontend's tab id, never the webview label: a label
/// has to satisfy Tauri and a restored tab has to come back with the id the
/// strip persisted.
pub struct BrowserRegistry {
    tabs: Mutex<HashMap<String, OwnedTab>>,
}

impl BrowserRegistry {
    pub fn new() -> Self {
        BrowserRegistry {
            tabs: Mutex::new(HashMap::new()),
        }
    }

    /// The label a browser id owns, or the refusal a command reports when the
    /// tab is gone. Every command starts here, which is what makes "no
    /// navigation after dispose" a property of the seam rather than of each
    /// command remembering to check. A cancelled claim resolves to nothing:
    /// the tab was closed, so there is no page to address.
    pub fn label_of(&self, id: &str) -> Result<String, String> {
        self.tabs
            .lock()
            .expect("browser registry poisoned")
            .get(id)
            .filter(|tab| !tab.cancelled)
            .map(|tab| tab.label.clone())
            .ok_or_else(|| "This browser tab is no longer open.".to_owned())
    }

    /// Claim an id for a freshly built webview. An id already owned is a
    /// refusal rather than a replacement: the caller is about to add a second
    /// webview under a label the first one still holds.
    pub fn claim(&self, id: &str, tab: OwnedTab) -> Result<(), String> {
        let mut tabs = self.tabs.lock().expect("browser registry poisoned");
        if tabs.contains_key(id) {
            return Err("This browser tab is already open.".to_owned());
        }
        tabs.insert(id.to_owned(), tab);
        Ok(())
    }

    /// Where the page was last put, and whether it is parked. A cancelled
    /// claim has no page to place.
    pub fn rect_of(&self, id: &str) -> Option<(LogicalRect, bool)> {
        self.tabs
            .lock()
            .expect("browser registry poisoned")
            .get(id)
            .filter(|tab| !tab.cancelled)
            .map(|tab| (tab.rect, tab.live.parked()))
    }

    /// Record where the page was put. The answer is whether the page was
    /// presented with a device-metrics override still on it, which the caller
    /// must clear: only presenting restores the real geometry.
    pub fn set_rect(&self, id: &str, rect: LogicalRect, parked: bool) -> bool {
        let mut tabs = self.tabs.lock().expect("browser registry poisoned");
        let Some(tab) = tabs.get_mut(id).filter(|tab| !tab.cancelled) else {
            return false;
        };
        {
            if !parked {
                // The last real size, kept past the park: a parked page is
                // measured against what the pane last showed it at.
                tab.live.set_size(Size {
                    width: rect.width,
                    height: rect.height,
                });
            }
            tab.rect = rect;
            tab.live.set_parked(parked);
        }
        !parked && tab.live.take_overridden()
    }

    /// Everything one addressable tab is, or None when the id is unknown,
    /// closed, or still being created. The workspace is carried out of here
    /// so the scoping check is one comparison at the seam.
    pub fn tab_of(&self, id: &str) -> Option<TabInfo> {
        let tabs = self.tabs.lock().expect("browser registry poisoned");
        let tab = tabs.get(id).filter(|tab| !tab.cancelled)?;
        let state = tab.state.lock().expect("browser state poisoned");
        Some(TabInfo {
            browser_id: id.to_owned(),
            label: tab.label.clone(),
            workspace: tab.workspace.clone(),
            url: state.url.clone(),
            title: state.title.clone(),
            live: Arc::clone(&tab.live),
            size: tab.live.size(),
            state: Arc::clone(&tab.state),
        })
    }

    /// The lock that serializes everything acting on one tab's page, or None
    /// when the tab is unknown, closed, or still being created.
    pub fn guard_of(&self, id: &str) -> Option<TabGuard> {
        self.tabs
            .lock()
            .expect("browser registry poisoned")
            .get(id)
            .filter(|tab| !tab.cancelled)
            .map(|tab| Arc::clone(&tab.guard))
    }

    /// One tab's driving state, read by the agent path and by the page's own
    /// navigation hook.
    pub fn drive_of(&self, id: &str) -> Option<Arc<AgentDrive>> {
        self.tabs
            .lock()
            .expect("browser registry poisoned")
            .get(id)
            .filter(|tab| !tab.cancelled)
            .map(|tab| Arc::clone(&tab.drive))
    }

    /// Hand a page's reports to a new watcher and answer with the state the
    /// page has already reported. None when there is no page to adopt, which
    /// is the caller's cue to open one.
    ///
    /// The pane that adopts an agent-opened page has to find the page that is
    /// already there: a second create for a live id is refused, so without
    /// this the user could only ever look at a page the agent had closed.
    pub fn attach(&self, id: &str, updates: Channel<BrowserUpdate>) -> Option<BrowserViewState> {
        let (sink, state) = {
            let tabs = self.tabs.lock().expect("browser registry poisoned");
            let tab = tabs.get(id).filter(|tab| !tab.cancelled)?;
            (Arc::clone(&tab.sink), Arc::clone(&tab.state))
        };
        // The page's own lock is taken with the registry's released: a report
        // arriving at this moment must not be able to hold the registry while
        // it waits for a channel.
        *sink.lock().expect("browser sink poisoned") = updates;
        let reported = state.lock().expect("browser state poisoned").clone();
        Some(reported)
    }

    /// Every live tab of one workspace. `browserId`s are UUIDs, so the map's
    /// own order is the only order there is to report them in.
    ///
    /// The ids are collected before each one is read: the registry lock is not
    /// reentrant, and a tab is read through [`Self::tab_of`].
    pub fn tabs_of(&self, workspace: &str) -> Vec<TabInfo> {
        let ids: Vec<String> = self
            .tabs
            .lock()
            .expect("browser registry poisoned")
            .iter()
            .filter(|(_, tab)| !tab.cancelled && tab.workspace == workspace)
            .map(|(id, _)| id.clone())
            .collect();
        ids.iter().filter_map(|id| self.tab_of(id)).collect()
    }

    /// A close that arrived before the child existed. Only the create can
    /// dispose of what it is about to get, so this leaves a mark instead of
    /// pretending the tab was never there. The bool says whether a create was
    /// in flight to mark.
    pub fn cancel(&self, id: &str) -> bool {
        let mut tabs = self.tabs.lock().expect("browser registry poisoned");
        match tabs.get_mut(id) {
            Some(tab) => {
                tab.cancelled = true;
                true
            }
            None => false,
        }
    }

    /// Whether a create that has just built its child still owns the id. A
    /// close that arrived in the meantime either cancelled the claim or
    /// released it, and in both cases the child it is holding is nobody's.
    pub fn claim_is_live(&self, id: &str) -> bool {
        self.tabs
            .lock()
            .expect("browser registry poisoned")
            .get(id)
            .is_some_and(|tab| !tab.cancelled)
    }

    /// Release an id. The bool is what makes a close idempotent: a second
    /// close of the same tab finds nothing and reports so instead of failing.
    pub fn release(&self, id: &str) -> bool {
        self.tabs
            .lock()
            .expect("browser registry poisoned")
            .remove(id)
            .is_some()
    }
}

impl Default for BrowserRegistry {
    fn default() -> Self {
        BrowserRegistry::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn owned(label: &str) -> OwnedTab {
        OwnedTab {
            label: label.to_owned(),
            rect: PARK_RECT,
            live: Arc::default(),
            cancelled: false,
            workspace: "ws-1".to_owned(),
            state: Arc::new(Mutex::new(BrowserViewState::default())),
            sink: Arc::new(Mutex::new(Channel::new(|_| Ok(())))),
            guard: TabGuard::default(),
            drive: Arc::default(),
        }
    }

    fn owned_in(label: &str, workspace: &str) -> OwnedTab {
        OwnedTab {
            workspace: workspace.to_owned(),
            ..owned(label)
        }
    }

    #[test]
    fn the_profile_is_a_fixed_folder_of_the_apps_own_data() {
        let base = Path::new("C:/Users/dev/AppData/Local/com.devboule.desktop");
        let dir = profile_dir(base);
        assert_eq!(dir, base.join("browser-profile"));
        // The app's own data is never the profile, and nothing a page says can
        // move it: the function takes the app's directory and returns one fixed
        // child of it.
        assert_ne!(dir, base.to_path_buf());
        assert_eq!(profile_dir(base), dir);
    }

    #[test]
    fn a_claimed_id_belongs_to_one_webview_label() {
        let registry = BrowserRegistry::new();
        registry
            .claim("tab-1", owned("browser-tab-1"))
            .expect("first claim");
        assert_eq!(registry.label_of("tab-1").expect("owned"), "browser-tab-1");
    }

    #[test]
    fn claiming_a_live_id_twice_is_refused() {
        let registry = BrowserRegistry::new();
        registry
            .claim("tab-1", owned("browser-tab-1"))
            .expect("first claim");
        assert!(registry.claim("tab-1", owned("browser-tab-1")).is_err());
        assert_eq!(registry.label_of("tab-1").expect("owned"), "browser-tab-1");
    }

    #[test]
    fn close_is_idempotent() {
        let registry = BrowserRegistry::new();
        registry
            .claim("tab-1", owned("browser-tab-1"))
            .expect("claim");
        assert!(registry.release("tab-1"), "the first close releases");
        assert!(!registry.release("tab-1"), "the second close finds nothing");
    }

    #[test]
    fn nothing_resolves_after_a_dispose() {
        let registry = BrowserRegistry::new();
        registry
            .claim("tab-1", owned("browser-tab-1"))
            .expect("claim");
        registry.release("tab-1");
        // Every command starts here, so this refusal is the whole of "no
        // navigation after dispose": there is no label left to navigate.
        assert!(registry.label_of("tab-1").is_err());
        assert!(registry.rect_of("tab-1").is_none());
        // And a rect write for a disposed tab is dropped, not resurrected.
        registry.set_rect("tab-1", PARK_RECT, true);
        assert!(registry.rect_of("tab-1").is_none());
    }

    #[test]
    fn a_parked_page_is_one_pixel_far_outside_the_window() {
        const { assert!(PARK_RECT.width == 1.0) };
        const { assert!(PARK_RECT.height == 1.0) };
        const { assert!(PARK_RECT.x < -1_000.0) };
        const { assert!(PARK_RECT.y < -1_000.0) };
    }

    #[test]
    fn a_close_that_arrives_before_the_child_exists_cancels_the_claim() {
        let registry = BrowserRegistry::new();
        registry
            .claim("tab-1", owned("browser-tab-1"))
            .expect("claim");
        assert!(
            registry.claim_is_live("tab-1"),
            "a claim is a create's to keep"
        );

        assert!(
            registry.cancel("tab-1"),
            "the close found the create in flight"
        );

        // Nothing addresses a page the user has already closed...
        assert!(registry.label_of("tab-1").is_err());
        assert!(registry.rect_of("tab-1").is_none());
        // ...and the create that is about to get a child knows to close it.
        assert!(!registry.claim_is_live("tab-1"));
    }

    #[test]
    fn a_cancelled_claim_reopens_under_its_own_id_once_it_is_released() {
        let registry = BrowserRegistry::new();
        registry
            .claim("tab-1", owned("browser-tab-1"))
            .expect("claim");
        registry.cancel("tab-1");
        registry.release("tab-1");

        registry
            .claim("tab-1", owned("browser-tab-1"))
            .expect("the same id claims again");

        assert!(registry.claim_is_live("tab-1"));
        assert_eq!(
            registry.label_of("tab-1").expect("reopened"),
            "browser-tab-1"
        );
    }

    #[test]
    fn a_cancelled_claim_takes_no_rectangle() {
        let registry = BrowserRegistry::new();
        registry
            .claim("tab-1", owned("browser-tab-1"))
            .expect("claim");
        registry.cancel("tab-1");
        let rect = LogicalRect {
            x: 1.0,
            y: 2.0,
            width: 300.0,
            height: 400.0,
        };
        registry.set_rect("tab-1", rect, false);

        assert!(registry.rect_of("tab-1").is_none());
    }

    #[test]
    fn a_close_of_an_unknown_tab_cancels_nothing() {
        let registry = BrowserRegistry::new();

        assert!(!registry.cancel("tab-never-existed"));
    }

    #[test]
    fn an_active_tab_is_a_page_and_a_parked_one_is_a_pixel() {
        let registry = BrowserRegistry::new();
        registry
            .claim("tab-1", owned("browser-tab-1"))
            .expect("claim");
        // What the pane measures at 1280x800: a browser tab is the centre area.
        let pane = LogicalRect {
            x: 455.0,
            y: 49.0,
            width: 770.0,
            height: 751.0,
        };
        registry.set_rect("tab-1", pane, false);

        let (rect, parked) = registry.rect_of("tab-1").expect("owned");
        assert!(!parked, "a placed page is active");
        assert!(
            rect.width > 1.0 && rect.height > 1.0,
            "an active page is never the parked one pixel: {rect:?}"
        );

        registry.set_rect("tab-1", PARK_RECT, true);

        assert_eq!(registry.rect_of("tab-1"), Some((PARK_RECT, true)));
    }

    #[test]
    fn rect_moves_are_remembered_per_tab() {
        let registry = BrowserRegistry::new();
        registry.claim("a", owned("browser-a")).expect("claim a");
        registry.claim("b", owned("browser-b")).expect("claim b");
        let rect = LogicalRect {
            x: 10.0,
            y: 20.0,
            width: 300.0,
            height: 400.0,
        };
        registry.set_rect("a", rect, false);
        assert_eq!(registry.rect_of("a"), Some((rect, false)));
        // Parked is the default; one tab's presentation never moves another's.
        assert_eq!(registry.rect_of("b"), Some((PARK_RECT, true)));
    }

    #[test]
    fn a_parked_tab_keeps_the_size_it_was_last_presented_at() {
        let registry = BrowserRegistry::new();
        registry.claim("a", owned("browser-a")).expect("claim a");
        // Never presented: the default, so an agent measures a parked page
        // against something a person could read.
        assert_eq!(registry.tab_of("a").expect("owned").size, DEFAULT_PRESENTED);

        let pane = LogicalRect {
            x: 0.0,
            y: 0.0,
            width: 770.0,
            height: 751.0,
        };
        registry.set_rect("a", pane, false);
        registry.set_rect("a", PARK_RECT, true);

        // Parking must not throw the size away: it is the only record of how
        // wide the page was when it was in front.
        assert_eq!(
            registry.tab_of("a").expect("owned").size,
            Size {
                width: 770.0,
                height: 751.0
            }
        );
    }

    #[test]
    fn a_tab_is_addressable_only_by_its_own_workspace() {
        let registry = BrowserRegistry::new();
        registry
            .claim("a", owned_in("browser-a", "ws-1"))
            .expect("claim a");
        registry
            .claim("b", owned_in("browser-b", "ws-2"))
            .expect("claim b");

        assert_eq!(registry.tab_of("a").expect("owned").workspace, "ws-1");
        let mine: Vec<String> = registry
            .tabs_of("ws-1")
            .into_iter()
            .map(|tab| tab.browser_id)
            .collect();
        assert_eq!(
            mine,
            vec!["a".to_owned()],
            "another workspace's tab is not listed"
        );
        // And the id itself is still real: scoping is a check, not a hiding.
        assert!(registry.tab_of("b").is_some());
    }

    #[test]
    fn a_tab_has_one_guard_and_a_closed_or_cancelled_tab_has_none() {
        let registry = BrowserRegistry::new();
        registry.claim("a", owned("browser-a")).expect("claim a");
        registry.claim("b", owned("browser-b")).expect("claim b");

        let first = registry.guard_of("a").expect("a live tab has a guard");
        let again = registry.guard_of("a").expect("and the same one every time");
        assert!(Arc::ptr_eq(&first, &again));
        assert!(
            !Arc::ptr_eq(&first, &registry.guard_of("b").expect("b has its own")),
            "one tab's command never waits on another tab"
        );

        registry.cancel("a");
        assert!(registry.guard_of("a").is_none());
        registry.release("b");
        assert!(registry.guard_of("b").is_none());
        assert!(registry.guard_of("never-existed").is_none());
    }

    #[test]
    fn a_closed_tab_stops_being_addressable() {
        let registry = BrowserRegistry::new();
        registry.claim("a", owned("browser-a")).expect("claim a");
        registry.release("a");
        assert!(registry.tab_of("a").is_none());
        assert!(registry.tabs_of("ws-1").is_empty());
    }
}
