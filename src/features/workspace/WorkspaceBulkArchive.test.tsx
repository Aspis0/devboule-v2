// @vitest-environment happy-dom

// Close outcomes: the counted ask and its cancel, what firing leaves behind
// (failures owned and cleared by the session that produced them), the focus
// trap, and which tab is active afterwards — including the measured Chrome
// rule that the right-clicked tab takes over after "close to the right".
// The menu's entries, the chip and the selection grammar have their own
// files.
import { act } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  afterEachHarness,
  beforeEachHarness,
  bulkErrorBlock,
  clickDialogButton,
  clickMenuEntry,
  dialog,
  liveSnapshot,
  modifiedClick,
  plainClick,
  pushSnapshots,
  renderWorkspace,
  resizeWindow,
  rightClick,
  settleCloseActs,
  tabElement,
  tabTitles,
  terminalSession,
  unmountWorkspace,
} from "./bulkCloseHarness";
import { sessionStop, sessionsList } from "../../lib/tauri";

/** A DOMRect at a chosen left edge and width: happy-dom computes no layout. */
function stubRect(left: number, width: number): DOMRect {
  return {
    left,
    width,
    right: left + width,
    top: 0,
    bottom: 0,
    x: left,
    y: 0,
    height: 0,
    toJSON: () => ({}),
  };
}

function stubScrollport(scrollport: HTMLElement, clientWidth: number): void {
  Object.defineProperty(scrollport, "clientWidth", { value: clientWidth, configurable: true });
  scrollport.getBoundingClientRect = () => stubRect(0, clientWidth);
}

function stubTab(
  tab: HTMLElement,
  contentLeft: number,
  width: number,
  scrollport: HTMLElement,
): void {
  tab.getBoundingClientRect = () => stubRect(contentLeft - scrollport.scrollLeft, width);
}

beforeEach(() => {
  beforeEachHarness();
});

afterEach(async () => {
  await afterEachHarness();
});

