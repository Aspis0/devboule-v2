// @vitest-environment happy-dom

import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => ({
  loadDesignHistory: vi.fn(),
  sessionsList: vi.fn(),
}));

vi.mock("./designHistory", async () => {
  const actual = await vi.importActual<typeof import("./designHistory")>("./designHistory");
  return { ...actual, loadDesignHistory: mocks.loadDesignHistory };
});

vi.mock("../../lib/tauri", () => ({
  sessionsList: mocks.sessionsList,
}));

import { DesignHistoryList } from "./DesignHistoryList";

describe("DesignHistoryList", () => {
  beforeEach(() => {
    mocks.loadDesignHistory.mockReset();
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
  });

  afterEach(() => {
    document.body.replaceChildren();
  });

  it("renders a read failure instead of saying there is no history", async () => {
    mocks.loadDesignHistory.mockResolvedValue(null);
    const container = document.createElement("div");
    document.body.appendChild(container);
    const root = createRoot(container);

    await act(async () => {
      root.render(<DesignHistoryList onOpen={vi.fn()} />);
      await Promise.resolve();
      await Promise.resolve();
    });

    expect(container.querySelector(".design-history-read-failure")?.textContent).toBe(
      "The daemon did not answer, so your saved designs could not be read.",
    );
    expect(container.querySelector(".design-history-empty")).toBeNull();

    await act(async () => root.unmount());
  });

  it("renders no history only after a successful empty read", async () => {
    mocks.loadDesignHistory.mockResolvedValue([]);
    const container = document.createElement("div");
    document.body.appendChild(container);
    const root = createRoot(container);

    await act(async () => {
      root.render(<DesignHistoryList onOpen={vi.fn()} />);
      await Promise.resolve();
      await Promise.resolve();
    });

    expect(container.querySelector(".design-history-empty")?.textContent).toBe(
      "No design history yet.",
    );
    expect(container.querySelector(".design-history-read-failure")).toBeNull();

    await act(async () => root.unmount());
  });

  it("keeps both read failures distinct when history and roster are unavailable", async () => {
    mocks.loadDesignHistory.mockResolvedValue(null);
    mocks.sessionsList.mockRejectedValue(new Error("daemon unavailable"));
    const container = document.createElement("div");
    document.body.appendChild(container);
    const root = createRoot(container);

    await act(async () => {
      root.render(<DesignHistoryList onOpen={vi.fn()} />);
      await Promise.resolve();
      await Promise.resolve();
    });

    expect(container.textContent).toContain("your saved designs could not be read");
    expect(container.textContent).toContain("these designs could not be checked");
    expect(container.querySelector(".design-history-empty")).toBeNull();
    expect(container.querySelector(".design-history-gone")).toBeNull();

    await act(async () => root.unmount());
  });

  it("renders rows as gone when an empty roster was read successfully", async () => {
    const container = document.createElement("div");
    document.body.appendChild(container);
    const root = createRoot(container);
    const onOpen = vi.fn();

    await act(async () => {
      root.render(<DesignHistoryList onOpen={onOpen} />);
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

  it("does not claim rows are gone when the roster read fails", async () => {
    mocks.sessionsList.mockRejectedValue(new Error("daemon unavailable"));
    const container = document.createElement("div");
    document.body.appendChild(container);
    const root = createRoot(container);
    const onOpen = vi.fn();

    await act(async () => {
      root.render(<DesignHistoryList onOpen={onOpen} />);
      await Promise.resolve();
      await Promise.resolve();
    });

    const region = container.querySelector('[aria-label="Design history"]');
    const row = region?.querySelector("li");
    expect(row?.textContent).toContain("Create the final card");
    expect(row?.textContent).not.toContain("The transcript is no longer in the journal.");
    expect(row?.querySelector("a, button")).toBeNull();
    expect(region?.querySelectorAll(".design-history-unavailable")).toHaveLength(1);
    expect(region?.textContent).toContain(
      "The daemon did not answer, so these designs could not be checked.",
    );

    await act(async () => root.unmount());
  });

  it("renders an available row as an open button", async () => {
    const entry = {
      sessionId: "saved-session",
      peerSessionId: "peer-1",
      createdAtMs: null,
      title: "Open the final card",
      savedAtMs: 100,
      origin: "design" as const,
    };
    mocks.loadDesignHistory.mockResolvedValue([entry]);
    mocks.sessionsList.mockResolvedValue([
      { id: entry.sessionId, peerSessionId: entry.peerSessionId },
    ]);
    const onOpen = vi.fn();
    const container = document.createElement("div");
    document.body.appendChild(container);
    const root = createRoot(container);

    await act(async () => {
      root.render(<DesignHistoryList onOpen={onOpen} />);
      await Promise.resolve();
      await Promise.resolve();
    });

    const button = container.querySelector<HTMLButtonElement>(".design-history-open");
    expect(button?.textContent).toContain("Open the final card");
    expect(container.querySelector(".design-history-gone")).toBeNull();
    await act(async () => button?.click());
    expect(onOpen).toHaveBeenCalledWith(entry);

    await act(async () => root.unmount());
  });

  it("renders the live session as the design already on the canvas", async () => {
    const entry = {
      sessionId: "live-session",
      peerSessionId: "peer-1",
      createdAtMs: null,
      title: "Current design",
      savedAtMs: 100,
      origin: "design" as const,
    };
    mocks.loadDesignHistory.mockResolvedValue([entry]);
    mocks.sessionsList.mockResolvedValue([]);
    const container = document.createElement("div");
    document.body.appendChild(container);
    const root = createRoot(container);

    await act(async () => {
      root.render(<DesignHistoryList liveSessionId={entry.sessionId} onOpen={vi.fn()} />);
      await Promise.resolve();
      await Promise.resolve();
    });

    const row = container.querySelector(".design-history-row");
    expect(row?.textContent).toContain("This design is on the canvas.");
    expect(row?.querySelector("button")).toBeNull();
    expect(row?.querySelector(".design-history-gone")).toBeNull();

    await act(async () => root.unmount());
  });
});
