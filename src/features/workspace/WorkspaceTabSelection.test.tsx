// @vitest-environment happy-dom

// Multi-select (ours, not translated): the click grammar, the polite
// announcement, and the selection's own close, which always asks about the
// live set. The menu's entries, the chip and the outcomes have their own
// files.
import { act } from "react";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import {
  afterEachHarness,
  beforeEachHarness,
  clickMenuEntry,
  DIALOG_SELECTOR,
  liveRegion,
  liveSnapshot,
  menuLabels,
  modifiedClick,
  plainClick,
  pressEscape,
  pushSnapshots,
  renderWorkspace,
  rightClick,
  tabElement,
} from "./bulkCloseHarness";
import { sharedSessionController } from "./workspaceSessions";
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

    expect(document.getElementById("workspace-session-tab-session-3")).toBeNull();
    // Explicit membership setup isolates selection pruning from the opener UI.
    await act(async () => {
      const controller = sharedSessionController();
      const row = controller.getState().sessions.find((session) => session.id === "session-3");
      if (row === undefined) throw new Error("session-3 not in roster");
      controller.open(row);
    });
    expect(tabElement("session-3").className).not.toContain("workspace-session-tab-multiselected");
    expect(tabElement("session-2").className).toContain("workspace-session-tab-multiselected");
    expect(liveRegion()?.textContent).toBe("1 tab selected");
  });

  it("closing selected tabs removes them locally and clears the selection", async () => {
    await renderWorkspace();
    await modifiedClick("agent-one", "ctrlKey");
    await modifiedClick("session-2", "ctrlKey");
    await rightClick("session-2");
    await clickMenuEntry("Close 2 tabs");
    expect(document.querySelector(DIALOG_SELECTOR)).toBeNull();
    expect(document.getElementById("workspace-session-tab-agent-one")).toBeNull();
    expect(document.getElementById("workspace-session-tab-session-2")).toBeNull();
    expect(tabElement("session-3").getAttribute("aria-selected")).toBe("true");
    expect(sessionStop).not.toHaveBeenCalled();
    expect(liveRegion()?.textContent).toBe("Selection cleared");
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

  it("the selection menu counts open tabs and offers no Delete", async () => {
    await renderWorkspace();
    await modifiedClick("agent-one", "ctrlKey");
    await modifiedClick("session-2", "ctrlKey");
    await rightClick("session-2");
    expect(menuLabels()).toEqual(["Close 2 tabs"]);
  });

  it("a selection of one closes without stopping the session", async () => {
    await renderWorkspace();
    await modifiedClick("session-2", "ctrlKey");
    await rightClick("session-2");
    expect(menuLabels()).toEqual(["Close"]);
    await clickMenuEntry("Close");
    expect(document.querySelector(DIALOG_SELECTOR)).toBeNull();
    expect(document.getElementById("workspace-session-tab-session-2")).toBeNull();
    expect(sessionStop).not.toHaveBeenCalled();
  });
});
