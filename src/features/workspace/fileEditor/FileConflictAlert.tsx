// Ported from Paseo's `packages/app/src/file-pane/conflict-alert.tsx`
// (Apache-2.0, Copyright (c) 2025-present Mohamed Boudra): the conflict
// banner — changed on disk (Overwrite/Reload), deleted, check failed
// (Retry). Short sentences, Devboule classes.
//
// Paseo source: `packages/app/src/file-pane/conflict-alert.tsx`.

export type FileConflictAlertState =
  | { kind: "changed"; canOverwrite: boolean; onReload(): void; onOverwrite(): void }
  | { kind: "deleted" }
  | { kind: "checkFailed"; retrying: boolean; onRetry(): void };

export function FileConflictAlert({ state }: { state: FileConflictAlertState }) {
  let title = "Changed on disk";
  if (state.kind === "deleted") title = "Deleted";
  else if (state.kind === "checkFailed") title = "Could not check the file";
  return (
    <div className="file-conflict-alert" data-testid="file-conflict-alert" role="alert">
      <span className="file-conflict-title">{title}</span>
      {state.kind === "changed" && state.canOverwrite ? (
        <span className="file-conflict-actions">
          <button type="button" className="workspace-secondary-action" onClick={state.onOverwrite}>
            Overwrite
          </button>
          <button type="button" className="workspace-secondary-action" onClick={state.onReload}>
            Reload
          </button>
        </span>
      ) : null}
      {state.kind === "changed" && !state.canOverwrite ? (
        <span className="file-conflict-actions">
          <button type="button" className="workspace-secondary-action" onClick={state.onReload}>
            Reload
          </button>
        </span>
      ) : null}
      {state.kind === "checkFailed" ? (
        <span className="file-conflict-actions">
          <button
            type="button"
            className="workspace-secondary-action"
            onClick={state.onRetry}
            disabled={state.retrying}
          >
            Retry
          </button>
        </span>
      ) : null}
    </div>
  );
}
