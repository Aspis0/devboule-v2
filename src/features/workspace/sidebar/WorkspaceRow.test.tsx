// @vitest-environment happy-dom

// What the workspace row prints: the name and its last activity on the first
// line, the facts the sidebar already loaded on the second. The row's menu,
// its title editor and its delete ask are pinned in WorkspaceTree.rename.

import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { Session } from "../../../types/ipc";
import { workspaceView, type WorkspaceView } from "../workspaceProjects";
import { LOCAL_HOST_ID, localWorkspaceKey } from "../hosts/hostIdentity";
import type { WorkspaceStat } from "./useWorkspaceStats";
import { WorkspaceRow } from "./WorkspaceRow";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

/** One roster row as the daemon sends it: `elapsedMs` counts milliseconds SINCE
 * the last observed output, so it grows with silence and is never an instant. */
const rosterSession = (over: Partial<Session> = {}): Session => ({
  id: "session-1",
  workspaceId: "workspace-1",
  kind: "claude",
  title: "agent",
  state: { type: "live", generation: 1 },
  elapsedMs: 0,
  ...over,
});

const workspace = (over: Partial<WorkspaceView> = {}): WorkspaceView => ({
  id: "workspace-1",
  projectId: "project-1",
  hostId: LOCAL_HOST_ID,
  title: "devboule-v2",
  displayTitle: "devboule-v2",
  isolation: "local",
  path: "C:\\devboule-v2",
  agents: { working: 0, waiting: 0 },
  elapsedMs: null,
  stateDot: null,
  ...over,
});

describe("the workspace row's two lines", () => {
  let container: HTMLDivElement;
  let root: Root;

  beforeEach(() => {
    container = document.createElement("div");
    document.body.appendChild(container);
  });

  afterEach(async () => {
    await act(async () => root.unmount());
    container.remove();
  });

  async function render(
    view: WorkspaceView,
    facts: { branch?: string; stat?: WorkspaceStat } = {},
  ): Promise<HTMLElement> {
    root = createRoot(container);
    await act(async () => {
      root.render(
        <WorkspaceRow
          workspace={view}
          workspaceKey={localWorkspaceKey(view.id)}
          projectName="devboule"
          selected={false}
          branch={facts.branch}
          stat={facts.stat}
          onSelect={vi.fn()}
          onRename={vi.fn(async () => null)}
          onDelete={vi.fn(async () => null)}
        />,
      );
    });
    const row = container.querySelector<HTMLElement>(".workspace-row");
    if (row === null) throw new Error("the workspace row did not render");
    return row;
  }

  it("prints the branch, the diff stats and the agent summary on the second line", async () => {
    const row = await render(workspace({ stateDot: "pulse", agents: { working: 2, waiting: 0 } }), {
      branch: "feat/sidebar-rows",
      stat: { additions: 12, deletions: 3 },
    });

    const facts = row.querySelector<HTMLElement>(".workspace-row-facts");
    if (facts === null) throw new Error("the row printed no facts line");
    expect(facts.querySelector(".workspace-row-branch")?.textContent).toBe("feat/sidebar-rows");
    expect(facts.querySelector(".sidebar-stat-add")?.textContent).toBe("+12");
    expect(facts.querySelector(".sidebar-stat-del")?.textContent).toBe("−3");
    expect(facts.textContent).toContain("2 working");
  });

  it("leaves out every part the sidebar has nothing for", async () => {
    // No branch (the read failed or the folder is not a repository), no diff,
    // no agent: one line, and no empty second line under it.
    const row = await render(workspace());

    expect(row.querySelector(".workspace-row-facts")).toBeNull();
    expect(row.querySelector(".workspace-row-title")?.textContent).toBe("devboule-v2");
  });

  it("prints the branch alone when it is the only fact", async () => {
    const row = await render(workspace(), { branch: "main" });

    const facts = row.querySelector<HTMLElement>(".workspace-row-facts");
    if (facts === null) throw new Error("the row printed no facts line");
    expect(facts.querySelector(".workspace-row-branch")?.textContent).toBe("main");
    expect(facts.querySelector(".sidebar-row-stats")).toBeNull();
    expect(facts.querySelector(".sidebar-row-agents")).toBeNull();
  });

  it("says an agent waits in the attention tone, beside its live dot", async () => {
    const row = await render(
      workspace({ stateDot: "attention", agents: { working: 2, waiting: 1 } }),
    );

    const facts = row.querySelector<HTMLElement>(".sidebar-row-agents");
    if (facts === null) throw new Error("the row printed no agent summary");
    expect(facts.querySelector(".sidebar-row-dot-attention")).not.toBeNull();
    expect(facts.querySelector(".sidebar-row-working")?.textContent).toBe("2 working");
    expect(facts.querySelector(".sidebar-row-waiting")?.textContent).toBe("1 waiting");
  });

  it("puts the last activity right of the name", async () => {
    const row = await render(
      workspaceView(workspace(), [rosterSession({ elapsedMs: 4 * 60_000 })]),
    );

    expect(row.querySelector(".workspace-row-line")?.textContent).toBe("devboule-v24m");
  });

  it("reads a session that just spoke as now, not as a year", async () => {
    // The row's age is the roster's silence measured as a duration. Anything
    // that subtracts it from a clock dates the agent to the epoch.
    const row = await render(workspaceView(workspace(), [rosterSession({ elapsedMs: 30_000 })]));

    expect(row.querySelector(".workspace-row-age")?.textContent).toBe("now");
  });

  it("reads a recovered-only workspace as no time at all", async () => {
    const row = await render(
      workspaceView(workspace(), [
        rosterSession({
          state: {
            type: "recovered",
            generation: 1,
            integrity: { kind: "unverifiable", droppedFrames: 0, droppedBytes: 0, trimmedBytes: 0 },
          },
          elapsedMs: null,
        }),
      ]),
    );

    expect(row.querySelector(".workspace-row-age")).toBeNull();
  });

  it("shows when a workspace's only session stopped", async () => {
    // An ended row still reports its last output, so the age is real and must
    // read as minutes — never as the epoch, never as nothing.
    const row = await render(
      workspaceView(workspace(), [
        rosterSession({
          state: { type: "ended", generation: 1, code: 0, integrity: { kind: "complete" } },
          elapsedMs: 4 * 60_000,
        }),
      ]),
    );

    expect(row.querySelector(".workspace-row-age")?.textContent).toBe("4m");
  });

  it("prints no time for a workspace whose roster carries no activity fact", async () => {
    const row = await render(workspace({ elapsedMs: null }));

    expect(row.querySelector(".workspace-row-age")).toBeNull();
  });
});
