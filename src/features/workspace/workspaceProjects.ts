import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { ChangeEvent } from "react";
import {
  projectsList,
  workspaceCreate,
  workspaceDelete,
  workspaceSetTitle,
  workspacesList,
} from "../../lib/tauri";
import { errorSentence, type ErrorSentence } from "../../lib/errorSentence";
import { workspaceDisplayTitles } from "../../lib/workspaceTitles";
import type { Project, Session, Workspace } from "../../types/ipc";
import { sessionNeedsApproval } from "./sessionAttention";
import {
  LOCAL_HOST_ID,
  localWorkspaceKey,
  workspaceKey,
  type HostId,
  type WorkspaceKey,
} from "./hosts/hostIdentity";

/** A workspace as loaded, with the host whose feed it arrived on — today
 * the only feed, so every row carries the local host. */
export interface HostWorkspace extends Workspace {
  hostId: HostId;
}

export interface WorkspaceProject extends Project {
  hostId: HostId;
  workspaces: WorkspaceView[];
  workspaceError?: ErrorSentence;
}

/** What a workspace's own agents are doing, counted from the roster. */
export interface WorkspaceAgents {
  /** Sessions with a live turn. */
  working: number;
  /** Sessions stopped on the user's approval. */
  waiting: number;
}

export interface WorkspaceView extends HostWorkspace {
  /** What the sidebar prints; `title` stays the stored row. */
  displayTitle: string;
  /** What the row's second line counts. */
  agents: WorkspaceAgents;
  /**
   * Milliseconds since the last output this workspace's own sessions
   * reported — the shortest silence among them, so the row can print when it
   * last spoke. Null when no roster row reports one, which is unknown, never
   * "long ago".
   */
  elapsedMs: number | null;
  /** The row's state dot, in the tab chips' vocabulary. */
  stateDot: "pulse" | "attention" | "unattended" | "idle" | null;
}

/** The UI's name for a workspace: it is never the value the daemon is sent. */
export function keyOfWorkspace(workspace: HostWorkspace): WorkspaceKey | null {
  return workspaceKey(workspace.hostId, workspace.id);
}

function sessionsOf(
  index: Map<WorkspaceKey, Session[]>,
  workspace: HostWorkspace,
): readonly Session[] {
  const key = keyOfWorkspace(workspace);
  return key === null ? [] : (index.get(key) ?? []);
}

function onLocalHost(workspaces: readonly Workspace[]): HostWorkspace[] {
  return workspaces.map((workspace) => ({ ...workspace, hostId: LOCAL_HOST_ID }));
}

interface ProjectRecord extends Project {
  hostId: HostId;
  workspaces: HostWorkspace[];
  workspaceError?: ErrorSentence;
}

export function reconcileProjectRecords(
  loaded: ProjectRecord[],
  current: ProjectRecord[],
): ProjectRecord[] {
  const currentById = new Map(current.map((project) => [project.id, project]));
  const loadedIds = new Set(loaded.map((project) => project.id));
  const reconciled = loaded.map((project) => {
    const currentProject = currentById.get(project.id);
    // A failed per-project read (workspaceError set) must not be treated as
    // "no workspaces": keep what was already held and let the error line ask
    // for a Retry. A successful reply is authoritative per project: a
    // workspace the daemon no longer lists is dropped (removed elsewhere),
    // and a workspace created through this UI is in the reply, because the
    // daemon minted it before the create returned.
    if (
      currentProject !== undefined &&
      project.workspaceError !== undefined &&
      project.workspaces.length === 0
    ) {
      return { ...project, workspaces: currentProject.workspaces };
    }
    return currentProject === undefined ? project : { ...project, workspaces: project.workspaces };
  });
  return [...reconciled, ...current.filter((project) => !loadedIds.has(project.id))];
}

export function workspaceView(
  workspace: HostWorkspace,
  sessions: readonly Session[] = [],
): WorkspaceView {
  return workspaceViewFromIndex(
    workspace,
    sessionsOf(buildSessionIndex(sessions), workspace),
    workspace.title,
  );
}

