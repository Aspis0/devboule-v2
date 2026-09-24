import { memo, type ReactNode } from "react";
import { ErrorText } from "../../../components/ErrorText";
import { firstGrapheme } from "../../../lib/graphemeBound";
import type { ErrorSentence } from "../../../lib/errorSentence";
import type { WorkspaceProject } from "../workspaceProjects";
import { avatarStyle } from "./avatars";
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
  /** The provider choice UI, rendered inside the project that opened it. */
  providerMenuFor: (projectId: string) => ReactNode;
  stats: ReadonlyMap<string, WorkspaceStat>;
}

/**
 * The project tree under the host header: project headers with avatars and a
 * hover-revealed "+", workspace rows with avatar, title, optional meta, `+N −M`
 * stats and the trailing state dot, and the quiet "New workspace" row.
 */
const DOT_LABELS: Record<string, string> = {
  pulse: "running",
  attention: "needs attention",
  unattended: "running unattended",
};

function WorkspaceTreeImpl({
  projects,
  loading,
  error,
  providerError,
  selectedWorkspace,
  onRetryProjects,
  onSelectWorkspace,
  onNewWorkspace,
  providerMenuFor,
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
          <ErrorText sentence={error.sentence} detail={error.detail} id="workspace-projects-error" />
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
              const stat = stats.get(workspace.id);
              return (
                <button
                  type="button"
                  className={`workspace-row${
                    selectedWorkspace === workspace.id ? " workspace-row-selected" : ""
                  }`}
                  key={workspace.id}
                  onClick={() => onSelectWorkspace(workspace.id)}
                  aria-pressed={selectedWorkspace === workspace.id}
                  aria-label={`${workspace.title}, ${project.name}${
                    workspace.stateDot !== null ? `, ${DOT_LABELS[workspace.stateDot]}` : ""
                  }`}
                  title={workspace.path ? workspace.path : undefined}
                >
                  <span
                    className="sidebar-avatar sidebar-avatar-workspace"
                    style={avatarStyle(workspace.id)}
                    aria-hidden="true"
                  >
                    {firstGrapheme(workspace.title)}
                  </span>
                  <span className="workspace-row-copy">
                    <span className="workspace-row-title">{workspace.title}</span>
                    {workspace.meta !== null ? (
                      <span className="workspace-row-meta">{workspace.meta}</span>
                    ) : null}
                  </span>
                  {stat !== undefined ? (
                    <span className="sidebar-row-stats">
                      <span className="sidebar-stat-add">+{stat.additions}</span>{" "}
                      <span className="sidebar-stat-del">−{stat.deletions}</span>
                    </span>
                  ) : null}
                  {workspace.stateDot !== null ? (
                    <span
                      role="img"
                      aria-label={DOT_LABELS[workspace.stateDot]}
                      className={`sidebar-row-dot sidebar-row-dot-${workspace.stateDot}${
                        workspace.stateDot === "pulse" ? " dot-pulse" : ""
                      }`}
                    />
                  ) : null}
                </button>
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
              {providerMenuFor(project.id)}
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

export const WorkspaceTree = memo(WorkspaceTreeImpl);
