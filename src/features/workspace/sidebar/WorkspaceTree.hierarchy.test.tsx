// @vitest-environment happy-dom

// The rail's hierarchy: a project with its count, its workspaces, and the
// selected workspace's agents nested right under its row.

import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { localWorkspaceKey, LOCAL_HOST_ID, type WorkspaceKey } from "../hosts/hostIdentity";
import type { WorkspaceProject, WorkspaceView } from "../workspaceProjects";
import type { AgentRowView } from "./agentRowViews";
import { WorkspaceTree, type WorkspaceTreeProps } from "./WorkspaceTree";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const workspace = (id: string, title: string): WorkspaceView => ({
  id,
  projectId: "p-1",
  hostId: LOCAL_HOST_ID,
  title,
  displayTitle: title,
  isolation: "worktree",
  path: `C:\\dev\\${id}`,
  agents: { working: 0, waiting: 0 },
  elapsedMs: null,
  stateDot: null,
});

const PROJECT: WorkspaceProject = {
  id: "p-1",
  name: "acme-web",
  path: "C:\\dev\\acme-web",
  hostId: LOCAL_HOST_ID,
  workspaces: [workspace("w-1", "Improve handoff"), workspace("w-2", "Cart recovery")],
};

const KEY_1 = localWorkspaceKey("w-1") as WorkspaceKey;
const KEY_2 = localWorkspaceKey("w-2") as WorkspaceKey;

const agent = (id: string, title: string): AgentRowView => ({
  id,
  kind: "claude",
  title,
  word: "idle",
  attention: false,
  working: false,
  age: "1h",
});

let container: HTMLDivElement;
let root: Root;

beforeEach(() => {
  container = document.createElement("div");
  document.body.append(container);
  root = createRoot(container);
});

afterEach(async () => {
  await act(async () => root.unmount());
  container.remove();
});

async function render(over: Partial<WorkspaceTreeProps> = {}): Promise<void> {
  await act(async () =>
    root.render(
      <WorkspaceTree
        projects={[PROJECT]}
        loading={false}
        error={null}
        providerError={null}
        selectedWorkspace={KEY_1}
        onRetryProjects={vi.fn()}
        onRetryProviders={vi.fn()}
        onSelectWorkspace={vi.fn()}
        onNewWorkspace={vi.fn()}
        onRenameWorkspace={vi.fn(async () => null)}
        onDeleteWorkspace={vi.fn(async () => null)}
        providerMenuAnchorProjectId={null}
        providerMenu={null}
        stats={new Map()}
        branches={new Map()}
        agentRows={
          new Map([
            [KEY_1, [agent("a-1", "Tighten handoff"), agent("a-2", "Draft notes")]],
            [KEY_2, [agent("a-3", "Backfill changelog")]],
          ])
        }
        activeSessionId="a-1"
        onOpenAgent={vi.fn()}
        {...over}
      />,
    ),
  );
}

const names = (): string[] =>
  [...container.querySelectorAll(".workspace-agent-name")].map((el) => el.textContent ?? "");

describe("the project, its workspaces and their agents", () => {
  it("counts the project's workspaces at its header, and says the count to a screen reader", async () => {
    await render();

    const count = container.querySelector(".workspace-project-count");
    expect(count?.querySelector("[aria-hidden]")?.textContent).toBe("2");
    expect(count?.querySelector(".sr-only")?.textContent).toBe("2 workspaces");
  });

  it("nests the selected workspace's agents right under its row, and no other workspace's", async () => {
    await render();

    expect(names()).toEqual(["Tighten handoff", "Draft notes"]);
    const items = container.querySelector(".workspace-project-items");
    const children = [...(items?.children ?? [])].map((child) => child.className);
    expect(children).toEqual(["workspace-row-wrap", "workspace-agent-rows", "workspace-row-wrap"]);
  });

  it("moves the agents with the selection", async () => {
    await render({ selectedWorkspace: KEY_2 });

    expect(names()).toEqual(["Backfill changelog"]);
  });

  it("lists nothing under a selected workspace that has no agents", async () => {
    await render({ agentRows: new Map() });

    expect(container.querySelector(".workspace-agent-rows")).toBeNull();
  });

  it("opens an agent through the one open road, naming the session", async () => {
    const onOpenAgent = vi.fn();
    await render({ onOpenAgent });

    await act(async () =>
      container.querySelectorAll<HTMLButtonElement>(".workspace-agent-row")[1]?.click(),
    );

    expect(onOpenAgent).toHaveBeenCalledWith("a-2");
  });

  it("marks the workspace row that is selected and the agent whose tab is in front", async () => {
    await render();

    expect(
      container.querySelector(".workspace-row-selected .workspace-row-title")?.textContent,
    ).toBe("Improve handoff");
    expect(
      container.querySelector(".workspace-agent-row-active .workspace-agent-name")?.textContent,
    ).toBe("Tighten handoff");
  });

  it("leaves the counts beside a selected workspace's name to a screen reader, since its agents say it below", async () => {
    const busy: WorkspaceProject = {
      ...PROJECT,
      workspaces: [
        {
          ...workspace("w-1", "Improve handoff"),
          agents: { working: 2, waiting: 1 },
          stateDot: "attention",
        },
        {
          ...workspace("w-2", "Cart recovery"),
          agents: { working: 1, waiting: 0 },
          stateDot: "pulse",
        },
      ],
    };
    await render({ projects: [busy] });

    const [selected, other] = [...container.querySelectorAll(".workspace-row-fact")];
    // The selected row keeps its dot, and says its counts only to assistive tech.
    expect(selected?.querySelector(".sidebar-row-dot-attention")).not.toBeNull();
    expect(selected?.querySelector(".sr-only")?.textContent).toContain("1 waiting");
    // An unselected row has no agents listed under it, so its counts stay on the line.
    expect(other?.querySelector(".sr-only")).toBeNull();
    expect(other?.textContent).toContain("1 working");
  });
});
