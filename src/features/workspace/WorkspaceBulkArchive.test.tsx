// @vitest-environment happy-dom

// Bulk tab removal, successor selection and separate session-close failures.
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  afterEachHarness,
  beforeEachHarness,
  bulkErrorBlock,
  clickDialogButton,
  clickMenuEntry,
  DIALOG_SELECTOR,
  dialog,
  liveSnapshot,
  lifecycleClose,
  modifiedClick,
  plainClick,
  pushSnapshots,
  renderWorkspace,
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

describe("local bulk tab removal", () => {
  it.each([
    ["Close to the left", "session-3"],
    ["Close to the right", "agent-one"],
    ["Close other tabs", "session-2"],
  ])("%s removes only the matching open tabs", async (label, survivor) => {
    await renderWorkspace();
    await rightClick(survivor);
    await clickMenuEntry(label);
    expect(tabTitles()).toHaveLength(1);
    expect(tabElement(survivor).getAttribute("aria-selected")).toBe("true");
    expect(document.querySelector(DIALOG_SELECTOR)).toBeNull();
    expect(sessionStop).not.toHaveBeenCalled();
  });

  it("a roster push keeps closed tabs closed", async () => {
    await renderWorkspace();
    await rightClick("session-3");
    await clickMenuEntry("Close other tabs");
    await pushSnapshots([
      liveSnapshot("agent-one", "Agent one", "acp"),
      liveSnapshot("session-2", "shell two"),
      liveSnapshot("session-3", "shell three"),
    ]);
    expect(tabTitles()).toHaveLength(1);
    expect(sessionStop).not.toHaveBeenCalled();
  });
});

describe("session close failures", () => {
  it("restores the refused session tab, selection and focus", async () => {
    await renderWorkspace();
    vi.mocked(sessionStop).mockRejectedValueOnce(new Error("daemon refused stop"));
    await lifecycleClose("session-2");
    await clickDialogButton("Close");
    await settleCloseActs();
    expect(tabElement("session-2").getAttribute("aria-selected")).toBe("true");
    expect(document.activeElement).toBe(tabElement("session-2"));
    expect(bulkErrorBlock().textContent).toContain("daemon refused stop");
    await lifecycleClose("session-2");
    await clickDialogButton("Close");
    await settleCloseActs();
    expect(document.body.textContent).not.toContain("These closes didn't go through:");
  });

  it("keeps session close failures across a surface remount", async () => {
    await renderWorkspace();
    vi.mocked(sessionStop).mockRejectedValueOnce(new Error("daemon refused stop"));
    await lifecycleClose("session-2");
    await clickDialogButton("Close");
    await settleCloseActs();
    await unmountWorkspace();
    await renderWorkspace();
    expect(bulkErrorBlock().textContent).toContain("shell two");
    expect(bulkErrorBlock().textContent).toContain("daemon refused stop");
  });

  it("dismisses a session close confirmation when its target leaves the roster", async () => {
    await renderWorkspace();
    await lifecycleClose("session-2");
    expect(dialog()).toBeTruthy();
    await pushSnapshots([
      liveSnapshot("agent-one", "Agent one", "acp"),
      liveSnapshot("session-3", "shell three"),
    ]);
    expect(document.querySelector(DIALOG_SELECTOR)).toBeNull();
    await pushSnapshots([
      liveSnapshot("agent-one", "Agent one", "acp"),
      liveSnapshot("session-2", "shell two"),
      liveSnapshot("session-3", "shell three"),
    ]);
    expect(document.querySelector(DIALOG_SELECTOR)).toBeNull();
    expect(sessionStop).not.toHaveBeenCalled();
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
    expect(document.querySelector(DIALOG_SELECTOR)).toBeNull();
    expect(sessionStop).not.toHaveBeenCalled();
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
    await settleCloseActs();

    // The closed tabs are gone from the strip; the survivor is active…
    expect(document.querySelector("#workspace-session-tab-session-2")).toBeNull();
    expect(document.querySelector("#workspace-session-tab-session-3")).toBeNull();
    expect(tabElement("session-1").getAttribute("aria-selected")).toBe("true");
    // …and slice 1's rule brought it into view (900 + 120 − 300).
    expect(scrollport.scrollLeft).toBe(720);
  });
});
