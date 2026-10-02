import { carriesCredentials } from "./urlCredentials";
import { externalUrlOpen } from "./tauri";

/**
 * True when this app hands the URL to the OS: http(s) with no credentials,
 * which is what the command opens. A URL that fails it keeps the browser's
 * own behaviour — cancelling its click would leave a link that opens nothing.
 */
export function opensExternally(url: string): boolean {
  return /^https?:\/\//i.test(url) && !carriesCredentials(url);
}

/**
 * Hands an http(s) URL to the system browser. The command, never the webview,
 * launches it, so a click cannot navigate the page; every other URL is refused
 * here and again in the command. A launch the OS refuses is dropped: the link's
 * own text stays on screen, and there is no failure UI for a click.
 */
export function openInBrowser(url: string): void {
  if (!opensExternally(url)) return;
  void externalUrlOpen(url).catch(() => undefined);
}
