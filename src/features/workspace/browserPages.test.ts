// @vitest-environment node

// The browser page store: one create per browser id, one channel for it, and
// every view of that page on that channel. A second `Channel` for a page that
// already exists is a channel the Rust side never writes, and a store entry
// left behind by a create that failed is a page no later mount can open.

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { BrowserUpdate, BrowserViewState } from "../../types/ipc";

const mocks = vi.hoisted(() => ({
  open: vi.fn(),
  close: vi.fn(),
  popup: vi.fn(),
}));

vi.mock("./browserController", () => ({ browserOpen: mocks.open, browserClose: mocks.close }));
vi.mock("./browserTabs", () => ({ requestBrowserPopup: mocks.popup }));

import {
  browserPagesSnapshot,
  closeBrowserPage,
  forgetBrowserPage,
  resetBrowserPagesForTests,
  subscribeBrowserPages,
  watchBrowserPage,
} from "./browserPages";

const LOADED: BrowserViewState = {
  url: "https://example.com/",
  title: "Example Domain",
  favicon: null,
  loading: false,
  canGoBack: false,
  canGoForward: false,
  error: null,
};

/** The callback the controller reports the page's own state on. */
function reportOn(): (update: BrowserUpdate) => void {
  return mocks.open.mock.calls[0]?.[2] as (update: BrowserUpdate) => void;
}

describe("the browser page store", () => {
  beforeEach(() => {
    resetBrowserPagesForTests();
    mocks.open.mockReset();
    mocks.close.mockReset();
    mocks.popup.mockReset();
    mocks.open.mockResolvedValue(LOADED);
    mocks.close.mockResolvedValue(undefined);
  });

  afterEach(() => {
    resetBrowserPagesForTests();
  });

  it("creates one page for two views of the same tab", async () => {
    const first: BrowserUpdate[] = [];
    const second: BrowserUpdate[] = [];
    watchBrowserPage("tab-1", "https://example.com/", (update) => first.push(update));
    watchBrowserPage("tab-1", "https://example.com/", (update) => second.push(update));

    expect(mocks.open).toHaveBeenCalledTimes(1);
    // Both views hear the page, which is what a shared channel buys: the
    // second mount did not get a channel of its own.
    const update: BrowserUpdate = {
      kind: "state",
      url: "https://example.com/next",
      title: "Next",
      favicon: null,
      loading: true,
      canGoBack: true,
      canGoForward: false,
      error: null,
    };
    reportOn()(update);
    expect(first).toEqual([update]);
    expect(second).toEqual([update]);
  });

  it("answers a view that arrives late with the page as it stands", async () => {
    const first = watchBrowserPage("tab-1", "https://example.com/", () => undefined);
    await expect(first.opened).resolves.toEqual(LOADED);
    const report = reportOn();
    report({
      kind: "state",
      url: "https://example.com/next",
      title: "Next",
      favicon: null,
      loading: true,
      canGoBack: true,
      canGoForward: false,
      error: null,
    });

    const late = watchBrowserPage("tab-1", "https://example.com/next", () => undefined);

    await expect(late.opened).resolves.toMatchObject({ url: "https://example.com/next" });
    expect(mocks.open).toHaveBeenCalledTimes(1);
  });

  it("stops answering a view that unsubscribed", async () => {
    const gone: BrowserUpdate[] = [];
    const staying: BrowserUpdate[] = [];
    const watched = watchBrowserPage("tab-1", "https://example.com/", (update) =>
      gone.push(update),
    );
    const report = reportOn();
    watchBrowserPage("tab-1", "https://example.com/", (update) => staying.push(update));
    watched.unwatch();

    report({
      kind: "state",
      url: "https://example.com/next",
      title: null,
      favicon: null,
      loading: false,
      canGoBack: false,
      canGoForward: false,
      error: null,
    });

    expect(gone).toEqual([]);
    expect(staying).toHaveLength(1);
  });

  it("lets a refused create be tried again, because no page was built", async () => {
    mocks.open.mockRejectedValueOnce("The main window is gone.");
    const failed = watchBrowserPage("tab-1", "https://example.com/", () => undefined);
    await expect(failed.opened).rejects.toBe("The main window is gone.");

    const again = watchBrowserPage("tab-1", "https://example.com/", () => undefined);
    await expect(again.opened).resolves.toEqual(LOADED);
    expect(mocks.open).toHaveBeenCalledTimes(2);
  });

  it("routes one window request per page, not one per view", () => {
    watchBrowserPage("tab-1", "https://example.com/", () => undefined);
    const report = reportOn();
    watchBrowserPage("tab-1", "https://example.com/", () => undefined);

    report({ kind: "newWindow", url: "https://example.com/other" });

    expect(mocks.popup).toHaveBeenCalledExactlyOnceWith("tab-1", "https://example.com/other");
  });

  it("drops an update that arrives after the tab was closed", async () => {
    const watched = watchBrowserPage("tab-1", "https://example.com/", () => undefined);
    const report = reportOn();
    await expect(watched.opened).resolves.toEqual(LOADED);
    let notified = 0;
    subscribeBrowserPages(() => {
      notified += 1;
    });
    forgetBrowserPage("tab-1");
    expect(notified).toBe(1);
    expect(browserPagesSnapshot().size).toBe(0);

    report({
      kind: "state",
      url: "https://example.com/next",
      title: null,
      favicon: null,
      loading: true,
      canGoBack: false,
      canGoForward: false,
      error: null,
    });

    expect(notified).toBe(1);
    expect(browserPagesSnapshot().size).toBe(0);
  });

  it("closes the page it forgets, and forgets nothing else", async () => {
    const watched = watchBrowserPage("tab-1", "https://example.com/", () => undefined);
    watchBrowserPage("tab-2", "https://example.org/", () => undefined);
    await expect(watched.opened).resolves.toEqual(LOADED);

    closeBrowserPage("tab-1");

    expect(mocks.close).toHaveBeenCalledExactlyOnceWith("tab-1");
    expect(browserPagesSnapshot().has("tab-1")).toBe(false);
    expect(browserPagesSnapshot().has("tab-2")).toBe(true);
  });

  it("publishes what each live page reported, and only when it changed", async () => {
    watchBrowserPage("tab-1", "https://example.com/", () => undefined);
    const report = reportOn();
    let notified = 0;
    subscribeBrowserPages(() => {
      notified += 1;
    });
    await Promise.resolve();
    expect(browserPagesSnapshot().get("tab-1")).toEqual(LOADED);
    expect(notified).toBe(1);

    report({
      kind: "state",
      url: "https://example.com/next",
      title: null,
      favicon: null,
      loading: true,
      canGoBack: false,
      canGoForward: false,
      error: null,
    });

    expect(browserPagesSnapshot().get("tab-1")?.loading).toBe(true);
    expect(notified).toBe(2);
  });

  it("says nothing when a parked page reports what it already said", async () => {
    // A parked page keeps running and keeps reporting. Its state is the
    // strip's whole input, so a repeat must not re-render every chip.
    const watched = watchBrowserPage("tab-1", "https://example.com/", () => undefined);
    await expect(watched.opened).resolves.toEqual(LOADED);
    const report = reportOn();
    let notified = 0;
    subscribeBrowserPages(() => {
      notified += 1;
    });
    const loaded = { kind: "state", ...LOADED } as const;

    report(loaded);
    report(loaded);

    expect(notified).toBe(0);
  });
});
