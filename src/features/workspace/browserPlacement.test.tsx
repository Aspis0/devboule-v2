// @vitest-environment happy-dom

// What a page does when something opens over it. The page is a child webview —
// a native window above the app — so a menu or a dialog cannot be painted on
// top of it: the page is parked while an overlay covers it and placed again
// when the last one goes. Both a real primitive and a real page area are
// mounted here; only the controller is a spy, because that is the seam the
// behaviour is made of.
//
// happy-dom computes no layout, so the two boxes are stubbed. The
// intersection test, the registry and the frame coalescing are all real.

import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => ({
  present: vi.fn(),
  park: vi.fn(),
  open: vi.fn(),
}));

vi.mock("./browserController", async (importOriginal) => ({
  ...(await importOriginal<typeof import("./browserController")>()),
  browserPresent: mocks.present,
  browserPark: mocks.park,
  browserOpen: mocks.open,
}));
vi.mock("./browserTabs", () => ({ patchBrowserTab: vi.fn(), requestBrowserPopup: vi.fn() }));

import { ConfirmDialog } from "../../components/ConfirmDialog";
import { BrowserTab } from "./BrowserTab";
import { resetBrowserPagesForTests } from "./browserPages";
import { resetBrowserOverlaysForTests } from "./browserOverlays";

/** The page's area: the whole centre area under the chrome row. */
const PAGE_RECT = { x: 455, y: 137, width: 770, height: 663 };
/** A dialog is a centred modal over everything, so it always covers. */
const DIALOG_RECT = { x: 340, y: 200, width: 600, height: 400 };

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

/**
 * The layout happy-dom does not compute: the page area reports `page`, and
 * the open dialog reports `dialog` at whichever box this names. Everything
 * else has no box at all, so a stray overlay can never look like it covers
 * something by accident.
 */
function stubLayout(
  page: { x: number; y: number; width: number; height: number },
  dialog: { x: number; y: number; width: number; height: number } | null,
): void {
  vi.spyOn(HTMLElement.prototype, "getBoundingClientRect").mockImplementation(function (
    this: HTMLElement,
  ) {
    if (this.hasAttribute("data-browser-id")) return box(page);
    if (dialog !== null && this.closest(".confirm-dialog") !== null) return box(dialog);
    return box({ x: 0, y: 0, width: 0, height: 0 });
  });
}

/** One animation frame's worth of waiting: the placement sends on frames. */
async function frame(): Promise<void> {
  await act(async () => {
    await new Promise((resolve) => requestAnimationFrame(() => resolve(null)));
  });
}

async function mount(children: React.ReactNode): Promise<{ unmount: () => Promise<void> }> {
  const container = document.createElement("div");
  document.body.append(container);
  const root = createRoot(container);
  await act(async () => {
    root.render(children);
  });
  return { unmount: async () => void (await act(async () => root.unmount())) };
}

describe("a page under an overlay", () => {
  beforeEach(() => {
    resetBrowserPagesForTests();
    resetBrowserOverlaysForTests();
    stubLayout(PAGE_RECT, null);
    mocks.present.mockReset();
    mocks.park.mockReset();
    mocks.present.mockResolvedValue(undefined);
    mocks.park.mockResolvedValue(undefined);
    mocks.open.mockResolvedValue({
      url: "https://example.com/",
      title: null,
      favicon: null,
      loading: false,
      canGoBack: false,
      canGoForward: false,
      error: null,
    });
  });

  afterEach(() => {
    vi.restoreAllMocks();
    document.body.innerHTML = "";
  });

  it("parks the page while a dialog covers it, and places it again after", async () => {
    stubLayout(PAGE_RECT, DIALOG_RECT);
    const { unmount } = await mount(
      <>
        <BrowserTab browserId="tab-1" url="https://example.com/" />
        <ConfirmDialog
          open
          title="Close this tab?"
          message="The page goes with it."
          confirmLabel="Close"
          tone="danger"
          onConfirm={() => undefined}
          onCancel={() => undefined}
        />
      </>,
    );
    await frame();

    expect(mocks.park).toHaveBeenCalledWith("tab-1");
    expect(mocks.present).not.toHaveBeenCalled();
    await unmount();
  });

  it("leaves the page alone for an overlay that does not touch it", async () => {
    stubLayout(PAGE_RECT, { x: 0, y: 0, width: 120, height: 40 });
    const { unmount } = await mount(
      <>
        <BrowserTab browserId="tab-1" url="https://example.com/" />
        <ConfirmDialog
          open
          title="Settings"
          message="Nothing over the page."
          confirmLabel="Save"
          tone="accent"
          onConfirm={() => undefined}
          onCancel={() => undefined}
        />
      </>,
    );
    await frame();

    expect(mocks.park).not.toHaveBeenCalled();
    expect(mocks.present).toHaveBeenCalledWith("tab-1", PAGE_RECT);
    await unmount();
  });

  it("places the page again at its latest rectangle when the dialog closes", async () => {
    stubLayout(PAGE_RECT, DIALOG_RECT);
    function Host({ asking }: { asking: boolean }) {
      return (
        <>
          <BrowserTab browserId="tab-1" url="https://example.com/" />
          <ConfirmDialog
            open={asking}
            title="Close this tab?"
            message="The page goes with it."
            confirmLabel="Close"
            tone="danger"
            onConfirm={() => undefined}
            onCancel={() => undefined}
          />
        </>
      );
    }
    const container = document.createElement("div");
    document.body.append(container);
    const root = createRoot(container);
    await act(async () => {
      root.render(<Host asking />);
    });
    await frame();
    expect(mocks.park).toHaveBeenCalledWith("tab-1");

    const moved = { ...PAGE_RECT, width: 500 };
    stubLayout(moved, DIALOG_RECT);
    await act(async () => {
      root.render(<Host asking={false} />);
    });
    await frame();

    expect(mocks.present).toHaveBeenCalledWith("tab-1", moved);
    await act(async () => root.unmount());
  });

  it("sends one rectangle per frame, and never the same one twice", async () => {
    const { unmount } = await mount(<BrowserTab browserId="tab-1" url="https://example.com/" />);
    await frame();
    expect(mocks.present).toHaveBeenCalledTimes(1);

    // A drag: the pane is measured at three sizes inside one frame, and then
    // the window resizes once more.
    await act(async () => {
      for (const width of [700, 660, 620, 600]) {
        stubLayout({ ...PAGE_RECT, width }, null);
        window.dispatchEvent(new Event("resize"));
      }
    });
    await frame();

    expect(mocks.present).toHaveBeenCalledTimes(2);
    expect(mocks.present).toHaveBeenLastCalledWith("tab-1", { ...PAGE_RECT, width: 600 });

    // The same rectangle again says nothing new.
    await act(async () => {
      window.dispatchEvent(new Event("resize"));
      window.dispatchEvent(new Event("scroll"));
    });
    await frame();

    expect(mocks.present).toHaveBeenCalledTimes(2);
    await unmount();
  });
});
