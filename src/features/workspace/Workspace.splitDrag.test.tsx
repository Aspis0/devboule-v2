// @vitest-environment happy-dom

// Dragging a browser tab out of the strip and into the workspace centre, end to
// end: the pane lands where the preview said, the page is parked while the
// preview covers it and presented exactly once when it is gone, and a tab
// dropped back on the row merges the panes.

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  afterEachHarness,
  beforeEachHarness,
  defaultSessions,
  plainClick,
  renderWorkspace,
  requestChildPermission,
  restartWorkspace,
} from "./bulkCloseHarness";

const mocks = vi.hoisted(() => ({
  close: vi.fn(),
  open: vi.fn(),
  present: vi.fn(),
  park: vi.fn(),
}));

vi.mock("./browserController", async (importOriginal) => ({
  ...(await importOriginal<typeof import("./browserController")>()),
  browserClose: mocks.close,
  browserOpen: mocks.open,
  browserPresent: mocks.present,
  browserPark: mocks.park,
}));

vi.mock("@tauri-apps/api/event", () => ({
  listen: () => Promise.resolve(() => undefined),
}));

import { act } from "react";
import { resetBrowserPagesForTests } from "./browserPages";
import { resetBrowserOverlaysForTests } from "./browserOverlays";
import {
  browserTabsFor,
  closeBrowserTab,
  openBrowserTab,
  resetBrowserLayoutForTests,
} from "./browserTabs";
import { localWorkspaceKey, type WorkspaceKey } from "./hosts/hostIdentity";
import { makeBrowserTab } from "./strip/toolTabs";
import { resetSplitPanesForTests, splitPaneFor } from "./split/splitPanes";
import { DRAG_SLOP_PX } from "./split/useTabDrag";

const WORKSPACE = localWorkspaceKey("workspace-1") as WorkspaceKey;

/** The layout happy-dom cannot compute: the centre a drop is read against, and
 * the strip above it. */
const CENTRE = { left: 0, top: 120, width: 1000, height: 800 };
const STRIP = { left: 0, top: 60, width: 1000, height: 40 };

function box(rect: { left: number; top: number; width: number; height: number }): DOMRect {
  return {
    ...rect,
    right: rect.left + rect.width,
    bottom: rect.top + rect.height,
    toJSON: () => ({}),
  } as DOMRect;
}

/** The two boxes a drop is read against, stubbed on the elements themselves:
 * a prototype-wide stub would take the strip's own measuring away with it. */
function stubBoxes(): void {
  const centre = document.querySelector(".workspace-center-panel");
  const strip = document.querySelector(".workspace-session-tabs");
  if (centre === null || strip === null) throw new Error("the workspace did not render");
  vi.spyOn(centre, "getBoundingClientRect").mockReturnValue(box(CENTRE));
  vi.spyOn(strip, "getBoundingClientRect").mockReturnValue(box(STRIP));
  // The page area and the preview only need boxes where they meet: the overlay
  // registry parks a page when an overlay covers its rectangle, so both boxes
  // have to exist for that to be decided. Everything else measures for real.
  const measured = HTMLElement.prototype.getBoundingClientRect;
  vi.spyOn(HTMLElement.prototype, "getBoundingClientRect").mockImplementation(function (
    this: HTMLElement,
  ) {
    if (this.hasAttribute("data-browser-id")) {
      return box({
        left: CENTRE.left,
        top: CENTRE.top + CENTRE.height / 2,
        width: CENTRE.width,
        height: CENTRE.height / 2,
      });
    }
    if (this.classList.contains("workspace-drop-preview")) return box(CENTRE);
    return measured.call(this);
  });
}

/** The workspace, with the boxes a drag is read against. */
async function renderSplitWorkspace(): Promise<void> {
  await renderWorkspace();
  stubBoxes();
}

/** The harness runs on fake timers, and the placement sends on an animation
 * frame: the frames are driven the way the app's own tests drive them. */
async function frames(count = 3): Promise<void> {
  await act(async () => {
    for (let turn = 0; turn < count; turn += 1) {
      await vi.advanceTimersByTimeAsync(40);
      await Promise.resolve();
    }
  });
}

