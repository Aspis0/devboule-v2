// @vitest-environment happy-dom

// The rail's hierarchy: a project with its count, its workspace rows and the
// "+ New workspace" row that closes the list. Agents live in the tabs, so the
// rail lists workspaces only and nests nothing under a row.

import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { localWorkspaceKey, LOCAL_HOST_ID, type WorkspaceKey } from "../hosts/hostIdentity";
import type { WorkspaceProject, WorkspaceView } from "../workspaceProjects";
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
        hostNames={new Map()}
        {...over}
      />,
    ),
  );
}

describe("the project and its workspaces", () => {
  it("counts the project's workspaces at its header, and says the count to a screen reader", async () => {
    await render();

    const count = container.querySelector(".workspace-project-count");
    expect(count?.querySelector("[aria-hidden]")?.textContent).toBe("2");
    expect(count?.querySelector(".sr-only")?.textContent).toBe("2 workspaces");
  });

  it("lists the project's workspaces, and closes the list with the new-workspace row", async () => {
    await render();

    const items = container.querySelector(".workspace-project-items");
    const children = [...(items?.children ?? [])].map((child) => child.className);
    expect(children).toEqual(["workspace-row-wrap", "workspace-row-wrap", "workspace-project-new"]);
  });

  it("nests no agents under a workspace: the tabs hold those", async () => {
    await render();

    expect(container.querySelector(".workspace-agent-rows")).toBeNull();
    expect(container.querySelector(".workspace-agent-row")).toBeNull();
  });

  it("marks the workspace row that is selected", async () => {
    await render();

    expect(
      container.querySelector(".workspace-row-selected .workspace-row-title")?.textContent,
    ).toBe("Improve handoff");
    expect(container.querySelectorAll(".workspace-row-agent-focused")).toHaveLength(0);
  });

  it("keeps a selected workspace's counts drawn: a waiting subagent shows nowhere else", async () => {
    const busy: WorkspaceProject = {
      ...PROJECT,
      workspaces: [
        {
          ...workspace("w-1", "Improve handoff"),
          agents: { working: 0, waiting: 1 },
          stateDot: "attention",
        },
        workspace("w-2", "Cart recovery"),
      ],
    };
    await render({ projects: [busy] });

    const fact = container.querySelector(".workspace-row-selected .workspace-row-fact");
    // The waiting one is a subagent the rail never listed.
    expect(fact?.textContent).toContain("1 waiting");
    expect(fact?.querySelector(".sr-only")).toBeNull();
  });

  it("shows no count, and says so, for a project whose workspaces did not load", async () => {
    await render({
      projects: [
        { ...PROJECT, workspaces: [], workspaceError: { sentence: "Pipe busy" } as never },
      ],
    });

    const count = container.querySelector(".workspace-project-count");
    expect(count?.querySelector("[aria-hidden]")).toBeNull();
    expect(count?.textContent).not.toContain("0");
    expect(count?.querySelector(".sr-only")?.textContent).toBe("workspaces could not be loaded");
  });
});
