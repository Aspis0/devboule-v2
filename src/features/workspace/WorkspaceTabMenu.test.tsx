// @vitest-environment happy-dom

// The tab context menu: Paseo's close entries, our Delete after a separator,
// the three ways to open it — including a right-click anywhere on the row —
// and the dismissals that restore focus. The chip, the selection grammar and
// the close outcomes have their own files.
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act } from "react";
import {
  afterEachHarness,
  beforeEachHarness,
  clickDialogButton,
  clickMenuEntry,
  contextMenuKey,
  defaultSessions,
  dialog,
  liveSnapshot,
  menu,
  menuLabels,
  pushSnapshots,
  recoveredAgentSession,
  renderWorkspace,
  resizeWindow,
  rightClick,
  settleCloseActs,
  shiftF10,
  tabElement,
  terminalSession,
} from "./bulkCloseHarness";
import { sessionClose, sessionStop, sessionsList } from "../../lib/tauri";

beforeEach(() => {
  beforeEachHarness();
});

afterEach(async () => {
  await afterEachHarness();
});

describe("the tab context menu", () => {
  it("lists Paseo's close entries, then Delete after a separator in the destructive tone", async () => {
    await renderWorkspace();

    await rightClick("agent-one");

    expect(menuLabels()).toEqual([
      "Close to the left",
      "Close to the right",
      "Close other tabs",
      "Close",
      "Delete",
    ]);
    const deleteEntry = [...document.querySelectorAll<HTMLButtonElement>("[role='menuitem']")].find(
      (item) => item.textContent === "Delete",
    );
    expect(deleteEntry?.className).toContain("workspace-menu-option-destructive");
    expect(menu().querySelector("[role='separator']")).not.toBeNull();
    const left = [...document.querySelectorAll<HTMLButtonElement>("[role='menuitem']")].find(
      (item) => item.textContent === "Close to the left",
    );
    expect(left?.disabled).toBe(true);
  });

  it("Shift+F10 opens the menu from the keyboard and focuses its first entry", async () => {
    await renderWorkspace();

    await shiftF10("session-2");

    expect(menuLabels()).toHaveLength(5);
    expect(document.activeElement?.textContent).toBe("Close to the left");
    expect(document.activeElement?.getAttribute("role")).toBe("menuitem");
  });

  it("the ContextMenu key opens the same menu from the keyboard", async () => {
    await renderWorkspace();

    await contextMenuKey("session-2");

    expect(menuLabels()).toHaveLength(5);
    expect(document.activeElement?.getAttribute("role")).toBe("menuitem");
  });

  it("a right-click over the close chip reaches the row and opens the menu", async () => {
    // Wiring, not hit-testing: the handler lives on the ROW, the chip is a
    // child of it, so a right-click over the chip bubbles to the same
    // handler. That nothing covers the label is the chip CSS's width —
    // pinned by the strip source test — and, in the end, the live check.
    await renderWorkspace();
    const chip = tabElement("session-2").parentElement?.querySelector(".workspace-session-chip");
    if (chip === null || chip === undefined) throw new Error("chip did not render");

    await act(async () => {
      chip.dispatchEvent(new MouseEvent("contextmenu", { bubbles: true }));
    });

    expect(menuLabels()).toHaveLength(5);
  });

  it("Close on an agent without a process archives at once — no ask", async () => {
    vi.mocked(sessionsList).mockResolvedValue([
      recoveredAgentSession("agent-old", "Old transcript"),
      terminalSession("session-2", "shell two"),
      terminalSession("session-3", "shell three"),
    ]);
    await renderWorkspace();

    await rightClick("agent-old");
    await clickMenuEntry("Close");
    await settleCloseActs();

    expect(document.querySelector('[role="dialog"]')).toBeNull();
    expect(vi.mocked(sessionStop)).toHaveBeenCalledWith("agent-old");
    expect(document.querySelector("#workspace-session-tab-agent-old")).toBeNull();
  });

  it("Delete asks, and confirming destroys through session_close", async () => {
    await renderWorkspace();

    await rightClick("session-2");
    await clickMenuEntry("Delete");
    const confirm = dialog();
    expect(confirm.textContent).toContain("Delete “shell two”?");
    expect(confirm.textContent).toContain("destroys the session");

    await clickDialogButton("Cancel");
    expect(vi.mocked(sessionClose)).not.toHaveBeenCalled();

    await rightClick("session-2");
    await clickMenuEntry("Delete");
    await clickDialogButton("Delete");
    await settleCloseActs();

    expect(vi.mocked(sessionClose)).toHaveBeenCalledWith("session-2");
    expect(vi.mocked(sessionStop)).not.toHaveBeenCalled();
    expect(document.querySelector("#workspace-session-tab-session-2")).toBeNull();
  });

  it("the menu survives a harmless republication of the same rows", async () => {
    await renderWorkspace();
    await rightClick("agent-one");
    expect(menuLabels()).toHaveLength(5);

    // An elapsed-time tick: new array, new row objects, same ids and
    // generations — nothing the menu's entries would act on differently.
    await pushSnapshots(defaultSessions().map((s) => liveSnapshot(s.id, s.title)));

    expect(menuLabels()).toHaveLength(5);
  });

  it("the menu closes when its anchor's generation changes, and focus returns to the anchor", async () => {
    await renderWorkspace();
    await rightClick("session-2");
    expect(menuLabels()).toHaveLength(5);

    await pushSnapshots([
      liveSnapshot("agent-one", "Agent one", "acp"),
      liveSnapshot("session-2", "shell two", "terminal", 2),
      liveSnapshot("session-3", "shell three"),
    ]);

    expect(document.querySelector("[role='menu']")).toBeNull();
    expect(document.activeElement).toBe(tabElement("session-2"));
  });

  it("the menu closes on window resize, handing focus back to the anchor when the menu had it", async () => {
    await renderWorkspace();
    await rightClick("session-2");
    expect(document.activeElement?.getAttribute("role")).toBe("menuitem");

    await resizeWindow();

    expect(document.querySelector("[role='menu']")).toBeNull();
    expect(document.activeElement).toBe(tabElement("session-2"));
  });
});