/** Press a chip, carry the pointer to `at`, and let go there. */
async function dragChipTo(at: { x: number; y: number }, browserId: string): Promise<void> {
  await dragChipByIdTo(at, makeBrowserTab(WORKSPACE, browserId).id);
}

/** The same gesture on any chip in the strip, named by its own id. */
async function dragChipByIdTo(at: { x: number; y: number }, tabId: string): Promise<void> {
  const chip = document.getElementById(`workspace-session-tab-${tabId}`);
  if (chip === null) throw new Error(`the chip did not render: ${tabId}`);
  const chipBox = { x: 200, y: STRIP.top + 20 };
  await act(async () => {
    chip.dispatchEvent(
      new PointerEvent("pointerdown", {
        clientX: chipBox.x,
        clientY: chipBox.y,
        pointerId: 1,
        bubbles: true,
      }),
    );
  });
  // Past the slop, so the press has become a drag.
  await act(async () => {
    window.dispatchEvent(
      new PointerEvent("pointermove", {
        clientX: chipBox.x + DRAG_SLOP_PX + 1,
        clientY: chipBox.y + DRAG_SLOP_PX + 1,
        pointerId: 1,
      }),
    );
  });
  await act(async () => {
    window.dispatchEvent(
      new PointerEvent("pointermove", { clientX: at.x, clientY: at.y, pointerId: 1 }),
    );
  });
  await act(async () => {
    window.dispatchEvent(
      new PointerEvent("pointerup", { clientX: at.x, clientY: at.y, pointerId: 1 }),
    );
  });
}

/** What every polite status region in the window is saying. */
function politeRegions(): string[] {
  return [...document.querySelectorAll('[role="status"][aria-live="polite"]')].map(
    (region) => region.textContent ?? "",
  );
}

/** Inside the centre's bottom band (15% of 800 is 120px). */
const BOTTOM = { x: 500, y: CENTRE.top + CENTRE.height - 10 };
/** Inside the centre's centred square (40% of each axis). */
const MIDDLE = { x: 500, y: CENTRE.top + CENTRE.height / 2 };
/** Inside the centre's top band. */
const TOP_EDGE = { x: 500, y: CENTRE.top + 10 };

beforeEach(() => {
  beforeEachHarness();
  for (const spy of [mocks.close, mocks.open, mocks.present, mocks.park]) spy.mockReset();
  mocks.open.mockResolvedValue({
    url: "https://example.test/",
    title: "Example",
    favicon: null,
    loading: false,
    canGoBack: false,
    canGoForward: false,
    error: null,
  });
  mocks.present.mockResolvedValue(undefined);
  mocks.park.mockResolvedValue(undefined);
  resetBrowserPagesForTests();
  resetBrowserOverlaysForTests();
  resetBrowserLayoutForTests();
  resetSplitPanesForTests();
});

afterEach(async () => {
  await afterEachHarness();
  vi.restoreAllMocks();
});

