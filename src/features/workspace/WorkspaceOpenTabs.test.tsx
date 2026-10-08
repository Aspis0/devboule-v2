// @vitest-environment happy-dom
import { act } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  agentSession,
  afterEachHarness,
  beforeEachHarness,
  chipClick,
  liveSnapshot,
  pushSnapshots,
  recoveredAgentSession,
  requestChildPermission,
  renderWorkspace,
  tabElement,
  tabTitles,
  unmountWorkspace,
} from "./bulkCloseHarness";
import { sessionClose, sessionPermissionRespond, sessionStop, sessionsList } from "../../lib/tauri";
import { resetSharedSessionControllerForTests, sharedSessionController } from "./workspaceSessions";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

beforeEach(beforeEachHarness);
afterEach(afterEachHarness);

describe("user-opened workspace tabs", () => {
  it("indicates child attention without opening a tab, then opens and answers its card from the pill", async () => {
    const parent = { ...agentSession("parent", "Coordinator"), createdAtMs: 1 };
    const child = { ...agentSession("child", "Child"), createdBy: parent.id, createdAtMs: 2 };
    vi.mocked(sessionsList).mockResolvedValue([parent, child]);
    sharedSessionController().open(parent);
    await renderWorkspace(false);
    await requestChildPermission(parent.id, child.id);
    await pushSnapshots([
      liveSnapshot(parent.id, parent.title, "acp"),
      {
        ...liveSnapshot(child.id, child.title, "acp"),
        createdBy: parent.id,
        attention: { reason: "permission", atMs: 1 },
      },
    ]);
    expect(tabTitles()).toHaveLength(1);
    expect(document.querySelector(".permission-card")).toBeNull();
    const pill = document.querySelector<HTMLButtonElement>('[data-testid="subagent-pill"]');
    expect(pill?.getAttribute("aria-label")).toContain("1 needs your approval");
    await act(async () => pill?.click());
    const row = document.querySelector<HTMLButtonElement>("button.workspace-subagent-row");
    expect(row?.textContent).toContain("Needs your approval");
    expect(row?.getAttribute("aria-label")).toContain("Open in tab");
    await act(async () => row?.click());
    expect(tabElement(child.id).getAttribute("aria-selected")).toBe("true");
    expect(document.querySelector(".permission-card")).not.toBeNull();
    await act(async () =>
      document.querySelector<HTMLButtonElement>(".permission-card-primary-action")?.click(),
    );
    expect(sessionPermissionRespond).toHaveBeenCalledWith(
      child.id,
      41,
      "child-ask",
      "allow_once",
      undefined,
      undefined,
    );
    await pushSnapshots([
      liveSnapshot(parent.id, parent.title, "acp"),
      { ...liveSnapshot(child.id, child.title, "acp"), createdBy: parent.id },
    ]);
    await chipClick(child.id);
    const clearedPill = document.querySelector<HTMLButtonElement>('[data-testid="subagent-pill"]');
    expect(clearedPill?.getAttribute("aria-label")).not.toContain("your approval");
    expect(clearedPill?.querySelector(".workspace-subagent-attention")).toBeNull();
    const trigger = document.querySelector(".workspace-rate");
    expect(trigger?.getAttribute("aria-label")).not.toContain("your approval");
    expect(trigger?.querySelector(".strip-dot-attention")).toBeNull();
    expect(document.querySelector(".sidebar-row-dot-attention")).toBeNull();
  });

  it("starts with no tabs when the old build has no selected session", async () => {
    await renderWorkspace(false);
    expect(tabTitles()).toEqual([]);
    expect(document.body.textContent).toContain("No tabs yet");
  });

  it("opens an agent-created child by clicking its pill row", async () => {
    const parent = { ...agentSession("parent", "Parent"), createdAtMs: 1 };
    const child = { ...agentSession("child", "Child"), createdBy: parent.id, createdAtMs: 2 };
    const recovered = { ...recoveredAgentSession("recovered", "Recovered"), createdAtMs: 3 };
    vi.mocked(sessionsList).mockResolvedValue([parent]);
    sharedSessionController().open(parent);
    await renderWorkspace(false);
    vi.mocked(sessionsList).mockResolvedValue([parent, child, recovered]);
    await pushSnapshots([
      liveSnapshot(parent.id, parent.title, "acp"),
      { ...liveSnapshot(child.id, child.title, "acp"), createdBy: parent.id },
      { ...liveSnapshot(recovered.id, recovered.title, "acp"), state: recovered.state },
    ]);
    expect(tabTitles()).toHaveLength(1);
    expect(sharedSessionController().getState().sessions).toHaveLength(3);
    await act(async () =>
      document.querySelector<HTMLButtonElement>('[data-testid="subagent-pill"]')?.click(),
    );
    const row = document.querySelector<HTMLButtonElement>("button.workspace-subagent-row");
    if (row === null) throw new Error("child row did not render");
    expect(row.textContent).toContain("Child");
    row.focus();
    await act(async () => {
      row.click();
    });
    expect(tabTitles()).toHaveLength(2);
    expect(tabElement("child").getAttribute("aria-selected")).toBe("true");
    expect(document.querySelector(".workspace-subagent-list")).toBeNull();
    expect(document.getElementById("workspace-session-tab-recovered")).toBeNull();
    expect(JSON.parse(localStorage.getItem("devboule.openSessionTabs")!).selected?.id).toBe(
      "child",
    );
    await unmountWorkspace();
    expect(JSON.parse(localStorage.getItem("devboule.openSessionTabs")!).selected?.id).toBe(
      "child",
    );
    resetSharedSessionControllerForTests();
    await renderWorkspace(false);
    expect(tabTitles()).toHaveLength(2);
    expect(sharedSessionController().getState().selectedSessionId).toBe("child");
    expect(tabElement("child").getAttribute("aria-selected")).toBe("true");
  });

  it("removes a tab while keeping the session and its closed state across restart", async () => {
    const row = { ...agentSession("parent", "Parent"), createdAtMs: 1 };
    vi.mocked(sessionsList).mockResolvedValue([row]);
    sharedSessionController().open(row);
    await renderWorkspace(false);
    await chipClick(row.id);
    expect(tabTitles()).toEqual([]);
    expect(sessionStop).not.toHaveBeenCalled();
    expect(sessionClose).not.toHaveBeenCalled();
    expect(sharedSessionController().getState().sessions[0].state.type).toBe("live");
    await unmountWorkspace();
    resetSharedSessionControllerForTests();
    await renderWorkspace(false);
    expect(tabTitles()).toEqual([]);
  });
});
