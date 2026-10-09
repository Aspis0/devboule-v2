// @vitest-environment happy-dom

// The workspace row's per-row host line (slice sidebar-like-paseo): the host
// rides a small second line with a server glyph, diff stats sit on the right
// when known, and a workspace named like its project never repeats that name
// — it is titled by its branch, or by its host when no branch is known.

import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { LOCAL_HOST_ID, localWorkspaceKey } from "../hosts/hostIdentity";
import type { WorkspaceView } from "../workspaceProjects";
import type { WorkspaceStat } from "./useWorkspaceStats";
import { WorkspaceRow } from "./WorkspaceRow";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const STAT: WorkspaceStat = { additions: 352, deletions: 66 };

function workspace(over: Partial<WorkspaceView> = {}): WorkspaceView {
  return {
    id: "workspace-1",
    projectId: "project-1",
    hostId: LOCAL_HOST_ID,
    title: "shell one",
    displayTitle: "shell one",
    isolation: "local",
    path: "C:\\code\\alpha",
    agents: { working: 0, waiting: 0 },
    elapsedMs: null,
    stateDot: null,
    ...over,
  };
}

describe("the workspace row's host line", () => {
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
    facts: { stat?: WorkspaceStat; branch?: string; projectName?: string; hostName?: string } = {},
  ): Promise<HTMLElement> {
    root = createRoot(container);
    await act(async () => {
      root.render(
        <WorkspaceRow
          workspace={view}
          workspaceKey={localWorkspaceKey(view.id)}
          projectName={facts.projectName ?? "Alpha"}
          hostName={facts.hostName ?? "This PC"}
          selected={false}
          agentFocused={false}
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

  it("prints the host on a small second line with a server glyph", async () => {
    const row = await render(workspace(), { branch: "main" });

    const sub = row.querySelector<HTMLElement>(".workspace-row-sub");
    if (sub === null) throw new Error("the row has no second line");
    expect(sub.textContent).toContain("This PC");
    const glyph = sub.querySelector("svg.workspace-row-host-glyph");
    if (glyph === null) throw new Error("the host line has no server glyph");
    expect(glyph.getAttribute("aria-hidden")).toBe("true");
  });

  it("keeps diff stats on the right when known", async () => {
    const row = await render(workspace(), { branch: "main", stat: STAT });

    expect(row.querySelector(".workspace-row-totals")?.textContent).toBe("+352 −66");
  });

  it("titles a same-named workspace by its branch, never the project name twice", async () => {
    const row = await render(workspace({ title: "Alpha", displayTitle: "Alpha" }), {
      branch: "feature-x",
      projectName: "Alpha",
    });

    expect(row.querySelector(".workspace-row-title")?.textContent).toBe("feature-x");
  });

  it("titles a same-named workspace by its host when no branch is known", async () => {
    const row = await render(workspace({ title: "Alpha", displayTitle: "Alpha" }), {
      projectName: "Alpha",
      hostName: "Marcolenovo",
    });

    const title = row.querySelector(".workspace-row-title")?.textContent ?? "";
    expect(title).toBe("Marcolenovo");
    expect(title).not.toBe("Alpha");
  });

  it("names the host for a screen reader", async () => {
    const row = await render(workspace(), { branch: "main", hostName: "Marcolenovo" });

    expect(row.getAttribute("aria-label")).toContain("Marcolenovo");
  });
});
