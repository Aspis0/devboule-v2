//! Which child webviews this app owns, and the one profile directory they
//! share. The registry is the only place that knows a browser id maps to a
//! webview label, so every command that touches a page resolves through it
//! and a disposed tab resolves to nothing at all.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use tauri::{LogicalPosition, LogicalSize};

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

/// One owned page: the webview label it lives under and where it was last
/// put. The channel a page reports down belongs to the hooks that built it,
/// not to the registry: only they write on it.
pub struct OwnedTab {
    pub label: String,
    pub rect: LogicalRect,
    pub parked: bool,
    /// A close that arrived before the child existed. A claim is made before
    /// `add_child` runs, so this is the one thing a close can leave behind
    /// that the create itself has to act on.
    pub cancelled: bool,
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
            .map(|tab| (tab.rect, tab.parked))
    }

    pub fn set_rect(&self, id: &str, rect: LogicalRect, parked: bool) {
        if let Some(tab) = self
            .tabs
            .lock()
            .expect("browser registry poisoned")
            .get_mut(id)
            .filter(|tab| !tab.cancelled)
        {
            tab.rect = rect;
            tab.parked = parked;
        }
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
            parked: true,
            cancelled: false,
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
}