describe("the counted ask", () => {
  it("Close other tabs asks with the counts; Cancel fires nothing and focus returns to the anchor", async () => {
    await renderWorkspace();
    // The last tab: Close to the right is disabled, and the remaining two
    // tabs are one agent and one terminal — the mixed count.
    await rightClick("session-3");
    const right = [...document.querySelectorAll<HTMLButtonElement>("[role='menuitem']")].find(
      (item) => item.textContent === "Close to the right",
    );
    expect(right?.disabled).toBe(true);

    await clickMenuEntry("Close other tabs");

    const confirm = dialog();
    expect(confirm.textContent).toContain("Close other tabs?");
    expect(confirm.textContent).toContain(
      "This will archive 1 agent(s) and archive 1 terminal(s). " +
        "The processes stop and every message stays in History.",
    );

    await clickDialogButton("Cancel");

    expect(document.querySelector('[role="dialog"]')).toBeNull();
    // Nothing was fired, so the right-clicked tab is still there to take
    // focus back.
    expect(document.activeElement).toBe(tabElement("session-3"));
    await settleCloseActs();
    expect(vi.mocked(sessionStop)).not.toHaveBeenCalled();
  });

  it("confirming fires at once — there is no window and nothing waits", async () => {
    await renderWorkspace();
    await rightClick("session-3");
    await clickMenuEntry("Close other tabs");
    await clickDialogButton("Close");

    // The daemon hears in the same breath as the click; the rows are already
    // hidden behind their closing marks and the survivor has focus.
    expect(vi.mocked(sessionStop)).toHaveBeenCalledTimes(2);
    expect(vi.mocked(sessionStop)).toHaveBeenCalledWith("agent-one");
    expect(vi.mocked(sessionStop)).toHaveBeenCalledWith("session-2");
    expect(document.querySelector("#workspace-session-tab-agent-one")).toBeNull();
    expect(document.activeElement?.id).toBe("workspace-session-tab-session-3");
  });

  it("the confirmation survives a harmless republication, closes on a generation change", async () => {
    await renderWorkspace();
    await rightClick("session-3");
    await clickMenuEntry("Close other tabs");
    expect(dialog().textContent).toContain("Close other tabs?");

    // An elapsed-time tick: same rows, same generations — the ask stands.
    await pushSnapshots([
      liveSnapshot("agent-one", "Agent one", "acp"),
      liveSnapshot("session-2", "shell two"),
      liveSnapshot("session-3", "shell three"),
    ]);
    expect(dialog().textContent).toContain("Close other tabs?");

    // A target resumed: what the user confirmed is no longer what is there.
    await pushSnapshots([
      liveSnapshot("agent-one", "Agent one", "acp"),
      liveSnapshot("session-2", "shell two", "terminal", 2),
      liveSnapshot("session-3", "shell three"),
    ]);

    expect(document.querySelector('[role="dialog"]')).toBeNull();
    await settleCloseActs();
    expect(vi.mocked(sessionStop)).not.toHaveBeenCalled();
  });

  it("the confirmation closes on window resize", async () => {
    await renderWorkspace();
    await rightClick("session-3");
    await clickMenuEntry("Close other tabs");
    expect(dialog()).toBeTruthy();

    await resizeWindow();

    expect(document.querySelector('[role="dialog"]')).toBeNull();
    await settleCloseActs();
    expect(vi.mocked(sessionStop)).not.toHaveBeenCalled();
  });

  it("Tab and Shift+Tab cycle inside the confirmation", async () => {
    await renderWorkspace();
    await rightClick("session-3");
    await clickMenuEntry("Close other tabs");
    // The ask opens with its primary button focused.
    expect(document.activeElement?.textContent).toBe("Close");

    await act(async () => {
      dialog().dispatchEvent(new KeyboardEvent("keydown", { key: "Tab", bubbles: true }));
    });
    expect(document.activeElement?.textContent).toBe("Cancel");

    await act(async () => {
      dialog().dispatchEvent(new KeyboardEvent("keydown", { key: "Tab", bubbles: true }));
    });
    // Cycled: Tab from the last button lands back on the first.
    expect(document.activeElement?.textContent).toBe("Close");

    await act(async () => {
      dialog().dispatchEvent(
        new KeyboardEvent("keydown", { key: "Tab", shiftKey: true, bubbles: true }),
      );
    });
    expect(document.activeElement?.textContent).toBe("Cancel");
    // The ask is still standing, and still the only ask.
    expect(document.querySelectorAll('[role="dialog"]')).toHaveLength(1);
  });
});

describe("failure ownership", () => {
  it("a failed close keeps its tab and names it, while the others go through", async () => {
    await renderWorkspace();
    vi.mocked(sessionStop)
      .mockResolvedValueOnce(undefined)
      .mockRejectedValueOnce(new Error("daemon refused stop"));

    await rightClick("agent-one");
    await clickMenuEntry("Close other tabs");
    await clickDialogButton("Close");
    await settleCloseActs();

    expect(vi.mocked(sessionStop)).toHaveBeenCalledTimes(2);
    // The success stays closed (hidden); the failure's tab is back…
    expect(tabTitles().some((title) => title.includes("shell two"))).toBe(false);
    expect(tabTitles().some((title) => title.includes("shell three"))).toBe(true);
    // …and the list says which one and why.
    expect(bulkErrorBlock().textContent).toContain("shell three");
    expect(bulkErrorBlock().textContent).toContain("daemon refused stop");
  });

  it("a later clean close of the same session clears its earlier failure", async () => {
    await renderWorkspace();
    vi.mocked(sessionStop)
      .mockResolvedValueOnce(undefined)
      .mockRejectedValueOnce(new Error("daemon refused stop"));

    await rightClick("session-3");
    await clickMenuEntry("Close other tabs");
    await clickDialogButton("Close");
    await settleCloseActs();
    expect(bulkErrorBlock().textContent).toContain("daemon refused stop");

    // The restored tab is closed again — this time the daemon takes it —
    // and the stale line for that session goes with its recovery.
    await chipClose("session-2");
    await settleCloseActs();

    expect(document.body.textContent).not.toContain("These closes didn't go through:");
    expect(tabTitles().some((title) => title.includes("shell two"))).toBe(false);
  });

  it("close failures are still shown when the Workspace mounts again", async () => {
    await renderWorkspace();
    vi.mocked(sessionStop)
      .mockResolvedValueOnce(undefined)
      .mockRejectedValueOnce(new Error("daemon refused stop"));

    await rightClick("agent-one");
    await clickMenuEntry("Close other tabs");
    await clickDialogButton("Close");
    await settleCloseActs();
    expect(bulkErrorBlock().textContent).toContain("daemon refused stop");

    // A surface switch and back: the store is the app's, not the mount's.
    await unmountWorkspace();
    await renderWorkspace();

    expect(document.body.textContent).toContain("These closes didn't go through:");
    expect(document.body.textContent).toContain("shell three");
  });
});

