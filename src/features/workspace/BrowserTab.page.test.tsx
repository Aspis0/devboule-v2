// @vitest-environment happy-dom

// A browser tab's page belongs to the controller's lifetime, not to the pane's
// mount. React mounts the pane twice under StrictMode, and a page created twice
// is a second child webview the controller refuses — the tab then reads as
// "already open" over a page that loaded fine the first time. So: one create
// per browser id, the rectangle is sent once the create has answered (there is
// nothing to place before then), and every mount of the pane still hears the
// page report.

import { act, StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { BrowserUpdate, BrowserViewState } from "../../types/ipc";

const mocks = vi.hoisted(() => ({
  open: vi.fn(),
  present: vi.fn(),
  park: vi.fn(),
  navigate: vi.fn(),
  history: vi.fn(),
  reload: vi.fn(),
}));

// `browserRectOf` stays real: the rectangle's path from the measured element to
// the wire is one of the things this file pins.
vi.mock("./browserController", async (importOriginal) => ({
  ...(await importOriginal<typeof import("./browserController")>()),
  browserOpen: mocks.open,
  browserPresent: mocks.present,
  browserPark: mocks.park,
  browserNavigate: mocks.navigate,
  browserHistory: mocks.history,
  browserReload: mocks.reload,
}));
vi.mock("./browserTabs", () => ({ patchBrowserTab: vi.fn(), requestBrowserPopup: vi.fn() }));

import { BrowserTab } from "./BrowserTab";
import { resetBrowserPagesForTests } from "./browserPages";

const PAGE_RECT = { x: 455, y: 137, width: 770, height: 663 };
const OPENED: BrowserViewState = {
  url: "https://example.com/",
  title: "Example Domain",
  favicon: null,
  loading: false,
  canGoBack: false,
  canGoForward: false,
  error: null,
};

/** The rectangle the pane's page area reports, standing in for the layout the
 * browser would measure. Only the value's path through to the controller is
 * under test here; the geometry itself belongs to `BrowserTab.pane.test.tsx`. */
function stubPageRect(): void {
  vi.spyOn(HTMLElement.prototype, "getBoundingClientRect").mockReturnValue({
    ...PAGE_RECT,
    top: PAGE_RECT.y,
    left: PAGE_RECT.x,
    right: PAGE_RECT.x + PAGE_RECT.width,
    bottom: PAGE_RECT.y + PAGE_RECT.height,
    toJSON: () => ({}),
  } as DOMRect);
}

async function mountTab(): Promise<{ container: HTMLElement; unmount: () => Promise<void> }> {
  const container = document.createElement("div");
  document.body.append(container);
  const root = createRoot(container);
  await act(async () => {
    root.render(
      <StrictMode>
        <BrowserTab browserId="tab-1" url="https://example.com/" />
      </StrictMode>,
    );
  });
  return { container, unmount: async () => void (await act(async () => root.unmount())) };
}

describe("a browser tab's page", () => {
  beforeEach(() => {
    resetBrowserPagesForTests();
    stubPageRect();
    mocks.open.mockReset();
    mocks.present.mockReset();
    mocks.park.mockReset();
    mocks.open.mockResolvedValue(OPENED);
    mocks.present.mockResolvedValue(undefined);
    mocks.park.mockResolvedValue(undefined);
  });

  afterEach(() => {
    vi.restoreAllMocks();
    document.body.innerHTML = "";
  });

  it("is created once for the two mounts of one tab", async () => {
    const { unmount } = await mountTab();
    expect(mocks.open).toHaveBeenCalledTimes(1);
    expect(mocks.open.mock.calls[0]?.[0]).toBe("tab-1");
    await unmount();
  });

  it("reports no refusal to a tab whose page opened", async () => {
    const { container, unmount } = await mountTab();
    expect(container.querySelector(".browser-error")).toBeNull();
    await unmount();
  });

  it("is placed once the create has answered, at the pane's own rectangle", async () => {
    let answer: ((state: BrowserViewState) => void) | null = null;
    mocks.open.mockImplementation(
      () =>
        new Promise<BrowserViewState>((resolve) => {
          answer = resolve;
        }),
    );
    const { unmount } = await mountTab();
    // Nothing exists to place yet: the create is still in flight.
    expect(mocks.present).not.toHaveBeenCalled();

    await act(async () => {
      answer?.(OPENED);
    });

    expect(mocks.present).toHaveBeenCalledTimes(1);
    expect(mocks.present).toHaveBeenCalledWith("tab-1", PAGE_RECT);
    await unmount();
  });

  it("keeps reporting to the mount that is still there", async () => {
    const { container, unmount } = await mountTab();
    const report = mocks.open.mock.calls[0]?.[2] as (update: BrowserUpdate) => void;
    expect(typeof report).toBe("function");

    await act(async () => {
      report({
        kind: "state",
        url: "https://example.com/next",
        title: "Next",
        favicon: null,
        loading: false,
        canGoBack: true,
        canGoForward: false,
        error: null,
      });
    });

    const address = container.querySelector<HTMLInputElement>(".browser-address");
    expect(address?.value).toBe("https://example.com/next");
    expect(
      container.querySelector<HTMLElement>('[role="tabpanel"]')?.getAttribute("aria-label"),
    ).toBe("Next");
    await unmount();
  });

  it("parks its page when the tab leaves the front, and keeps it owned", async () => {
    const { unmount } = await mountTab();
    await unmount();
    expect(mocks.park).toHaveBeenCalledWith("tab-1");
    expect(mocks.open).toHaveBeenCalledTimes(1);
  });
});
