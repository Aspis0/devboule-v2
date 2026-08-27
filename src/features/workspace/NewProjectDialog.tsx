import { memo, useCallback, useEffect, useRef, useState } from "react";
import type { FormEvent } from "react";

type ProjectCreationRoute = "existing" | "new" | "clone";

const PROJECT_CREATION_ROUTES: readonly {
  id: ProjectCreationRoute;
  label: string;
  description: string;
}[] = [
  {
    id: "existing",
    label: "Existing folder",
    description: "Open a repository or folder already on disk.",
  },
  { id: "new", label: "New folder", description: "Create a project folder at a path." },
  { id: "clone", label: "Clone from GitHub", description: "Start from a GitHub repository URL." },
];

function isGitHubRepositoryUrl(value: string): boolean {
  return /^https:\/\/(?:www\.)?github\.com\/[\w.-]+\/[\w.-]+(?:\.git)?\/?(?:[?#].*)?$/i.test(value);
}

interface NewProjectDialogProps {
  open: boolean;
  onClose: () => void;
  onCreate: (draft: { route: ProjectCreationRoute; value: string }) => void;
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
  open,
  onClose,
  onCreate,
}: NewProjectDialogProps) {
  const [route, setRoute] = useState<ProjectCreationRoute>("existing");
  const [value, setValue] = useState("");
  const [error, setError] = useState<string | null>(null);
  const dialogRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!open) return;

    setRoute("existing");
    setValue("");
    setError(null);

    const dialog = dialogRef.current;
    if (dialog === null) return;

    const initialFocus = dialog.querySelector<HTMLElement>("[data-dialog-initial-focus]");
    initialFocus?.focus();

    const handleDialogKeyDown = (event: globalThis.KeyboardEvent) => {
      if (event.key === "Escape") {
        event.preventDefault();
        onClose();
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
  }, [onClose, open]);

  const handleRouteChange = useCallback((nextRoute: ProjectCreationRoute) => {
    setRoute(nextRoute);
    setValue("");
    setError(null);
  }, []);

  const handleSubmit = useCallback(
    (event: FormEvent<HTMLFormElement>) => {
      event.preventDefault();
      const trimmedValue = value.trim();
      if (!trimmedValue) {
        setError(route === "clone" ? "Enter a GitHub repository URL." : "Enter a folder path.");
        return;
      }
      if (route === "clone" && !isGitHubRepositoryUrl(trimmedValue)) {
        setError("Use a GitHub repository URL such as https://github.com/org/repo.");
        return;
      }

      onCreate({ route, value: trimmedValue });
    },
    [onCreate, route, value],
  );

  if (!open) return null;

  const inputLabel =
    route === "clone"
      ? "GitHub repository URL"
      : route === "new"
        ? "Folder path to create"
        : "Folder path";
  const inputPlaceholder =
    route === "clone" ? "https://github.com/org/repo" : "C:\\Users\\you\\project";
  const submitLabel =
    route === "clone" ? "Clone project" : route === "new" ? "Create project" : "Add project";

  return (
    <div
      className="workspace-project-dialog-backdrop"
      onMouseDown={(event) => {
        if (event.target === event.currentTarget) onClose();
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
            <h2 id="workspace-project-dialog-title">New project</h2>
          </div>
          <button
            type="button"
            className="workspace-dialog-close"
            onClick={onClose}
            aria-label="Close new project dialog"
          >
            ×
          </button>
        </div>
        <p className="workspace-project-dialog-copy">
          A project is a repository or folder on disk. Workspaces and sessions are added after it
          exists.
        </p>
        <div
          className="workspace-project-route-list"
          role="tablist"
          aria-label="Project creation route"
        >
          {PROJECT_CREATION_ROUTES.map((projectRoute) => (
            <button
              type="button"
              role="tab"
              aria-selected={route === projectRoute.id}
              aria-controls={`workspace-project-route-${projectRoute.id}`}
              className={`workspace-project-route${route === projectRoute.id ? " workspace-project-route-selected" : ""}`}
              key={projectRoute.id}
              onClick={() => handleRouteChange(projectRoute.id)}
            >
              <span className="workspace-project-route-label">{projectRoute.label}</span>
              <span className="workspace-project-route-description">
                {projectRoute.description}
              </span>
            </button>
          ))}
        </div>
        <form onSubmit={handleSubmit}>
          <div id={`workspace-project-route-${route}`} role="tabpanel" aria-label={inputLabel}>
            <label className="workspace-project-input-label" htmlFor="workspace-project-input">
              {inputLabel}
            </label>
            <input
              id="workspace-project-input"
              data-dialog-initial-focus="true"
              value={value}
              onChange={(event) => {
                setValue(event.target.value);
                setError(null);
              }}
              placeholder={inputPlaceholder}
              aria-invalid={error !== null}
              aria-describedby={error !== null ? "workspace-project-error" : undefined}
            />
            {error !== null ? (
              <div id="workspace-project-error" className="workspace-project-error" role="alert">
                {error}
              </div>
            ) : null}
          </div>
          <div className="workspace-project-dialog-note">
            Mock only · no filesystem, git, or network access.
          </div>
          <div className="workspace-project-dialog-actions">
            <button type="button" className="workspace-secondary-action" onClick={onClose}>
              Cancel
            </button>
            <button type="submit" className="workspace-primary-action">
              {submitLabel}
            </button>
          </div>
        </form>
      </div>
    </div>
  );
});

export type { ProjectCreationRoute };
