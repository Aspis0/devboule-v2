// One channel per browser page, shared by every view of that page, plus the
// last state each page reported. The create happens once per browser id for the
// life of the process: React mounts a pane twice under StrictMode, and a second
// `Channel` for a page that already exists is a channel the Rust side never
// writes — the page would load, report its title once and then go silent.
//
// The live state lives here rather than in the tab layout because the layout is
// persisted: a tab restored after a restart has a record and no page, and that
// is what "not loading" has to mean for its chip.

import { browserClose, browserOpen } from "./browserController";
import { requestBrowserPopup } from "./browserTabs";
import type { BrowserChord, BrowserUpdate, BrowserViewState } from "../../types/ipc";

/** A view of a page, told what the page reports. */
type PageWatch = (update: BrowserUpdate) => void;

interface Page {
  id: string;
  /** What the page last reported, null until the create answers. */
  state: BrowserViewState | null;
  /** The one create every watcher of this page shares. */
  opened: Promise<BrowserViewState>;
  watchers: Set<PageWatch>;
}

const pages = new Map<string, Page>();
const listeners = new Set<() => void>();
let reported: Map<string, BrowserViewState> = new Map();

/** Hand a new identity of the reports to the strip: `useSyncExternalStore`
 * reads the map's identity to tell a change from a no-op. */
function publish(next: Map<string, BrowserViewState>): void {
  reported = next;
  for (const listener of [...listeners]) listener();
}

/** Two reports of the same page. Every field, because a title that changed
 * only its case is a chip that has to change. */
function samePage(one: BrowserViewState, two: BrowserViewState): boolean {
  return (
    one.url === two.url &&
    one.title === two.title &&
    one.favicon === two.favicon &&
    one.loading === two.loading &&
    one.canGoBack === two.canGoBack &&
    one.canGoForward === two.canGoForward &&
    one.error === two.error
  );
}

function remember(page: Page, state: BrowserViewState): void {
  // A parked page keeps running, and its hooks keep reporting. Those reports
  // are the strip's whole input, so one that says nothing new must not walk
  // every chip again.
  if (page.state !== null && samePage(page.state, state)) return;
  page.state = state;
  publish(new Map(reported).set(page.id, state));
  for (const watcher of [...page.watchers]) watcher({ kind: "state", ...state });
}

/** What one page reports, read once and shared by every view of it. A page
 * asking for a window of its own, or a chord pressed inside it, is a request
 * about the page rather than its state, so each is routed once however many
 * views are mounted. An update that arrives after the tab was closed has no
 * page to land on and is dropped. */
function report(id: string, update: BrowserUpdate): void {
  if (update.kind === "newWindow") {
    requestBrowserPopup(id, update.url);
    return;
  }
  if (update.kind === "chord") {
    chordWatchers.get(id)?.(update.chord);
    return;
  }
  const page = pages.get(id);
  if (page === undefined) return;
  const { kind: _state, ...state } = update;
  remember(page, state);
}

/** The pane currently watching one page, for a chord the page's own focus
 * sent down the channel. It is a pane while a pane is watching it and nothing
 * after that, which is the whole lifetime a chord has. */
const chordWatchers = new Map<string, (chord: BrowserChord) => void>();

/** Hand a page's chords to the pane showing it. */
export function watchBrowserChords(id: string, onChord: (chord: BrowserChord) => void): () => void {
  chordWatchers.set(id, onChord);
  return () => {
    if (chordWatchers.get(id) === onChord) chordWatchers.delete(id);
  };
}

function create(id: string, url: string): Page {
  const opened = browserOpen(id, url, (update) => report(id, update));
  const page: Page = { id, state: null, opened, watchers: new Set() };
  pages.set(id, page);
  void opened.then(
    (state) => remember(page, state),
    () => {
      // Nothing was built, so no page owns this id: a later mount creates
      // again rather than reading a promise that can only refuse.
      if (pages.get(id) === page) pages.delete(id);
    },
  );
  return page;
}

/**
 * Show a page, and follow it. `opened` answers with the page's state as soon
 * as the page has one, and `unwatch` is the caller's half of the subscription.
 */
export function watchBrowserPage(
  id: string,
  url: string,
  onUpdate: PageWatch,
): { opened: Promise<BrowserViewState>; unwatch: () => void } {
  const page = pages.get(id) ?? create(id, url);
  page.watchers.add(onUpdate);
  return {
    // A view that arrives after the page has settled must not read the
    // create's answer: a page that finished loading a while ago reports
    // nothing again, so that answer would be the state it had at open.
    opened: page.state === null ? page.opened : Promise.resolve(page.state),
    unwatch: () => {
      page.watchers.delete(onUpdate);
    },
  };
}

/** The controller owns the page no more, so its record and its last report go
 * with it: a chip must never read a page that has been closed. */
export function forgetBrowserPage(id: string): void {
  pages.delete(id);
  if (reported.delete(id)) publish(new Map(reported));
}

/**
 * The tab is gone: dispose of its page and stop reading it. A native close is
 * the only thing that releases a child webview, and a page outlives its pane
 * on purpose — so this is the pairing every close path needs, not the pane's.
 */
export function closeBrowserPage(id: string): void {
  void browserClose(id).catch(() => undefined);
  forgetBrowserPage(id);
}

/** What each live page last reported, for the chips that name it. */
export function browserPagesSnapshot(): ReadonlyMap<string, BrowserViewState> {
  return reported;
}

export function subscribeBrowserPages(listener: () => void): () => void {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

export function resetBrowserPagesForTests(): void {
  pages.clear();
  listeners.clear();
  chordWatchers.clear();
  reported = new Map();
}
