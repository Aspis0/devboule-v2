// @vitest-environment happy-dom

// Focus after the last agent under the selected row is archived: the row itself.
import { act, type ReactNode, type RefObject } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { ConfirmProvider } from "../../../components/ConfirmHost";
import { localWorkspaceKey, LOCAL_HOST_ID, type WorkspaceKey } from "../hosts/hostIdentity";
import type { WorkspaceProject, WorkspaceView } from "../workspaceProjects";
import type { AgentRowsProps } from "./AgentRows";
import type { RailAgent, RailAgents } from "./railAgents";
import { WorkspaceTree, type WorkspaceTreeProps } from "./WorkspaceTree";

vi.mock("../popoverPlace", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../popoverPlace")>();
  return {
    ...actual,
    AnchoredPopover: (props: {
      containerRef?: RefObject<HTMLDivElement | null>;
      className?: string;
      children?: ReactNode;
    }) => (
      <div
        ref={(node: HTMLDivElement | null) => {
          if (props.containerRef) props.containerRef.current = node;
        }}
        className={props.className}
      >
        {props.children}
      </div>
    ),
  };
});

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
  workspaces: [workspace("w-1", "Improve handoff")],
};

const KEY_1 = localWorkspaceKey("w-1") as WorkspaceKey;

const child: RailAgent = {
  id: "web",
  kind: "claude",
  provider: "Claude",
  title: "Website redesign",
  state: "finished",
  child: true,
  parentId: "parent",
  parentTitle: "cia",
  hasChildren: false,
  generation: 1,
  archivable: true,
  turnStartedAtMs: null,
  turnEndedAtMs: null,
};

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

function treeProps(
  agents: RailAgents,
  onArchiveAgent: AgentRowsProps["onArchiveAgent"],
): WorkspaceTreeProps {
  return {
    projects: [PROJECT],
    loading: false,
    error: null,
    providerError: null,
    selectedWorkspace: KEY_1,
    onRetryProjects: vi.fn(),
    onRetryProviders: vi.fn(),
    onSelectWorkspace: vi.fn(),
    onNewWorkspace: vi.fn(),
    onRenameWorkspace: vi.fn(async () => null),
    onArchiveWorkspace: vi.fn(async () => null),
    onRemoveProject: vi.fn(async () => null),
    providerMenuAnchorProjectId: null,
    providerMenu: null,
    stats: new Map(),
    branches: new Map(),
    hostNames: new Map(),
    selectedAgents: {
      agents,
      activeId: null,
      onOpenAgent: vi.fn(),
      onOpenHistory: vi.fn(),
      onArchiveAgent,
    },
  };
}

async function renderTree(agents: RailAgents, onArchiveAgent: AgentRowsProps["onArchiveAgent"]) {
  await act(async () =>
    root.render(
      <ConfirmProvider>
        <WorkspaceTree {...treeProps(agents, onArchiveAgent)} />
      </ConfirmProvider>,
    ),
  );
}

describe("focus after the last agent under the selected row is archived", () => {
  it("moves focus to the workspace row once the agent list is gone", async () => {
    const finish: { resolve: (sentence: string | null) => void } = { resolve: () => undefined };
    const onArchiveAgent = vi.fn(
      () =>
        new Promise<string | null>((resolve) => {
          finish.resolve = resolve;
        }),
    );
    await renderTree({ rows: [child], hidden: 0 }, onArchiveAgent);
    const row = container.querySelector<HTMLElement>('[role="treeitem"]');
    if (row === null) throw new Error("the agent did not render");
    row.focus();
    await act(async () => {
      row.dispatchEvent(
        new KeyboardEvent("keydown", { key: "F10", shiftKey: true, bubbles: true }),
      );
    });
    await act(async () => container.querySelector<HTMLButtonElement>('[role="menuitem"]')?.click());
    await act(async () =>
      document.querySelector<HTMLButtonElement>(".confirm-dialog-confirm")?.click(),
    );

    await renderTree({ rows: [], hidden: 0 }, vi.fn());
    await act(async () => finish.resolve(null));

    expect(document.activeElement).toBe(container.querySelector("button.workspace-row"));
  });
});
