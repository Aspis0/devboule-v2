// @vitest-environment happy-dom

// A browser tab's page is a child webview, so the pane's job is to be the
// rectangle that webview is placed over: the whole centre area, not a chat
// column inside it. The chrome is one row across the top of that area and the
// page takes everything below it — an inset either side would be an inset the
// native child never covers.

import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { act, StrictMode } from "react";
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

const PANE_RECT = { x: 455, y: 49, width: 770, height: 751 };

/** The rule body a selector owns, for the assertions jsdom cannot make: happy
 * DOM lays nothing out, so the geometry has to be read off the stylesheet. */
function ruleBody(selector: string): string {
  const css = readFileSync(resolve(import.meta.dirname, "BrowserTab.css"), "utf8");
  const at = css.indexOf(`${selector} {`);
  expect(at, `${selector} has no rule`).toBeGreaterThan(-1);
  return css.slice(at, css.indexOf("}", at));
}

async function mountPane(): Promise<HTMLElement> {
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
  return container;
}

describe("the browser tab's pane", () => {
  beforeEach(() => {
    resetBrowserPagesForTests();
    mocks.open.mockReset();
    mocks.present.mockReset();
    mocks.park.mockReset();
    mocks.open.mockResolvedValue({
      url: "https://example.com/",
      title: null,
      favicon: null,
      loading: false,
      canGoBack: false,
      canGoForward: false,
      error: null,
    });
    mocks.present.mockResolvedValue(undefined);
    mocks.park.mockResolvedValue(undefined);
  });

  afterEach(() => {
    document.body.innerHTML = "";
  });

  it("is not the conversation lane, which would cap it at 760 px and centre it", async () => {
    const container = await mountPane();
    const pane = container.querySelector<HTMLElement>('[role="tabpanel"]');
    expect(pane?.className.split(" ")).toEqual(["workspace-browser-pane"]);
    expect(ruleBody(".workspace-browser-pane")).not.toMatch(/max-width|margin|padding/);
  });

  it("gives the page the pane's whole content box, so the measured rect is the rect placed", () => {
    const page = ruleBody(".browser-page-area");
    expect(page).not.toMatch(/margin|padding|border/);
    expect(page).toMatch(/flex:\s*1/);
  });

  it("puts the chrome on one 36 px row and lets the address take the rest", () => {
    expect(ruleBody(".browser-chrome")).toMatch(/height:\s*36px/);
    expect(ruleBody(".browser-address")).toMatch(/flex:\s*1/);
  });

  it("hands the controller the rectangle the page area measured, unshifted", async () => {
    vi.spyOn(HTMLElement.prototype, "getBoundingClientRect").mockReturnValue({
      ...PANE_RECT,
      top: PANE_RECT.y,
      left: PANE_RECT.x,
      right: PANE_RECT.x + PANE_RECT.width,
      bottom: PANE_RECT.y + PANE_RECT.height,
      toJSON: () => ({}),
    } as DOMRect);
    await mountPane();
    await act(async () => {
      await Promise.resolve();
    });
    expect(mocks.present).toHaveBeenCalledWith("tab-1", PANE_RECT);
    vi.restoreAllMocks();
  });
});