describe("a refused close", () => {
  it("restores selection and focus to the failed active tab, and never shows the empty state", async () => {
    vi.mocked(sessionsList).mockResolvedValue([
      terminalSession("session-1", "shell one"),
      terminalSession("session-2", "shell two"),
    ]);
    await renderWorkspace();
    await plainClick("session-1");
    expect(tabElement("session-1").getAttribute("aria-selected")).toBe("true");
    vi.mocked(sessionStop).mockRejectedValueOnce(new Error("daemon refused stop"));

    await chipClose("session-1");
    await settleCloseActs();

    // The act was refused, so the row came back — and it is the active tab
    // again, selected and focused, with no empty state beside it.
    expect(tabElement("session-1").getAttribute("aria-selected")).toBe("true");
    expect(document.activeElement).toBe(tabElement("session-1"));
    expect(document.body.textContent).not.toContain("No tabs yet");
    expect(bulkErrorBlock().textContent).toContain("daemon refused stop");
  });

  it("a refused active tab takes selection back from the successor that moved", async () => {
    vi.mocked(sessionsList).mockResolvedValue([
      terminalSession("session-1", "shell one"),
      terminalSession("session-2", "shell two"),
      terminalSession("session-3", "shell three"),
    ]);
    await renderWorkspace();
    await plainClick("session-2");
    vi.mocked(sessionStop)
      .mockRejectedValueOnce(new Error("daemon refused stop"))
      .mockResolvedValueOnce(undefined);

    // Close other tabs from session-1: the closed set holds the ACTIVE
    // session-2. Its act is refused while session-3's goes through.
    await rightClick("session-1");
    await clickMenuEntry("Close other tabs");
    await clickDialogButton("Close");
    await settleCloseActs();

    expect(tabTitles().some((title) => title.includes("shell three"))).toBe(false);
    expect(tabElement("session-2").getAttribute("aria-selected")).toBe("true");
    expect(document.activeElement).toBe(tabElement("session-2"));
  });

  it("a dismissed ask never mounts again when its removed target returns", async () => {
    await renderWorkspace();
    await rightClick("session-3");
    await clickMenuEntry("Close other tabs");
    expect(dialog()).toBeTruthy();

    // The ask's target shell two leaves the roster: the ask is dismissed.
    await pushSnapshots([
      liveSnapshot("agent-one", "Agent one", "acp"),
      liveSnapshot("session-3", "shell three"),
    ]);
    expect(document.querySelector('[role="dialog"]')).toBeNull();

    // The same row returns with the SAME id and generation — reopened from
    // History, say. The old ask stays dead; no new action, no new ask.
    await pushSnapshots([
      liveSnapshot("agent-one", "Agent one", "acp"),
      liveSnapshot("session-2", "shell two"),
      liveSnapshot("session-3", "shell three"),
    ]);

    expect(document.querySelector('[role="dialog"]')).toBeNull();
    await settleCloseActs();
    expect(vi.mocked(sessionStop)).not.toHaveBeenCalled();
  });
});

