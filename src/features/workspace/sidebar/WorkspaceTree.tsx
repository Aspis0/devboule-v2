import { useMemo, type ReactNode } from "react";
import { ErrorText } from "../../../components/ErrorText";
import { firstGrapheme } from "../../../lib/graphemeBound";
import type { ErrorSentence } from "../../../lib/errorSentence";
import type { WorkspaceProject } from "../workspaceProjects";
import { keyOfWorkspace } from "../workspaceProjects";
import { LOCAL_HOST_ID, type HostId, type WorkspaceKey } from "../hosts/hostIdentity";
import { avatarStyle } from "./avatars";
import { hostLabel } from "./hostNames";
import { WorkspaceRow } from "./WorkspaceRow";
import type { WorkspaceStat } from "./useWorkspaceStats";

export interface WorkspaceTreeProps {
  projects: readonly WorkspaceProject[];
  loading: boolean;
  error: ErrorSentence | null;
  providerError: ErrorSentence | null;
  selectedWorkspace: WorkspaceKey | null;
  onRetryProjects: () => void;
  /** Re-reads the provider catalog behind the provider-error block. */
  onRetryProviders: () => void;
  onSelectWorkspace: (workspaceKey: WorkspaceKey) => void;
  onNewWorkspace: (trigger: HTMLButtonElement, projectId: string) => void;
  /** Persists a row's new title; answers with the refusal, if one came. */
  onRenameWorkspace: (workspaceId: string, title: string) => Promise<ErrorSentence | null>;
  /** Deletes a row's workspace; answers with the refusal, if one came. */
  onDeleteWorkspace: (workspaceId: string) => Promise<ErrorSentence | null>;
  /** The project whose anchor hosts the provider choice UI. */
  providerMenuAnchorProjectId: string | null;
  /** The provider choice UI itself (popover or consent card). */
  providerMenu: ReactNode;
  stats: ReadonlyMap<WorkspaceKey, WorkspaceStat>;
  /** Each workspace's branch, from the same status read as `stats`. */
  branches: ReadonlyMap<WorkspaceKey, string>;
  /** Each host's short label by id, for the rows' second line. */
  hostNames: ReadonlyMap<HostId, string>;
  /** Whether each host answers right now; the local host always does. A
   * surface that does not carry the map treats every host as offline. */
  hostOnline?: ReadonlyMap<HostId, boolean>;
}

/**
 * The folder a project lives in: the one thing two projects sharing a name do
 * not share. Null when the path gives no parent to name.
 */
function parentFolder(path: string): string | null {
  const parts = path.split(/[\\/]+/).filter((part) => part !== "");
  return parts.length >= 2 ? parts[parts.length - 2] : null;
}

/**
 * The project tree: every project keeps its header — avatar, name, workspace
 * count — above its rows and a "+ New workspace" row below them, and a header
 * whose name a row would only repeat carries the folder the project sits in
 * instead of leaving two identical headers. What a row prints beside its name
 * is decided by the row itself (WorkspaceRow). The rail lists workspaces only:
 * an agent is reached through its tab.
 */
export function WorkspaceTree({
  projects,
  loading,
  error,
  providerError,
  selectedWorkspace,
  onRetryProjects,
  onRetryProviders,
  onSelectWorkspace,
  onNewWorkspace,
  onRenameWorkspace,
  onDeleteWorkspace,
  providerMenuAnchorProjectId,
  providerMenu,
  stats,
  branches,
  hostNames,
  hostOnline,
}: WorkspaceTreeProps) {
  // Derived once per project list, not once per render: two headers may hold
  // the same name, and only the folder separates them.
  const sharedNames = useMemo(() => {
    const counts = new Map<string, number>();
    for (const project of projects) {
      counts.set(project.name, (counts.get(project.name) ?? 0) + 1);
    }
    return counts;
  }, [projects]);

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
          <button type="button" className="workspace-secondary-action" onClick={onRetryProviders}>
            Retry
          </button>
        </div>
      ) : null}
      {projects.map((project) => {
        const folder = (sharedNames.get(project.name) ?? 0) > 1 ? parentFolder(project.path) : null;
        return (
          <div
            className="workspace-project"
            key={project.id}
            role="group"
            aria-label={folder === null ? project.name : `${project.name} in ${folder}`}
          >
            <div className="workspace-project-heading sidebar-project-head">
              <span
                className="sidebar-avatar sidebar-avatar-project"
                style={avatarStyle(project.id)}
                aria-hidden="true"
              >
                {firstGrapheme(project.name)}
              </span>
              <span className="workspace-project-name">{project.name}</span>
              {folder === null ? null : <span className="workspace-project-folder">{folder}</span>}
              <span className="workspace-project-count">
                {project.workspaceError === undefined ? (
                  <>
                    <span aria-hidden="true">{project.workspaces.length}</span>
                    <span className="sr-only">
                      {project.workspaces.length === 1
                        ? "1 workspace"
                        : `${project.workspaces.length} workspaces`}
                    </span>
                  </>
                ) : (
                  <span className="sr-only">workspaces could not be loaded</span>
                )}
              </span>
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
                const selected = key !== null && selectedWorkspace === key;
                const local = project.hostId === LOCAL_HOST_ID;
                return (
                  <WorkspaceRow
                    key={key ?? workspace.id}
                    workspace={workspace}
                    workspaceKey={key}
                    projectName={project.name}
                    hostName={hostLabel(hostNames, workspace.hostId)}
                    selected={selected}
                    stat={key === null ? undefined : stats.get(key)}
                    branch={key === null ? undefined : branches.get(key)}
                    onSelect={onSelectWorkspace}
                    onRename={onRenameWorkspace}
                    onDelete={onDeleteWorkspace}
                    remote={!local}
                    hostOffline={!local && !(hostOnline?.get(project.hostId) ?? false)}
                  />
                );
              })}
              {/* A remote project's "+ New workspace" belongs to its host and
                  is not built yet; it must never fall through to this daemon. */}
              {project.hostId === LOCAL_HOST_ID ? (
                <button
                  type="button"
                  className="workspace-project-new"
                  onClick={(event) => onNewWorkspace(event.currentTarget, project.id)}
                  aria-label={`New workspace in ${project.name}`}
                >
                  <span aria-hidden="true">+</span> New workspace
                </button>
              ) : null}
            </div>
            {providerMenuAnchorProjectId === project.id ? providerMenu : null}
          </div>
        );
      })}
      {error === null && !loading && projects.length === 0 ? (
        <div className="workspace-empty">No matching workspaces</div>
      ) : null}
    </>
  );
}
