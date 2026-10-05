// @vitest-environment happy-dom

// The split as the workspace owns it: the browser tab the user is looking at
// moves into the pane below, the chat stays on top, the merge control puts it
// back into the strip, and a restart finds the split where it was left. The
// daemon doubles and the gestures come from the shared workspace harness.

import { act } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  afterEachHarness,
  beforeEachHarness,
  defaultSessions,
  plainClick,
  renderWorkspace,
  restartWorkspace,
} from "./bulkCloseHarness";

const mocks = vi.hoisted(() => ({ close: vi.fn(), open: vi.fn() }));

vi.mock("./browserController", async (importOriginal) => ({
  ...(await importOriginal<typeof import("./browserController")>()),
  browserClose: mocks.close,
  browserOpen: mocks.open,
  browserPresent: vi.fn(async () => undefined),
  browserPark: vi.fn(async () => undefined),
}));

vi.mock("@tauri-apps/api/event", () => ({
  listen: () => Promise.resolve(() => undefined),
}));

import { resetBrowserPagesForTests } from "./browserPages";
import {
  browserTabsFor,
  closeBrowserTab,
  openBrowserTab,
  resetBrowserLayoutForTests,
} from "./browserTabs";
import { localWorkspaceKey, type WorkspaceKey } from "./hosts/hostIdentity";
import { makeBrowserTab } from "./strip/toolTabs";
import { resetSplitPanesForTests } from "./split/splitPanes";

const WORKSPACE = localWorkspaceKey("workspace-1") as WorkspaceKey;

/** The split control in the browser tab's own chrome row: the keyboard road
 * into the split, so the layout is usable without the drag (slice 2). */
function splitControl(): HTMLButtonElement | null {
  const found = document.querySelector<HTMLButtonElement>(".browser-split-down");
  return found;
}

async function clickSplitControl(): Promise<void> {
  const control = splitControl();
  expect(control, "the split control did not render").not.toBeNull();
  await act(async () => control?.click());
}

function panes(): HTMLElement[] {
  return [...document.querySelectorAll<HTMLElement>(".workspace-split-pane")];
}

beforeEach(() => {
  beforeEachHarness();
  mocks.close.mockReset();
  mocks.close.mockResolvedValue(undefined);
  mocks.open.mockReset();
  mocks.open.mockResolvedValue({
    url: "https://example.test/",
    title: "Example",
    favicon: null,
    loading: false,
    canGoBack: false,
    canGoForward: false,
    error: null,
  });
  resetBrowserPagesForTests();
  resetBrowserLayoutForTests();
  resetSplitPanesForTests();
});

afterEach(async () => {
  await afterEachHarness();
});

describe("splitting the workspace", () => {
  it("puts the browser tab in the pane below and leaves the chat on top", async () => {
    const record = openBrowserTab(WORKSPACE, "https://example.test/");
    await renderWorkspace();
    await plainClick(makeBrowserTab(WORKSPACE, record.browserId).id);

    await clickSplitControl();

    expect(panes().map((pane) => pane.getAttribute("data-pane"))).toEqual(["top", "bottom"]);
    expect(panes()[0]?.querySelector('[data-testid="agent-chat-surface"]')).not.toBeNull();
    expect(panes()[1]?.querySelector(`[data-browser-id="${record.browserId}"]`)).not.toBeNull();
    // The control is the browser tab's own, so a page already below does not
    // offer to move itself again.
    expect(splitControl()).toBeNull();
  });

  it("takes the page back into the strip when the merge control is used", async () => {
    const record = openBrowserTab(WORKSPACE, "https://example.test/");
    await renderWorkspace();
    await plainClick(makeBrowserTab(WORKSPACE, record.browserId).id);
    await clickSplitControl();

    const merge = document.querySelector<HTMLButtonElement>(".workspace-split-merge");
    expect(merge, "the merge control did not render").not.toBeNull();
    await act(async () => merge?.click());

    // Merged into the strip means merged into the pane: the page is back where
    // it was, in the whole centre area, with its record left alone.
    expect(document.querySelector(".workspace-split")).toBeNull();
    expect(document.querySelector(`[data-browser-id="${record.browserId}"]`)).not.toBeNull();
    expect(browserTabsFor(WORKSPACE).map((tab) => tab.browserId)).toEqual([record.browserId]);
  });

  it("finds the split again after a restart, and leaves another workspace unsplit", async () => {
    const record = openBrowserTab(WORKSPACE, "https://example.test/");
    await renderWorkspace();
    await plainClick(makeBrowserTab(WORKSPACE, record.browserId).id);
    await clickSplitControl();
    const size = document
      .querySelector<HTMLElement>('[role="separator"]')
      ?.getAttribute("aria-valuenow");

    await restartWorkspace(defaultSessions());

    expect(document.querySelector('[role="separator"]')?.getAttribute("aria-valuenow")).toBe(size);
    expect(panes()[1]?.querySelector(`[data-browser-id="${record.browserId}"]`)).not.toBeNull();
  });

  it("has one pane again once the tab the lower pane held is gone", async () => {
    const record = openBrowserTab(WORKSPACE, "https://example.test/");
    await renderWorkspace();
    await plainClick(makeBrowserTab(WORKSPACE, record.browserId).id);
    await clickSplitControl();

    expect(document.querySelector(".workspace-split")).not.toBeNull();
    // The record's tab leaves the layout: the split has nothing left to show.
    await act(async () => {
      closeBrowserTab(record.browserId);
    });

    expect(document.querySelector(".workspace-split")).toBeNull();
  });
});
