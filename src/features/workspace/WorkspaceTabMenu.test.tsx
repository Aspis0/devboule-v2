// @vitest-environment happy-dom

// The tab context menu: the close entries, our Delete after a separator,
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
  DIALOG_SELECTOR,
  dialog,
  headerMenuSeamFor,
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
  tabTitles,
  terminalSession,
} from "./bulkCloseHarness";
import {
  daemonStatus,
  sessionClose,
  sessionSetName,
  sessionStop,
  sessionsList,
} from "../../lib/tauri";

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

    expect(document.querySelector(DIALOG_SELECTOR)).toBeNull();
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
    // Delete destroys: the sole affirmative is the filled danger.
    const confirmButton = confirm.querySelector<HTMLButtonElement>(".confirm-dialog-confirm");
    expect(confirmButton?.textContent).toBe("Delete");
    expect(confirmButton?.classList.contains("confirm-dialog-confirm-danger")).toBe(true);

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

  it("Enter on the opened delete ask activates Cancel: nothing is destroyed", async () => {
    await renderWorkspace();

    await rightClick("session-2");
    await clickMenuEntry("Delete");
    // The ask opens with Cancel focused.
    const focused = document.activeElement;
    expect(focused?.textContent).toBe("Cancel");
    await act(async () => {
      focused!.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true }));
    });
    // No Enter handler confirms: the key alone changes nothing.
    expect(vi.mocked(sessionClose)).not.toHaveBeenCalled();
    expect(document.querySelector(DIALOG_SELECTOR)).not.toBeNull();
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

describe("the tab menu rename", () => {
  function daemonWithCapabilities(capabilities: string[]): void {
    vi.mocked(daemonStatus).mockResolvedValue({
      state: "connected",
      pid: 42,
      instanceId: "daemon-test",
      // The protocol this app speaks: a daemon that completes the handshake
      // with it is a 12-speaking daemon, which is what makes the advertised
      // `sessions` capability mean the rename frame.
      protocolVersion: 12,
      clients: 1,
      capabilities,
      message: null,
    });
  }

  // The capability override is per-test: the workspace polls the daemon
  // status, and a later test in any order must inherit the harness's own
  // default, not this describe's.
  beforeEach(() => {
    daemonWithCapabilities(["sessions"]);
  });

  afterEach(() => {
    daemonWithCapabilities(["typed_permissions"]);
    // The refusal test's rejection outlives its test otherwise — the harness
    // clears calls, not implementations, and a shuffled later test would
    // inherit it.
    vi.mocked(sessionSetName).mockResolvedValue(undefined);
  });

  function renameField(): HTMLInputElement {
    const field = dialog().querySelector<HTMLInputElement>("input");
    if (field === null) throw new Error("rename input did not render");
    return field;
  }

  async function fillRename(field: HTMLInputElement, value: string): Promise<void> {
    const setValue = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")?.set;
    if (setValue === undefined) throw new Error("input value setter did not exist");
    await act(async () => {
      setValue.call(field, value);
      field.dispatchEvent(new Event("input", { bubbles: true }));
    });
  }

  it("offers Rename first on an agent tab when the daemon advertises the capability", async () => {
    await renderWorkspace();

    await rightClick("agent-one");

    expect(menuLabels()).toEqual([
      "Rename",
      "Close to the left",
      "Close to the right",
      "Close other tabs",
      "Close",
      "Delete",
    ]);
  });

  it("hides Rename on a terminal tab", async () => {
    await renderWorkspace();

    await rightClick("session-2");

    expect(menuLabels()).not.toContain("Rename");
  });

  it("hides Rename when the daemon does not advertise the capability", async () => {
    daemonWithCapabilities([]);
    await renderWorkspace();

    await rightClick("agent-one");

    expect(menuLabels()).not.toContain("Rename");
  });

  it("renames an agent: the call, then the title from the daemon's push", async () => {
    await renderWorkspace();

    await rightClick("agent-one");
    await clickMenuEntry("Rename");
    expect(renameField().value).toBe("Agent one");
    await fillRename(renameField(), "Renamed agent");
    await clickDialogButton("Rename");

    expect(sessionSetName).toHaveBeenCalledWith("agent-one", "Renamed agent");
    // No optimistic title: the tab keeps the old name until the daemon's
    // roster push carries the new one.
    expect(tabTitles().some((title) => title.includes("Agent one"))).toBe(true);
    expect(tabTitles().some((title) => title.includes("Renamed agent"))).toBe(false);

    await pushSnapshots([
      {
        id: "agent-one",
        workspaceId: "workspace-1",
        kind: "acp",
        title: "Agent one",
        displayName: "Renamed agent",
        state: { type: "silent", generation: 1 },
        elapsedMs: 0,
      },
    ]);

    expect(tabTitles().some((title) => title.includes("Renamed agent"))).toBe(true);
    expect(document.activeElement).toBe(tabElement("agent-one"));
  });

  it("the pane kebab hides Rename on a recovered session", async () => {
    // The seam's second gate, at the pane: the flow's entry is pinned in
    // useTabCloseFlow.rename.test.tsx, this is Workspace's own condition.
    daemonWithCapabilities(["sessions"]);
    vi.mocked(sessionsList).mockResolvedValue([recoveredAgentSession("agent-one", "Agent one")]);
    await renderWorkspace();

    expect(headerMenuSeamFor("agent-one")?.onRename ?? null).toBeNull();
  });

  it("the pane kebab offers Rename on a live session with the capability", async () => {
    daemonWithCapabilities(["sessions"]);
    await renderWorkspace();

    expect(typeof headerMenuSeamFor("agent-one")?.onRename).toBe("function");
  });

  it("hides Rename on a recovered agent — the daemon's road needs a live process", async () => {
    // The daemon reaches the session record only through a live registry
    // entry; a journal-replayed row is refused with process_gone. Offering
    // the entry would be offering a dialog that can only fail.
    vi.mocked(sessionsList).mockResolvedValue([recoveredAgentSession("agent-one", "Agent one")]);
    await renderWorkspace();

    await rightClick("agent-one");

    expect(menuLabels()).not.toContain("Rename");
  });

  // The ended-but-live half of the recovered decision is pinned in
  // useTabCloseFlow.rename.test.tsx instead: the strip's stripSessions drops
  // ended rows the daemon still lists, so an ended agent never reaches a
  // tab to right-click — the entry logic is the flow's to assert.

  it("keeps the draft and shows the daemon's refusal verbatim", async () => {
    // A refusal the client mirror cannot predict: the session departed
    // between the click and the call (the daemon's own process-gone words).
    vi.mocked(sessionSetName).mockRejectedValue({
      code: "invalid_request",
      message: "This terminal process is gone.",
    });
    await renderWorkspace();

    await rightClick("agent-one");
    await clickMenuEntry("Rename");
    await fillRename(renameField(), "Renamed agent");
    await clickDialogButton("Rename");

    expect(dialog().textContent).toContain("This terminal process is gone.");
    expect(renameField().value).toBe("Renamed agent");
  });
});
