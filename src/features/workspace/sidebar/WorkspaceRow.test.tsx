// @vitest-environment happy-dom

// What the workspace row prints: its name (or the branch when the heading
// above already printed that name), at most one trailing fact, and the detail
// a tooltip carries for every row — reachable without selecting it. The row's
// menu, its title editor and its delete ask are pinned in WorkspaceTree.rename.

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

describe("the workspace row's one line", () => {
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
    facts: { stat?: WorkspaceStat; branch?: string; projectName?: string } = {},
  ): Promise<HTMLElement> {
    root = createRoot(container);
    await act(async () => {
      root.render(
        <WorkspaceRow
          workspace={view}
          workspaceKey={localWorkspaceKey(view.id)}
          projectName={facts.projectName ?? "devboule"}
          selected={false}
          agentsListed={false}
          stat={facts.stat}
          branch={facts.branch}
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

  it("prints the name and its fact as siblings on the first line, and no second line without a branch or totals", async () => {
    const row = await render(workspace({ stateDot: "pulse", agents: { working: 2, waiting: 0 } }));

    expect([...row.children].map((child) => child.className)).toEqual([
      "sidebar-avatar sidebar-avatar-workspace",
      "workspace-row-body",
    ]);
    expect(
      [...(row.querySelector(".workspace-row-line")?.children ?? [])].map(
        (child) => child.className,
      ),
    ).toEqual(["workspace-row-title", "workspace-row-fact"]);
    expect(row.querySelector(".workspace-row-sub")).toBeNull();
    expect(row.classList.contains("workspace-row-two")).toBe(false);
    expect(row.querySelector(".workspace-row-fact")?.textContent).toBe("2 working");
    expect(row.querySelector(".sidebar-row-dot-pulse")).not.toBeNull();
  });

  it("prints the approval and the count beside it, never one instead of the other", async () => {
    const row = await render(
      workspace({ stateDot: "attention", agents: { working: 2, waiting: 1 } }),
    );

    const fact = row.querySelector<HTMLElement>(".workspace-row-fact");
    if (fact === null) throw new Error("the row printed no fact");
    expect(fact.textContent).toBe("1 waiting\u00A0· 2 working");
    // The ask keeps the attention tone; the count beside it stays muted.
    expect(fact.querySelector(".sidebar-row-waiting")?.textContent).toBe("1 waiting");
    expect(fact.querySelector(".sidebar-row-dot-attention")).not.toBeNull();
    expect(fact.querySelectorAll("span")).toHaveLength(3);
  });

  it("says an agent waits in the attention tone when it waits alone", async () => {
    const row = await render(
      workspace({ stateDot: "attention", agents: { working: 0, waiting: 1 } }),
    );

    const fact = row.querySelector<HTMLElement>(".workspace-row-fact");
    if (fact === null) throw new Error("the row printed no fact");
    expect(fact.textContent).toBe("1 waiting");
    expect(fact.className).toContain("workspace-row-fact");
    expect(fact.querySelector(".sidebar-row-waiting")).not.toBeNull();
  });

  it("prints the name alone when the sidebar has no fact for it", async () => {
    const row = await render(workspace());

    expect(row.querySelector(".workspace-row-fact")).toBeNull();
    expect(row.querySelector(".workspace-row-title")?.textContent).toBe("devboule-v2");
  });

  it("puts the branch in mono on a second line, with the uncommitted totals at its far end", async () => {
    const row = await render(workspace(), {
      branch: "feat/handoff",
      stat: { additions: 12, deletions: 3 },
    });

    const sub = row.querySelector(".workspace-row-sub");
    expect(sub?.querySelector(".workspace-row-branch")?.textContent).toBe("feat/handoff");
    expect(sub?.querySelector(".workspace-row-totals")?.textContent).toBe("+12 −3");
    // The totals are the second line's, not a fact of the first.
    expect(row.querySelector(".workspace-row-fact")).toBeNull();
    expect(row.classList.contains("workspace-row-two")).toBe(true);
  });

  it("leaves out a clean tree's zero totals, and keeps one line when the branch is unknown too", async () => {
    const row = await render(workspace(), { stat: { additions: 0, deletions: 0 } });

    expect(row.querySelector(".workspace-row-fact")).toBeNull();
    expect(row.querySelector(".workspace-row-sub")).toBeNull();
  });

  it("does not print a branch twice when it already stands in for the name", async () => {
    const row = await render(workspace({ displayTitle: "devboule" }), { branch: "main" });

    expect(row.querySelector(".workspace-row-title")?.textContent).toBe("main");
    expect(row.querySelector(".workspace-row-sub")).toBeNull();
  });

  it("keeps the last activity right of the name, and the totals on the second line", async () => {
    const row = await render(workspace({ elapsedMs: 4 * 60_000 }), {
      branch: "main",
      stat: { additions: 12, deletions: 3 },
    });

    expect(row.querySelector(".workspace-row-fact")?.textContent).toBe("4m");
    expect(row.querySelector(".workspace-row-totals")?.textContent).toBe("+12 −3");
  });

  it("reads a session that just spoke as now, not as a year", async () => {
    // The row's age is the roster's silence measured as a duration. Anything
    // that subtracts it from a clock dates the agent to the epoch.
    const row = await render(
      workspaceView(workspace(), [rosterSession({ state: { type: "silent", generation: 1 } })]),
    );

    expect(row.querySelector(".workspace-row-fact")?.textContent).toBe("now");
  });

  it("shows when a workspace's only session stopped", async () => {
    const row = await render(
      workspaceView(workspace(), [
        rosterSession({
          state: { type: "ended", generation: 1, code: 0, integrity: { kind: "complete" } },
          elapsedMs: 4 * 60_000,
        }),
      ]),
    );

    expect(row.querySelector(".workspace-row-fact")?.textContent).toBe("4m");
  });

  it("prints no time for a workspace whose roster carries no activity fact", async () => {
    const row = await render(workspace({ elapsedMs: null }));

    expect(row.querySelector(".workspace-row-fact")).toBeNull();
  });

  it("speaks the branch when the heading above already printed this name", async () => {
    const row = await render(workspace({ displayTitle: "devboule" }), {
      projectName: "devboule",
      branch: "main",
    });

    expect(row.querySelector(".workspace-row-title")?.textContent).toBe("main");
    // The title it stands in for stays in the accessible name.
    expect(row.getAttribute("aria-label")).toBe("main, devboule");
  });

  it("keeps its own name when it differs from the project's", async () => {
    const row = await render(workspace({ displayTitle: "figures" }), {
      projectName: "paperlab-studio",
      branch: "docs/figures",
    });

    expect(row.querySelector(".workspace-row-title")?.textContent).toBe("figures");
    expect(row.getAttribute("aria-label")).toBe("figures, paperlab-studio");
  });

  it("keeps its own name when the project has no branch to speak instead", async () => {
    const row = await render(workspace({ displayTitle: "devboule" }), { projectName: "devboule" });

    expect(row.querySelector(".workspace-row-title")?.textContent).toBe("devboule");
    expect(row.getAttribute("aria-label")).toBe("devboule");
  });

  it("carries the fact in the row's accessible name", async () => {
    const row = await render(workspace({ stateDot: "pulse", agents: { working: 2, waiting: 0 } }));

    expect(row.getAttribute("aria-label")).toBe("devboule-v2, devboule, 2 working");
  });

  it("hands the branch and the totals to a pointer's tooltip and to a screen reader", async () => {
    // The row prints one fact; the rest of what the sidebar knows about it is
    // on the row itself, so nothing waits for a selection to become readable.
    // `title` reaches a pointer only — the description is what a screen reader
    // announces, and what it points at must be text a sighted row never draws.
    const row = await render(workspace(), {
      branch: "feat/ux-sidebar-clean",
      stat: { additions: 128, deletions: 4 },
    });

    const detail = "C:\\devboule-v2 · feat/ux-sidebar-clean · +128 −4";
    expect(row.getAttribute("title")).toBe(detail);

    const described = row.getAttribute("aria-describedby");
    if (described === null) throw new Error("the row describes nothing");
    const description = document.getElementById(described);
    expect(description?.textContent).toBe(detail);
    expect(description?.className).toBe("sr-only");
  });

  it("names only the path when branch and totals have nothing to say", async () => {
    const row = await render(workspace(), { stat: { additions: 0, deletions: 0 } });

    expect(row.getAttribute("title")).toBe("C:\\devboule-v2");
  });
});
