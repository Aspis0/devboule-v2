// @vitest-environment happy-dom

// The rail's agents: the selected workspace lists its agents under its row, the
// other rows say only how many agents work or wait in them.

import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { localWorkspaceKey, LOCAL_HOST_ID, type WorkspaceKey } from "../hosts/hostIdentity";
import type { WorkspaceProject, WorkspaceView } from "../workspaceProjects";
import type { RailAgent } from "./railAgents";
import { WorkspaceTree, type WorkspaceTreeProps } from "./WorkspaceTree";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const workspace = (
  id: string,
  title: string,
  agents = { working: 0, waiting: 0 },
): WorkspaceView => ({
  id,
  projectId: "p-1",
  hostId: LOCAL_HOST_ID,
  title,
  displayTitle: title,
  isolation: "worktree",
  path: `C:\\dev\\${id}`,
  agents,
  stateDot: null,
});

const PROJECT: WorkspaceProject = {
  id: "p-1",
  name: "acme-web",
  path: "C:\\dev\\acme-web",
  hostId: LOCAL_HOST_ID,
  workspaces: [
    workspace("w-1", "Improve handoff"),
    workspace("w-2", "Cart recovery", { working: 2, waiting: 0 }),
    workspace("w-3", "Checkout", { working: 0, waiting: 1 }),
  ],
};

const KEY_1 = localWorkspaceKey("w-1") as WorkspaceKey;

const agent = (over: Partial<RailAgent>): RailAgent => ({
  id: "session",
  kind: "claude",
  provider: "Claude",
  title: "cia",
  state: "idle",
  child: false,
  parentId: null,
  parentTitle: null,
  hasChildren: false,
  ...over,
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

const rowFor = (title: string) =>
  [...container.querySelectorAll<HTMLButtonElement>("button.workspace-row")].find(
    (button) => button.querySelector(".workspace-row-title")?.textContent === title,
  );

describe("the agents in the rail", () => {
  it("lists the selected workspace's agents directly under its row", async () => {
    await render({
      selectedAgents: {
        agents: { rows: [agent({ id: "root" })], hidden: 2 },
        activeId: null,
        onOpenAgent: vi.fn(),
        onOpenHistory: vi.fn(),
      },
    });
    const wrap = rowFor("Improve handoff")?.closest(".workspace-row-wrap");
    expect(wrap?.nextElementSibling?.classList.contains("agent-rows")).toBe(true);
    expect(container.querySelector(".agent-row-more")?.textContent).toBe("+2 more in History");
  });

  it("lists no agents when the surface does not carry them", async () => {
    await render();
    expect(container.querySelector(".agent-rows")).toBeNull();
  });

  it("shows the count line on a workspace that is not selected, and nothing on the selected one", async () => {
    await render();
    expect(rowFor("Cart recovery")?.querySelector(".workspace-row-count")?.textContent).toBe(
      "2 working",
    );
    expect(rowFor("Checkout")?.querySelector(".workspace-row-count")?.textContent).toBe(
      "1 needs you",
    );
    expect(rowFor("Improve handoff")?.querySelector(".workspace-row-count")).toBeNull();
  });
});
