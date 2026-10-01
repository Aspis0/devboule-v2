// @vitest-environment happy-dom

// Tool chips in the strip: their menu survives roster pushes, their close
// lands focus and the roving stop like a session close, bulk entries from a
// tool anchor never take the anchor, and middle-click closes locally.

import { act } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  afterEachHarness,
  beforeEachHarness,
  flush,
  liveSnapshot,
  menuLabels,
  menu,
  middleClick,
  chipClick,
  clickMenuEntry,
  plainClick,
  pushSnapshots,
  renderWorkspace,
  rightClick,
  settleCloseActs,
  tabElement,
  tabTitles,
} from "./bulkCloseHarness";
import { sessionStop, workspaceGitStatus } from "../../lib/tauri";
import { toolTabId } from "./strip/toolTabs";

beforeEach(() => {
  beforeEachHarness();
});

afterEach(async () => {
  await afterEachHarness();
});

function statusWithRow(path: string) {
  return {
    isGit: true,
    dirty: true,
    branch: "main",
    totals: { additions: 3, deletions: 1 },
    rows: [
      {
        path,
        renamedFrom: null,
        additions: 3,
        deletions: 1,
        status: "modified" as const,
        capped: false,
      },
    ],
    error: null,
  };
}

async function openActiveDiffTab(): Promise<string> {
  vi.mocked(workspaceGitStatus).mockResolvedValue(statusWithRow("src/writer.ts"));
  await renderWorkspace();
  await act(async () => {
    document.querySelector<HTMLButtonElement>(".workspace-file-change")?.click();
  });
  await flush();
  await act(async () => {
    document.querySelector<HTMLButtonElement>('[aria-label="Open diff in a tab"]')?.click();
  });
  await flush();
  const id = toolTabId("diff", "workspace-1", "src/writer.ts");
  expect(tabElement(id).getAttribute("aria-selected")).toBe("true");
  return id;
}

describe("tool chips", () => {
  it.each([true, false])(
    "reports the diff path copy result (success: %s) and keeps the tab open",
    async (success) => {
      const original = Object.getOwnPropertyDescriptor(navigator, "clipboard");
      const writeText = vi.fn(async () => {
        if (!success) throw new Error("denied");
      });
      Object.defineProperty(navigator, "clipboard", { value: { writeText }, configurable: true });
      try {
        const id = await openActiveDiffTab();
        await rightClick(id);
        expect(menuLabels()).not.toContain("Copy session ID");
        expect(menuLabels()).toContain("Copy relative path");
        await clickMenuEntry("Copy relative path");
        expect(writeText).toHaveBeenCalledExactlyOnceWith("src/writer.ts");
        expect(menuLabels()).toContain(success ? "Copied" : "Copy failed");
        expect(menu().parentElement?.querySelector('[role="status"]')?.textContent).toBe(
          success ? "Relative path copied" : "Relative path copy failed",
        );
        expect(tabElement(id)).not.toBeNull();
        expect(sessionStop).not.toHaveBeenCalled();
      } finally {
        if (original !== undefined) Object.defineProperty(navigator, "clipboard", original);
        else Reflect.deleteProperty(navigator, "clipboard");
      }
    },
  );

  it("a plain tool-anchor menu survives a session roster push", async () => {
    const id = await openActiveDiffTab();
    await rightClick(id);
    expect(menuLabels()).toEqual([
      "Copy relative path",
      "Close to the left",
      "Close to the right",
      "Close other tabs",
      "Close",
    ]);

    await pushSnapshots([
      liveSnapshot("agent-one", "Agent one", "acp"),
      liveSnapshot("session-2", "shell two"),
      liveSnapshot("session-3", "shell three"),
    ]);

    expect(menuLabels()).toEqual([
      "Copy relative path",
      "Close to the left",
      "Close to the right",
      "Close other tabs",
      "Close",
    ]);
  });

  it("closing a tool tab moves focus to the successor", async () => {
    const id = await openActiveDiffTab();
    await chipClick(id);
    await settleCloseActs();
    expect(document.activeElement).toBe(tabElement("session-3"));
  });

  it("the roving stop sits on the tool chip while it is active", async () => {
    const id = await openActiveDiffTab();
    expect(tabElement(id).getAttribute("tabindex")).toBe("0");
    expect(tabElement("session-3").getAttribute("tabindex")).toBe("-1");

    await act(async () => {
      const tab = tabElement(id);
      tab.focus();
      tab.dispatchEvent(new KeyboardEvent("keydown", { key: "Delete", bubbles: true }));
    });
    await settleCloseActs();
    await flush();
    expect(tabElement("session-3").getAttribute("tabindex")).toBe("0");
  });

  it("close others from a tool anchor takes every session and keeps the anchor", async () => {
    const id = await openActiveDiffTab();
    await rightClick(id);
    await clickMenuEntry("Close other tabs");
    expect(document.querySelector("[role='alertdialog']")).toBeNull();

    await settleCloseActs();
    await flush();
    await flush();
    await act(async () => {});
    expect(vi.mocked(sessionStop)).not.toHaveBeenCalled();
    expect(tabTitles()).toHaveLength(1);
    expect(tabElement(id).getAttribute("aria-selected")).toBe("true");
  });

  it("middle-click closes a tool tab locally", async () => {
    const id = await openActiveDiffTab();
    await middleClick(id);
    await settleCloseActs();
    expect(vi.mocked(sessionStop)).not.toHaveBeenCalled();
    expect(document.querySelector(`#${CSS.escape(`workspace-session-tab-${id}`)}`)).toBeNull();
    expect(tabTitles()).toHaveLength(3);
  });

  it("clicking the active tool tab keeps it selected", async () => {
    const id = await openActiveDiffTab();
    await plainClick(id);
    await flush();
    expect(tabElement(id).getAttribute("aria-selected")).toBe("true");
  });
});
