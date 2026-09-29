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
  middleClick,
  chipClick,
  clickDialogButton,
  clickMenuEntry,
  dialog,
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
  it("a plain tool-anchor menu survives a session roster push", async () => {
    const id = await openActiveDiffTab();
    await rightClick(id);
    expect(menuLabels()).toEqual([
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
    const confirm = dialog();
    expect(confirm.textContent).toContain("Close other tabs?");
    expect(confirm.textContent).not.toContain("tab closes too");

    await clickDialogButton("Close");
    await settleCloseActs();
    await flush();
    await flush();
    await act(async () => {});
    expect(vi.mocked(sessionStop)).toHaveBeenCalledTimes(3);
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
