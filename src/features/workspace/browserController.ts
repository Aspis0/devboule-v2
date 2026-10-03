// The frontend's door to the browser tab controller in Rust: one channel per
// tab for what the page reports about itself, and the commands that place,
// navigate, park and dispose it.
//
// The channel is a Tauri `Channel` rather than an app-wide event because a
// browser tab's updates belong to the tab that asked for them: there is one
// subscriber, it subscribes at create, and it stops existing at close. An app
// event would need a filter every listener applies.

import { Channel, invoke } from "@tauri-apps/api/core";

/** What a browser page reports back. Mirrors `BrowserViewState` in
 * `src-tauri/src/browser.rs`; the two are read against each other by
 * `browserContract.test.ts`. */
export interface BrowserViewState {
  url: string;
  title: string | null;
  favicon: string | null;
  loading: boolean;
  canGoBack: boolean;
  canGoForward: boolean;
  /** Why the last navigation was refused, in the words the chrome shows. */
  error: string | null;
}

/** A rectangle in the units `browser_present` expects. The main webview's
 * CSS pixels and Tauri logical pixels are the same unit on every platform
 * this ships on — both are physical pixels over 96 dpi, measured 0 px apart
 * at 125% — so `getBoundingClientRect()` is handed over unchanged. */
export interface LogicalRect {
  x: number;
  y: number;
  width: number;
  height: number;
}

export type BrowserUpdate =
  | {
      kind: "state";
      url: string;
      title: string | null;
      favicon: string | null;
      loading: boolean;
      canGoBack: boolean;
      canGoForward: boolean;
      error: string | null;
    }
  | { kind: "newWindow"; url: string };

export function browserRectOf(rect: DOMRect): LogicalRect {
  return { x: rect.left, y: rect.top, width: rect.width, height: rect.height };
}

export function browserOpen(
  id: string,
  url: string,
  onUpdate: (update: BrowserUpdate) => void,
): Promise<BrowserViewState> {
  return invoke<BrowserViewState>("browser_open", {
    id,
    url,
    updates: new Channel<BrowserUpdate>(onUpdate),
  });
}

export function browserPresent(id: string, rect: LogicalRect): Promise<void> {
  return invoke("browser_present", { id, rect });
}

export function browserPark(id: string): Promise<void> {
  return invoke("browser_park", { id });
}

export function browserNavigate(id: string, url: string): Promise<void> {
  return invoke("browser_navigate", { id, url });
}

export function browserHistory(id: string, act: "back" | "forward" | "stop"): Promise<void> {
  return invoke("browser_history", { id, act });
}

export function browserReload(id: string): Promise<void> {
  return invoke("browser_reload", { id });
}

export function browserClose(id: string): Promise<void> {
  return invoke("browser_close", { id });
}
