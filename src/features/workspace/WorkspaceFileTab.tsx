// The File tab: one file's read worn as the spec's header (icon, basename,
// muted `dir · size · N lines`, Preview | Source) and a body that renders
// Markdown in Preview and numbered text in Source. The tab owns its read —
// the Files panel's selection never drives it — seeds from Workspace's cell
// cache so a revisited tab does not flash empty, and re-reads when the
// already active tab is clicked again. Only text routes here: the Files row
// offers no tab for a path that needs media staging.

import { useId, useEffect, useRef, useState, type ReactNode } from "react";
import { MarkdownText } from "../../components/MarkdownText";
import { ErrorText } from "../../components/ErrorText";
import type { WorkspaceFileContent } from "../../types/ipc";
import { StripKindMark } from "./strip/StripKindMark";
import { formatSize } from "./FilesPreview";
import { FileTabSource } from "./FileTabSource";
import { fileTabMode, setFileTabMode, type FileTabMode } from "./fileTabMode";
import { OpenInEditorAction } from "./OpenInEditorAction";
import { useWorkspaceFilePreview, type PreviewCell } from "./useWorkspaceFilePreview";
import { toolContentKey } from "./toolContentCache";
import { parseWorkspaceKey, type WorkspaceKey } from "./hosts/hostIdentity";
import "./fileTab.css";

/** `.md` and `.markdown`, case-insensitive: Preview's only kinds. */
function isMarkdownPath(path: string): boolean {
  return /\.markdown$|\.md$/i.test(path);
}

function lastSeparator(path: string): number {
  return Math.max(path.lastIndexOf("/"), path.lastIndexOf("\\"));
}

function basename(path: string): string {
  return path.slice(lastSeparator(path) + 1);
}

function parentDir(path: string): string {
  const cut = lastSeparator(path);
  return cut === -1 ? "" : path.slice(0, cut);
}

/** The file's own line count — only when the whole file is on screen:
 * the window started at line 1 and the wire says nothing follows and
 * nothing was cut. Any other window's count is not the file's, and the
 * header never shows one. */
function wholeFileLineCount(reply: WorkspaceFileContent | null): number | null {
  if (
    reply === null ||
    reply.status !== "ok" ||
    reply.fromLine !== 1 ||
    reply.hasMore !== false ||
    reply.truncated !== false ||
    reply.lines === null
  ) {
    return null;
  }
  return reply.lines;
}

function headerMeta(reply: WorkspaceFileContent | null, path: string): string {
  const parts: string[] = [];
  const dir = parentDir(path);
  if (dir !== "") parts.push(dir);
  if (reply !== null && reply.size !== null) parts.push(formatSize(reply.size));
  const lines = wholeFileLineCount(reply);
  if (lines !== null) parts.push(lines === 1 ? "1 line" : `${lines} lines`);
  return parts.join(" · ");
}

/** The window's disclosure row: the wire's own sentence for a cut line and
 * the control that asks for the next window — and nothing else, so no
 * window measure can dress up as the file's count. */
function WindowFooter({
  window,
  readMore,
}: {
  window: WorkspaceFileContent;
  readMore: () => Promise<void>;
}) {
  if (window.truncated !== true && window.hasMore !== true) return null;
  return (
    <div className="workspace-file-tab-window">
      {window.truncated ? (
        <span>{window.note ?? "the last line is cut short at the window's cap"}</span>
      ) : null}
      {window.hasMore === true ? (
        <button
          type="button"
          className="workspace-secondary-action"
          onClick={() => void readMore()}
        >
          Read more
        </button>
      ) : null}
    </div>
  );
}

