import { Fragment, useCallback, useEffect, useRef, useState } from "react";
import { projectsList, workspacesList } from "../../../lib/tauri";
import { errorSentence, type ErrorSentence } from "../../../lib/errorSentence";
import { workspaceDisplayTitles } from "../../../lib/workspaceTitles";
import { ErrorText } from "../../../components/ErrorText";
import { NewProjectDialog } from "../../../components/NewProjectDialog";
import type { Project, Workspace } from "../../../types/ipc";
import { SettingsAdvanced, SettingsRow, SettingsSection } from "../rows";
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
      {loading ? <div role="status">Loading projects…</div> : null}
      {error !== null ? (
        <div role="alert">
          <ErrorText sentence={error.sentence} detail={error.detail} id="settings-projects-error" />
          <button type="button" onClick={() => void loadProjects()}>
            Retry
          </button>
        </div>
      ) : null}
      <SettingsSection
        label="Projects"
        action={
          <button
            type="button"
            className="settings-add"
            aria-label="Add project"
            title="Add project"
            ref={addProjectRef}
            onClick={() => setDialogOpen(true)}
          >
            +
          </button>
        }
      >
        {error === null
          ? projects.map((project) => {
              const workspaces = workspacesByProject[project.id];
              const workspaceError = workspaceErrors[project.id];
              // A local workspace's path IS its project's path by construction,
              // so a path line renders only where a row's checkout differs.
              const titles = workspaceDisplayTitles(workspaces ?? []);
              const count = workspaces?.length ?? 0;
              return (
                <div data-settings-project key={project.id}>
                  <SettingsRow
                    title={project.name}
                    description={project.path}
                    control={
                      workspaceError !== undefined ? null : (
                        <span>
                          {count} workspace{count === 1 ? "" : "s"}
                        </span>
                      )
                    }
                  />
                  {workspaceError !== undefined ? (
                    <div role="alert">
                      <ErrorText
                        sentence={`Workspaces unavailable: ${workspaceError.sentence}`}
                        detail={workspaceError.detail}
                        id={`settings-workspaces-error-${project.id}`}
                      />
                      <button type="button" onClick={() => void loadProjects()}>
                        Retry
                      </button>
                    </div>
                  ) : (
                    <SettingsAdvanced>
                      {(workspaces ?? []).map((workspace) => (
                        // Render what the daemon sent: no project-path
                        // fallback, no joined path. Same contract as Session.cwd.
                        <Fragment key={workspace.id}>
                          <span className="settings-card-meta">
                            {titles.get(workspace.id) ?? workspace.title}
                          </span>
                          {workspace.path && workspace.path !== project.path ? (
                            <span className="settings-card-meta">{workspace.path}</span>
                          ) : null}
                        </Fragment>
                      ))}
                    </SettingsAdvanced>
                  )}
                </div>
              );
            })
          : null}
        {!loading && error === null && projects.length === 0 ? (
          <p className="settings-status" role="status">
            No projects registered
          </p>
        ) : null}
      </SettingsSection>
      <SettingsAdvanced>
        <p>
          A project is a git repository or any directory this daemon can reach. Workspaces live
          inside it.
        </p>
      </SettingsAdvanced>

      <NewProjectDialog open={dialogOpen} onClose={closeDialog} onCreate={handleProjectAdded} />
    </div>
  );
}
