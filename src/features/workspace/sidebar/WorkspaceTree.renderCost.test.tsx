// @vitest-environment happy-dom

// What a poll and a selection cost the rail. Workspace re-renders whenever the
// daemon republishes (every two seconds), so the rows are memoised and their
// props are stable references: a re-render with nothing changed must not reach
// a row, and a change must reach only the rows it is about.

import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { WorkspaceKey } from "../hosts/hostIdentity";
import { LOCAL_HOST_ID, localWorkspaceKey } from "../hosts/hostIdentity";
import type { WorkspaceProject, WorkspaceView } from "../workspaceProjects";
import { WorkspaceTree, type WorkspaceTreeProps } from "./WorkspaceTree";
import type { WorkspaceStat } from "./useWorkspaceStats";

/** The row's own render, counted: one per row that actually re-rendered. */
const counters = vi.hoisted(() => ({ rowRenders: 0 }));

vi.mock("./rowFact", async (importOriginal) => {
  const actual = await importOriginal<typeof import("./rowFact")>();
  return {
    ...actual,
    rowFact: (...args: Parameters<typeof actual.rowFact>) => {
      counters.rowRenders += 1;
      return actual.rowFact(...args);
    },
  };
});

const workspace = (id: string, title: string): WorkspaceView => ({
  id,
  projectId: "project-1",
  hostId: LOCAL_HOST_ID,
  title,
  displayTitle: title,
  isolation: "worktree",
  path: `C:\\dev\\worktrees\\${id}`,
  agents: { working: 0, waiting: 0 },
  elapsedMs: null,
  stateDot: null,
});

const W1 = workspace("w-1", "devboule-v2");
const W2 = workspace("w-2", "devboule-v2 2");
const W3 = workspace("w-3", "release-prep");

const PROJECT: WorkspaceProject = {
  id: "project-1",
  name: "devboule-v2",
  path: "C:\\dev\\devboule-v2",
  hostId: LOCAL_HOST_ID,
  workspaces: [W1, W2, W3],
};

const KEY1 = localWorkspaceKey("w-1") as WorkspaceKey;
const KEY3 = localWorkspaceKey("w-3") as WorkspaceKey;

/** Built once: the app derives these in a memo, so the rows see the same
 *  references on every render of the tree. */
const STATS = new Map<WorkspaceKey, WorkspaceStat>([
  [KEY1, { additions: 12, deletions: 3 }],
  [KEY3, { additions: 0, deletions: 0 }],
]);
const BRANCHES = new Map<WorkspaceKey, string>([
  [KEY1, "main"],
  [KEY3, "release/0.9"],
]);

const onRename = vi.fn(async () => null);
const onDelete = vi.fn(async () => null);
const onSelect = vi.fn();
const onNewWorkspace = vi.fn();

function treeProps(over: Partial<WorkspaceTreeProps> = {}): WorkspaceTreeProps {
  return {
    projects: [PROJECT],
    loading: false,
    error: null,
    providerError: null,
    selectedWorkspace: null,
    onRetryProjects: vi.fn(),
    onSelectWorkspace: onSelect,
    onNewWorkspace,
    onRenameWorkspace: onRename,
    onDeleteWorkspace: onDelete,
    providerMenuAnchorProjectId: null,
    providerMenu: null,
    stats: STATS,
    branches: BRANCHES,
    ...over,
  };
}

describe("what the rail re-renders", () => {
  let container: HTMLDivElement;
  let root: Root;

  beforeEach(() => {
    counters.rowRenders = 0;
    container = document.createElement("div");
    document.body.appendChild(container);
    root = createRoot(container);
  });

  afterEach(async () => {
    await act(async () => root.unmount());
    container.remove();
  });

  /** The tree renders again with the same contents, as a poll does. */
  async function repaint(over: Partial<WorkspaceTreeProps> = {}): Promise<void> {
    await act(async () => {
      root.render(<WorkspaceTree {...treeProps(over)} />);
    });
  }

  it("reaches no row when nothing changed, however often the tree repaints", async () => {
    await repaint();
    const mounted = counters.rowRenders;
    expect(mounted).toBe(3);

    for (let poll = 0; poll < 5; poll += 1) await repaint();

    expect(counters.rowRenders).toBe(mounted);
  });

  it("reaches the one row a selection changed, and no other", async () => {
    await repaint();
    const mounted = counters.rowRenders;

    await repaint({ selectedWorkspace: KEY3 });

    expect(counters.rowRenders).toBe(mounted + 1);
    expect(container.querySelectorAll(".workspace-row-selected")).toHaveLength(1);
  });

  it("reaches the one row whose facts changed", async () => {
    await repaint();
    const mounted = counters.rowRenders;
    const changed = new Map(STATS).set(KEY1, { additions: 90, deletions: 1 });

    await repaint({ stats: changed });

    expect(counters.rowRenders).toBe(mounted + 1);
    expect(container.querySelectorAll(".workspace-row-fact")[0]?.textContent).toBe("+90 −1");
  });
});
