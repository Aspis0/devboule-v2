// @vitest-environment happy-dom

// What the workspace row prints: the name and its last activity on the first
// line, the facts the sidebar already loaded on the second. The row's menu,
// its title editor and its delete ask are pinned in WorkspaceTree.rename.

import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { WorkspaceView } from "../workspaceProjects";
import { LOCAL_HOST_ID, localWorkspaceKey } from "../hosts/hostIdentity";
import type { WorkspaceStat } from "./useWorkspaceStats";
import { WorkspaceRow } from "./WorkspaceRow";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const NOW = 1_700_000_000_000;
const silentFor = (ms: number): number => NOW - ms;

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
          now={NOW}
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
    const row = await render(workspace({ elapsedMs: silentFor(4 * 60_000) }));

    expect(row.querySelector(".workspace-row-line")?.textContent).toBe("devboule-v24m");
  });

  it("prints no time for a workspace whose roster carries no activity fact", async () => {
    const row = await render(workspace({ elapsedMs: null }));

    expect(row.querySelector(".workspace-row-age")).toBeNull();
  });
});
