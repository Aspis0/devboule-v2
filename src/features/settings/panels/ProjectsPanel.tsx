import { useCallback, useEffect, useRef, useState } from "react";
import { projectsList, workspacesList } from "../../../lib/tauri";
import { errorSentence, type ErrorSentence } from "../../../lib/errorSentence";
import { ErrorText } from "../../../components/ErrorText";
import { NewProjectDialog } from "../../../components/NewProjectDialog";
import type { Project, Workspace } from "../../../types/ipc";
import "../projects.css";
export function ProjectsPanel() {
  const [projects, setProjects] = useState<Project[]>([]);
  const [workspacesByProject, setWorkspacesByProject] = useState<Record<string, Workspace[]>>({});
  const [workspaceErrors, setWorkspaceErrors] = useState<Record<string, ErrorSentence>>({});
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<ErrorSentence | null>(null);
  const [dialogOpen, setDialogOpen] = useState(false);
  const addProjectRef = useRef<HTMLButtonElement>(null);

  const loadProjects = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const listed = await projectsList();
      const nextWorkspaces: Record<string, Workspace[]> = {};
      const nextErrors: Record<string, ErrorSentence> = {};
      await Promise.all(
        listed.map(async (project) => {
          try {
            nextWorkspaces[project.id] = await workspacesList(project.id);
          } catch (cause: unknown) {
            nextErrors[project.id] = errorSentence(cause);
          }
        }),
      );
      setProjects(listed);
      setWorkspacesByProject(nextWorkspaces);
      setWorkspaceErrors(nextErrors);
    } catch (cause: unknown) {
      setProjects([]);
      setWorkspacesByProject({});
      setWorkspaceErrors({});
      setError(errorSentence(cause));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void loadProjects();
  }, [loadProjects]);

  const closeDialog = useCallback(() => {
    setDialogOpen(false);
    addProjectRef.current?.focus();
  }, []);

  const handleProjectAdded = useCallback(async (project: Project) => {
    const workspaces = await workspacesList(project.id);
    setProjects((current) => {
      const index = current.findIndex((entry) => entry.id === project.id);
      if (index < 0) return [...current, project];
      return current.map((entry, entryIndex) => (entryIndex === index ? project : entry));
    });
    setWorkspacesByProject((current) => ({ ...current, [project.id]: workspaces }));
    setError(null);
  }, []);

  return (
    <div id="settings-panel-projects">
      <div className="proj-stack">
        {loading ? <div role="status">Loading projects…</div> : null}
        {error !== null ? (
          <div role="alert">
            <ErrorText
              sentence={error.sentence}
              detail={error.detail}
              id="settings-projects-error"
            />
            <button type="button" onClick={() => void loadProjects()}>
              Retry
            </button>
          </div>
        ) : null}
        {error === null && projects.length > 0 ? (
          <div className="proj-card">
            {projects.map((project) => {
              const workspaces = workspacesByProject[project.id];
              const workspaceCount = workspaces?.length;
              const workspaceError = workspaceErrors[project.id];
              return (
                <div className="proj-row settings-project-card" key={project.id}>
                  <span className="settings-card-copy">
                    <span className="settings-card-title proj-name">{project.name}</span>
                    <span className="settings-card-meta">{project.path}</span>
                    {(workspaces ?? []).map((workspace) =>
                      // Render exactly what the daemon sent: no project-path
                      // fallback, no joined path. Same contract as Session.cwd.
                      // A local workspace's path IS its project's path by
                      // construction, so repeating it prints the same line
                      // three times — skip only that duplicate.
                      workspace.path && workspace.path !== project.path ? (
                        <span className="settings-card-meta" key={workspace.id}>
                          {workspace.path}
                        </span>
                      ) : null,
                    )}
                  </span>
                  {workspaceError !== undefined ? (
                    <span role="alert">
                      <ErrorText
                        sentence={`Workspaces unavailable: ${workspaceError.sentence}`}
                        detail={workspaceError.detail}
                        id={`settings-workspaces-error-${project.id}`}
                      />
                      <button type="button" onClick={() => void loadProjects()}>
                        Retry
                      </button>
                    </span>
                  ) : (
                    <span className="settings-card-value">
                      {workspaceCount ?? 0} workspace{workspaceCount === 1 ? "" : "s"}
                    </span>
                  )}
                </div>
              );
            })}
          </div>
        ) : null}
        {!loading && error === null && projects.length === 0 ? (
          <div role="status">No projects registered</div>
        ) : null}
        <button
          className="settings-dashed-action"
          type="button"
          ref={addProjectRef}
          onClick={() => setDialogOpen(true)}
        >
          <span aria-hidden="true">+</span>Add project
        </button>
      </div>

      <NewProjectDialog open={dialogOpen} onClose={closeDialog} onCreate={handleProjectAdded} />
    </div>
  );
}
