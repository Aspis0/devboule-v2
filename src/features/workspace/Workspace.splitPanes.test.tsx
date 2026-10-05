// @vitest-environment happy-dom

// What each pane of a split holds, read the way the user reads it. The trap
// this file exists for: the strip's selection and the pane's contents stop
// being the same fact once a split is open — the tab in the pane below is
// still the selected one, and the pane above is showing a conversation. Every
// reader has to ask the panes, never the selection.

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  afterEachHarness,
  beforeEachHarness,
  plainClick,
  renderWorkspace,
  requestChildPermission,
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

import { act } from "react";
import { resetBrowserPagesForTests } from "./browserPages";
import { openBrowserTab, resetBrowserLayoutForTests } from "./browserTabs";
import { localWorkspaceKey, type WorkspaceKey } from "./hosts/hostIdentity";
import { makeBrowserTab } from "./strip/toolTabs";
import { resetSplitPanesForTests, splitPaneFor } from "./split/splitPanes";

const WORKSPACE = localWorkspaceKey("workspace-1") as WorkspaceKey;
/** The session the workspace lands on, and the pane above the split. */
const SESSION = "agent-one";

function panes() {
  return [...document.querySelectorAll<HTMLElement>(".workspace-split-pane")].map((pane) => ({
    which: pane.getAttribute("data-pane"),
    text: (pane.textContent ?? "").trim(),
    browserId: pane.querySelector("[data-browser-id]")?.getAttribute("data-browser-id") ?? null,
  }));
}

async function clickSplitControl(): Promise<void> {
  const control = document.querySelector<HTMLButtonElement>(".browser-split-down");
  expect(control, "the split control did not render").not.toBeNull();
  await act(async () => control?.click());
}

/** A browser tab in front, then split into the pane below. */
async function openAndSplit(): Promise<string> {
  const record = openBrowserTab(WORKSPACE, "https://example.test/");
  await renderWorkspace();
  await plainClick(makeBrowserTab(WORKSPACE, record.browserId).id);
  await clickSplitControl();
  return record.browserId;
}

beforeEach(() => {
  beforeEachHarness();
  mocks.close.mockReset();
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

describe("what each pane holds", () => {
  it("puts the browser tab below and the conversation above", async () => {
    const browserId = await openAndSplit();

    const [top, bottom] = panes();
    expect(top?.which).toBe("top");
    expect(bottom?.which).toBe("bottom");
    expect(top?.browserId).toBeNull();
    expect(top?.text).toContain(SESSION);
    expect(bottom?.browserId).toBe(browserId);
  });

  it("keeps the pane above on the conversation when its own tab is selected again", async () => {
    const browserId = await openAndSplit();
    // The strip's selection is the tab in the pane below; the pane above must
    // still be the conversation, and the page must not open a second time.
    await plainClick(makeBrowserTab(WORKSPACE, browserId).id);

    const [top, bottom] = panes();
    expect(top?.text).toContain(SESSION);
    expect(top?.browserId).toBeNull();
    expect(bottom?.browserId).toBe(browserId);
    expect(mocks.open).toHaveBeenCalledTimes(1);
  });

  it("shows a waiting permission card while the conversation is above and the tool is below", async () => {
    const browserId = await openAndSplit();
    // The tab in the pane below is the strip's selection while the card waits
    // on the session the pane above renders.
    await plainClick(makeBrowserTab(WORKSPACE, browserId).id);
    await requestChildPermission(SESSION, SESSION);

    expect(document.body.textContent).toContain("Run command");
  });

  it("still hides the card when a tool tab really is in front of the pane", async () => {
    const record = openBrowserTab(WORKSPACE, "https://example.test/");
    await renderWorkspace();
    await plainClick(makeBrowserTab(WORKSPACE, record.browserId).id);
    await requestChildPermission(SESSION, SESSION);

    expect(document.body.textContent).not.toContain("Run command");
  });

  it("hands the merged tab the focus when the merge control is used", async () => {
    const browserId = await openAndSplit();
    const merge = document.querySelector<HTMLButtonElement>(".workspace-split-merge");
    expect(merge, "the merge control did not render").not.toBeNull();
    merge?.focus();
    await act(async () => merge?.click());

    // The control that was pressed goes with the split; focus lands on the tab
    // that was below, whose chip is where it now lives.
    const chip = document.getElementById(
      `workspace-session-tab-${makeBrowserTab(WORKSPACE, browserId).id}`,
    );
    expect(document.activeElement).toBe(chip);
    expect(splitPaneFor(WORKSPACE)).toBeNull();
  });
});
