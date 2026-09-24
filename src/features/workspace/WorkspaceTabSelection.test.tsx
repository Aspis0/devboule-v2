// @vitest-environment happy-dom

// Multi-select (ours — Paseo has none): the click grammar, the polite
// announcement, and the selection's own close, which always asks about the
// live set. The menu's entries, the chip and the outcomes have their own
// files.
import { act } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  afterEachHarness,
  beforeEachHarness,
  clickDialogButton,
  clickMenuEntry,
  dialog,
  liveRegion,
  liveSnapshot,
  menuLabels,
  modifiedClick,
  plainClick,
  pressEscape,
  pushSnapshots,
  renderWorkspace,
  rightClick,
  settleCloseActs,
  tabElement,
} from "./bulkCloseHarness";
import { sessionStop } from "../../lib/tauri";

beforeEach(() => {
  beforeEachHarness();
});

afterEach(async () => {
  await afterEachHarness();
});

describe("multi-select", () => {
  it("Ctrl+click and Cmd+click toggle membership without moving the active tab", async () => {
    await renderWorkspace();
    await plainClick("agent-one");
    expect(tabElement("agent-one").getAttribute("aria-selected")).toBe("true");

    await modifiedClick("session-2", "ctrlKey");
    expect(tabElement("session-2").className).toContain("workspace-session-tab-multiselected");
    expect(tabElement("agent-one").getAttribute("aria-selected")).toBe("true");
    expect(liveRegion()?.textContent).toBe("1 tab selected");

    await modifiedClick("session-3", "metaKey");
    expect(tabElement("session-3").className).toContain("workspace-session-tab-multiselected");
    expect(liveRegion()?.textContent).toBe("2 tabs selected");

    await modifiedClick("session-2", "ctrlKey");
    expect(tabElement("session-2").className).not.toContain("workspace-session-tab-multiselected");
    expect(liveRegion()?.textContent).toBe("1 tab selected");
  });

  it("Shift+click selects the range from the active tab to the clicked one", async () => {
    await renderWorkspace();
    await plainClick("agent-one");

    await act(async () => {
      tabElement("session-3").dispatchEvent(
        new MouseEvent("click", { bubbles: true, shiftKey: true }),
      );
    });

    for (const id of ["agent-one", "session-2", "session-3"]) {
      expect(tabElement(id).className).toContain("workspace-session-tab-multiselected");
    }
    expect(liveRegion()?.textContent).toBe("3 tabs selected");
    // The range is membership, not activation: the active tab stays put.
    expect(tabElement("agent-one").getAttribute("aria-selected")).toBe("true");
    expect(tabElement("session-3").getAttribute("aria-selected")).toBe("false");
  });

  it("a plain click and Escape clear the selection, announced politely", async () => {
    await renderWorkspace();
    const region = liveRegion();
    expect(region?.getAttribute("role")).toBe("status");
    expect(region?.getAttribute("aria-live")).toBe("polite");

    await modifiedClick("session-2", "ctrlKey");
    expect(liveRegion()?.textContent).toBe("1 tab selected");

    await pressEscape();
    expect(tabElement("session-2").className).not.toContain("workspace-session-tab-multiselected");
    expect(liveRegion()?.textContent).toBe("Selection cleared");

    await modifiedClick("session-2", "ctrlKey");
    await plainClick("session-3");
    expect(tabElement("session-2").className).not.toContain("workspace-session-tab-multiselected");
    expect(liveRegion()?.textContent).toBe("Selection cleared");
  });

  it("a removed session is pruned from the stored selection, and stays unselected when it returns", async () => {
    await renderWorkspace();
    await modifiedClick("session-2", "ctrlKey");
    await modifiedClick("session-3", "ctrlKey");
    expect(liveRegion()?.textContent).toBe("2 tabs selected");

    // The daemon's roster no longer has shell three.
    await pushSnapshots([
      liveSnapshot("agent-one", "Agent one", "acp"),
      liveSnapshot("session-2", "shell two"),
    ]);
    expect(liveRegion()?.textContent).toBe("1 tab selected");

    // It comes back with the same id: it does NOT silently regain its
    // selection — the stored set was pruned, not merely filtered.
    await pushSnapshots([
      liveSnapshot("agent-one", "Agent one", "acp"),
      liveSnapshot("session-2", "shell two"),
      liveSnapshot("session-3", "shell three"),
    ]);

    expect(tabElement("session-3").className).not.toContain("workspace-session-tab-multiselected");
    expect(tabElement("session-2").className).toContain("workspace-session-tab-multiselected");
    expect(liveRegion()?.textContent).toBe("1 tab selected");
  });

  it("cancelling the selection's ask leaves the selection exactly as it was", async () => {
    await renderWorkspace();
    await modifiedClick("agent-one", "ctrlKey");
    await modifiedClick("session-2", "ctrlKey");

    await rightClick("session-2");
    await clickMenuEntry("Close 2 tabs");
    await clickDialogButton("Cancel");

    // The selection ends only when the ask is CONFIRMED. A cancel — or an
    // Escape, or a dismissal — leaves it and its announcement standing.
    expect(document.querySelector('[role="dialog"]')).toBeNull();
    expect(liveRegion()?.textContent).toBe("2 tabs selected");
    expect(tabElement("agent-one").className).toContain("workspace-session-tab-multiselected");
    expect(tabElement("session-2").className).toContain("workspace-session-tab-multiselected");
    expect(vi.mocked(sessionStop)).not.toHaveBeenCalled();
  });

  it("the selection menu closes when one of its targets is removed", async () => {
    await renderWorkspace();
    await modifiedClick("agent-one", "ctrlKey");
    await modifiedClick("session-2", "ctrlKey");

    await rightClick("session-2");
    expect(menuLabels()).toEqual(["Close 2 tabs"]);

    // One of the menu's OWN targets (Agent one) leaves the roster while the
    // ANCHOR — shell two, the right-clicked tab — stays. Anchor-only
    // validity would keep the menu open over a changed target set.
    await pushSnapshots([
      liveSnapshot("session-2", "shell two"),
      liveSnapshot("session-3", "shell three"),
    ]);

    expect(document.querySelector("[role='menu']")).toBeNull();
  });

  it("right-click on a selected tab offers the selection's own close, with the counts", async () => {
    await renderWorkspace();
    await modifiedClick("agent-one", "ctrlKey");
    await modifiedClick("session-2", "ctrlKey");

    await rightClick("session-2");

    // The selection menu has no Delete: it cannot offer destruction by
    // count.
    expect(menuLabels()).toEqual(["Close 2 tabs"]);
    await clickMenuEntry("Close 2 tabs");

    const confirm = dialog();
    expect(confirm.textContent).toContain("Close 2 tabs?");
    expect(confirm.textContent).toContain(
      "This will archive 1 agent(s) and archive 1 terminal(s).",
    );

    await clickDialogButton("Close");
    await settleCloseActs();

    expect(vi.mocked(sessionStop)).toHaveBeenCalledTimes(2);
    expect(vi.mocked(sessionStop)).toHaveBeenCalledWith("agent-one");
    expect(vi.mocked(sessionStop)).toHaveBeenCalledWith("session-2");
  });

  it("a selection of one still asks before closing, naming the live set", async () => {
    await renderWorkspace();
    await modifiedClick("session-2", "ctrlKey");

    await rightClick("session-2");
    expect(menuLabels()).toEqual(["Close"]);

    await clickMenuEntry("Close");

    // Even shrunken to one, the selection menu goes through the ask — never
    // straight at the daemon.
    const confirm = dialog();
    expect(confirm.textContent).toContain("Close 1 tab?");
    expect(vi.mocked(sessionStop)).not.toHaveBeenCalled();

    await clickDialogButton("Close");
    await settleCloseActs();
    expect(vi.mocked(sessionStop)).toHaveBeenCalledWith("session-2");
  });
});
