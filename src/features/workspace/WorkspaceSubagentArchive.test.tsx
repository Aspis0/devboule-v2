// @vitest-environment happy-dom

// The archive act at workspace level: a child already open as a tab, whose
// row the act's roster read drops, takes its tab with it.
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act } from "react";
import {
  agentSession,
  afterEachHarness,
  beforeEachHarness,
  clickDialogButton,
  pushSnapshots,
  renderWorkspace,
  settleCloseActs,
} from "./bulkCloseHarness";
import { sessionClose, sessionsList } from "../../lib/tauri";
import type { Session } from "../../types/ipc";

beforeEach(() => {
  beforeEachHarness();
});

afterEach(async () => {
  await afterEachHarness();
});

function parentSession(): Session {
  return agentSession("parent-1", "Parent");
}

function childSession(): Session {
  return { ...agentSession("child-1", "Child"), createdBy: "parent-1" };
}

async function openMenu(): Promise<void> {
  const pill = document.querySelector<HTMLButtonElement>('[data-testid="subagent-pill"]');
  if (pill === null) throw new Error("subagent pill did not render");
  await act(async () => {
    pill.click();
  });
}

describe("archiving a child that is open as a tab", () => {
  it("closes the child's tab when the act's roster read drops its row", async () => {
    vi.mocked(sessionsList).mockResolvedValue([parentSession(), childSession()]);
    await renderWorkspace();

    // The child is open as a tab now, while it still runs.
    expect(document.querySelector("#workspace-session-tab-child-1")).not.toBeNull();

    // It finishes: the roster keeps its row and its tab.
    await pushSnapshots([
      {
        id: "parent-1",
        workspaceId: "workspace-1",
        kind: "acp",
        title: "Parent",
        state: { type: "live", generation: 1 },
        elapsedMs: 0,
      },
      {
        id: "child-1",
        workspaceId: "workspace-1",
        kind: "acp",
        title: "Child",
        state: { type: "ended", generation: 1, code: 0, integrity: { kind: "complete" } },
        elapsedMs: 0,
      },
    ]);
    expect(document.querySelector("#workspace-session-tab-child-1")).not.toBeNull();

    await openMenu();
    const action = document.querySelector<HTMLButtonElement>('[data-testid="subagent-archive"]');
    if (action === null) throw new Error("archive action did not render");
    expect(action.textContent).toBe("Archive 1 finished subagent");
    await act(async () => {
      action.click();
    });

    // The daemon would answer the next roster read without the closed child.
    vi.mocked(sessionsList).mockResolvedValue([parentSession()]);
    await clickDialogButton("Archive");
    await settleCloseActs();

    expect(vi.mocked(sessionClose)).toHaveBeenCalledTimes(1);
    expect(vi.mocked(sessionClose)).toHaveBeenCalledWith("child-1");
    expect(document.querySelector("#workspace-session-tab-child-1")).toBeNull();
    expect(document.querySelector("#workspace-session-tab-parent-1")).not.toBeNull();
    expect(document.querySelector('[data-testid="subagent-pill"]')).toBeNull();
  });
});
