// @vitest-environment happy-dom

// The create-project road of the hook, with the daemon answering the one thing
// it always answers for a project it just added: no workspaces.

import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("../../lib/tauri", () => ({
  projectsList: vi.fn(async () => []),
  workspaceCreate: vi.fn(),
  workspaceDelete: vi.fn(),
  workspaceSetTitle: vi.fn(),
  workspacesList: vi.fn(async () => []),
}));

import { workspacesList } from "../../lib/tauri";
import type { Project } from "../../types/ipc";
import { useWorkspaceProjects, type WorkspaceProject } from "./workspaceProjects";
import type { WorkspaceKey } from "./hosts/hostIdentity";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const project: Project = { id: "project-1", name: "devboule", path: "C:\\devboule" };

function Probe(props: {
  onState: (state: {
    selectedKey: WorkspaceKey | null;
    projects: readonly WorkspaceProject[];
    error: { sentence: string } | null;
    handleCreateProject: (project: Project) => Promise<void>;
  }) => void;
}) {
  const state = useWorkspaceProjects(null);
  props.onState({
    selectedKey: state.selectedKey,
    projects: state.projects,
    error: state.error,
    handleCreateProject: state.handleCreateProject,
  });
  return null;
}

describe("useWorkspaceProjects, creating a project", () => {
  let holder: HTMLDivElement | null = null;
  let root: Root | null = null;
  let latest: {
    selectedKey: WorkspaceKey | null;
    projects: readonly WorkspaceProject[];
    error: { sentence: string } | null;
    handleCreateProject: (project: Project) => Promise<void>;
  };

  beforeEach(() => {
    holder = document.createElement("div");
    document.body.appendChild(holder);
    root = createRoot(holder);
  });

  afterEach(async () => {
    await act(async () => root?.unmount());
    holder?.remove();
    holder = null;
    root = null;
  });

  it("keeps the selection as it was when the new project lists no workspace", async () => {
    vi.mocked(workspacesList).mockResolvedValue([]);
    await act(async () => {
      root!.render(
        <Probe
          onState={(state) => {
            latest = state;
          }}
        />,
      );
    });
    expect(latest.selectedKey).toBeNull();

    await act(async () => {
      await latest.handleCreateProject(project);
    });

    expect(vi.mocked(workspacesList)).toHaveBeenCalledWith(project.id);
    expect(latest.error).toBeNull();
    expect(latest.projects.map((row) => row.id)).toEqual([project.id]);
    expect(latest.selectedKey).toBeNull();
  });
});
