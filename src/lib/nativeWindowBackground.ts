import { invoke } from "@tauri-apps/api/core";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { getCurrentWindow } from "@tauri-apps/api/window";

/**
 * The OS window behind the webview, both layers: one call that hands them the
 * ground the page is actually showing, so a resize or restore never flashes
 * the native layer's colour.
 *
 * The colour is read computed, not mirrored: what the page paints is what the
 * gaps must match, even if a token ever drifts from its mirror. A missing or
 * non-hex token is said once and keeps the startup colour; the Tauri side
 * only accepts 3- and 6-digit hex (an alpha of 00 would turn the layer
 * transparent instead of keeping anything).
 */

// Module state; tests take a fresh instance through vi.resetModules.
const warnedKinds = new Set<string>();

function warnOnce(kind: string, message: string): void {
  if (warnedKinds.has(kind)) return;
  warnedKinds.add(kind);
  console.warn(message);
}

export function syncNativeWindowBackground(): void {
  // Outside Tauri (browser dev, unit tests) there is no bridge: no call, no
  // rejection, nothing in the console — whatever the token reads.
  if (typeof window === "undefined" || !("__TAURI_INTERNALS__" in window)) return;
  const ground = getComputedStyle(document.documentElement).getPropertyValue("--ground-app").trim();
  if (!/^#[0-9a-fA-F]{3}([0-9a-fA-F]{3})?$/.test(ground)) {
    warnOnce(
      "bad-token",
      `devboule: --ground-app is not a usable native colour ("${ground}"); keeping the startup background.`,
    );
    return;
  }
  // The 2.11.1 wrappers send { color } but the Rust command reads `value`
  // (fixed upstream in 2.12.0): invoke directly until the stack moves there.
  // Static imports, evaluated once: concurrent duplicate dynamic imports can
  // resolve to different module instances under test doubles.
  void invoke("plugin:window|set_background_color", {
    label: getCurrentWindow().label,
    value: ground,
  })
    .then(() =>
      invoke("plugin:webview|set_webview_background_color", {
        label: getCurrentWebview().label,
        value: ground,
      }),
    )
    .catch(() =>
      warnOnce(
        "invoke-refused",
        "devboule: native background invoke refused; keeping the startup background.",
      ),
    );
}
