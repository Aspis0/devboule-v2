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
        onArchiveWorkspace={vi.fn(async () => null)}
        onRemoveProject={vi.fn(async () => null)}
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
  it("heads each project with its badge, a + that starts a workspace, and a kebab: no count", async () => {
    await render();

    const head = container.querySelector(".sidebar-project-head");
    expect(head?.querySelector(".sidebar-avatar-project")?.textContent).toBe("a");
    expect(head?.querySelector('button[aria-label="New workspace in acme-web"]')).not.toBeNull();
    expect(head?.querySelector('button[aria-label="Actions for acme-web"]')).not.toBeNull();
    expect(container.querySelector(".workspace-project-count")).toBeNull();
  });

  it("lists the project's workspaces, and no row closes the list: the + sits in the header", async () => {
    await render();

    const items = container.querySelector(".workspace-project-items");
    const children = [...(items?.children ?? [])].map((child) => child.className);
    expect(children).toEqual(["workspace-row-wrap", "workspace-row-wrap"]);
    expect(items?.querySelector(".workspace-project-new")).toBeNull();
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

  it("shows a selected workspace's approval owed as the attention dot, with no count printed", async () => {
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

    const row = container.querySelector(".workspace-row-selected");
    expect(row?.querySelector(".workspace-row-status-attention")).not.toBeNull();
    expect(row?.getAttribute("aria-label")).toContain("needs you");
    expect(row?.textContent).not.toContain("waiting");
  });

  it("shows no count for a project whose workspaces did not load, and says why in its alert", async () => {
    await render({
      projects: [
        { ...PROJECT, workspaces: [], workspaceError: { sentence: "Pipe busy" } as never },
      ],
    });

    expect(container.querySelector(".workspace-project-count")).toBeNull();
    expect(container.querySelector('[role="alert"]')?.textContent).toContain(
      "Could not load this project's workspaces",
    );
  });
});