describe("dragging a browser tab into the centre", () => {
  it("splits with the page below when the pointer is over the bottom band", async () => {
    const record = openBrowserTab(WORKSPACE, "https://example.test/");
    await renderSplitWorkspace();
    await plainClick(makeBrowserTab(WORKSPACE, record.browserId).id);
    mocks.present.mockClear();

    await dragChipTo(BOTTOM, record.browserId);

    expect(splitPaneFor(WORKSPACE)?.lowerTabId).toBe(
      makeBrowserTab(WORKSPACE, record.browserId).id,
    );
    const panes = [...document.querySelectorAll(".workspace-split-pane")];
    expect(panes.map((pane) => pane.getAttribute("data-pane"))).toEqual(["top", "bottom"]);
    expect(panes[1]?.querySelector(`[data-browser-id="${record.browserId}"]`)).not.toBeNull();
  });

  it("parks the page while the preview is over it and presents it once when the preview is gone", async () => {
    const record = openBrowserTab(WORKSPACE, "https://example.test/");
    await renderSplitWorkspace();
    await plainClick(makeBrowserTab(WORKSPACE, record.browserId).id);
    await frames();
    mocks.present.mockClear();
    mocks.park.mockClear();

    const chip = document.getElementById(
      `workspace-session-tab-${makeBrowserTab(WORKSPACE, record.browserId).id}`,
    );
    if (chip === null) throw new Error("the browser chip did not render");
    await act(async () => {
      chip.dispatchEvent(
        new PointerEvent("pointerdown", { clientX: 200, clientY: 80, pointerId: 1, bubbles: true }),
      );
    });
    await act(async () => {
      window.dispatchEvent(
        new PointerEvent("pointermove", { clientX: 500, clientY: BOTTOM.y, pointerId: 1 }),
      );
    });
    // The preview is up over the page: the native child is told to get out of
    // the way, and it is the overlay registry that tells it.
    expect(document.querySelector(".workspace-drop-preview")).not.toBeNull();
    await frames();
    expect(mocks.park).toHaveBeenCalledWith(record.browserId);
    const parkedWhilePreviewing = mocks.present.mock.calls.length;
    expect(parkedWhilePreviewing).toBe(0);

    await act(async () => {
      window.dispatchEvent(
        new PointerEvent("pointerup", { clientX: 500, clientY: BOTTOM.y, pointerId: 1 }),
      );
    });
    await frames();

    expect(document.querySelector(".workspace-drop-preview")).toBeNull();
    // Exactly one placement after the drop: the page comes back at the
    // rectangle the pane below now measures, and not once per frame.
    expect(mocks.present.mock.calls.length - parkedWhilePreviewing).toBe(1);
    expect(mocks.open).toHaveBeenCalledTimes(1);
  });

  it("is a plain selection from the centre, and leaves no split", async () => {
    const record = openBrowserTab(WORKSPACE, "https://example.test/");
    await renderSplitWorkspace();

    await dragChipTo(MIDDLE, record.browserId);

    expect(splitPaneFor(WORKSPACE)).toBeNull();
    expect(document.querySelector(".workspace-split")).toBeNull();
    expect(
      document
        .querySelector(".workspace-session-tab[aria-selected='true']")
        ?.textContent?.includes("Example"),
    ).toBe(true);
  });

  it("leaves the tab where it was on Escape", async () => {
    const record = openBrowserTab(WORKSPACE, "https://example.test/");
    await renderSplitWorkspace();
    const chip = document.getElementById(
      `workspace-session-tab-${makeBrowserTab(WORKSPACE, record.browserId).id}`,
    );
    if (chip === null) throw new Error("the browser chip did not render");

    await act(async () => {
      chip.dispatchEvent(
        new PointerEvent("pointerdown", { clientX: 200, clientY: 80, pointerId: 1, bubbles: true }),
      );
    });
    await act(async () => {
      window.dispatchEvent(
        new PointerEvent("pointermove", { clientX: 500, clientY: BOTTOM.y, pointerId: 1 }),
      );
    });
    expect(document.querySelector(".workspace-drop-preview")).not.toBeNull();
    await act(async () => {
      window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    });

    expect(document.querySelector(".workspace-drop-preview")).toBeNull();
    expect(splitPaneFor(WORKSPACE)).toBeNull();
  });

  it("reorders the pane below's tab on the row and keeps the split", async () => {
    const record = openBrowserTab(WORKSPACE, "https://example.test/");
    await renderSplitWorkspace();
    await plainClick(makeBrowserTab(WORKSPACE, record.browserId).id);
    await act(async () => {
      document.querySelector<HTMLButtonElement>(".browser-split-down")?.click();
    });
    expect(splitPaneFor(WORKSPACE)).not.toBeNull();

    await dragChipTo({ x: 500, y: STRIP.top + 20 }, record.browserId);

    expect(splitPaneFor(WORKSPACE)).not.toBeNull();
    expect(document.querySelector(".workspace-split")).not.toBeNull();
  });

  it("merges the pane below back through its own merge control", async () => {
    const record = openBrowserTab(WORKSPACE, "https://example.test/");
    await renderSplitWorkspace();
    await plainClick(makeBrowserTab(WORKSPACE, record.browserId).id);
    await act(async () => {
      document.querySelector<HTMLButtonElement>(".browser-split-down")?.click();
    });
    await act(async () => {
      document.querySelector<HTMLButtonElement>(".workspace-split-merge")?.click();
    });

    expect(splitPaneFor(WORKSPACE)).toBeNull();
    expect(document.querySelector(".workspace-split")).toBeNull();
    // The tab is in front again, and its page is back in the whole centre.
    expect(document.querySelector(`[data-browser-id="${record.browserId}"]`)).not.toBeNull();
  });

  it("picks up no chip the pane below cannot hold, so none of them can reach a pane", async () => {
    const browser = openBrowserTab(WORKSPACE, "https://example.test/");
    await renderSplitWorkspace();

    // A conversation, and a tool tab the pane below has no room for.
    await dragChipByIdTo(BOTTOM, "agent-one");
    expect(splitPaneFor(WORKSPACE)).toBeNull();
    expect(document.querySelector(".workspace-split")).toBeNull();
    expect(document.body.classList.contains("workspace-is-dragging-tab")).toBe(false);
    // The drag changed nothing at all: the conversation is still what the
    // workspace is showing, and the page is still where it was.
    expect(document.querySelector(".workspace-session-tab[aria-selected='true']")?.id).toBe(
      "workspace-session-tab-agent-one",
    );
    expect(browserTabsFor(WORKSPACE).map((tab) => tab.browserId)).toEqual([browser.browserId]);
  });

  it("swaps the panes on a top drop, and brings the swap back after a restart", async () => {
    // Two pages with a place each, and a third the person drags in.
    const below = openBrowserTab(WORKSPACE, "https://below.test/");
    const front = openBrowserTab(WORKSPACE, "https://front.test/");
    const dragged = openBrowserTab(WORKSPACE, "https://dragged.test/");
    const belowId = makeBrowserTab(WORKSPACE, below.browserId).id;
    const frontId = makeBrowserTab(WORKSPACE, front.browserId).id;
    const draggedId = makeBrowserTab(WORKSPACE, dragged.browserId).id;
    await renderSplitWorkspace();

    // One page below: the control splits the tab in front into the pane below.
    await plainClick(belowId);
    await act(async () => {
      document.querySelector<HTMLButtonElement>(".browser-split-down")?.click();
    });
    expect(splitPaneFor(WORKSPACE)?.lowerTabId).toBe(belowId);

    // A second page in front, and a third the person drags to the top edge: the
    // page in front moves below and the dragged one takes the pane above.
    await plainClick(frontId);
    expect(splitPaneFor(WORKSPACE)?.lowerTabId).toBe(belowId);
    await dragChipTo(TOP_EDGE, dragged.browserId);

    expect(splitPaneFor(WORKSPACE)?.lowerTabId).toBe(frontId);
    // Every page in this file answers the mocked open with the same url, so the
    // chips all read alike: which tab is in front is its id.
    expect(document.querySelector(".workspace-session-tab[aria-selected='true']")?.id).toBe(
      `workspace-session-tab-${draggedId}`,
    );
    let panes = [...document.querySelectorAll(".workspace-split-pane")];
    expect(panes[0]?.querySelector(`[data-browser-id="${dragged.browserId}"]`)).not.toBeNull();
    expect(panes[1]?.querySelector(`[data-browser-id="${front.browserId}"]`)).not.toBeNull();

    // A restart finds the swap: the pane below holds the page the swap put there.
    // What was in front is not remembered — a browser tab is not a tab the next
    // run can open — so the conversation is back in front, and its card with it.
    await restartWorkspace(defaultSessions());
    expect(splitPaneFor(WORKSPACE)?.lowerTabId).toBe(frontId);
    panes = [...document.querySelectorAll(".workspace-split-pane")];
    expect(panes[1]?.querySelector(`[data-browser-id="${front.browserId}"]`)).not.toBeNull();
    expect(panes[0]?.querySelector("[data-browser-id]")).toBeNull();

    // A page that is in neither pane, brought to the front, takes the card away
    // with the surface it would have been shown on; the conversation brings it
    // back. (Selecting the page already in the pane below leaves the
    // conversation in front, and the card with it.)
    await plainClick(belowId);
    await requestChildPermission("agent-one", "agent-one");
    expect(document.body.textContent).not.toContain("Run command");
    await plainClick("agent-one");
    await requestChildPermission("agent-one", "agent-one");
    expect(document.body.textContent).toContain("Run command");
  });

  it("says what the keyboard's act did, and leaves the focus on the tab it moved", async () => {
    const record = openBrowserTab(WORKSPACE, "https://example.test/");
    await renderSplitWorkspace();
    const tabId = makeBrowserTab(WORKSPACE, record.browserId).id;
    const chip = document.getElementById(`workspace-session-tab-${tabId}`);
    if (chip === null) throw new Error("the browser chip did not render");

    await act(async () => {
      chip.dispatchEvent(new MouseEvent("contextmenu", { bubbles: true }));
    });
    const entry = [...document.querySelectorAll<HTMLButtonElement>("[role='menuitem']")].find(
      (item) => item.textContent === "Move to the pane below",
    );
    expect(entry, "the menu did not offer the split").not.toBeUndefined();
    await act(async () => entry?.click());

    expect(splitPaneFor(WORKSPACE)?.lowerTabId).toBe(tabId);
    expect(politeRegions()).toContain("Moved to the pane below.");
    expect(document.activeElement).toBe(chip);
  });

  it("offers the way back out of the pane below, and says so", async () => {
    const record = openBrowserTab(WORKSPACE, "https://example.test/");
    await renderSplitWorkspace();
    await plainClick(makeBrowserTab(WORKSPACE, record.browserId).id);
    await act(async () => {
      document.querySelector<HTMLButtonElement>(".browser-split-down")?.click();
    });
    const tabId = makeBrowserTab(WORKSPACE, record.browserId).id;
    await act(async () => {
      document
        .getElementById(`workspace-session-tab-${tabId}`)
        ?.dispatchEvent(new MouseEvent("contextmenu", { bubbles: true }));
    });
    const entry = [...document.querySelectorAll<HTMLButtonElement>("[role='menuitem']")].find(
      (item) => item.textContent === "Move out of the pane below",
    );
    expect(entry, "the menu did not offer the way out").not.toBeUndefined();
    await act(async () => entry?.click());

    expect(splitPaneFor(WORKSPACE)).toBeNull();
    expect(politeRegions()).toContain("Moved out of the pane below.");
    expect(document.activeElement).toBe(document.getElementById(`workspace-session-tab-${tabId}`));
  });
});

