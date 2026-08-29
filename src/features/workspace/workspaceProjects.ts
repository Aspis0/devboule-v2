import { useCallback, useMemo, useRef, useState } from "react";
import type { ChangeEvent } from "react";
import { MOCK_PROJECTS, type MockProject, type MockWorkspace } from "./mockData";
import type { ProjectCreationRoute } from "./NewProjectDialog";

function projectNameFromDraft(route: ProjectCreationRoute, value: string): string {
  if (route === "clone") {
    const repositoryPath = value.split(/[?#]/, 1)[0].replace(/\/+$/, "");
    const repositoryName = repositoryPath
      .split("/")
      .pop()
      ?.replace(/\.git$/i, "");
    return repositoryName || "cloned-project";
  }

  const pathWithoutTrailingSeparators = value.replace(/[\\/]+$/, "");
  return pathWithoutTrailingSeparators.split(/[\\/]/).pop() || "new-project";
}

function cloneProjects(): MockProject[] {
  return MOCK_PROJECTS.map((project) => ({
    ...project,
    workspaces: project.workspaces.map((workspace) => ({ ...workspace })),
  }));
}

export function useWorkspaceProjects() {
  const [projects, setProjects] = useState<MockProject[]>(cloneProjects);
  const [selectedWorkspace, setSelectedWorkspace] = useState("rust-core");
  const [search, setSearch] = useState("");
  const [projectDialogOpen, setProjectDialogOpen] = useState(false);
  const newProjectTriggerRef = useRef<HTMLButtonElement>(null);

  const addWorkspace = useCallback((projectId: string = "devboule") => {
    const id = `mock-workspace-${Date.now()}`;
    const workspace: MockWorkspace = {
      id,
      projectId,
      title: "new-workspace",
      meta: "idle · 0 d",
      isolation: "worktree",
      dotTone: "border",
    };

    setProjects((currentProjects) =>
      currentProjects.map((project) =>
        project.id === projectId
          ? { ...project, workspaces: [...project.workspaces, workspace] }
          : project,
      ),
    );
    setSelectedWorkspace(id);
  }, []);

  const openProjectDialog = useCallback(() => setProjectDialogOpen(true), []);
  const closeProjectDialog = useCallback(() => {
    setProjectDialogOpen(false);
    newProjectTriggerRef.current?.focus();
  }, []);
  const handleCreateProject = useCallback(
    ({ route, value }: { route: ProjectCreationRoute; value: string }) => {
      const trimmedValue = value.trim();
      const project: MockProject = {
        id: `mock-project-${Date.now()}`,
        name: projectNameFromDraft(route, trimmedValue),
        path: trimmedValue,
        workspaces: [],
      };

      setProjects((currentProjects) => [...currentProjects, project]);
      setSearch("");
      closeProjectDialog();
    },
    [closeProjectDialog],
  );

  function handleSearchChange(event: ChangeEvent<HTMLInputElement>) {
    setSearch(event.target.value);
  }

  const query = useMemo(() => search.trim().toLowerCase(), [search]);
  const visibleProjects = useMemo(
    () =>
      projects
        .map((project) => ({
          ...project,
          workspaces: project.workspaces.filter(
            (workspace) =>
              !query ||
              `${project.name} ${workspace.title} ${workspace.meta}`.toLowerCase().includes(query),
          ),
        }))
        .filter(
          (project) =>
            !query || project.workspaces.length > 0 || project.name.toLowerCase().includes(query),
        ),
    [projects, query],
  );

  return {
    visibleProjects,
    selectedWorkspace,
    setSelectedWorkspace,
    search,
    handleSearchChange,
    addWorkspace,
    projectDialogOpen,
    openProjectDialog,
    closeProjectDialog,
    handleCreateProject,
    newProjectTriggerRef,
  };
}
