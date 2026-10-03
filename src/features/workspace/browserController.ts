// The frontend's door to the browser tab controller in Rust: one channel per
// tab for what the page reports about itself, and the commands that place,
// navigate, park and dispose it.
//
// The channel is a Tauri `Channel` rather than an app-wide event because a
// browser tab's updates belong to the tab that asked for them: there is one
// subscriber, it subscribes at create, and it stops existing at close. An app
// event would need a filter every listener applies.

import { Channel } from "@tauri-apps/api/core";
import { invokeTyped } from "../../lib/tauri";
import type { BrowserUpdate, BrowserViewState, LogicalRect } from "../../types/ipc";

export function browserRectOf(rect: DOMRect): LogicalRect {
  return { x: rect.left, y: rect.top, width: rect.width, height: rect.height };
}

export function browserOpen(
  id: string,
  url: string,
  workspaceId: string,
  onUpdate: (update: BrowserUpdate) => void,
): Promise<BrowserViewState> {
  // Rust claims the page for a workspace and adopts an existing one, so this
  // is the only place a page is ever created for an id.
  return invokeTyped("browser_open", {
    id,
    url,
    workspaceId,
    updates: new Channel<BrowserUpdate>(onUpdate),
  });
}

export function browserPresent(id: string, rect: LogicalRect): Promise<void> {
  return invokeTyped("browser_present", { id, rect });
}

export function browserPark(id: string): Promise<void> {
  return invokeTyped("browser_park", { id });
}

export function browserNavigate(id: string, url: string): Promise<void> {
  return invokeTyped("browser_navigate", { id, url });
}

export function browserHistory(id: string, act: "back" | "forward" | "stop"): Promise<void> {
  return invokeTyped("browser_history", { id, act });
}

export function browserReload(id: string): Promise<void> {
  return invokeTyped("browser_reload", { id });
}

export function browserClose(id: string): Promise<void> {
  return invokeTyped("browser_close", { id });
}