export function WorkspaceFileTab({
  workspaceKey,
  path,
  refreshNonce,
  cache,
}: {
  workspaceKey: WorkspaceKey;
  path: string;
  /** Bumped when the already-active tab is clicked again: re-reads. */
  refreshNonce: number;
  /** The last landed cells, owned by Workspace: the seed while re-reading. */
  cache: Map<string, PreviewCell>;
}) {
  const errorId = useId();
  // What the daemon is addressed by, read off the tab's own key.
  const workspaceId = parseWorkspaceKey(workspaceKey).workspaceId;
  const cacheKey = toolContentKey(workspaceKey, path);
  const { preview, selection, select, refresh, readMore } = useWorkspaceFilePreview(workspaceId, {
    path,
    cell: cache.get(cacheKey) ?? { reply: null, staged: null, failure: null },
  });
  useEffect(() => {
    select(path);
  }, [select, path]);
  useEffect(() => {
    if (preview.reply !== null || preview.staged !== null || preview.failure !== null) {
      cache.set(cacheKey, preview);
    }
  }, [preview, cache, cacheKey]);
  // A re-click re-reads through the hook's own refresh, which keeps the old
  // cell until the new reply lands. The mount's select already started the
  // first read, so the initial nonce is skipped, never replayed.
  const lastRefreshRef = useRef(refreshNonce);
  useEffect(() => {
    if (lastRefreshRef.current === refreshNonce) return;
    lastRefreshRef.current = refreshNonce;
    refresh();
  }, [refreshNonce, refresh]);

  // The mode starts from the app-run memory; choosing writes it back, so
  // the next File tab — this one revisited or another — opens the same way.
  const [mode, setMode] = useState<FileTabMode>(() => fileTabMode());
  const chooseMode = (next: FileTabMode): void => {
    setFileTabMode(next);
    setMode(next);
  };

  const markdown = isMarkdownPath(path);
  const reply = preview.reply;
  const text = reply !== null && reply.status === "ok" && reply.kind === "text" ? reply : null;

  let body: ReactNode;
  if (preview.failure !== null) {
    body = (
      <div className="workspace-diff-note workspace-diff-note-error" role="alert">
        <ErrorText
          sentence={preview.failure.sentence}
          detail={preview.failure.detail}
          id={errorId}
        />
      </div>
    );
  } else if (preview.staged !== null) {
    // Media cannot open a File tab (the Files row offers tabs to text
    // only); if a stage's answer ever lands here anyway, say what is true
    // instead of drawing a pane that pretends to read it.
    body =
      preview.staged.status === "refused" ? (
        <div className="workspace-diff-note workspace-diff-note-error" role="alert">
          {preview.staged.error}
        </div>
      ) : (
        <div className="workspace-diff-note" role="status">
          This file is shown in the Files panel.
        </div>
      );
  } else if (reply === null) {
    body = (
      <div className="workspace-diff-note" role="status">
        Loading file…
      </div>
    );
  } else if (reply.error !== null) {
    body = (
      <div className="workspace-diff-note workspace-diff-note-error" role="alert">
        {reply.error}
      </div>
    );
  } else if (reply.status === "binary") {
    body = (
      <div className="workspace-diff-note">This file is binary; there is no content to show.</div>
    );
  } else if (text !== null && markdown && mode === "preview") {
    body = (
      <div className="workspace-file-tab-preview">
        <MarkdownText text={text.content ?? ""} />
        <WindowFooter window={text} readMore={readMore} />
      </div>
    );
  } else if (text !== null) {
    body = (
      <div className="workspace-file-tab-source-body">
        <FileTabSource window={text} />
        <WindowFooter window={text} readMore={readMore} />
      </div>
    );
  } else {
    // An ok reply no branch above renders (a kind without a reader):
    // say so — a bare header reads as a hang.
    body = <div className="workspace-diff-note">This file can't be shown here.</div>;
  }

  if (selection !== path) {
    return (
      <div className="workspace-diff-note" role="status">
        Loading file…
      </div>
    );
  }
  return (
    <div className="workspace-file-tab">
      <header className="workspace-file-tab-header">
        <StripKindMark kind="file" />
        <span className="workspace-file-tab-name" title={path}>
          {basename(path)}
        </span>
        <span className="workspace-file-tab-meta">{headerMeta(reply, path)}</span>
        {markdown ? (
          <div className="workspace-file-tab-seg" role="group" aria-label="File view">
            {(["preview", "source"] as const).map((candidate) => (
              <button
                key={candidate}
                type="button"
                className={`workspace-file-tab-seg-button${
                  mode === candidate ? " workspace-file-tab-seg-button-is-on" : ""
                }`}
                aria-pressed={mode === candidate}
                onClick={() => chooseMode(candidate)}
              >
                {candidate === "preview" ? "Preview" : "Source"}
              </button>
            ))}
          </div>
        ) : null}
        <OpenInEditorAction workspaceId={workspaceId} path={path} />
      </header>
      {body}
    </div>
  );
}
