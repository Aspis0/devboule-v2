// @vitest-environment happy-dom

// Closing a browser chip from the strip. The page is a child webview in the
// Rust process and the chip's record is persisted by the strip, so a close has
// to take both: a record left behind is a chip that names a page already gone.

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  afterEachHarness,
  beforeEachHarness,
  chipClick,
  renderWorkspace,
  settleCloseActs,
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
import { browserTabsFor, openBrowserTab, resetBrowserLayoutForTests } from "./browserTabs";
import { localWorkspaceKey, type WorkspaceKey } from "./hosts/hostIdentity";
import { makeBrowserTab } from "./strip/toolTabs";

const WORKSPACE = localWorkspaceKey("workspace-1") as WorkspaceKey;

beforeEach(() => {
  beforeEachHarness();
  mocks.close.mockReset();
  mocks.close.mockResolvedValue(undefined);
  mocks.open.mockReset();
  mocks.open.mockResolvedValue({
    url: "https://example.test/",
    title: null,
    favicon: null,
    loading: false,
    canGoBack: false,
    canGoForward: false,
    error: null,
  });
  resetBrowserPagesForTests();
  resetBrowserLayoutForTests();
});

afterEach(async () => {
  await afterEachHarness();
});

describe("closing a browser chip", () => {
  it("disposes the page and drops the chip's record", async () => {
    const record = openBrowserTab(WORKSPACE, "https://example.test/");
    await renderWorkspace();

    await chipClick(makeBrowserTab(WORKSPACE, record.browserId).id);
    await settleCloseActs();

    expect(mocks.close).toHaveBeenCalledWith(record.browserId);
    expect(browserTabsFor(WORKSPACE)).toEqual([]);
  });
});
