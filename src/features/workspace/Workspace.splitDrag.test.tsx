// @vitest-environment happy-dom

// Dragging a browser tab out of the strip and into the workspace centre, end to
// end: the pane lands where the preview said, the page is parked while the
// preview covers it and presented exactly once when it is gone, and a tab
// dropped back on the row merges the panes.

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  afterEachHarness,
  beforeEachHarness,
  plainClick,
  renderWorkspace,
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
import { openBrowserTab, resetBrowserLayoutForTests } from "./browserTabs";
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
  const chip = document.getElementById(
    `workspace-session-tab-${makeBrowserTab(WORKSPACE, browserId).id}`,
  );
  if (chip === null) throw new Error("the browser chip did not render");
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

/** Inside the centre's bottom band (15% of 800 is 120px). */
const BOTTOM = { x: 500, y: CENTRE.top + CENTRE.height - 10 };
/** Inside the centre's centred square (40% of each axis). */
const MIDDLE = { x: 500, y: CENTRE.top + CENTRE.height / 2 };

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

  it("merges when the pane below's tab is dropped back on the row", async () => {
    const record = openBrowserTab(WORKSPACE, "https://example.test/");
    await renderSplitWorkspace();
    await plainClick(makeBrowserTab(WORKSPACE, record.browserId).id);
    await act(async () => {
      document.querySelector<HTMLButtonElement>(".browser-split-down")?.click();
    });
    expect(splitPaneFor(WORKSPACE)).not.toBeNull();

    await dragChipTo({ x: 500, y: STRIP.top + 20 }, record.browserId);

    expect(splitPaneFor(WORKSPACE)).toBeNull();
    expect(document.querySelector(".workspace-split")).toBeNull();
    // The tab is in front again, and its page is back in the whole centre.
    expect(document.querySelector(`[data-browser-id="${record.browserId}"]`)).not.toBeNull();
  });

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
