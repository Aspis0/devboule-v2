// @vitest-environment happy-dom

// A tab an agent opened. Two halves meet here: Rust creates the page and
// announces it as an event, and the frontend has to turn that event into a
// chip, open the pane when the user clicks it, and let the pane ADOPT the page
// that is already there rather than ask for a second one at the same address.
//
// StrictMode is not decoration: React mounts a pane twice on purpose, so a
// pane that opened a second page would leave one of them loading with nobody
// listening to it.

import { act, StrictMode, type ReactElement } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { BrowserViewState } from "../../types/ipc";

const mocks = vi.hoisted(() => ({
  open: vi.fn(),
  present: vi.fn(),
  park: vi.fn(),
  listeners: new Map<string, (event: { payload: unknown }) => void>(),
}));

vi.mock("./browserController", async (importOriginal) => ({
  ...(await importOriginal<typeof import("./browserController")>()),
  browserOpen: mocks.open,
  browserPresent: mocks.present,
  browserPark: mocks.park,
}));
vi.mock("./browserTabs", async (importOriginal) => ({
  ...(await importOriginal<typeof import("./browserTabs")>()),
  requestBrowserPopup: vi.fn(),
}));
vi.mock("@tauri-apps/api/event", () => ({
  listen: (event: string, handler: (event: { payload: unknown }) => void) => {
    mocks.listeners.set(event, handler);
    return Promise.resolve(() => mocks.listeners.delete(event));
  },
}));

import { BrowserTab } from "./BrowserTab";
import { applyBrowserTabEvent } from "./browserTabEvents";
import { resetBrowserPagesForTests } from "./browserPages";
import { browserLayoutSnapshot, resetBrowserLayoutForTests } from "./browserTabs";
import { localWorkspaceKey, type WorkspaceKey } from "./hosts/hostIdentity";

const WORKSPACE = localWorkspaceKey("w-1") as WorkspaceKey;
const WORKSPACE_ID = "w-1";

/** The state Rust answers `browser_open` with for a page that already exists:
 * the page the agent was reading. */
const ADOPTED: BrowserViewState = {
  url: "https://example.test/report",
  title: "Quarterly report",
  favicon: null,
  loading: false,
  canGoBack: false,
  canGoForward: false,
  error: null,
};

function agentOpenedTab(): void {
  applyBrowserTabEvent({
    kind: "opened",
    browserId: "tab-agent",
    workspaceId: WORKSPACE_ID,
    url: "https://example.test/report",
  });
}

async function mount(node: ReactElement): Promise<() => void> {
  const host = document.createElement("div");
  document.body.append(host);
  const root = createRoot(host);
  await act(async () => {
    root.render(node);
  });
  return () => {
    act(() => root.unmount());
    host.remove();
  };
}

describe("a tab an agent opened", () => {
  beforeEach(() => {
    mocks.open.mockReset();
    mocks.present.mockReset();
    mocks.park.mockReset();
    mocks.open.mockResolvedValue(ADOPTED);
    mocks.present.mockResolvedValue(undefined);
    mocks.park.mockResolvedValue(undefined);
    resetBrowserPagesForTests();
    resetBrowserLayoutForTests();
  });

  afterEach(() => {
    document.body.replaceChildren();
  });

  it("becomes a chip in the workspace it belongs to", () => {
    agentOpenedTab();

    const tabs = browserLayoutSnapshot().tabs;
    expect(tabs).toHaveLength(1);
    expect(tabs[0]?.browserId).toBe("tab-agent");
    expect(tabs[0]?.workspaceKey).toBe(WORKSPACE);
    expect(tabs[0]?.title).toBeNull();
    // And it does not steal the pane the user is looking at.
    expect(browserLayoutSnapshot().activeByWorkspace[WORKSPACE]).toBeUndefined();
  });

  it("opens with one call for that id, under StrictMode, and shows the page", async () => {
    agentOpenedTab();
    const unmount = await mount(
      <StrictMode>
        <BrowserTab
          browserId="tab-agent"
          url="https://example.test/report"
          workspaceKey={WORKSPACE}
        />
      </StrictMode>,
    );
    await act(async () => {
      await Promise.resolve();
    });

    expect(mocks.open).toHaveBeenCalledTimes(1);
    const [id, url, workspaceId] = mocks.open.mock.calls[0] ?? [];
    expect(id).toBe("tab-agent");
    expect(url).toBe("https://example.test/report");
    // Rust scopes the page by the daemon's own id, which is the half of the
    // key the app composes.
    expect(workspaceId).toBe(WORKSPACE_ID);

    // The adopted page's own title is on the chrome: this is the page the
    // agent was reading, not a fresh one.
    expect(document.querySelector(".browser-address")?.getAttribute("value")).toBe(ADOPTED.url);
    unmount();
  });

  it("leaves the chip behind when the user closes the pane's tab", () => {
    agentOpenedTab();
    applyBrowserTabEvent({ kind: "closed", browserId: "tab-agent" });

    expect(browserLayoutSnapshot().tabs).toHaveLength(0);
  });

  it("ignores an event that is not a tab it can file", () => {
    agentOpenedTab();
    for (const payload of [
      { kind: "opened", browserId: "", workspaceId: WORKSPACE_ID, url: "https://x.test" },
      { kind: "opened", browserId: "tab-2", workspaceId: "", url: "https://x.test" },
      { kind: "opened", browserId: "tab-2", workspaceId: "w-1" },
      { kind: "opened", browserId: "tab-2" },
      { kind: "closed" },
      "browser:tab",
      null,
    ]) {
      applyBrowserTabEvent(payload);
    }

    expect(browserLayoutSnapshot().tabs.map((tab) => tab.browserId)).toEqual(["tab-agent"]);
  });

  it("does not add the same id twice", () => {
    agentOpenedTab();
    agentOpenedTab();

    expect(browserLayoutSnapshot().tabs).toHaveLength(1);
  });
});
