import { memo, useEffect, useRef, useState, type FormEvent } from "react";
import { getFocusableElements } from "../../lib/focusableElements";
import { isImeComposition } from "../../lib/imeComposition";
import { useModalOpen } from "../../lib/modalOpen";
import type { ErrorSentence } from "../../lib/errorSentence";
import { ChoiceGroup, type ChoiceOption } from "../../components/ChoiceGroup";
import { ErrorText } from "../../components/ErrorText";
import "./Workspace.css";

/** The "Where" value for this machine. A paired host carries its own device id. */
export const THIS_PC = "this-pc";

const ISOLATION_OPTIONS: readonly ChoiceOption<NewWorkspaceChoice["isolation"]>[] = [
  { value: "local", label: "Project folder" },
  { value: "worktree", label: "New worktree" },
];

export interface SheetProject {
  id: string;
  name: string;
  /** THIS_PC's device id, or the paired host the project lives on. */
  hostId: string;
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
  // from the preset project and the defaults every time. The Where choice
  // decides which projects are offered: the preset's own host first.
  const presetHost = projects.find((project) => project.id === presetProjectId)?.hostId ?? THIS_PC;
  const [chosenWhere, setChosenWhere] = useState<string | null>(null);
  const where = chosenWhere ?? presetHost;
  const hostProjects = projects.filter((project) => project.hostId === where);
  const [chosenProjectId, setProjectId] = useState<string | null>(null);
  const projectId =
    hostProjects.find((project) => project.id === chosenProjectId)?.id ??
    hostProjects.find((project) => project.id === presetProjectId)?.id ??
    hostProjects[0]?.id ??
    null;
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
    // The first tab stop is the selected Project, the dialog's first control.
    dialog.querySelector<HTMLElement>('[role="radio"][tabindex="0"]')?.focus();

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

  const whereOptions: readonly ChoiceOption<string>[] = [
    { value: THIS_PC, label: "This PC" },
    ...pairedHosts.map((host): ChoiceOption<string> => ({
      value: host.id,
      label: host.name,
      hint: host.online ? undefined : "offline",
    })),
  ];

  const canCreate = projectId !== null && !submitting;

  // Not memoized: it is the form's own submit, read once per submit, and a
  // memo over these inputs cannot be kept (the compiler said so).
  const handleSubmit = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    if (projectId === null || submittingRef.current) return;
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
  };

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
            {hostProjects.length === 0 ? (
              <p className="new-workspace-note" role="note">
                {where === THIS_PC
                  ? "No project here yet. Add one first."
                  : "That PC lists no project yet."}
              </p>
            ) : (
              <ChoiceGroup
                orientation="vertical"
                options={hostProjects.map((project) => ({
                  value: project.id,
                  label: project.name,
                }))}
                value={projectId ?? ""}
                onChange={setProjectId}
                disabled={submitting}
                ariaLabel="Project"
              />
            )}
          </fieldset>

          {pairedHosts.length > 0 ? (
            <fieldset className="new-workspace-group">
              <legend>Where</legend>
              <ChoiceGroup
                options={whereOptions}
                value={where}
                onChange={(next) => {
                  setChosenWhere(next);
                  setProjectId(null);
                }}
                disabled={submitting}
                ariaLabel="Where"
              />
            </fieldset>
          ) : null}

          <fieldset className="new-workspace-group">
            <legend>Isolation</legend>
            <ChoiceGroup
              options={ISOLATION_OPTIONS}
              value={isolation}
              onChange={setIsolation}
              disabled={submitting}
              ariaLabel="Isolation"
            />
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