export function buildSessionIndex(sessions: readonly Session[]): Map<WorkspaceKey, Session[]> {
  // One pass over the roster, not one filter per workspace: pushes arrive
  // often and the sidebar derives every row from the same array.
  const index = new Map<WorkspaceKey, Session[]>();
  for (const session of sessions) {
    if (session.workspaceId === null) continue;
    const key = localWorkspaceKey(session.workspaceId);
    if (key === null) continue;
    const list = index.get(key);
    if (list === undefined) index.set(key, [session]);
    else list.push(session);
  }
  return index;
}

/** The shortest silence among the roster rows: the instant the workspace
 * last spoke, as an elapsed time so the row can read a clock of its own. */
function shortestElapsed(sessions: readonly Session[]): number | null {
  let shortest: number | null = null;
  for (const session of sessions) {
    const elapsed = session.elapsedMs;
    if (typeof elapsed !== "number" || !Number.isFinite(elapsed)) continue;
    if (shortest === null || elapsed < shortest) shortest = elapsed;
  }
  return shortest;
}

function workspaceViewFromIndex(
  workspace: HostWorkspace,
  sessionsOfWorkspace: readonly Session[],
  displayTitle: string,
): WorkspaceView {
  const live = sessionsOfWorkspace.filter((session) => session.state.type === "live");
  const waiting = sessionsOfWorkspace.filter(sessionNeedsApproval);
  const unattended = sessionsOfWorkspace.some(
    (session) => session.state.type !== "ended" && session.unattended === "yes",
  );
  // A silent session still holds its process: quiet, never idle — so only a
  // workspace with no live, silent, waiting or unattended session reads idle.
  const settled =
    sessionsOfWorkspace.length > 0 &&
    !sessionsOfWorkspace.some(
      (session) => session.state.type === "live" || session.state.type === "silent",
    );
  return {
    ...workspace,
    displayTitle,
    agents: { working: live.length, waiting: waiting.length },
    elapsedMs: shortestElapsed(sessionsOfWorkspace),
    stateDot:
      waiting.length > 0
        ? "attention"
        : unattended
          ? "unattended"
          : live.length > 0
            ? "pulse"
            : settled
              ? "idle"
              : null,
  };
}

/** One project's row views: the equal titles numbered apart first, so every
 * row is named before search narrows the list. */
export function projectView(
  project: ProjectRecord,
  byWorkspace: Map<WorkspaceKey, Session[]>,
): WorkspaceProject {
  const titles = workspaceDisplayTitles(project.workspaces);
  return {
    ...project,
    workspaces: project.workspaces.map((workspace) =>
      workspaceViewFromIndex(
        workspace,
        sessionsOf(byWorkspace, workspace),
        titles.get(workspace.id) ?? workspace.title,
      ),
    ),
  };
}

