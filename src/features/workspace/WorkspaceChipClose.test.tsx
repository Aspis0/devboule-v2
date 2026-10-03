// @vitest-environment happy-dom

// Tab removal, the pane's separate session close and focus restoration.
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act } from "react";
import {
  agentSession,
  afterEachHarness,
  beforeEachHarness,
  chipButton,
  chipClick,
  lifecycleClose,
  clickDialogButton,
  DIALOG_SELECTOR,
  dialog,
  middleClick,
  plainClick,
  recoveredAgentSession,
  renderWorkspace,
  settleCloseActs,
  tabElement,
  terminalSession,
} from "./bulkCloseHarness";
import { OLDER_BUILD_PENDING_KEY } from "./strip/closeActions";
import { sessionStop, sessionsList } from "../../lib/tauri";

beforeEach(() => {
  beforeEachHarness();
});

afterEach(async () => {
  await afterEachHarness();
});

describe("the close chip", () => {
  it("a plain click selects the tab and fires nothing", async () => {
    await renderWorkspace();

    await plainClick("session-2");

    // A hover hit area must not own the click: the click selects, the tab stays,
    // the daemon hears nothing. That the chip never covers the label is its 48
    // px CSS rule (pinned by the strip source test); the live check sees pixels.
    expect(tabElement("session-2").getAttribute("aria-selected")).toBe("true");
    expect(vi.mocked(sessionStop)).not.toHaveBeenCalled();
    expect(document.querySelector(DIALOG_SELECTOR)).toBeNull();
    expect(document.body.textContent).not.toContain("didn't go through");
  });

  it("the chip renders per tab, in the trailing overlay, with its own label", async () => {
    await renderWorkspace();

    const chip = chipButton("session-2");
    expect(chip.getAttribute("aria-label")).toBe("Close shell two");
    // The sibling invariant, asserted and not just named: the chip overlay's
    // parent is the row, the row's tab button is the very tab in question,
    // and the tab does NOT contain the chip — nesting it would bubble its
    // clicks into the tab's selection handler.
    const row = chip.closest(".workspace-session-row");
    const tab = row?.querySelector("button[role='tab']");
    expect(tab).toBe(document.querySelector("#workspace-session-tab-session-2"));
    expect(chip.closest(".workspace-session-chip")?.parentElement).toBe(row);
    expect(tab?.contains(chip)).toBe(false);
    expect(chipButton("agent-one").getAttribute("aria-label")).toBe("Close Agent one");
  });

  it("closing a terminal from its pane asks before stopping", async () => {
    await renderWorkspace();

    await lifecycleClose("session-2");

    // A terminal's close is destructive: the ask stands between the click
    // and the daemon, and nothing fires without it.
    expect(document.querySelector(DIALOG_SELECTOR)).not.toBeNull();
    const confirm = dialog();
    // The title names the shell the tab shows — the tab's own title.
    expect(confirm.textContent).toContain("Close terminal “shell two”?");
    expect(confirm.textContent).toContain("The process stops and every message stays in History.");
    // The destructive sole affirmative is the filled danger.
    const confirmButton = confirm.querySelector<HTMLButtonElement>(".confirm-dialog-confirm");
    expect(confirmButton?.classList.contains("confirm-dialog-confirm-danger")).toBe(true);
    expect(vi.mocked(sessionStop)).not.toHaveBeenCalled();

    await clickDialogButton("Close");
    expect(vi.mocked(sessionStop)).toHaveBeenCalledWith("session-2");
    expect(document.querySelector("#workspace-session-tab-session-2")).toBeNull();
  });

  it("the ask owns the chord and Delete: the tab does not move and no second ask opens", async () => {
    await renderWorkspace();
    await plainClick("session-2");
    await lifecycleClose("session-2");

    const confirm = dialog();
    expect(confirm.textContent).toContain("Close terminal “shell two”?");
    const selectedBefore = tabElement("session-2").getAttribute("aria-selected");

    // Both keys from inside the ask, where the user's hands are while it
    // stands: the chord must die at the dialog, and Delete must not reach a
    // chip behind the scrim.
    await act(async () => {
      document.activeElement?.dispatchEvent(
        new KeyboardEvent("keydown", { key: "]", altKey: true, shiftKey: true, bubbles: true }),
      );
    });
    expect(tabElement("session-2").getAttribute("aria-selected")).toBe(selectedBefore);
    expect(document.activeElement).toBe(
      confirm.querySelector<HTMLButtonElement>(".confirm-dialog-cancel"),
    );

    await act(async () => {
      document.activeElement?.dispatchEvent(
        new KeyboardEvent("keydown", { key: "Delete", bubbles: true }),
      );
    });
    // The same ask still stands: nothing closed, and no second ask replaced it.
    expect(dialog().textContent).toContain("Close terminal “shell two”?");
    expect(vi.mocked(sessionStop)).not.toHaveBeenCalled();
  });

  it("a silent agent asks before it is archived: silence is not idleness", async () => {
    // The default roster's agent is silent — the daemon flipped its stream
    // to Silent on an output threshold alone, which is exactly what a long
    // tool call looks like. The policy asks, because no roster field says
    // the turn has ended.
    await renderWorkspace();

    await lifecycleClose("agent-one");

    const confirm = dialog();
    expect(confirm.textContent).toContain("Archive running agent?");
    expect(vi.mocked(sessionStop)).not.toHaveBeenCalled();

    await clickDialogButton("Cancel");
    expect(vi.mocked(sessionStop)).not.toHaveBeenCalled();
    expect(document.querySelector("#workspace-session-tab-agent-one")).not.toBeNull();
  });

  it("an agent without a process archives at once — no ask, no window", async () => {
    vi.mocked(sessionsList).mockResolvedValue([
      recoveredAgentSession("agent-old", "Old transcript"),
      terminalSession("session-2", "shell two"),
    ]);
    await renderWorkspace();

    await lifecycleClose("agent-old");

    expect(document.querySelector(DIALOG_SELECTOR)).toBeNull();
    await settleCloseActs();
    expect(vi.mocked(sessionStop)).toHaveBeenCalledWith("agent-old");
    expect(document.querySelector("#workspace-session-tab-agent-old")).toBeNull();
  });

  it("a running agent asks before it is archived", async () => {
    vi.mocked(sessionsList).mockResolvedValue([
      agentSession("agent-live", "Busy one"),
      terminalSession("session-2", "shell two"),
    ]);
    await renderWorkspace();

    await lifecycleClose("agent-live");

    const confirm = dialog();
    expect(confirm.textContent).toContain("Archive running agent?");
    expect(confirm.textContent).toContain("This agent is still running");
    // Archiving a running agent is the destructive sole affirmative: filled danger.
    const confirmButton = confirm.querySelector<HTMLButtonElement>(".confirm-dialog-confirm");
    expect(confirmButton?.classList.contains("confirm-dialog-confirm-danger")).toBe(true);

    await clickDialogButton("Cancel");
    expect(vi.mocked(sessionStop)).not.toHaveBeenCalled();
    expect(document.querySelector("#workspace-session-tab-agent-live")).not.toBeNull();
  });

  it("a middle click removes a silent agent tab without stopping it", async () => {
    await renderWorkspace();
    await middleClick("agent-one");
    expect(document.querySelector(DIALOG_SELECTOR)).toBeNull();
    expect(document.querySelector("#workspace-session-tab-agent-one")).toBeNull();
    expect(sessionStop).not.toHaveBeenCalled();
  });

  it("after cancelling a session close, focus returns to its tab", async () => {
    await renderWorkspace();

    await lifecycleClose("session-2");
    await clickDialogButton("Cancel");

    expect(document.querySelector(DIALOG_SELECTOR)).toBeNull();
    expect(document.activeElement).toBe(tabElement("session-2"));
  });

  it("closing the active tab via the chip selects the nearest survivor", async () => {
    await renderWorkspace();
    await plainClick("session-2");
    expect(tabElement("session-2").getAttribute("aria-selected")).toBe("true");

    await chipClick("session-2");
    await settleCloseActs();

    expect(document.querySelector("#workspace-session-tab-session-2")).toBeNull();
    expect(tabElement("session-3").getAttribute("aria-selected")).toBe("true");
    expect(document.activeElement?.id).toBe("workspace-session-tab-session-3");
  });

  it("an older build's persisted undo records are dropped unread on startup", async () => {
    // A record the old build armed for its 5-second window: the new build
    // has no window, so the record must never fire — it is litter.
    window.localStorage.setItem(
      OLDER_BUILD_PENDING_KEY,
      JSON.stringify([
        {
          id: "session-2",
          title: "shell two",
          kind: "archive",
          generation: 1,
          dueAt: 0,
        },
      ]),
    );

    await renderWorkspace();
    // Wait out the OLD build's five-second window: a regression that reads
    // the record, re-arms it and clears the key would fire inside it, and
    // only time past the window proves nothing fires at all.
    await act(async () => {
      await vi.advanceTimersByTimeAsync(5000 + 1000);
    });

    expect(window.localStorage.getItem(OLDER_BUILD_PENDING_KEY)).toBeNull();
    expect(vi.mocked(sessionStop)).not.toHaveBeenCalled();
    expect(tabElement("session-2")).not.toBeNull();
  });
});

describe("the strip's session count", () => {
  it("reads the strip, not the roster: a closed tab stops counting at once", async () => {
    await renderWorkspace();
    const countText = () => document.body.textContent ?? "";

    expect(countText()).toContain("3 open sessions");
    expect(countText()).not.toContain("2 open sessions");

    await chipClick("session-2");
    await settleCloseActs();

    // Removing the tab changes the count while the roster still has the session.
    expect(countText()).toContain("2 open sessions");
    expect(countText()).not.toContain("3 open sessions");
  });
});
