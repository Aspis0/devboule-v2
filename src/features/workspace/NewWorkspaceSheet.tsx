import { memo, useCallback, useEffect, useRef, useState, type FormEvent } from "react";
import { getFocusableElements } from "../../lib/focusableElements";
import { isImeComposition } from "../../lib/imeComposition";
import { useModalOpen } from "../../lib/modalOpen";
import type { ErrorSentence } from "../../lib/errorSentence";
import { ErrorText } from "../../components/ErrorText";
import "./Workspace.css";

/** The "Where" value for this machine. A paired host carries its own device id. */
export const THIS_PC = "this-pc";

export interface SheetProject {
  id: string;
  name: string;
}

export interface SheetHost {
  id: string;
  name: string;
  online: boolean;
}

export interface NewWorkspaceChoice {
  projectId: string;
  /** THIS_PC, or a paired host's device id. */
  where: string;
  isolation: "local" | "worktree";
  /** The starting branch for a new worktree; null lets the daemon pick one. */
  branch: string | null;
}

export interface NewWorkspaceSheetProps {
  projects: readonly SheetProject[];
  /** The project the "+" was pressed on; the first project when opened from the top. */
  presetProjectId: string | null;
  /** Paired hosts other than this PC. The Where row is drawn only when there is one. */
  pairedHosts: readonly SheetHost[];
  /** Creates the workspace and answers with the refusal, if one came back. */
  onCreate: (choice: NewWorkspaceChoice) => Promise<ErrorSentence | null>;
  onClose: () => void;
}

/** The refusal a host choice gets: creating on another PC is not built yet. */
export const REMOTE_CREATE_UNAVAILABLE = "Creating a workspace on another PC is not available yet.";

/**
 * One sheet for a new workspace, in the order the choices depend on: which
 * project, where (this PC or a paired host), how it is isolated, and the
 * branch a worktree starts from. The agent is not chosen here: it is chosen in
 * the composer of the first message, where the provider belongs.
 */
