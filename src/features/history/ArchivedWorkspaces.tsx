import { useCallback, useState } from "react";
import { ErrorText } from "../../components/ErrorText";
import { errorSentence, type ErrorSentence } from "../../lib/errorSentence";
import { archivedList, projectAdd, workspaceRestore } from "../../lib/tauri";
import type { ArchivedList, Project, Workspace } from "../../types/ipc";

export interface ArchivedWorkspacesProps {
  /** Runs after something came back, so the sidebar re-reads the list. */
  onRestored: () => void;
}

/**
 * What the list took off, with a way back: an archived workspace is put on
 * the list again, and a removed project is listed again by adding its folder.
 * Read only when opened, so the History page does not poll the daemon.
 */
export function ArchivedWorkspaces({ onRestored }: ArchivedWorkspacesProps) {
  const [open, setOpen] = useState(false);
  const [archived, setArchived] = useState<ArchivedList | null>(null);
  const [error, setError] = useState<ErrorSentence | null>(null);
  const [busy, setBusy] = useState<string | null>(null);

  const load = useCallback(async () => {
    try {
      setArchived(await archivedList());
      setError(null);
    } catch (cause: unknown) {
      setError(errorSentence(cause));
    }
  }, []);

  const restoreWorkspace = async (workspace: Workspace) => {
    setBusy(workspace.id);
    try {
      await workspaceRestore(workspace.id);
      setError(null);
      await load();
      onRestored();
    } catch (cause: unknown) {
      setError(errorSentence(cause));
    } finally {
      setBusy(null);
    }
  };

  const restoreProject = async (project: Project) => {
    setBusy(project.id);
    try {
      await projectAdd(project.path);
      setError(null);
      await load();
      onRestored();
    } catch (cause: unknown) {
      setError(errorSentence(cause));
    } finally {
      setBusy(null);
    }
  };

  const projectNames = new Map(
    (archived?.projects ?? []).map((project) => [project.id, project.name]),
  );
  const count = archived === null ? null : archived.projects.length + archived.workspaces.length;

  return (
    <section className="history-archived" aria-label="Archived">
      <button
        type="button"
        className="history-archived-toggle"
        aria-expanded={open}
        onClick={() => {
          // Read when opened, from the press that opens it.
          if (!open) void load();
          setOpen(!open);
        }}
      >
        Archived{count === null || count === 0 ? "" : ` · ${count}`}
      </button>
      {open ? (
        <div className="history-archived-body">
          {error === null ? null : (
            <div className="history-archived-error" role="alert">
              <ErrorText
                sentence={error.sentence}
                detail={error.detail}
                id="history-archived-error"
              />
            </div>
          )}
          {archived === null ? null : archived.projects.length + archived.workspaces.length ===
            0 ? (
            <p className="history-empty">Nothing is archived.</p>
          ) : (
            <ul className="history-archived-list">
              {archived.projects.map((project) => (
                <li key={`project-${project.id}`} className="history-archived-row">
                  <span className="history-archived-name">{project.name}</span>
                  <span className="history-archived-kind">project</span>
                  <button
                    type="button"
                    className="workspace-secondary-action"
                    aria-label={`Restore project ${project.name}`}
                    disabled={busy !== null}
                    onClick={() => void restoreProject(project)}
                  >
                    Restore
                  </button>
                </li>
              ))}
              {archived.workspaces.map((workspace) => (
                <li key={`workspace-${workspace.id}`} className="history-archived-row">
                  <span className="history-archived-name">{workspace.title}</span>
                  <span className="history-archived-kind">
                    {projectNames.get(workspace.projectId) ?? "workspace"}
                  </span>
                  <button
                    type="button"
                    className="workspace-secondary-action"
                    aria-label={`Restore ${workspace.title}`}
                    disabled={busy !== null}
                    onClick={() => void restoreWorkspace(workspace)}
                  >
                    Restore
                  </button>
                </li>
              ))}
            </ul>
          )}
        </div>
      ) : null}
    </section>
  );
}
