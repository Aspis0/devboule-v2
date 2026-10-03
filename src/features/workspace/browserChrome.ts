// What one browser tab's chrome is showing right now, as a value the
// component renders without deciding anything: which of the one reload button
// and Stop it is, whether the history buttons can be pressed, what the bar
// reads, and whether there is an error line under it.
//
// Two rules live here rather than in the component because they are the ones
// that drift: the bar must keep showing the address the user typed while they
// are typing it, and a refused navigation must leave both the bar and the
// page where the last good address was.

import { browserUrlRefusal, normalizeBrowserUrl } from "./browserUrl";

/** The single reload control, which becomes Stop while the page loads. */
export type ReloadAction = "reload" | "stop";

export interface BrowserPage {
  /** The address the page is actually on, after every redirect it survived. */
  url: string;
  loading: boolean;
  canGoBack: boolean;
  canGoForward: boolean;
  /** Why the last navigation was refused, in the chrome's own words. */
  error: string | null;
}

export interface BrowserChrome {
  /** What the page is on. */
  barValue: string;
  loading: boolean;
  canGoBack: boolean;
  canGoForward: boolean;
  error: string | null;
  action: ReloadAction;
}

/**
 * The chrome for a page, with the user's uncommitted keystrokes folded in.
 *
 * A refused address never reaches here: it becomes the error line and the
 * page underneath stays where it was, so `url` is always a page that loaded
 * or a page the tab is opening.
 */
export function browserChrome(page: BrowserPage, draft: string | null): BrowserChrome {
  return {
    barValue: draft ?? page.url,
    loading: page.loading,
    canGoBack: page.canGoBack,
    canGoForward: page.canGoForward,
    error: page.error,
    action: page.loading ? "stop" : "reload",
  };
}

/** What Enter in the bar should do. A refused address navigates nothing and
 * says why in the error line; it never falls back to the page's own address,
 * which would look like the refusal did work. */
export function submitBrowserAddress(
  draft: string,
): { url: string; error: null } | { url: null; error: string } {
  const url = normalizeBrowserUrl(draft);
  return url === null
    ? { url: null, error: browserUrlRefusal(draft) ?? "That is not a web address." }
    : { url, error: null };
}
