import { memo, useCallback, useEffect, useRef, useState } from "react";
import type { FormEvent } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { projectAdd, reasonFromCause } from "../../lib/tauri";
import type { Project } from "../../types/ipc";
import "./Workspace.css";

interface NewProjectDialogProps {
  open: boolean;
  onClose: () => void;
  onCreate: (project: Project) => void | Promise<void>;
}

function getFocusableElements(container: HTMLElement): HTMLElement[] {
  return Array.from(
    container.querySelectorAll<HTMLElement>(
      'button:not([disabled]), input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [href], [tabindex]:not([tabindex="-1"])',
    ),
  ).filter(
    (element) => !element.hasAttribute("hidden") && element.getAttribute("aria-hidden") !== "true",
  );
}

export const NewProjectDialog = memo(function NewProjectDialog({
  open: isOpen,
  onClose,
  onCreate,
}: NewProjectDialogProps) {
  const [value, setValue] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [choosing, setChoosing] = useState(false);
  const [submitting, setSubmitting] = useState(false);
  const submittingRef = useRef(false);
  const dialogRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!isOpen) return;

    setValue("");
    setError(null);
    setChoosing(false);
    setSubmitting(false);
    submittingRef.current = false;

    const dialog = dialogRef.current;
    if (dialog === null) return;

    const initialFocus = dialog.querySelector<HTMLElement>("[data-dialog-initial-focus]");
    initialFocus?.focus();

    const handleDialogKeyDown = (event: globalThis.KeyboardEvent) => {
      if (event.key === "Escape") {
        event.preventDefault();
        if (!submittingRef.current) onClose();
        return;
      }
      if (event.key !== "Tab") return;

      const focusableElements = getFocusableElements(dialog);
      if (focusableElements.length === 0) {
        event.preventDefault();
        dialog.focus();
        return;
      }

      const firstElement = focusableElements[0];
      const lastElement = focusableElements[focusableElements.length - 1];
      if (!dialog.contains(document.activeElement)) {
        event.preventDefault();
        firstElement.focus();
      } else if (event.shiftKey && document.activeElement === firstElement) {
        event.preventDefault();
        lastElement.focus();
      } else if (!event.shiftKey && document.activeElement === lastElement) {
        event.preventDefault();
        firstElement.focus();
      }
    };

    document.addEventListener("keydown", handleDialogKeyDown);
    return () => document.removeEventListener("keydown", handleDialogKeyDown);
  }, [isOpen, onClose]);

  const handleChooseFolder = useCallback(async () => {
    if (choosing || submitting) return;
    setChoosing(true);
    setError(null);
    try {
      const selected = await open({ directory: true });
      if (typeof selected === "string") {
        setValue(selected);
        setError(null);
        dialogRef.current?.querySelector<HTMLInputElement>("#workspace-project-input")?.focus();
      }
    } catch (cause: unknown) {
      setError(reasonFromCause(cause));
    } finally {
      setChoosing(false);
    }
  }, [choosing, submitting]);

  const handleSubmit = useCallback(
    async (event: FormEvent<HTMLFormElement>) => {
      event.preventDefault();
      const path = value.trim();
      if (!path) {
        setError("Choose or enter an absolute folder path.");
        return;
      }
      if (submitting) return;

      setSubmitting(true);
      submittingRef.current = true;
      setError(null);
      try {
        const project = await projectAdd(path);
        await onCreate(project);
        onClose();
      } catch (cause: unknown) {
        setError(reasonFromCause(cause));
      } finally {
        setSubmitting(false);
        submittingRef.current = false;
      }
    },
    [onClose, onCreate, submitting, value],
  );

  if (!isOpen) return null;

  return (
    <div
      className="workspace-project-dialog-backdrop"
      onMouseDown={(event) => {
        if (event.target === event.currentTarget && !submitting) onClose();
      }}
    >
      <div
        ref={dialogRef}
        className="workspace-project-dialog"
        role="dialog"
        aria-modal="true"
        aria-labelledby="workspace-project-dialog-title"
        tabIndex={-1}
      >
        <div className="workspace-project-dialog-header">
          <div>
            <div className="workspace-dialog-eyebrow">Project</div>
            <h2 id="workspace-project-dialog-title">Add project</h2>
          </div>
          <button
            type="button"
            className="workspace-dialog-close"
            onClick={onClose}
            aria-label="Close add project dialog"
            disabled={submitting}
          >
            ×
          </button>
        </div>
        <p className="workspace-project-dialog-copy">
          Register a repository or folder that already exists on disk. Workspaces and sessions are
          added after it exists.
        </p>
        <form onSubmit={handleSubmit}>
          <label className="workspace-project-input-label" htmlFor="workspace-project-input">
            Existing folder
          </label>
          <div className="workspace-project-picker-row">
            <button
              type="button"
              className="workspace-secondary-action workspace-project-picker-button"
              data-dialog-initial-focus="true"
              onClick={() => void handleChooseFolder()}
              disabled={choosing || submitting}
            >
              {choosing ? "Choosing…" : "Choose folder…"}
            </button>
            <span className="workspace-project-picker-hint">or type an absolute path</span>
          </div>
          <input
            id="workspace-project-input"
            value={value}
            onChange={(event) => {
              setValue(event.target.value);
              setError(null);
            }}
            placeholder="C:\\Users\\you\\project"
            aria-invalid={error !== null}
            aria-describedby={error !== null ? "workspace-project-error" : undefined}
            disabled={submitting}
          />
          {error !== null ? (
            <div id="workspace-project-error" className="workspace-project-error" role="alert">
              {error}
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
            <button type="submit" className="workspace-primary-action" disabled={submitting}>
              {submitting ? "Adding…" : "Add project"}
            </button>
          </div>
        </form>
      </div>
    </div>
  );
});
