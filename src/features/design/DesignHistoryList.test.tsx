// @vitest-environment happy-dom

import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => ({
  loadDesignHistory: vi.fn(),
  historyEntryStatus: vi.fn(),
  sessionsList: vi.fn(),
}));

vi.mock("./designHistory", () => ({
  loadDesignHistory: mocks.loadDesignHistory,
  historyEntryStatus: mocks.historyEntryStatus,
}));

vi.mock("../../lib/tauri", () => ({
  sessionsList: mocks.sessionsList,
}));

import { DesignHistoryList } from "./DesignHistoryList";

describe("DesignHistoryList", () => {
  beforeEach(() => {
    mocks.loadDesignHistory.mockReset();
    mocks.historyEntryStatus.mockReset();
    mocks.sessionsList.mockReset();
    mocks.loadDesignHistory.mockResolvedValue([
      {
        sessionId: "missing-session",
        peerSessionId: "peer-1",
        createdAtMs: null,
        title: "Create the final card",
        savedAtMs: 100,
        origin: "design",
      },
    ]);
    mocks.sessionsList.mockResolvedValue([]);
    mocks.historyEntryStatus.mockReturnValue("gone");
  });

  afterEach(() => {
    document.body.replaceChildren();
  });

  it("renders a gone row as plain, non-openable list content", async () => {
    const container = document.createElement("div");
    document.body.appendChild(container);
    const root = createRoot(container);

    await act(async () => {
      root.render(<DesignHistoryList />);
      await Promise.resolve();
      await Promise.resolve();
    });

    const region = container.querySelector('[aria-label="Design history"]');
    const row = region?.querySelector("li");
    expect(region?.querySelector("h2")?.textContent).toBe("History");
    expect(region?.querySelector("ul")).not.toBeNull();
    expect(row?.textContent).toContain("Create the final card");
    expect(row?.textContent).toContain("The transcript is no longer in the journal.");
    expect(row?.querySelector("a, button")).toBeNull();

    await act(async () => root.unmount());
  });
});