export function useWorkspaceProjects(restoredWorkspaceKey: WorkspaceKey | null) {
  const [projectRecords, setProjectRecords] = useState<ProjectRecord[]>([]);
  const [sessionFacts, setSessionFactsState] = useState<Session[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<ErrorSentence | null>(null);
  // Starts on the workspace the surface was last showing, so returning to it
  // lands where the user left; a null one (a fresh start, no cell) takes the
  // first listed row when the list settles, as this hook always has.
  const [selectedKey, setSelectedKey] = useState<WorkspaceKey | null>(restoredWorkspaceKey);
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
            return {
              ...project,
              hostId: LOCAL_HOST_ID,
              workspaces: onLocalHost(await workspacesList(project.id)),
            };
          } catch (cause: unknown) {
            return {
              ...project,
              hostId: LOCAL_HOST_ID,
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
  // The session index is built once per roster, not once per project.
  const sessionIndex = useMemo(() => buildSessionIndex(sessionFacts), [sessionFacts]);
  const projectViews = useMemo(
    () => projectRecords.map((project) => projectView(project, sessionIndex)),
    [projectRecords, sessionIndex],
  );

  useEffect(() => {
    if (loading || error !== null) return;
    const keys = projectRecords
      .flatMap((project) => project.workspaces.map(keyOfWorkspace))
      .filter((key) => key !== null);
    setSelectedKey((current) =>
      current !== null && keys.includes(current) ? current : (keys[0] ?? null),
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
            ? {
                ...project,
                workspaces: [...project.workspaces, { ...workspace, hostId: LOCAL_HOST_ID }],
              }
            : project,
        ),
      );
      setSelectedKey(keyOfWorkspace({ ...workspace, hostId: LOCAL_HOST_ID }));
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
        setSelectedKey(keyOfWorkspace(existing));
        return existing;
      }
      return addWorkspace(projectId);
    },
    [addWorkspace, projectRecords],
  );

  /**
   * Answers with the stored row, so the sidebar needs no reload; a refusal
   * comes back as the sentence the row renders.
   */
  const renameWorkspace = useCallback(
    async (workspaceId: string, title: string): Promise<ErrorSentence | null> => {
      try {
        const renamed = await workspaceSetTitle(workspaceId, title);
        setProjectRecords((currentProjects) =>
          currentProjects.map((project) =>
            project.id !== renamed.projectId
              ? project
              : {
                  ...project,
                  workspaces: project.workspaces.map((workspace) =>
                    workspace.id === renamed.id
                      ? { ...workspace, title: renamed.title }
                      : workspace,
                  ),
                },
          ),
        );
        return null;
      } catch (cause: unknown) {
        return errorSentence(cause);
      }
    },
    [],
  );

  /**
   * After a success the list is re-read, never patched: the row leaves when
   * the daemon's reply no longer carries it, and a refusal comes back as the
   * sentence the row renders — with the list untouched, so the row stays.
   */
  const deleteWorkspace = useCallback(
    async (workspaceId: string): Promise<ErrorSentence | null> => {
      try {
        await workspaceDelete(workspaceId);
      } catch (cause: unknown) {
        return errorSentence(cause);
      }
      await loadProjects();
      return null;
    },
    [loadProjects],
  );

  const openProjectDialog = useCallback(() => setProjectDialogOpen(true), []);
  const closeProjectDialog = useCallback(() => {
    setProjectDialogOpen(false);
    newProjectTriggerRef.current?.focus();
  }, []);
  const handleCreateProject = useCallback(async (project: Project): Promise<void> => {
    try {
      const workspaces = onLocalHost(await workspacesList(project.id));
      setProjectRecords((currentProjects) => {
        const next = { ...project, hostId: LOCAL_HOST_ID, workspaces };
        const existingIndex = currentProjects.findIndex((current) => current.id === project.id);
        if (existingIndex < 0) return [...currentProjects, next];
        return currentProjects.map((current, index) => (index === existingIndex ? next : current));
      });
      // A project the daemon added has no workspace until one is created, so
      // the list may be empty: the selection stays where it was.
      const firstKey = workspaces.at(0);
      setSelectedKey(
        (current) => current ?? (firstKey === undefined ? null : keyOfWorkspace(firstKey)),
      );
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
              // Both spellings: a row is found by what it prints (its
              // numbered title) and by what the journal stores under it.
              `${project.name} ${workspace.title} ${workspace.displayTitle}`
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
    // The unfiltered list: search must not veto selection-to-workspace
    // navigation, which reads this list's workspace ids.
    projects: projectViews,
    visibleProjects,
    loading,
    error,
    selectedKey,
    setSelectedKey,
    setSessionFacts,
    search,
    handleSearchChange,
    addWorkspace,
    reuseOrCreateWorkspace,
    renameWorkspace,
    deleteWorkspace,
    projectDialogOpen,
    openProjectDialog,
    closeProjectDialog,
    handleCreateProject,
    newProjectTriggerRef,
    retryProjects: loadProjects,
  };
}
