// The app-wide event Rust sends about one of this app's browser tabs: one an
// agent opened, one an agent closed, and what any page now says about itself.
// The strip is the only thing that has to hear it — it is what persists the
// records — and a tab nobody is showing still reports, so a chip an agent
// opened is named by the page's own title rather than by its hostname.
//
// One listener for the whole app, started by the surface that owns the strip,
// because the tab model is app-lifetime state and a listener per pane would
// register one per mounted browser tab.

import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { addAgentTab, closeBrowserTab, patchBrowserTab } from "./browserTabs";
import { localWorkspaceKey, type WorkspaceKey } from "./hosts/hostIdentity";

/** The event name both languages use; declared once, in the Rust half too. */
export const BROWSER_TAB_EVENT = "browser:tab";

/** What Rust sends: a chip to add, a chip to take away, or what a page says. */
export type BrowserTabEvent =
  | { kind: "opened"; browserId: string; workspaceId: string; url: string }
  | { kind: "closed"; browserId: string }
  | {
      kind: "state";
      browserId: string;
      url: string;
      title: string | null;
      favicon: string | null;
    };

function isText(value: unknown): value is string {
  return typeof value === "string";
}

/** Whether this payload is one of the three events above, and nothing else. */
function isTabEvent(value: unknown): value is BrowserTabEvent {
  if (typeof value !== "object" || value === null) return false;
  const event = value as Record<string, unknown>;
  if (!isText(event.browserId) || event.browserId === "") return false;
  // A close names the tab it takes away and nothing else.
  if (event.kind === "closed") return true;
  if (!isText(event.url)) return false;
  if (event.kind === "opened") {
    return isText(event.workspaceId) && event.workspaceId !== "";
  }
  // A page may have no title and no icon yet; that is a state, not a reason to
  // drop what it did say.
  return (
    event.kind === "state" &&
    (event.title === null || isText(event.title)) &&
    (event.favicon === null || isText(event.favicon))
  );
}

/** One event, handled. Exported for the test that drives it without a bridge. */
export function applyBrowserTabEvent(value: unknown): void {
  if (!isTabEvent(value)) return;
  if (value.kind === "closed") {
    closeBrowserTab(value.browserId);
    return;
  }
  if (value.kind === "state") {
    // A tab no record holds is a page that is not in the strip: the frontend
    // opens those, and an event cannot be half a record.
    patchBrowserTab(value.browserId, {
      url: value.url,
      title: value.title,
      favicon: value.favicon,
    });
    return;
  }
  // Rust scopes a tab by the daemon's workspace id, which is the half of the
  // key the app composes; a chip is filed under the key, so they are put back
  // together here and not in Rust.
  const workspaceKey: WorkspaceKey | null = localWorkspaceKey(value.workspaceId);
  if (workspaceKey === null) return;
  addAgentTab(workspaceKey, value.browserId, value.url);
}

/**
 * Follow the event for as long as the strip lives. Anything that is not a tab
 * event is dropped rather than half-applied: a chip with no workspace has
 * nowhere to go.
 */
export async function watchBrowserTabs(): Promise<UnlistenFn> {
  const unlisten = await listen<unknown>(BROWSER_TAB_EVENT, (event) => {
    applyBrowserTabEvent(event.payload);
  });
  return unlisten;
}
