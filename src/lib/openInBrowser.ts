import { externalUrlOpen } from "./tauri";

/**
 * Hands an http(s) URL to the system browser. The command, never the webview,
 * launches it, so a click cannot navigate the page; every other scheme is
 * refused here and again in the command. A launch the OS refuses is dropped:
 * the link's own text stays on screen, and there is no failure UI for a click.
 */
export function openInBrowser(url: string): void {
  if (!/^https?:\/\//i.test(url)) return;
  void externalUrlOpen(url).catch(() => undefined);
}
