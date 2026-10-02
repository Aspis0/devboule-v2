import { opensExternally } from "./externalUrl";
import { externalUrlOpen } from "./tauri";

/**
 * Hands a URL the command opens to the system browser. The command, never the
 * webview, launches it, so a click cannot navigate the page; every other URL is
 * refused here and again in the command. A launch the OS refuses is dropped:
 * the link's own text stays on screen, and there is no failure UI for a click.
 */
export function openInBrowser(url: string): void {
  if (!opensExternally(url)) return;
  void externalUrlOpen(url).catch(() => undefined);
}
