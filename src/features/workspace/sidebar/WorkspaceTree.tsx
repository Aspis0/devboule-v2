import { useMemo, type ReactNode } from "react";
import { ErrorText } from "../../../components/ErrorText";
import { firstGrapheme } from "../../../lib/graphemeBound";
import type { ErrorSentence } from "../../../lib/errorSentence";
import type { WorkspaceProject } from "../workspaceProjects";
import { hostProjectKey, keyOfWorkspace } from "../workspaceProjects";
import { LOCAL_HOST_ID, type HostId, type WorkspaceKey } from "../hosts/hostIdentity";
import { avatarStyle } from "./avatars";
import { hostLabel } from "./hostNames";
import { ProjectMenu } from "./ProjectMenu";
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
  /** Takes a row's workspace off the list (its folder stays); answers with the refusal, if one came. */
  onArchiveWorkspace: (workspaceId: string, hostId: HostId) => Promise<ErrorSentence | null>;
  /** Takes a project off the list; its folder stays. Answers with the refusal, if one came. */
  onRemoveProject: (projectId: string, hostId: HostId) => Promise<ErrorSentence | null>;
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
 * The project tree: every project keeps its header — its square badge and
 * name, with the "+" that starts a workspace and the kebab that removes the
 * project at its far edge — above its rows. No count: the rows are the count.
 * Two projects sharing a name are told apart by the folder each sits in. What
 * a row prints is decided by the row itself (WorkspaceRow). The rail lists
 * workspaces only: an agent is reached through its tab.
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
  onArchiveWorkspace,
  onRemoveProject,
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
            key={hostProjectKey(project.hostId, project.id)}
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
              <button
                type="button"
                className="workspace-project-new"
                onClick={(event) => onNewWorkspace(event.currentTarget, project.id)}
                aria-label={`New workspace in ${project.name}`}
                title="New workspace"
              >
                <span aria-hidden="true">+</span>
              </button>
              <ProjectMenu
                projectName={project.name}
                onRemove={() => onRemoveProject(project.id, project.hostId)}
              />
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
                    hostName={hostLabel(hostNames, workspace.hostId)}
                    selected={selected}
                    stat={key === null ? undefined : stats.get(key)}
                    branch={key === null ? undefined : branches.get(key)}
                    onSelect={onSelectWorkspace}
                    onRename={onRenameWorkspace}
                    onArchive={onArchiveWorkspace}
                    remote={!local}
                    hostOffline={!local && !(hostOnline?.get(project.hostId) ?? false)}
                  />
                );
              })}
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
