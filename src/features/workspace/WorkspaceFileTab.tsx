// The File tab: an editable file, or the old read-only body when the
// file cannot be edited. Local workspace text up to 128 KiB opens in the
// in-app editor (Paseo's model, CodeMirror 6, autosave, conflict banner);
// binary and over-cap files keep the read-only body they always had, and
// a missing file opens empty — the first save creates it. Files on a
// paired host go over the held peer link, and absolute or `~` paths go
// app-only to this machine's own files; both edit straight in the tab,
// with no Preview split and no external-editor pencil (that launch exists
// only for folders this machine holds). Markdown on the local workspace
// keeps its Preview | Source split — Preview renders, Source edits.

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
import { EditableFilePane } from "./EditableFilePane";
import {
  isOutsidePath,
  isAbsolutePath,
  OUTSIDE_WORKSPACE,
  EDITOR_MIN_DIALECT,
} from "./fileEditor/useEditableFile";
import { toolContentKey } from "./toolContentCache";
import { LOCAL_HOST_ID, parseWorkspaceKey, type WorkspaceKey } from "./hosts/hostIdentity";
import { refreshWorkspaceDaemon, useWorkspaceDaemon } from "./workspaceDaemon";
import "./fileTab.css";

/** `.md` and `.markdown`, case-insensitive: Preview's only kinds. */
function isMarkdownPath(path: string): boolean {
  return /\.markdown$|\.md$/i.test(path);
}

/** Mirrors `DOES_NOT_EXIST` in `workspace_files.rs` (pinned by the
 * daemon's own tests): the one refusal that routes to the editor instead
 * of the read-only body, because a missing file opens empty and the first
 * save creates it. */
const DOES_NOT_EXIST = "the requested path does not exist";

/** Bytes the editor opens whole: mirrors `MAX_EDITABLE_FILE_BYTES` in
 * `workspace_file_edit.rs` (128 KiB — sized so the worst escaping still
 * fits the 1 MiB frame with margin). Over it the tab keeps the windowed
 * read-only body, so a big log stays pageable instead of becoming a
 * refusal that could take the connection down. */
const MAX_EDITABLE_FILE_BYTES = 128 * 1024;

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

/** Whether the windowed read says the editor owns this file: text the
 * editor opens whole, a missing file the first save creates, or an
 * absolute path the workspace road refused as outside (the editor
 * retries it on the app road). Binary, over-cap and other refusals stay
 * on the read-only body they always had. */
function isEditorFile(reply: WorkspaceFileContent | null, path: string): boolean {
  if (reply === null) return false;
  if (reply.status === "binary" || reply.status === "too_large") return false;
  if (reply.status === "refused")
    return (
      reply.error === DOES_NOT_EXIST || (reply.error === OUTSIDE_WORKSPACE && isAbsolutePath(path))
    );
  return (
    reply.status === "ok" &&
    reply.kind === "text" &&
    (reply.size === null || reply.size <= MAX_EDITABLE_FILE_BYTES)
  );
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
  const { hostId, workspaceId } = parseWorkspaceKey(workspaceKey);
  // The editor frames need a daemon that speaks them (dialect 33+). A
  // stale daemon cannot decode them and would drop the connection, so
  // the tab stays read-only with a short note instead of sending. An
  // unknown version (connecting) waits rather than risks the link.
  const daemon = useWorkspaceDaemon();
  const version = daemon.protocolVersion;
  const known = version !== null;
  const stale = version !== null && version < EDITOR_MIN_DIALECT;
  const canEdit = known && !stale;
  // A paired host's file and the human's own `~` file never take the
  // windowed road: the windowed read confines to the local workspace, so
  // it cannot serve either. Both edit straight in the tab, under the
  // same header as local files — minus the external-editor pencil, which
  // can only open folders this machine holds. (`~` always means this
  // machine's home, even under a remote key: the far home is not
  // addressable, and guessing it would open the wrong file.)
  if (hostId !== LOCAL_HOST_ID || isOutsidePath(path)) {
    if (!known) {
      return daemon.state === "connected" ? (
        <div className="workspace-diff-note" role="status">
          Loading file…
        </div>
      ) : (
        <div className="workspace-diff-note" role="alert">
          {daemon.message ?? "daemon unreachable"}
        </div>
      );
    }
    if (stale) return <StaleDaemonNote what="edit files" />;
    return (
      <div className="workspace-file-tab">
        <EditorTabHeader workspaceId={null} path={path} seg={null} />
        <EditableFilePane workspaceKey={workspaceKey} path={path} refreshNonce={refreshNonce} />
      </div>
    );
  }
  return (
    <LocalWorkspaceFileTab
      workspaceKey={workspaceKey}
      workspaceId={workspaceId}
      path={path}
      refreshNonce={refreshNonce}
      cache={cache}
      canEdit={canEdit}
      staleKnown={stale}
    />
  );
}

/** The tab header the editor branch keeps: file name, parent folder,
 * the Preview/Source split for Markdown, and the external-editor pencil
 * where it can work (a workspace folder this machine holds — never a
 * remote or outside path). The editor's own bar below says size, lines
 * and save state, so the header carries no second helping of those. */
function EditorTabHeader({
  workspaceId,
  path,
  seg,
}: {
  /** Null for remote and outside files: no pencil there. */
  workspaceId: string | null;
  path: string;
  seg: ReactNode;
}) {
  return (
    <header className="workspace-file-tab-header">
      <StripKindMark kind="file" />
      <span className="workspace-file-tab-name" title={path}>
        {basename(path)}
      </span>
      <span className="workspace-file-tab-meta">{parentDir(path)}</span>
      {seg}
      {workspaceId !== null && !isOutsidePath(path) ? (
        <OpenInEditorAction workspaceId={workspaceId} path={path} />
      ) : null}
    </header>
  );
}

/** What a stale tab shows when the daemon predates the road it needs:
 * the file stays unreadable (no frame can fetch it), this says why,
 * and Retry hurries the version check — an upgraded daemon opens the
 * tab on its own when the new version lands. Short, because the fix is
 * a restart, not a decision. */
export function StaleDaemonNote({ what }: { what: string }): ReactNode {
  return (
    <div className="workspace-diff-note" role="status">
      <span>Update the daemon to {what}.</span>{" "}
      <button
        type="button"
        className="workspace-secondary-action"
        onClick={() => refreshWorkspaceDaemon()}
      >
        Retry
      </button>
    </div>
  );
}

function LocalWorkspaceFileTab({
  workspaceKey,
  workspaceId,
  path,
  refreshNonce,
  cache,
  canEdit,
  staleKnown,
}: {
  workspaceKey: WorkspaceKey;
  workspaceId: string;
  path: string;
  refreshNonce: number;
  cache: Map<string, PreviewCell>;
  /** False when the daemon predates the editor frames: read-only. */
  canEdit: boolean;
  /** True when that staleness is confirmed (not merely unknown). */
  staleKnown: boolean;
}) {
  const errorId = useId();
  // What the daemon is addressed by, read off the tab's own key.
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

  if (canEdit && isEditorFile(reply, path) && (!markdown || mode === "source")) {
    return (
      <div className="workspace-file-tab">
        <EditorTabHeader
          workspaceId={workspaceId}
          path={path}
          seg={
            markdown ? (
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
            ) : null
          }
        />
        <EditableFilePane workspaceKey={workspaceKey} path={path} refreshNonce={refreshNonce} />
      </div>
    );
  }

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
      {staleKnown ? <StaleDaemonNote what="edit files" /> : null}
      {body}
    </div>
  );
}
