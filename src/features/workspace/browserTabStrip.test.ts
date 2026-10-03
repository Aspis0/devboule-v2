// @vitest-environment happy-dom

// A browser tab's half of the strip: it takes its place among the sessions
// and the other tool tabs, it is named by the page it is on, and a page
// asking for a window of its own becomes another tab in the SAME workspace
// rather than a window nothing manages.

import { beforeEach, describe, expect, it, vi } from "vitest";
import { localWorkspaceKey, type WorkspaceKey } from "./hosts/hostIdentity";
import {
  composeStripTabs,
  makeBrowserTab,
  makeToolTab,
  toolTabKindLabel,
  toolTabSubject,
} from "./strip/toolTabs";
import {
  browserTabsFor,
  closeBrowserTab,
  openBrowserTab,
  patchBrowserTab,
  resetBrowserLayoutForTests,
  routeBrowserPopup,
  requestBrowserPopup,
} from "./browserTabs";
import { BROWSER_START_URL } from "./browserUrl";
import type { Session } from "../../types/ipc";

function at(workspaceId: string): WorkspaceKey {
  const key = localWorkspaceKey(workspaceId);
  if (key === null) throw new Error(`not a workspace key: ${workspaceId}`);
  return key;
}

const ALPHA = at("alpha");
const BETA = at("beta");

function session(id: string): Session {
  return {
    id,
    workspaceId: "alpha",
    kind: "terminal",
    title: id,
    state: { type: "live", generation: 1 },
    elapsedMs: 0,
  };
}

describe("a browser tab in the composed strip", () => {
  beforeEach(() => {
    resetBrowserLayoutForTests();
  });

  it("sits after the sessions and among the other tool tabs", () => {
    const composed = composeStripTabs(
      [session("s1")],
      [
        makeToolTab("diff", ALPHA, "src/app.ts"),
        makeBrowserTab(ALPHA, "browser-1"),
        makeToolTab("file", ALPHA, "README.md"),
      ],
    );
    expect(composed.map((tab) => tab.id)).toEqual([
      "s1",
      makeToolTab("diff", ALPHA, "src/app.ts").id,
      makeBrowserTab(ALPHA, "browser-1").id,
      makeToolTab("file", ALPHA, "README.md").id,
    ]);
    // And it is a tool tab, so the strip's own selection, close and keyboard
    // roads take it without knowing a browser exists.
    expect(composed[2]?.type).toBe("tool");
    expect(toolTabKindLabel(makeBrowserTab(ALPHA, "browser-1"))).toBe("Browser");
  });

  it("keeps two workspaces' browser tabs under different ids", () => {
    expect(makeBrowserTab(ALPHA, "same-id").id).not.toBe(makeBrowserTab(BETA, "same-id").id);
  });

  it("names itself by the browser id, not by a path", () => {
    const tab = makeBrowserTab(ALPHA, "browser-1");
    expect(toolTabSubject(tab)).toBe("browser-1");
  });

  it("carries the page it is on into the tab's record", () => {
    const record = openBrowserTab(ALPHA);
    patchBrowserTab(record.browserId, {
      url: "https://example.org/",
      title: "Example",
      favicon: "https://example.org/icon.png",
    });
    const [stored] = browserTabsFor(ALPHA);
    expect(stored).toMatchObject({
      browserId: record.browserId,
      url: "https://example.org/",
      title: "Example",
      favicon: "https://example.org/icon.png",
    });
  });

  it("opens a tab where a page asked for one, on that page's address", () => {
    const record = openBrowserTab(BETA, "https://example.net/docs");
    expect(record.url).toBe("https://example.net/docs");
    expect(browserTabsFor(BETA)).toHaveLength(1);
    expect(browserTabsFor(ALPHA)).toHaveLength(0);
  });

  it("opens on the start page when nothing asked for an address", () => {
    expect(openBrowserTab(ALPHA).url).toBe(BROWSER_START_URL);
  });

  it("leaves the strip alone once its record is closed", () => {
    const record = openBrowserTab(ALPHA);
    closeBrowserTab(record.browserId);
    expect(browserTabsFor(ALPHA)).toEqual([]);
  });
});

describe("a page asking for a window of its own", () => {
  beforeEach(() => {
    resetBrowserLayoutForTests();
  });

  it("reaches the strip, which is the only thing that knows the workspace", () => {
    const source = openBrowserTab(BETA);
    const listener = vi.fn();
    const stop = routeBrowserPopup(listener);
    requestBrowserPopup(source.browserId, "https://example.net/next");
    expect(listener).toHaveBeenCalledWith(source.browserId, "https://example.net/next");
    stop();
  });

  it("is dropped, not leaked, once the strip has stopped listening", () => {
    const source = openBrowserTab(BETA);
    const stop = routeBrowserPopup(vi.fn());
    stop();
    requestBrowserPopup(source.browserId, "https://example.net/next");
    // Nothing opened: no native popup, no half-registered tab.
    expect(browserTabsFor(BETA)).toHaveLength(1);
  });

  it("never opens a window of its own — the request is data, not a window", () => {
    const source = openBrowserTab(BETA);
    let routed: { sourceId: string; url: string } | null = null;
    const stop = routeBrowserPopup((sourceId, url) => {
      routed = { sourceId, url };
      const owner = browserTabsFor(BETA).find((tab) => tab.browserId === sourceId);
      if (owner !== undefined) openBrowserTab(owner.workspaceKey, url);
    });
    requestBrowserPopup(source.browserId, "https://example.net/next");
    stop();

    expect(routed).not.toBeNull();
    // The new tab landed in the ASKING workspace, and the source workspace
    // gained exactly one tab.
    expect(browserTabsFor(BETA).map((tab) => tab.url)).toEqual([
      BROWSER_START_URL,
      "https://example.net/next",
    ]);
    expect(browserTabsFor(ALPHA)).toHaveLength(0);
  });
});
