// @vitest-environment happy-dom

// A page in the lower pane. The page is a child webview in Rust, drawn over a
// rectangle this app measures, so two things have to hold when the pane
// changes: the page is told the new rectangle (through the one-frame
// coalescing that already exists), and it is never opened twice — moving a
// page between panes must not reload it.

import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => ({
  open: vi.fn(),
  present: vi.fn(),
  park: vi.fn(),
  navigate: vi.fn(),
  history: vi.fn(),
  reload: vi.fn(),
}));

vi.mock("../browserController", async (importOriginal) => ({
  ...(await importOriginal<typeof import("../browserController")>()),
  browserOpen: mocks.open,
  browserPresent: mocks.present,
  browserPark: mocks.park,
  browserNavigate: mocks.navigate,
  browserHistory: mocks.history,
  browserReload: mocks.reload,
}));
vi.mock("../browserTabs", () => ({ patchBrowserTab: vi.fn(), requestBrowserPopup: vi.fn() }));

import { BrowserTab } from "../BrowserTab";
import { resetBrowserPagesForTests } from "../browserPages";
import { resetBrowserOverlaysForTests } from "../browserOverlays";
import { localWorkspaceKey, type WorkspaceKey } from "../hosts/hostIdentity";
import { SplitPane } from "./SplitPane";

const WORKSPACE = localWorkspaceKey("w-1") as WorkspaceKey;

/** The page area's box, and where the pane's own chrome sits above it. */
const TOP_PANE_RECT = { x: 400, y: 60, width: 800, height: 420 };
const BOTTOM_PANE_RECT = { x: 400, y: 490, width: 800, height: 360 };

function box(rect: { x: number; y: number; width: number; height: number }): DOMRect {
  return {
    ...rect,
    top: rect.y,
    left: rect.x,
    right: rect.x + rect.width,
    bottom: rect.y + rect.height,
    toJSON: () => ({}),
  } as DOMRect;
}

/** happy-dom computes no layout: the page area reports whatever `page` says,
 * and the panes report the boxes they are given. */
function stubLayout(page: { x: number; y: number; width: number; height: number }): void {
  vi.spyOn(HTMLElement.prototype, "getBoundingClientRect").mockImplementation(function (
    this: HTMLElement,
  ) {
    if (this.hasAttribute("data-browser-id")) return box(page);
    if (this.classList.contains("workspace-split-bottom")) return box(BOTTOM_PANE_RECT);
    if (this.classList.contains("workspace-split-top")) return box(TOP_PANE_RECT);
    if (this.classList.contains("workspace-split")) {
      return box({
        x: TOP_PANE_RECT.x,
        y: TOP_PANE_RECT.y,
        width: TOP_PANE_RECT.width,
        height: TOP_PANE_RECT.y + TOP_PANE_RECT.height + BOTTOM_PANE_RECT.height - TOP_PANE_RECT.y,
      });
    }
    return box({ x: 0, y: 0, width: 0, height: 0 });
  });
}

/** A mount needs two turns: the create's answer arrives first, and only then
 * does the placement have a rectangle to send on the next frame. */
async function frames(count = 3): Promise<void> {
  await act(async () => {
    for (let turn = 0; turn < count; turn += 1) {
      await new Promise((resolve) => requestAnimationFrame(() => resolve(null)));
    }
  });
}

let container: HTMLDivElement;
let root: ReturnType<typeof createRoot>;

/** The workspace with the page in the top pane, and the same page moved into
 * the lower pane by the split record naming it. */
function renderWorkspace(pane: "top" | "bottom"): void {
  const page = (
    <BrowserTab browserId="tab-1" url="https://example.test/" workspaceKey={WORKSPACE} />
  );
  act(() => {
    root.render(
      <SplitPane
        split={{ size: 0.55, lowerTabId: "tool:browser:w-1:tab-1" }}
        onResize={() => undefined}
        onMerge={() => undefined}
        lowerLabel="example.test"
        lower={<div>{pane === "bottom" ? page : null}</div>}
      >
        <div>{pane === "top" ? page : <p>chat</p>}</div>
      </SplitPane>,
    );
  });
}

describe("a page in the lower pane", () => {
  beforeEach(() => {
    resetBrowserPagesForTests();
    resetBrowserOverlaysForTests();
    stubLayout(TOP_PANE_RECT);
    mocks.open.mockReset();
    mocks.present.mockReset();
    mocks.park.mockReset();
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
    container = document.createElement("div");
    document.body.append(container);
    root = createRoot(container);
  });

  afterEach(() => {
    act(() => root.unmount());
    container.remove();
    vi.restoreAllMocks();
  });

  it("is placed over the lower pane's own rectangle", async () => {
    stubLayout({
      x: BOTTOM_PANE_RECT.x,
      y: BOTTOM_PANE_RECT.y + 30,
      width: BOTTOM_PANE_RECT.width,
      height: BOTTOM_PANE_RECT.height - 30,
    });
    renderWorkspace("bottom");
    await frames();

    expect(mocks.present).toHaveBeenCalledWith("tab-1", {
      x: BOTTOM_PANE_RECT.x,
      y: BOTTOM_PANE_RECT.y + 30,
      width: BOTTOM_PANE_RECT.width,
      height: BOTTOM_PANE_RECT.height - 30,
    });
  });

  it("follows the divider without being opened again, so the page never reloads", async () => {
    renderWorkspace("top");
    await frames();
    expect(mocks.open).toHaveBeenCalledTimes(1);
    expect(mocks.present).toHaveBeenLastCalledWith("tab-1", TOP_PANE_RECT);

    // The move: same page, same component identity, lower pane.
    stubLayout({
      x: BOTTOM_PANE_RECT.x,
      y: BOTTOM_PANE_RECT.y,
      width: BOTTOM_PANE_RECT.width,
      height: BOTTOM_PANE_RECT.height,
    });
    renderWorkspace("bottom");
    await frames();

    expect(mocks.open).toHaveBeenCalledTimes(1);
    expect(mocks.present).toHaveBeenLastCalledWith("tab-1", BOTTOM_PANE_RECT);
  });

  it("sends one rectangle per frame while the divider moves the pane under it", async () => {
    renderWorkspace("bottom");
    await frames();
    const placed = mocks.present.mock.calls.length;

    // A drag: the lower pane is measured at four heights inside one frame.
    await act(async () => {
      for (const height of [340, 300, 260, 220]) {
        stubLayout({ ...BOTTOM_PANE_RECT, height });
        window.dispatchEvent(new Event("resize"));
      }
    });
    await frames();

    expect(mocks.present.mock.calls.length).toBe(placed + 1);
    expect(mocks.present).toHaveBeenLastCalledWith("tab-1", { ...BOTTOM_PANE_RECT, height: 220 });
    expect(mocks.open).toHaveBeenCalledTimes(1);
  });
});
