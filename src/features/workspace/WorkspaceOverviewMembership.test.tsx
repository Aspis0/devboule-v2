// @vitest-environment happy-dom
import { act } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  agentSession,
  afterEachHarness,
  beforeEachHarness,
  chipClick,
  endedAgentSession,
  recoveredAgentSession,
  renderWorkspace,
  silentAgentSession,
  tabElement,
  terminalSession,
} from "./bulkCloseHarness";
import { sessionsList } from "../../lib/tauri";
import { sharedSessionController } from "./workspaceSessions";
import type { Session } from "../../types/ipc";

beforeEach(beforeEachHarness);
afterEach(afterEachHarness);

const fullRow = (row: Session): Session => ({ ...row, createdAtMs: 1 });
function trigger(): HTMLButtonElement {
  const button = document.querySelector<HTMLButtonElement>(".workspace-rate");
  if (button === null) throw new Error("Overview trigger did not render");
  return button;
}

function subagentPill(): HTMLButtonElement {
  const button = document.querySelector<HTMLButtonElement>('[data-testid="subagent-pill"]');
  if (button === null) throw new Error("Subagent pill did not render");
  return button;
}
const optionIds = () =>
  [...document.querySelectorAll<HTMLElement>("[data-overview-option]")]
    .map((option) => option.dataset.overviewOption)
    .sort();

describe("workspace overview eligibility", () => {
  it.each(["live", "ended"] as const)(
    "filters a %s terminal's pane attention while preserving its open tab",
    async (state) => {
      const row: Session = {
        ...terminalSession("terminal", "Terminal"),
        state:
          state === "ended"
            ? { type: "ended", generation: 1, code: 0, integrity: { kind: "complete" } }
            : { type: "live", generation: 1 },
        attention: { reason: "permission", atMs: 1 },
      };
      vi.mocked(sessionsList).mockResolvedValue([row]);
      sharedSessionController().open(row);
      await renderWorkspace(false);
      expect(tabElement(row.id).getAttribute("aria-selected")).toBe("true");
      const header = document.querySelector(".workspace-terminal-toolbar");
      expect(header).not.toBeNull();
      if (state === "ended") {
        expect(header?.querySelector(".workspace-dot-attention")).toBeNull();
        expect(header?.textContent).toContain("Stopped");
      } else {
        expect(header?.querySelector(".workspace-dot-attention")).not.toBeNull();
        expect(header?.textContent).toContain("Needs your approval");
      }
    },
  );

  it("fixture membership skips ended rows until explicitly opened", async () => {
    const ended = endedAgentSession("ended", "Ended");
    const live = agentSession("live", "Live");
    vi.mocked(sessionsList).mockResolvedValue([ended, live]);
    await renderWorkspace();
    const controller = sharedSessionController();
    expect(controller.getState().sessions).toHaveLength(2);
    expect(controller.getState().openSessions.map((row) => row.id)).toEqual([live.id]);
    expect(tabElement(live.id).getAttribute("aria-selected")).toBe("true");
    expect(document.querySelector("#workspace-session-tab-ended")).toBeNull();
    await act(async () => controller.open(ended));
    expect(tabElement(ended.id).getAttribute("aria-selected")).toBe("true");
  });

  it("matches main's live/silent/recovered membership plus already-open ended tabs", async () => {
    const rows = [
      agentSession("live", "Live"),
      silentAgentSession("silent", "Silent"),
      terminalSession("terminal", "Terminal"),
      recoveredAgentSession("recovered", "Recovered"),
      endedAgentSession("closed-ended", "Closed ended"),
      endedAgentSession("open-ended", "Open ended"),
      { ...agentSession("legacy", "Legacy"), workspaceId: null },
      { ...agentSession("other", "Other workspace"), workspaceId: "workspace-2" },
    ].map(fullRow);
    vi.mocked(sessionsList).mockResolvedValue(rows);
    const controller = sharedSessionController();
    controller.open(rows[5]);
    controller.open(rows[0]);
    await renderWorkspace(false);
    await act(async () => trigger().click());
    const baselineIds = rows
      .filter(
        (row) =>
          (row.workspaceId === "workspace-1" || row.workspaceId === null) &&
          (row.state.type === "live" ||
            row.state.type === "silent" ||
            row.state.type === "recovered"),
      )
      .map((row) => row.id);
    expect(optionIds()).toEqual([...baselineIds, "open-ended"].sort());
    expect(controller.getState().sessions).toHaveLength(rows.length);
    expect(tabElement("open-ended")).not.toBeNull();
    await act(async () => trigger().click());
    await chipClick("open-ended");
    await act(async () => trigger().click());
    expect(optionIds()).toEqual(baselineIds.sort());
    expect(controller.getState().sessions.some((row) => row.id === "open-ended")).toBe(true);
  });

  it.each([false, true])(
    "an ended child's stale attention lights nothing with open tab=%s",
    async (open) => {
      const parent = fullRow(agentSession("parent", "Coordinator"));
      const child = fullRow({
        ...endedAgentSession("child", "Ended child"),
        createdBy: parent.id,
        attention: { reason: "permission", atMs: 1 },
        unattended: "yes",
      });
      vi.mocked(sessionsList).mockResolvedValue([parent, child]);
      const controller = sharedSessionController();
      if (open) controller.open(child);
      controller.open(parent);
      await renderWorkspace(false);
      expect(document.querySelector(".sidebar-row-dot-attention")).toBeNull();
      expect(document.querySelector(".sidebar-row-dot-unattended")).toBeNull();
      expect(trigger().getAttribute("aria-label")).not.toContain("needs your approval");
      expect(trigger().querySelector(".strip-dot-attention")).toBeNull();
      if (open) expect(tabElement(child.id).querySelector(".strip-dot-attention")).toBeNull();
      const pill = subagentPill();
      expect(pill.getAttribute("aria-label")).not.toContain("working");
      expect(pill.getAttribute("aria-label")).not.toContain("needs your approval");
      expect(pill.querySelector(".workspace-subagent-attention")).toBeNull();
      await act(async () => pill.click());
      expect(
        document.querySelector(".workspace-subagent-row")?.getAttribute("aria-label"),
      ).toContain("Ended child, finished");
      expect(
        document.querySelector(".workspace-subagent-row .workspace-subagent-attention"),
      ).toBeNull();
      await act(async () => pill.click());
      await act(async () => trigger().click());
      expect(optionIds()).toEqual(open ? [child.id, parent.id] : [parent.id]);
      expect(document.querySelector(".workspace-overview-attention")).toBeNull();
    },
  );

  it.each(["finished", "error"] as const)(
    "does not call a live child's %s raise a request for approval",
    async (reason) => {
      const parent = fullRow(agentSession("parent", "Coordinator"));
      const child = fullRow({
        ...agentSession("child", "Child"),
        createdBy: parent.id,
        attention: { reason, atMs: 1 },
      });
      vi.mocked(sessionsList).mockResolvedValue([parent, child]);
      sharedSessionController().open(parent);
      await renderWorkspace(false);
      expect(document.querySelector(".sidebar-row-dot-attention")).toBeNull();
      expect(trigger().getAttribute("aria-label")).not.toContain("your approval");
      expect(trigger().querySelector(".strip-dot-attention")).toBeNull();
      const pill = subagentPill();
      expect(pill.getAttribute("aria-label")).not.toContain("your approval");
      await act(async () => trigger().click());
      const childOption = document.querySelector('[data-overview-option="child"]');
      expect(childOption?.textContent).toContain(reason === "finished" ? "Done" : "Failed");
      expect(childOption?.textContent).not.toContain("Needs your approval");
    },
  );
});