export const NewWorkspaceSheet = memo(function NewWorkspaceSheet({
  projects,
  presetProjectId,
  pairedHosts,
  onCreate,
  onClose,
}: NewWorkspaceSheetProps) {
  // Mounted fresh for each opening (the caller keys it), so the choices start
  // from the preset project and the defaults every time.
  const [chosenProjectId, setProjectId] = useState<string | null>(null);
  const projectId = chosenProjectId ?? presetProjectId ?? projects[0]?.id ?? null;
  const [where, setWhere] = useState(THIS_PC);
  const [isolation, setIsolation] = useState<NewWorkspaceChoice["isolation"]>("local");
  const [branch, setBranch] = useState("");
  const [error, setError] = useState<ErrorSentence | null>(null);
  const [submitting, setSubmitting] = useState(false);
  const submittingRef = useRef(false);
  const dialogRef = useRef<HTMLDivElement>(null);

  useModalOpen(true);

  useEffect(() => {
    const dialog = dialogRef.current;
    if (dialog === null) return;
    dialog.querySelector<HTMLElement>("[data-dialog-initial-focus]")?.focus();

    const onKeyDown = (event: globalThis.KeyboardEvent) => {
      if (event.key === "Escape") {
        if (isImeComposition(event)) return;
        event.preventDefault();
        if (!submittingRef.current) onClose();
        return;
      }
      if (event.key !== "Tab") return;
      const focusable = getFocusableElements(dialog);
      if (focusable.length === 0) {
        event.preventDefault();
        dialog.focus();
        return;
      }
      const first = focusable[0];
      const last = focusable[focusable.length - 1];
      if (!dialog.contains(document.activeElement)) {
        event.preventDefault();
        first.focus();
      } else if (event.shiftKey && document.activeElement === first) {
        event.preventDefault();
        last.focus();
      } else if (!event.shiftKey && document.activeElement === last) {
        event.preventDefault();
        first.focus();
      }
    };
    document.addEventListener("keydown", onKeyDown);
    return () => document.removeEventListener("keydown", onKeyDown);
  }, [onClose]);

  const remoteChoice = where !== THIS_PC;
  const canCreate = projectId !== null && !remoteChoice && !submitting;

  const handleSubmit = useCallback(
    async (event: FormEvent<HTMLFormElement>) => {
      event.preventDefault();
      if (projectId === null || submittingRef.current) return;
      if (remoteChoice) {
        setError({ sentence: REMOTE_CREATE_UNAVAILABLE, detail: null });
        return;
      }
      submittingRef.current = true;
      setSubmitting(true);
      setError(null);
      const trimmed = branch.trim();
      const refusal = await onCreate({
        projectId,
        where,
        isolation,
        branch: isolation === "worktree" && trimmed !== "" ? trimmed : null,
      });
      submittingRef.current = false;
      setSubmitting(false);
      if (refusal === null) onClose();
      else setError(refusal);
    },
    [branch, isolation, onClose, onCreate, projectId, remoteChoice, where],
  );

  return (
    <div
      className="workspace-project-dialog-backdrop"
      onMouseDown={(event) => {
        if (event.target === event.currentTarget && !submitting) onClose();
      }}
    >
      <div
        ref={dialogRef}
        className="workspace-project-dialog new-workspace-sheet"
        role="dialog"
        aria-modal="true"
        aria-labelledby="new-workspace-title"
        tabIndex={-1}
      >
        <div className="workspace-project-dialog-header">
          <h2 id="new-workspace-title">New workspace</h2>
          <button
            type="button"
            className="workspace-dialog-close"
            onClick={onClose}
            aria-label="Close new workspace"
            disabled={submitting}
          >
            ×
          </button>
        </div>
        <form onSubmit={(event) => void handleSubmit(event)}>
          <fieldset className="new-workspace-group">
            <legend>Project</legend>
            {projects.map((project, index) => (
              <label key={project.id} className="new-workspace-option">
                <input
                  type="radio"
                  name="new-workspace-project"
                  value={project.id}
                  checked={projectId === project.id}
                  onChange={() => setProjectId(project.id)}
                  data-dialog-initial-focus={index === 0 ? "true" : undefined}
                  disabled={submitting}
                />
                {project.name}
              </label>
            ))}
          </fieldset>

          {pairedHosts.length > 0 ? (
            <fieldset className="new-workspace-group">
              <legend>Where</legend>
              <label className="new-workspace-option">
                <input
                  type="radio"
                  name="new-workspace-where"
                  value={THIS_PC}
                  checked={where === THIS_PC}
                  onChange={() => setWhere(THIS_PC)}
                  disabled={submitting}
                />
                This PC
              </label>
              {pairedHosts.map((host) => (
                <label key={host.id} className="new-workspace-option">
                  <input
                    type="radio"
                    name="new-workspace-where"
                    value={host.id}
                    checked={where === host.id}
                    onChange={() => setWhere(host.id)}
                    disabled={submitting}
                  />
                  {host.name}
                  {host.online ? null : <span className="new-workspace-offline">offline</span>}
                </label>
              ))}
              {remoteChoice ? (
                <p className="new-workspace-note" role="note">
                  {REMOTE_CREATE_UNAVAILABLE}
                </p>
              ) : null}
            </fieldset>
          ) : null}

          <fieldset className="new-workspace-group">
            <legend>Isolation</legend>
            <label className="new-workspace-option">
              <input
                type="radio"
                name="new-workspace-isolation"
                value="local"
                checked={isolation === "local"}
                onChange={() => setIsolation("local")}
                disabled={submitting}
              />
              Project folder
            </label>
            <label className="new-workspace-option">
              <input
                type="radio"
                name="new-workspace-isolation"
                value="worktree"
                checked={isolation === "worktree"}
                onChange={() => setIsolation("worktree")}
                disabled={submitting}
              />
              New worktree
            </label>
          </fieldset>

          {isolation === "worktree" ? (
            <label className="workspace-project-input-label" htmlFor="new-workspace-branch">
              Starting branch
              <input
                id="new-workspace-branch"
                value={branch}
                onChange={(event) => setBranch(event.target.value)}
                placeholder="Chosen for you if empty"
                disabled={submitting}
              />
            </label>
          ) : null}

          {error !== null ? (
            <div className="workspace-project-error" role="alert">
              <ErrorText sentence={error.sentence} detail={error.detail} id="new-workspace-error" />
            </div>
          ) : null}

          <div className="workspace-project-dialog-actions">
            <button
              type="button"
              className="workspace-secondary-action"
              onClick={onClose}
              disabled={submitting}
            >
              Cancel
            </button>
            <button type="submit" className="workspace-primary-action" disabled={!canCreate}>
              {submitting ? "Creating…" : "Create workspace"}
            </button>
          </div>
        </form>
      </div>
    </div>
  );
});
