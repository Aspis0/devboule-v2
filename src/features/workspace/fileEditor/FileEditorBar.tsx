// Ported from Paseo's `packages/app/src/file-pane/bar.tsx`
// (Apache-2.0, Copyright (c) 2025-present Mohamed Boudra): the editor's
// status strip — size, line count, dirty dot, saving spinner, save failure,
// cursor. Devboule classes, minimal text (no labels for what the dot and
// the spinner already say).
//
// Paseo source: `packages/app/src/file-pane/bar.tsx`.

import type { FileEditorStatus } from "./model";

function formatSize(size: number): string {
  if (size < 1024) return `${size} B`;
  if (size < 1024 * 1024) return `${(size / 1024).toFixed(1)} KB`;
  return `${(size / (1024 * 1024)).toFixed(1)} MB`;
}

export function FileEditorBar({
  size,
  lineCount,
  editorStatus,
  cursor,
  saveWarning,
}: {
  size: number;
  lineCount?: number;
  editorStatus?: FileEditorStatus;
  cursor?: { line: number; column: number };
  /** The last save's identity warning, if the bytes landed but the
   * target's owner, attributes or permissions did not fully follow. */
  saveWarning?: string | null;
}) {
  return (
    <div className="file-editor-bar" data-testid="file-editor-bar">
      <span className="file-editor-meta">{formatSize(size)}</span>
      {lineCount !== undefined ? (
        <span className="file-editor-meta">
          {lineCount === 1 ? "1 line" : `${lineCount} lines`}
        </span>
      ) : null}
      <span className="file-editor-spacer" aria-hidden="true" />
      <span className="file-editor-status">
        {editorStatus === "dirty" ? (
          <span className="file-editor-dirty" aria-label="Unsaved changes" />
        ) : null}
        {editorStatus === "saving" ? (
          <span className="file-editor-saving" role="status" aria-label="Saving" />
        ) : null}
        {editorStatus === "error" ? <span className="file-editor-error">Save failed</span> : null}
        {saveWarning ? (
          <span className="file-editor-meta" role="status" title={saveWarning}>
            {saveWarning}
          </span>
        ) : null}
        {cursor ? (
          <span className="file-editor-meta">
            Ln {cursor.line}, Col {cursor.column}
          </span>
        ) : null}
      </span>
    </div>
  );
}
