// The editable File pane: Paseo's `EditableFilePane`
// (`packages/app/src/file-pane/pane.tsx`, Apache-2.0, Copyright (c)
// 2025-present Mohamed Boudra) mapped onto this app — the model owns the
// buffer, the bar shows size/lines/dirty/saving/cursor, the conflict
// banner offers Overwrite/Reload/Retry, and a reload over local edits
// asks through the shared ConfirmDialog first. Styling and sentences are
// this app's own; the save/dirty/conflict logic is Paseo's, in `model.ts`.

import { useCallback, useEffect, useState, useSyncExternalStore } from "react";
import { ConfirmDialog } from "../../components/ConfirmDialog";
import type { WorkspaceKey } from "./hosts/hostIdentity";
import { FileEditorBar } from "./fileEditor/FileEditorBar";
import { FileConflictAlert, type FileConflictAlertState } from "./fileEditor/FileConflictAlert";
import { FileEditorView } from "./fileEditor/FileEditorView";
import { getFileConflictCallout } from "./fileEditor/model";
import { useEditableFile } from "./fileEditor/useEditableFile";
import "./fileEditor.css";

export function EditableFilePane({
  workspaceKey,
  path,
  refreshNonce,
}: {
  workspaceKey: WorkspaceKey;
  path: string;
  refreshNonce: number;
}) {
  const { model, loading, failure, refresh, reopen, reopening } = useEditableFile(
    workspaceKey,
    path,
    refreshNonce,
  );
  const [cursor, setCursor] = useState({ line: 1, column: 1 });
  const [askingReload, setAskingReload] = useState(false);
  const snapshot = useSyncExternalStore(
    (listener) => model?.subscribe(listener) ?? (() => undefined),
    () => model?.getSnapshot() ?? null,
    () => model?.getSnapshot() ?? null,
  );

  useEffect(() => {
    setAskingReload(false);
  }, [path, workspaceKey]);

  // While the reload ask is up the buffer must not save underneath it:
  // suspend the pending autosave, resume on either answer. The manual
  // save and the conflict roads are untouched — only the timer pauses.
  useEffect(() => {
    if (!askingReload || !model) return;
    const resume = model.suspendAutosave();
    return resume;
  }, [askingReload, model]);

  // Window close or quit with an unsaved or in-flight buffer: flush
  // first, then let the browser ask. The save is already on the wire
  // when the dialog opens; staying keeps the banner (and the failure,
  // if the save failed), leaving keeps whatever landed.
  useEffect(() => {
    if (!model) return;
    const onBeforeUnload = (event: BeforeUnloadEvent) => {
      const status = model.getSnapshot().status;
      if (status !== "dirty" && status !== "saving" && status !== "error") return;
      void model.save();
      event.preventDefault();
    };
    window.addEventListener("beforeunload", onBeforeUnload);
    return () => window.removeEventListener("beforeunload", onBeforeUnload);
  }, [model]);

  const handleReload = useCallback(() => {
    if (!model) return;
    if (!snapshot?.modified) {
      void model.reload();
      return;
    }
    setAskingReload(true);
  }, [model, snapshot?.modified]);
  const handleOverwrite = useCallback(() => void model?.overwrite(), [model]);

  if (loading || !model || !snapshot) {
    return (
      <div className="workspace-diff-note" role="status">
        Loading file…
      </div>
    );
  }

  const callout = getFileConflictCallout(snapshot);
  const conflict: FileConflictAlertState | undefined =
    callout === null
      ? undefined
      : callout.kind === "deleted"
        ? { kind: "deleted" }
        : callout.kind === "checkFailed"
          ? { kind: "checkFailed", retrying: reopening, onRetry: refresh }
          : {
              kind: "changed",
              canOverwrite: callout.canOverwrite,
              onReload: handleReload,
              onOverwrite: handleOverwrite,
            };
  const size =
    snapshot.observedVersion.status === "ready"
      ? snapshot.observedVersion.size
      : snapshot.content.length;

  return (
    <div className="workspace-file-tab">
      <header className="workspace-file-tab-header">
        <FileEditorBar
          size={size}
          lineCount={snapshot.content.split("\n").length}
          editorStatus={snapshot.status}
          cursor={cursor}
        />
      </header>
      {failure !== null ? (
        <div className="workspace-diff-note workspace-diff-note-error" role="alert">
          <span>{failure}</span>
          <button
            type="button"
            className="workspace-secondary-action"
            onClick={reopen}
            disabled={reopening}
          >
            Retry
          </button>
        </div>
      ) : null}
      {snapshot.status === "error" && snapshot.error !== null ? (
        <div className="workspace-diff-note workspace-diff-note-error" role="alert">
          {snapshot.error}
        </div>
      ) : null}
      {conflict ? <FileConflictAlert state={conflict} /> : null}
      <FileEditorView
        model={model}
        filename={path.split("/").pop() ?? path}
        lineStart={null}
        lineEnd={null}
        navigationRevision={0}
        onCursorChange={setCursor}
      />
      <ConfirmDialog
        open={askingReload}
        title="Reload file?"
        message="Your changes will be lost."
        confirmLabel="Reload"
        tone="danger"
        onConfirm={() => {
          setAskingReload(false);
          void model.reload();
        }}
        onCancel={() => setAskingReload(false)}
      />
    </div>
  );
}