describe("what is active afterwards", () => {
  it("after 'close to the right' with a later tab active, the right-clicked tab becomes active", async () => {
    vi.mocked(sessionsList).mockResolvedValue([
      terminalSession("session-1", "shell one"),
      terminalSession("session-2", "shell two"),
      terminalSession("session-3", "shell three"),
      terminalSession("session-4", "shell four"),
      terminalSession("session-5", "shell five"),
      terminalSession("session-6", "shell six"),
    ]);
    await renderWorkspace();
    await plainClick("session-6");
    expect(tabElement("session-6").getAttribute("aria-selected")).toBe("true");

    await rightClick("session-3");
    await clickMenuEntry("Close to the right");
    await clickDialogButton("Close");
    await settleCloseActs();

    // Tabs four to six are gone; the tab the user acted on is active — not
    // the first tab (the measured defect), not nothing.
    for (const id of ["session-4", "session-5", "session-6"]) {
      expect(document.querySelector(`#workspace-session-tab-${id}`)).toBeNull();
    }
    expect(tabElement("session-3").getAttribute("aria-selected")).toBe("true");
    expect(tabElement("session-1").getAttribute("aria-selected")).toBe("false");
  });

  it("when every tab closes, no tab is active and the empty state shows", async () => {
    await renderWorkspace();
    for (const id of ["agent-one", "session-2", "session-3"]) {
      await modifiedClick(id, "ctrlKey");
    }

    await rightClick("session-2");
    await clickMenuEntry("Close 3 tabs");
    await clickDialogButton("Close");
    await settleCloseActs();

    expect(vi.mocked(sessionStop)).toHaveBeenCalledTimes(3);
    expect(tabTitles()).toHaveLength(0);
    expect(document.querySelector(".workspace-session-tab-selected")).toBeNull();
    expect(document.body.textContent).toContain("No tabs yet");
  });

  it("after a bulk close the active tab exists and the strip scrolls to it", async () => {
    vi.mocked(sessionsList).mockResolvedValue([
      terminalSession("session-1", "shell one"),
      terminalSession("session-2", "shell two"),
      terminalSession("session-3", "shell three"),
    ]);
    await renderWorkspace();
    const scrollport = document.querySelector<HTMLElement>(".workspace-session-tabs-scroll");
    const tabOne = document.querySelector<HTMLElement>("#workspace-session-tab-session-1");
    const tabTwo = document.querySelector<HTMLElement>("#workspace-session-tab-session-2");
    const tabThree = document.querySelector<HTMLElement>("#workspace-session-tab-session-3");
    if (scrollport === null || tabOne === null || tabTwo === null || tabThree === null) {
      throw new Error("strip did not render");
    }
    stubScrollport(scrollport, 300);
    stubTab(tabOne, 900, 120, scrollport);
    stubTab(tabTwo, 0, 120, scrollport);
    stubTab(tabThree, 400, 120, scrollport);

    // session-2 is the active tab — and lands inside the closed set.
    await plainClick("session-2");
    expect(tabElement("session-2").getAttribute("aria-selected")).toBe("true");

    await rightClick("session-1");
    await clickMenuEntry("Close other tabs");
    await clickDialogButton("Close");
    await settleCloseActs();

    // The closed tabs are gone from the strip; the survivor is active…
    expect(document.querySelector("#workspace-session-tab-session-2")).toBeNull();
    expect(document.querySelector("#workspace-session-tab-session-3")).toBeNull();
    expect(tabElement("session-1").getAttribute("aria-selected")).toBe("true");
    // …and slice 1's rule brought it into view (900 + 120 − 300).
    expect(scrollport.scrollLeft).toBe(720);
  });
});

// The chip is the topic of its own file; here one chip close stands in for
// "the user closes the restored tab again".

async function chipClose(id: string): Promise<void> {
  const row = tabElement(id).closest(".workspace-session-row");
  const chip = row?.querySelector<HTMLButtonElement>(".workspace-session-chip-close");
  if (chip === null || chip === undefined) throw new Error(`close chip did not render: ${id}`);
  await act(async () => chip.click());
  // A terminal asks: confirm it.
  await clickDialogButton("Close");
}
