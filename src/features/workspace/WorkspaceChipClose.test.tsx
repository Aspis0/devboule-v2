// @vitest-environment happy-dom

// The tab's trailing "×" and its neighbouring paths: a plain click selects
// and fires nothing (the P0 defect this shape exists to kill), the chip runs
// the close policy, a middle click closes, and an older build's persisted
// undo records are startup litter. Hit areas are wiring + CSS here; only the
// live check sees pixels.
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act } from "react";
import {
  agentSession,
  afterEachHarness,
  beforeEachHarness,
  chipButton,
  chipClick,
  clickDialogButton,
  dialog,
  middleClick,
  plainClick,
  recoveredAgentSession,
  renderWorkspace,
  settleCloseActs,
  tabElement,
  terminalSession,
} from "./bulkCloseHarness";
import { OLDER_BUILD_PENDING_KEY } from "./closeActions";
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

    // The P0 defect — a hover hit area owning the click and archiving the
    // tab — is dead: the click selects, the tab stays, the daemon hears
    // nothing. That the chip never covers the label is its 48 px CSS rule
    // (pinned by the strip source test) and, in the end, the live check.
    expect(tabElement("session-2").getAttribute("aria-selected")).toBe("true");
    expect(vi.mocked(sessionStop)).not.toHaveBeenCalled();
    expect(document.querySelector('[role="dialog"]')).toBeNull();
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

  it("closing a terminal chip asks first, and confirming fires at once", async () => {
    await renderWorkspace();

    await chipClick("session-2");

    // A terminal's close is destructive: the ask stands between the click
    // and the daemon, and nothing fires without it.
    expect(document.querySelector('[role="dialog"]')).not.toBeNull();
    const confirm = dialog();
    expect(confirm.textContent).toContain("Close terminal?");
    expect(vi.mocked(sessionStop)).not.toHaveBeenCalled();

    await clickDialogButton("Close");
    expect(vi.mocked(sessionStop)).toHaveBeenCalledWith("session-2");
    expect(document.querySelector("#workspace-session-tab-session-2")).toBeNull();
  });

  it("a silent agent asks before it is archived: silence is not idleness", async () => {
    // The default roster's agent is silent — the daemon flipped its stream
    // to Silent on an output threshold alone, which is exactly what a long
    // tool call looks like. The policy asks, because no roster field says
    // the turn has ended.
    await renderWorkspace();

    await chipClick("agent-one");

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

    await chipClick("agent-old");

    expect(document.querySelector('[role="dialog"]')).toBeNull();
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

    await chipClick("agent-live");

    const confirm = dialog();
    expect(confirm.textContent).toContain("Archive running agent?");
    expect(confirm.textContent).toContain("This agent is still running");

    await clickDialogButton("Cancel");
    expect(vi.mocked(sessionStop)).not.toHaveBeenCalled();
    expect(document.querySelector("#workspace-session-tab-agent-live")).not.toBeNull();
  });

  it("a middle click closes by the same policy: a silent agent asks", async () => {
    await renderWorkspace();

    await middleClick("agent-one");

    expect(document.querySelector('[role="dialog"]')).not.toBeNull();
    expect(dialog().textContent).toContain("Archive running agent?");
    expect(vi.mocked(sessionStop)).not.toHaveBeenCalled();
  });

  it("after a chip cancel, focus returns to the tab the ask came from", async () => {
    await renderWorkspace();

    await chipClick("session-2");
    await clickDialogButton("Cancel");

    expect(document.querySelector('[role="dialog"]')).toBeNull();
    expect(document.activeElement).toBe(tabElement("session-2"));
  });

  it("closing the active tab via the chip selects Paseo's successor", async () => {
    await renderWorkspace();
    await plainClick("session-2");
    expect(tabElement("session-2").getAttribute("aria-selected")).toBe("true");

    await chipClick("session-2");
    await clickDialogButton("Close");
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

    expect(countText()).toContain("3 sessions");
    expect(countText()).not.toContain("2 sessions");

    await chipClick("session-2");
    await clickDialogButton("Close");
    await settleCloseActs();

    // The daemon does not push a roster update after session_stop, so the
    // roster still carries the row — the count reads the strip's rows and
    // falls the moment the tab is gone.
    expect(countText()).toContain("2 sessions");
    expect(countText()).not.toContain("3 sessions");
  });
});
