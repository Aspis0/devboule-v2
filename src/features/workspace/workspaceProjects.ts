import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { ChangeEvent } from "react";
import { projectsList, reasonFromCause, workspaceCreate, workspacesList } from "../../lib/tauri";
import type { Project, Session, Workspace } from "../../types/ipc";

export interface WorkspaceProject extends Project {
  workspaces: WorkspaceView[];
  workspaceError?: string;
}

export interface WorkspaceView extends Workspace {
  meta: string;
  dotTone: "green" | "border";
}

interface ProjectRecord extends Project {
  workspaces: Workspace[];
  workspaceError?: string;
}

function reconcileProjectRecords(
  loaded: ProjectRecord[],
  current: ProjectRecord[],
): ProjectRecord[] {
  const currentById = new Map(current.map((project) => [project.id, project]));
  const loadedIds = new Set(loaded.map((project) => project.id));
  const reconciled = loaded.map((project) => {
    const currentProject = currentById.get(project.id);
    if (currentProject === undefined) return project;
    const loadedWorkspaceIds = new Set(project.workspaces.map((workspace) => workspace.id));
    return {
      ...project,
      workspaces: [
        ...project.workspaces,
        ...currentProject.workspaces.filter((workspace) => !loadedWorkspaceIds.has(workspace.id)),
      ],
    };
  });
  return [...reconciled, ...current.filter((project) => !loadedIds.has(project.id))];
}

export function workspaceView(
  workspace: Workspace,
  sessions: readonly Session[] = [],
): WorkspaceView {
  const liveSessions = sessions.filter(
    (session) => session.workspaceId === workspace.id && session.state.type === "live",
  ).length;
  const sessionLabel = `${liveSessions} live session${liveSessions === 1 ? "" : "s"}`;
  return {
    ...workspace,
    meta: `${sessionLabel} · ${workspace.isolation}`,
    dotTone: liveSessions > 0 ? "green" : "border",
  };
}

function projectView(project: ProjectRecord, sessions: readonly Session[]): WorkspaceProject {
  return {
    ...project,
    workspaces: project.workspaces.map((workspace) => workspaceView(workspace, sessions)),
  };
}

export function useWorkspaceProjects() {
  const [projectRecords, setProjectRecords] = useState<ProjectRecord[]>([]);
  const [sessionFacts, setSessionFactsState] = useState<Session[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [selectedWorkspace, setSelectedWorkspace] = useState<string | null>(null);
  const [search, setSearch] = useState("");
  const [projectDialogOpen, setProjectDialogOpen] = useState(false);
  const newProjectTriggerRef = useRef<HTMLButtonElement>(null);
  const loadGenerationRef = useRef(0);

  const loadProjects = useCallback(async () => {
    const generation = ++loadGenerationRef.current;
    setLoading(true);
    try {
      const listedProjects = await projectsList();
      const records = await Promise.all(
        listedProjects.map(async (project): Promise<ProjectRecord> => {
          try {
            return { ...project, workspaces: await workspacesList(project.id) };
          } catch (cause: unknown) {
            return {
              ...project,
              workspaces: [],
              workspaceError: reasonFromCause(cause),
            };
          }
        }),
      );
      if (generation !== loadGenerationRef.current) return;
      setProjectRecords((current) => reconcileProjectRecords(records, current));
      setLoading(false);
      setError(null);
    } catch (cause: unknown) {
      if (generation !== loadGenerationRef.current) return;
      setLoading(false);
      setError(reasonFromCause(cause));
    }
  }, []);

  // There is deliberately no mount load: until the daemon first answers
  // "connected" the IPC pipe is not open, so a startup load only races it and
  // latches a bogus error. Workspace loads projects exactly once per
  // connected transition (first connect and every reconnect) via
  // retryProjects; manual retries go through the same path.
  const projectViews = useMemo(
    () => projectRecords.map((project) => projectView(project, sessionFacts)),
    [projectRecords, sessionFacts],
  );

  useEffect(() => {
    if (loading || error !== null) return;
    const workspaceIds = projectRecords.flatMap((project) =>
      project.workspaces.map((workspace) => workspace.id),
    );
    setSelectedWorkspace((current) =>
      current !== null && workspaceIds.includes(current) ? current : (workspaceIds[0] ?? null),
    );
  }, [error, loading, projectRecords]);

  const setSessionFacts = useCallback((sessions: readonly Session[]) => {
    setSessionFactsState([...sessions]);
  }, []);

  const addWorkspace = useCallback(async (projectId: string): Promise<Workspace | null> => {
    try {
      const workspace = await workspaceCreate(projectId, "local");
      setProjectRecords((currentProjects) =>
        currentProjects.map((project) =>
          project.id === projectId
            ? { ...project, workspaces: [...project.workspaces, workspace] }
            : project,
        ),
      );
      setSelectedWorkspace(workspace.id);
      setError(null);
      return workspace;
    } catch (cause: unknown) {
      setError(reasonFromCause(cause));
      return null;
    }
  }, []);

  const openProjectDialog = useCallback(() => setProjectDialogOpen(true), []);
  const closeProjectDialog = useCallback(() => {
    setProjectDialogOpen(false);
    newProjectTriggerRef.current?.focus();
  }, []);
  const handleCreateProject = useCallback(async (project: Project): Promise<void> => {
    try {
      const workspaces = await workspacesList(project.id);
      setProjectRecords((currentProjects) => {
        const next = { ...project, workspaces };
        const existingIndex = currentProjects.findIndex((current) => current.id === project.id);
        if (existingIndex < 0) return [...currentProjects, next];
        return currentProjects.map((current, index) => (index === existingIndex ? next : current));
      });
      setSelectedWorkspace((current) => current ?? workspaces[0]?.id ?? null);
      setSearch("");
      setError(null);
    } catch (cause: unknown) {
      const message = reasonFromCause(cause);
      setError(message);
      throw cause;
    }
  }, []);

  function handleSearchChange(event: ChangeEvent<HTMLInputElement>) {
    setSearch(event.target.value);
  }

  const query = useMemo(() => search.trim().toLowerCase(), [search]);
  const visibleProjects = useMemo(
    () =>
      projectViews
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
            !query ||
            project.workspaceError !== undefined ||
            project.workspaces.length > 0 ||
            project.name.toLowerCase().includes(query),
        ),
    [projectViews, query],
  );

  return {
    visibleProjects,
    loading,
    error,
    selectedWorkspace,
    setSelectedWorkspace,
    setSessionFacts,
    search,
    handleSearchChange,
    addWorkspace,
    projectDialogOpen,
    openProjectDialog,
    closeProjectDialog,
    handleCreateProject,
    newProjectTriggerRef,
    retryProjects: loadProjects,
  };
}
