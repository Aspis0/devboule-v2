// @vitest-environment happy-dom

// The remove-project road of the hook: once the daemon has taken a project off
// its list, the sidebar drops the row at once and no later read brings it back.

import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("../../lib/tauri", () => ({
  projectsList: vi.fn(async () => []),
  projectRemove: vi.fn(async () => undefined),
  workspaceArchive: vi.fn(),
  workspaceCreate: vi.fn(),
  workspaceDelete: vi.fn(),
  workspaceSetTitle: vi.fn(),
  workspacesList: vi.fn(async () => []),
}));

import { projectRemove, projectsList, workspacesList } from "../../lib/tauri";
import type { Project } from "../../types/ipc";
import { useWorkspaceProjects, type WorkspaceProject } from "./workspaceProjects";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const design: Project = { id: "project-design", name: "design-sandbox", path: "C:\\design" };
const devboule: Project = { id: "project-devboule", name: "devboule", path: "C:\\devboule" };

type Latest = {
  projects: readonly WorkspaceProject[];
  removeProject: (projectId: string) => Promise<{ sentence: string } | null>;
  retryProjects: () => Promise<void>;
};

function Probe(props: { onState: (state: Latest) => void }) {
  const state = useWorkspaceProjects(null);
  props.onState({
    projects: state.projects,
    removeProject: state.removeProject,
    retryProjects: state.retryProjects,
  });
  return null;
}

describe("useWorkspaceProjects, removing a project", () => {
  let holder: HTMLDivElement | null = null;
  let root: Root | null = null;
  let latest: Latest;

  // The hook does not load on mount: the first read is the connected transition's.
  const renderProbe = async () => {
    await act(async () => {
      root!.render(
        <Probe
          onState={(state) => {
            latest = state;
          }}
        />,
      );
    });
    await act(async () => {
      await latest.retryProjects();
    });
  };

  beforeEach(() => {
    holder = document.createElement("div");
    document.body.appendChild(holder);
    root = createRoot(holder);
    vi.mocked(workspacesList).mockResolvedValue([]);
    vi.mocked(projectRemove).mockResolvedValue(undefined);
  });

  afterEach(async () => {
    await act(async () => root?.unmount());
    holder?.remove();
    holder = null;
    root = null;
    vi.mocked(projectsList).mockReset();
  });

  it("drops the row at once, and a later read does not bring it back", async () => {
    vi.mocked(projectsList).mockResolvedValue([design, devboule]);
    await renderProbe();
    expect(latest.projects.map((row) => row.id)).toEqual([design.id, devboule.id]);

    // The daemon stops listing the project as soon as the removal lands.
    vi.mocked(projectsList).mockResolvedValue([devboule]);
    let refusal: { sentence: string } | null = null;
    await act(async () => {
      refusal = await latest.removeProject(design.id);
    });

    expect(refusal).toBeNull();
    expect(vi.mocked(projectRemove)).toHaveBeenCalledWith(design.id);
    expect(latest.projects.map((row) => row.id)).toEqual([devboule.id]);

    await act(async () => {
      await latest.retryProjects();
    });
    expect(latest.projects.map((row) => row.id)).toEqual([devboule.id]);
  });

  it("keeps the row when the daemon refuses the removal", async () => {
    vi.mocked(projectsList).mockResolvedValue([design]);
    await renderProbe();
    vi.mocked(projectRemove).mockRejectedValueOnce(new Error("a session still runs here"));

    let refusal: { sentence: string } | null = null;
    await act(async () => {
      refusal = await latest.removeProject(design.id);
    });

    expect(refusal).not.toBeNull();
    expect(latest.projects.map((row) => row.id)).toEqual([design.id]);
  });
});
