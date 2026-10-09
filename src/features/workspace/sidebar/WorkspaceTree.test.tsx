// @vitest-environment happy-dom

// What the tree says about identity: every project keeps its header and closes
// with a New workspace row of its own, two projects that share a name carry
// the folder that tells them apart, and a row never prints the name its header
// above it has already printed — the branch speaks instead, or the host with
// no branch known.

import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { WorkspaceKey } from "../hosts/hostIdentity";
import { LOCAL_HOST_ID, localWorkspaceKey } from "../hosts/hostIdentity";
import type { WorkspaceProject, WorkspaceView } from "../workspaceProjects";
import { WorkspaceTree, type WorkspaceTreeProps } from "./WorkspaceTree";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const workspace = (projectId: string, id: string, title: string, path: string): WorkspaceView => ({
  id,
  projectId,
  hostId: LOCAL_HOST_ID,
  title,
  displayTitle: title,
  isolation: "worktree",
  path,
  agents: { working: 0, waiting: 0 },
  elapsedMs: null,
  stateDot: null,
});

const project = (
  id: string,
  name: string,
  path: string,
  workspaces: WorkspaceView[],
): WorkspaceProject => ({
  id,
  name,
  path,
  hostId: LOCAL_HOST_ID,
  workspaces,
});

/** Two projects the daemon may hand us that share a name and nothing else. */
const TWIN_A = project("p-a", "design-sandbox", "C:\\clients\\alpha\\design-sandbox", [
  workspace("p-a", "w-a", "design-sandbox", "C:\\clients\\alpha\\design-sandbox"),
]);
const TWIN_B = project("p-b", "design-sandbox", "C:\\clients\\beta\\design-sandbox", [
  workspace("p-b", "w-b", "design-sandbox", "C:\\clients\\beta\\design-sandbox"),
]);
const SOLO = project("p-c", "paperlab-studio", "C:\\dev\\paperlab-studio", [
  workspace("p-c", "w-c", "paperlab-studio", "C:\\dev\\paperlab-studio"),
  workspace("p-c", "w-d", "figures", "C:\\dev\\figures"),
]);

const KEY_W_A = localWorkspaceKey("w-a") as WorkspaceKey;
const KEY_W_C = localWorkspaceKey("w-c") as WorkspaceKey;

function treeProps(over: Partial<WorkspaceTreeProps> = {}): WorkspaceTreeProps {
  return {
    projects: [],
    loading: false,
    error: null,
    providerError: null,
    selectedWorkspace: null,
    onRetryProjects: vi.fn(),
    onRetryProviders: vi.fn(),
    onSelectWorkspace: vi.fn(),
    onNewWorkspace: vi.fn(),
    onRenameWorkspace: vi.fn(async () => null),
    onDeleteWorkspace: vi.fn(async () => null),
    providerMenuAnchorProjectId: null,
    providerMenu: null,
    stats: new Map(),
    branches: new Map<WorkspaceKey, string>([
      [KEY_W_A, "main"],
      [KEY_W_C, "feat/sidebar"],
    ]),
    agentRows: new Map(),
    activeSessionId: null,
    onOpenAgent: vi.fn(),
    hostNames: new Map([[LOCAL_HOST_ID, "This PC"]]),
    ...over,
  };
}

describe("the tree's identity", () => {
  let container: HTMLDivElement;
  let root: Root;

  beforeEach(() => {
    container = document.createElement("div");
    document.body.appendChild(container);
    root = createRoot(container);
  });

  afterEach(async () => {
    await act(async () => root.unmount());
    container.remove();
  });

  async function render(over: Partial<WorkspaceTreeProps> = {}): Promise<void> {
    await act(async () => {
      root.render(<WorkspaceTree {...treeProps(over)} />);
    });
  }

  function heads(): HTMLElement[] {
    return [...container.querySelectorAll<HTMLElement>(".workspace-project-heading")];
  }

  function folders(): string[] {
    return [...container.querySelectorAll<HTMLElement>(".workspace-project-folder")].map(
      (el) => el.textContent ?? "",
    );
  }

  function groups(): string[] {
    return [...container.querySelectorAll<HTMLElement>('[role="group"]')].map(
      (el) => el.getAttribute("aria-label") ?? "",
    );
  }

  it("gives every project a header and closes it with a New workspace row", async () => {
    await render({ projects: [TWIN_A, SOLO] });

    expect(heads()).toHaveLength(2);
    // No hover-only control in the header: the create row below the rows is
    // the project's one creation path.
    expect(container.querySelectorAll(".workspace-project-add")).toHaveLength(0);
    const rows = [...container.querySelectorAll<HTMLButtonElement>(".workspace-project-new")];
    expect(rows).toHaveLength(2);
    expect(rows.map((row) => row.getAttribute("aria-label"))).toEqual([
      "New workspace in design-sandbox",
      "New workspace in paperlab-studio",
    ]);
  });

  it("tells two projects sharing a name apart with the folder each sits in", async () => {
    await render({ projects: [TWIN_A, TWIN_B] });

    expect(folders()).toEqual(["alpha", "beta"]);
    expect(groups()).toEqual(["design-sandbox in alpha", "design-sandbox in beta"]);
    expect(heads()[0]?.textContent).toContain("design-sandbox");
  });

  it("carries no folder where the name is already the only one of its kind", async () => {
    await render({ projects: [SOLO] });

    expect(folders()).toEqual([]);
    expect(groups()).toEqual(["paperlab-studio"]);
  });

  it("lets the branch speak for a row whose name its header printed", async () => {
    await render({ projects: [TWIN_A] });

    const title = container.querySelector<HTMLElement>(".workspace-row-title");
    expect(title?.textContent).toBe("main");
  });

  it("speaks the branch where the header printed the name, keeps the name where it did not", async () => {
    await render({ projects: [SOLO] });

    expect(
      [...container.querySelectorAll<HTMLElement>(".workspace-row-title")].map(
        (el) => el.textContent,
      ),
    ).toEqual(["feat/sidebar", "figures"]);
  });

  it("names an unknown host without leaking its raw id", async () => {
    const gone = project("p-g", "field", "C:\\field", [
      { ...workspace("p-g", "w-g", "field work", "C:\\field"), hostId: "peer-gone" as never },
    ]);
    await render({ projects: [gone] });

    const host = container.querySelector(".workspace-row-host");
    expect(host?.textContent).toContain("Unknown host");
    expect(host?.textContent).not.toContain("peer-gone");
  });
});
