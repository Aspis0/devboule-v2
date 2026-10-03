import { useEffect, useState, type ReactNode } from "react";
import { ErrorText } from "../../../components/ErrorText";
import { firstGrapheme } from "../../../lib/graphemeBound";
import type { ErrorSentence } from "../../../lib/errorSentence";
import type { WorkspaceProject } from "../workspaceProjects";
import { keyOfWorkspace } from "../workspaceProjects";
import type { WorkspaceKey } from "../hosts/hostIdentity";
import { avatarStyle } from "./avatars";
import { WorkspaceRow } from "./WorkspaceRow";
import type { WorkspaceStat } from "./useWorkspaceStats";

export interface WorkspaceTreeProps {
  projects: readonly WorkspaceProject[];
  loading: boolean;
  error: ErrorSentence | null;
  providerError: ErrorSentence | null;
  selectedWorkspace: WorkspaceKey | null;
  onRetryProjects: () => void;
  onSelectWorkspace: (workspaceKey: WorkspaceKey) => void;
  onNewWorkspace: (trigger: HTMLButtonElement, projectId: string) => void;
  /** Persists a row's new title; answers with the refusal, if one came. */
  onRenameWorkspace: (workspaceId: string, title: string) => Promise<ErrorSentence | null>;
  /** Deletes a row's workspace; answers with the refusal, if one came. */
  onDeleteWorkspace: (workspaceId: string) => Promise<ErrorSentence | null>;
  /** The project whose new-row wrap hosts the provider choice UI. */
  providerMenuAnchorProjectId: string | null;
  /** The provider choice UI itself (popover or consent card). */
  providerMenu: ReactNode;
  stats: ReadonlyMap<WorkspaceKey, WorkspaceStat>;
  /** Each workspace's branch, from the same status read as `stats`. */
  branches: ReadonlyMap<WorkspaceKey, string>;
}

/** One clock for the rows' last-activity labels. The roster pushes whenever a
 * turn moves, but a workspace nobody has touched must still see its own label
 * age, and a timer per row would be a timer per row. */
function useRowClock(): number {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const id = window.setInterval(() => setNow(Date.now()), 30_000);
    return () => window.clearInterval(id);
  }, []);
  return now;
}

/**
 * The project tree under the host header: project headers with avatars and a
 * hover-revealed "+", the workspace rows (each its own component: label,
 * facts, context menu, in-place title editor and delete ask), and the quiet
 * "New workspace" row.
 */
export function WorkspaceTree({
  projects,
  loading,
  error,
  providerError,
  selectedWorkspace,
  onRetryProjects,
  onSelectWorkspace,
  onNewWorkspace,
  onRenameWorkspace,
  onDeleteWorkspace,
  providerMenuAnchorProjectId,
  providerMenu,
  stats,
  branches,
}: WorkspaceTreeProps) {
  const now = useRowClock();
  return (
    <>
      {loading ? (
        <div className="workspace-empty" role="status">
          Loading projects…
        </div>
      ) : null}
      {error !== null ? (
        <div className="workspace-project-error" role="alert">
          <ErrorText
            sentence={error.sentence}
            detail={error.detail}
            id="workspace-projects-error"
          />
          <button type="button" className="workspace-secondary-action" onClick={onRetryProjects}>
            Retry
          </button>
        </div>
      ) : null}
      {providerError !== null ? (
        <div className="workspace-project-error" role="alert">
          <ErrorText
            sentence={providerError.sentence}
            detail={providerError.detail}
            id="workspace-provider-error"
          />
        </div>
      ) : null}
      {projects.map((project) => (
        <div className="workspace-project" key={project.id} role="group" aria-label={project.name}>
          <div className="workspace-project-heading sidebar-project-head">
            <span
              className="sidebar-avatar sidebar-avatar-project"
              style={avatarStyle(project.id)}
              aria-hidden="true"
            >
              {firstGrapheme(project.name)}
            </span>
            <span className="workspace-project-name">{project.name}</span>
            <button
              type="button"
              className="workspace-project-add"
              onClick={(event) => onNewWorkspace(event.currentTarget, project.id)}
              title="New workspace in this project"
              aria-label={`New workspace in ${project.name}`}
            >
              +
            </button>
          </div>
          {project.workspaceError !== undefined ? (
            <div className="workspace-project-error" role="alert">
              <ErrorText
                sentence={`Could not load this project's workspaces: ${project.workspaceError.sentence}`}
                detail={project.workspaceError.detail}
                id={`workspace-project-workspaces-error-${project.id}`}
              />
              <button
                type="button"
                className="workspace-secondary-action"
                onClick={onRetryProjects}
              >
                Retry
              </button>
            </div>
          ) : null}
          <div className="workspace-project-items">
            {project.workspaces.map((workspace) => {
              const key = keyOfWorkspace(workspace);
              return (
                <WorkspaceRow
                  key={workspace.id}
                  workspace={workspace}
                  workspaceKey={key}
                  projectName={project.name}
                  selected={key !== null && selectedWorkspace === key}
                  stat={key === null ? undefined : stats.get(key)}
                  branch={key === null ? undefined : branches.get(key)}
                  now={now}
                  onSelect={onSelectWorkspace}
                  onRename={onRenameWorkspace}
                  onDelete={onDeleteWorkspace}
                />
              );
            })}
            <div className="workspace-new-row-wrap">
              <button
                type="button"
                className="workspace-new-row"
                onClick={(event) => onNewWorkspace(event.currentTarget, project.id)}
              >
                <span aria-hidden="true">+</span>New workspace
              </button>
              {providerMenuAnchorProjectId === project.id ? providerMenu : null}
            </div>
          </div>
        </div>
      ))}
      {error === null && !loading && projects.length === 0 ? (
        <div className="workspace-empty">No matching workspaces</div>
      ) : null}
    </>
  );
}
