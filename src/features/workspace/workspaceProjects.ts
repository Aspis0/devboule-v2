import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { ChangeEvent } from "react";
import { projectsList, workspaceCreate, workspacesList } from "../../lib/tauri";
import { errorSentence, type ErrorSentence } from "../../lib/errorSentence";
import type { Project, Session, Workspace } from "../../types/ipc";

export interface WorkspaceProject extends Project {
  workspaces: WorkspaceView[];
  workspaceError?: ErrorSentence;
}

export interface WorkspaceView extends Workspace {
  /** A word only when the row differs from the norm; null renders no line. */
  meta: string | null;
  /** The row's trailing state dot, in the tab chips' vocabulary. */
  stateDot: "pulse" | "attention" | "unattended" | null;
}

interface ProjectRecord extends Project {
  workspaces: Workspace[];
  workspaceError?: ErrorSentence;
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
  const sessionsOfWorkspace = sessions.filter((session) => session.workspaceId === workspace.id);
  const recovered = sessionsOfWorkspace.filter((session) => session.state.type === "recovered");
  // The meta line appears only when the row differs from the norm (spec): a
  // recovered transcript is the anomaly worth a word. Live counts and the
  // isolation word are the norm and stay off the row.
  const meta = recovered.length > 0 ? `${recovered.length} recovered` : null;
  // The trailing dot speaks the tab chips' vocabulary; the avatar never does.
  const attention = sessionsOfWorkspace.some((session) => session.attention !== undefined);
  const unattended = sessionsOfWorkspace.some((session) => session.unattended === "yes");
  const running = sessionsOfWorkspace.some((session) => session.state.type === "live");
  return {
    ...workspace,
    meta,
    stateDot: attention ? "attention" : unattended ? "unattended" : running ? "pulse" : null,
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
  const [error, setError] = useState<ErrorSentence | null>(null);
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
              workspaceError: errorSentence(cause),
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
      setError(errorSentence(cause));
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
      setError(errorSentence(cause));
      return null;
    }
  }, []);

  /**
   * The project-level "+" policy: reuse the project's existing local
   * workspace — selecting it — and mint one only when the project has none.
   * Spawning an agent is not a reason to create a look-alike row; distinct
   * workspaces arrive with the worktree slice.
   */
  const reuseOrCreateWorkspace = useCallback(
    async (projectId: string): Promise<Workspace | null> => {
      const existing = projectRecords
        .find((project) => project.id === projectId)
        ?.workspaces.find((workspace) => workspace.isolation === "local");
      if (existing !== undefined) {
        setSelectedWorkspace(existing.id);
        return existing;
      }
      return addWorkspace(projectId);
    },
    [addWorkspace, projectRecords],
  );

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
      setError(errorSentence(cause));
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
              `${project.name} ${workspace.title} ${workspace.meta ?? ""}`
                .toLowerCase()
                .includes(query),
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
    reuseOrCreateWorkspace,
    projectDialogOpen,
    openProjectDialog,
    closeProjectDialog,
    handleCreateProject,
    newProjectTriggerRef,
    retryProjects: loadProjects,
  };
}
