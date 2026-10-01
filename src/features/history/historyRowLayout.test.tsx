// @vitest-environment happy-dom

// Overflow guard for the History row, rewritten for the two-line mechanism
// (pass 5): the row is a fixed 42 px flex line, the copy is the only child
// that may shrink, and the actions never shrink. The assertions below read
// computed styles off a real rendered row; happy-dom computes no layout, so
// a squeezed-at-210 px proof still belongs to the photographs (the original
// header named them: a meta line wrapping one character per line, 785 px
// tall, while the action overflowed sideways and grew the panel scrollbar).
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("../../lib/tauri", () => ({
  journalUsage: vi.fn(async () => ({
    totalBytes: 400,
    sessionCount: 1,
    deletedByUser: 0,
    deletedByRetention: 0,
    unreclaimable: { bytesOver: 0, sessionsOver: 0, agedOut: 0 },
    limits: {
      snapshotEveryBytes: 65_536,
      sessionMaxBytes: 512,
      maxBytes: 1024,
      maxSessions: 10,
      maxAgeMs: 0,
    },
    perSession: [
      {
        id: "session-long",
        title: "Work on the requested design change for the sidebar agent list panel",
        kind: "acp",
        bytes: 400,
        updatedAtMs: Date.now(),
      },
    ],
  })),
  sessionsList: vi.fn(async () => []),
  sessionDelete: vi.fn(),
  sessionResume: vi.fn(),
  workspaceGitStatus: vi.fn(async () => ({
    isGit: false,
    dirty: false,
    branch: null,
    totals: { additions: 0, deletions: 0 },
    rows: [],
    error: null,
  })),
}));

import { HistoryPanel } from "./HistoryPanel";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

describe("a history row holds its two lines at any width", () => {
  let container: HTMLDivElement | null = null;
  let root: Root | null = null;

  beforeEach(() => {
    const style = document.createElement("style");
    style.setAttribute("data-history-layout-proof", "");
    style.textContent = readFileSync(resolve(import.meta.dirname, "./history.css"), "utf8");
    document.head.appendChild(style);
  });

  afterEach(async () => {
    await act(async () => root?.unmount());
    root = null;
    container?.remove();
    container = null;
    document.querySelectorAll("style[data-history-layout-proof]").forEach((el) => el.remove());
  });

  async function renderRow(): Promise<HTMLElement> {
    container = document.createElement("div");
    container.style.width = "210px";
    document.body.appendChild(container);
    root = createRoot(container);
    await act(async () => {
      root!.render(<HistoryPanel search="" />);
    });
    for (let hop = 0; hop < 4; hop += 1) {
      await act(async () => undefined);
    }
    const row = container.querySelector<HTMLElement>(".history-row");
    if (!row) throw new Error("History row did not render");
    return row;
  }

  it("fixes the row height so wrapped copy cannot grow it", async () => {
    const row = await renderRow();
    expect(getComputedStyle(row).height).toBe("42px");
  });

  it("clips the copy and the title line instead of growing", async () => {
    // min-width is not asserted: happy-dom reports "0" for flex items with
    // and without the rule, so the declaration is guarded by review, not here.
    const row = await renderRow();
    const copy = row.querySelector<HTMLElement>(".history-row-copy");
    const titleLine = row.querySelector<HTMLElement>(".history-row-title-line");
    if (!copy || !titleLine) throw new Error("History row copy did not render");
    expect(getComputedStyle(copy).overflow).toBe("hidden");
    expect(getComputedStyle(titleLine).overflow).toBe("hidden");
  });

  it("ellipsizes the title instead of wrapping it", async () => {
    const row = await renderRow();
    const title = row.querySelector<HTMLElement>(".workspace-row-title");
    if (!title) throw new Error("History row title did not render");
    const style = getComputedStyle(title);
    expect(style.whiteSpace).toBe("nowrap");
    expect(style.overflow).toBe("hidden");
    expect(style.textOverflow).toBe("ellipsis");
  });

  it("keeps the actions out of the shrink order", async () => {
    const row = await renderRow();
    const actions = row.querySelector<HTMLElement>(".history-row-actions");
    if (!actions) throw new Error("History actions did not render");
    expect(getComputedStyle(actions).flexShrink).toBe("0");
  });
});
