// The app-wide event Rust sends when an agent opens or closes one of this
// app's browser tabs. The strip is the only thing that has to hear it: a tab
// an agent opened is a chip in the workspace it belongs to, exactly like a tab
// the user opened, and an agent's `close_tab` takes the chip with the page.
//
// One listener for the whole app, started by the surface that owns the strip,
// because the tab model is app-lifetime state and a listener per pane would
// register one per mounted browser tab.

import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { addAgentTab, closeBrowserTab } from "./browserTabs";
import { localWorkspaceKey, type WorkspaceKey } from "./hosts/hostIdentity";

/** The event name both languages use; declared once, in the Rust half too. */
export const BROWSER_TAB_EVENT = "browser:tab";

/** What Rust sends: a chip to add, or a chip to take away. */
export type BrowserTabEvent =
  | { kind: "opened"; browserId: string; workspaceId: string; url: string }
  | { kind: "closed"; browserId: string };

function isTabEvent(value: unknown): value is BrowserTabEvent {
  if (typeof value !== "object" || value === null) return false;
  const event = value as { kind?: unknown; browserId?: unknown; workspaceId?: unknown };
  if (typeof event.browserId !== "string" || event.browserId === "") return false;
  if (event.kind === "closed") return true;
  return (
    event.kind === "opened" &&
    typeof event.workspaceId === "string" &&
    event.workspaceId !== "" &&
    typeof (value as { url?: unknown }).url === "string"
  );
}

/** One event, handled. Exported for the test that drives it without a bridge. */
export function applyBrowserTabEvent(value: unknown): void {
  if (!isTabEvent(value)) return;
  if (value.kind === "closed") {
    closeBrowserTab(value.browserId);
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
