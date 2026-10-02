import type { ReactNode } from "react";
import { ErrorText } from "../../../components/ErrorText";
import { firstGrapheme } from "../../../lib/graphemeBound";
import type { ErrorSentence } from "../../../lib/errorSentence";
import type { WorkspaceProject } from "../workspaceProjects";
import { avatarStyle } from "./avatars";
import { WorkspaceRow } from "./WorkspaceRow";
import type { WorkspaceStat } from "./useWorkspaceStats";

export interface WorkspaceTreeProps {
  projects: readonly WorkspaceProject[];
  loading: boolean;
  error: ErrorSentence | null;
  providerError: ErrorSentence | null;
  selectedWorkspace: string | null;
  onRetryProjects: () => void;
  onSelectWorkspace: (workspaceId: string) => void;
  onNewWorkspace: (trigger: HTMLButtonElement, projectId: string) => void;
  /** Persists a row's new title; answers with the refusal, if one came. */
  onRenameWorkspace: (workspaceId: string, title: string) => Promise<ErrorSentence | null>;
  /** The project whose new-row wrap hosts the provider choice UI. */
  providerMenuAnchorProjectId: string | null;
  /** The provider choice UI itself (popover or consent card). */
  providerMenu: ReactNode;
  stats: ReadonlyMap<string, WorkspaceStat>;
}

/**
 * The project tree under the host header: project headers with avatars and a
 * hover-revealed "+", the workspace rows (each its own component: label,
 * stats, context menu and in-place title editor), and the quiet "New
 * workspace" row.
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
  providerMenuAnchorProjectId,
  providerMenu,
  stats,
}: WorkspaceTreeProps) {
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
            {project.workspaces.map((workspace) => (
              <WorkspaceRow
                key={workspace.id}
                workspace={workspace}
                projectName={project.name}
                selected={selectedWorkspace === workspace.id}
                stat={stats.get(workspace.id)}
                onSelect={onSelectWorkspace}
                onRename={onRenameWorkspace}
              />
            ))}
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