describe("a page an agent closes mid-gesture", () => {
  it("ends the gesture instead of dropping a tab that is gone", async () => {
    const record = openBrowserTab(WORKSPACE, "https://example.test/");
    await renderSplitWorkspace();
    const tabId = makeBrowserTab(WORKSPACE, record.browserId).id;
    const chip = document.getElementById(`workspace-session-tab-${tabId}`);
    if (chip === null) throw new Error("the browser chip did not render");

    await act(async () => {
      chip.dispatchEvent(
        new PointerEvent("pointerdown", { clientX: 200, clientY: 80, pointerId: 1, bubbles: true }),
      );
    });
    await act(async () => {
      window.dispatchEvent(
        new PointerEvent("pointermove", { clientX: BOTTOM.x, clientY: BOTTOM.y, pointerId: 1 }),
      );
    });
    expect(document.querySelector(".workspace-drop-preview")).not.toBeNull();

    // The tab closes under the pointer.
    await act(async () => {
      closeBrowserTab(record.browserId);
    });
    await act(async () => {
      window.dispatchEvent(
        new PointerEvent("pointermove", {
          clientX: BOTTOM.x,
          clientY: BOTTOM.y - 40,
          pointerId: 1,
        }),
      );
    });

    expect(document.querySelector(".workspace-drop-preview")).toBeNull();
    expect(splitPaneFor(WORKSPACE)).toBeNull();
  });
});

describe("the old suite's menu case", () => {
  it("offers the same act from the tab menu, for a keyboard that cannot drag", async () => {
    const record = openBrowserTab(WORKSPACE, "https://example.test/");
    await renderSplitWorkspace();
    const tabId = makeBrowserTab(WORKSPACE, record.browserId).id;
    const chip = document.getElementById(`workspace-session-tab-${tabId}`);
    if (chip === null) throw new Error("the browser chip did not render");

    await act(async () => {
      chip.dispatchEvent(new MouseEvent("contextmenu", { bubbles: true }));
    });
    const entry = [...document.querySelectorAll<HTMLButtonElement>("[role='menuitem']")].find(
      (item) => item.textContent === "Move to the pane below",
    );
    expect(entry, "the menu did not offer the split").not.toBeUndefined();
    await act(async () => entry?.click());

    expect(splitPaneFor(WORKSPACE)?.lowerTabId).toBe(tabId);
  });
});
