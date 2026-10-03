// @vitest-environment happy-dom

// What the browser tab model does while the app runs, and what a restart of
// it finds: the tabs, their pages and each workspace's selection.

import { beforeEach, describe, expect, it, vi } from "vitest";
import { localWorkspaceKey, type WorkspaceKey } from "./hosts/hostIdentity";
import {
  activeBrowserTabFor,
  browserLayoutSnapshot,
  browserTabsFor,
  closeBrowserTab,
  openBrowserTab,
  patchBrowserTab,
  pruneBrowserTabs,
  resetBrowserLayoutForTests,
  subscribeBrowserLayout,
} from "./browserTabs";
import { BROWSER_START_URL } from "./browserUrl";
import { BROWSER_TABS_STORAGE_KEY, clearBrowserLayout } from "./browserTabStorage";

function workspace(id: string): WorkspaceKey {
  const key = localWorkspaceKey(id);
  if (key === null) throw new Error(`not a workspace key: ${id}`);
  return key;
}

const ALPHA = workspace("alpha");
const BETA = workspace("beta");

describe("browser tabs", () => {
  beforeEach(() => {
    resetBrowserLayoutForTests();
    clearBrowserLayout();
  });
  it("opens a tab on the start page and makes it the workspace's", () => {
    const record = openBrowserTab(ALPHA);
    expect(record.url).toBe(BROWSER_START_URL);
    expect(browserTabsFor(ALPHA)).toEqual([record]);
    expect(activeBrowserTabFor(ALPHA)).toBe(record.browserId);
  });

  it("mints a different id per tab", () => {
    const first = openBrowserTab(ALPHA);
    const second = openBrowserTab(ALPHA);
    expect(second.browserId).not.toBe(first.browserId);
    expect(browserTabsFor(ALPHA)).toHaveLength(2);
    expect(activeBrowserTabFor(ALPHA)).toBe(second.browserId);
  });

  it("keeps one workspace's tabs out of another's", () => {
    const alpha = openBrowserTab(ALPHA);
    const beta = openBrowserTab(BETA);
    expect(browserTabsFor(ALPHA).map((tab) => tab.browserId)).toEqual([alpha.browserId]);
    expect(browserTabsFor(BETA).map((tab) => tab.browserId)).toEqual([beta.browserId]);
    expect(activeBrowserTabFor(ALPHA)).toBe(alpha.browserId);
  });

  it("re-records the page's own title, favicon and address", () => {
    const record = openBrowserTab(ALPHA);
    patchBrowserTab(record.browserId, {
      url: "https://example.org/",
      title: "Example",
      favicon: "https://example.org/icon.png",
    });
    expect(browserTabsFor(ALPHA)[0]).toMatchObject({
      url: "https://example.org/",
      title: "Example",
      favicon: "https://example.org/icon.png",
    });
  });

  it("closes a tab and forgets which workspace was showing it", () => {
    const record = openBrowserTab(ALPHA);
    closeBrowserTab(record.browserId);
    expect(browserTabsFor(ALPHA)).toEqual([]);
    expect(activeBrowserTabFor(ALPHA)).toBeNull();
  });

  it("drops the tabs of a workspace the project list no longer holds", () => {
    const kept = openBrowserTab(ALPHA);
    const dropped = openBrowserTab(BETA);
    pruneBrowserTabs(new Set([ALPHA]));
    expect(browserTabsFor(ALPHA).map((tab) => tab.browserId)).toEqual([kept.browserId]);
    expect(browserTabsFor(BETA)).toEqual([]);
    expect(activeBrowserTabFor(ALPHA)).toBe(kept.browserId);
    expect(browserLayoutSnapshot().activeByWorkspace[BETA]).toBeUndefined();
    expect(dropped.browserId).not.toBe(kept.browserId);
  });

  it("tells its subscribers when the layout changes, and only then", () => {
    let notified = 0;
    const stop = subscribeBrowserLayout(() => {
      notified += 1;
    });
    const record = openBrowserTab(ALPHA);
    expect(notified).toBe(1);
    // The page reported the same thing it reported before: no new identity,
    // so no re-render of every chip behind it.
    patchBrowserTab(record.browserId, { url: BROWSER_START_URL, title: null, favicon: null });
    expect(notified).toBe(1);
    closeBrowserTab(record.browserId);
    expect(notified).toBe(2);
    stop();
    openBrowserTab(ALPHA);
    expect(notified).toBe(2);
  });
});

describe("browser tab persistence", () => {
  beforeEach(() => {
    resetBrowserLayoutForTests();
    clearBrowserLayout();
    vi.resetModules();
  });

  /** A second run of the app: the module is loaded again, so it reads what
   * the first run left in storage instead of what it held in memory. */
  async function restart(): Promise<typeof import("./browserTabs")> {
    vi.resetModules();
    return import("./browserTabs");
  }

  it("brings both workspaces' tabs and their pages back after a restart", async () => {
    const alpha = openBrowserTab(ALPHA);
    const beta = openBrowserTab(BETA);
    patchBrowserTab(alpha.browserId, { url: "https://example.org/", title: null, favicon: null });
    patchBrowserTab(beta.browserId, {
      url: "https://example.net/",
      title: "Example Net",
      favicon: "https://example.net/icon.png",
    });
    // Alpha keeps two tabs, and lands on the second one.
    openBrowserTab(ALPHA);

    const next = await restart();

    expect(next.browserTabsFor(ALPHA).map((tab) => tab.url)).toEqual([
      "https://example.org/",
      BROWSER_START_URL,
    ]);
    expect(next.browserTabsFor(BETA)).toEqual([
      expect.objectContaining({
        browserId: beta.browserId,
        url: "https://example.net/",
        title: "Example Net",
        favicon: "https://example.net/icon.png",
      }),
    ]);
  });

  it("brings back which tab each workspace was showing", async () => {
    const alpha = openBrowserTab(ALPHA);
    openBrowserTab(ALPHA);
    openBrowserTab(BETA);

    const next = await restart();

    expect(next.activeBrowserTabFor(ALPHA)).not.toBe(alpha.browserId);
    expect(next.activeBrowserTabFor(BETA)).not.toBeNull();
    expect(next.activeBrowserTabFor(ALPHA)).toBe(next.browserTabsFor(ALPHA)[1]?.browserId ?? null);
  });

  it("reads no tabs from a record of another version", async () => {
    localStorage.setItem(BROWSER_TABS_STORAGE_KEY, JSON.stringify({ v: 99, tabs: [] }));
    const next = await restart();
    expect(next.browserLayoutSnapshot().tabs).toEqual([]);
  });

  it("costs one bad record, not the whole layout", async () => {
    localStorage.setItem(
      BROWSER_TABS_STORAGE_KEY,
      JSON.stringify({
        v: 1,
        tabs: [
          { browserId: "good", workspaceKey: ALPHA, url: "https://example.com/" },
          { browserId: "", workspaceKey: ALPHA, url: "https://example.com/" },
          { browserId: "no-workspace", url: "https://example.com/" },
          { browserId: "not-web", workspaceKey: ALPHA, url: "javascript:alert(1)" },
        ],
        activeByWorkspace: { [ALPHA]: "good", [BETA]: "gone" },
      }),
    );
    const { readBrowserLayout } = await import("./browserTabStorage");
    const layout = readBrowserLayout();
    expect(layout.tabs.map((tab) => tab.browserId)).toEqual(["good"]);
    expect(layout.activeByWorkspace).toEqual({ [ALPHA]: "good" });
  });
});
