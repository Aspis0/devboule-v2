// The Diff tab surface: header plus unified/split body over one file's reply.
// DiffCard still serves the Changes panel alone.

import { useId, useMemo, useState, type ReactNode } from "react";
import type { WorkspaceGitDiffLine, WorkspaceGitFileDiff } from "../../types/ipc";
import { ErrorText } from "../../components/ErrorText";
import type { ChangesReply } from "./useWorkspaceChanges";
import { withDiffLineNumbers, type NumberedDiffLine } from "./diffLineNumbers";
import { DIFF_LINE_MARKER } from "./diffMarker";
import { toSplitRows, type SplitDiffRow } from "./diffSplitRows";
import "./panel/diffTab.css";

export type DiffTabMode = "unified" | "split";

// Unified is the default; the choice outlives any one tab mount, for the
// app run only — Workspace mounts just the active tool pane, so module
// memory is exactly per-run memory with no reload persistence.
let rememberedMode: DiffTabMode = "unified";

/** Test hook: the memory persists across mounts by design; tests reset it. */
export function resetDiffTabModeMemoryForTests(): void {
  rememberedMode = "unified";
}

function rememberMode(next: DiffTabMode): void {
  rememberedMode = next;
}

// The tree file rows' document glyph, at the header's 14px icon size.
function FileGlyph(): ReactNode {
  return (
    <svg
      className="diff-tab-file-icon"
      width="14"
      height="14"
      viewBox="0 0 24 24"
      fill="none"
      aria-hidden="true"
    >
      <path
        d="M15 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V7Z"
        stroke="currentColor"
        strokeWidth="2"
        strokeLinejoin="round"
      />
      <path
        d="M14 2v4a2 2 0 0 0 2 2h4"
        stroke="currentColor"
        strokeWidth="2"
        strokeLinejoin="round"
      />
    </svg>
  );
}

// One exported map names the row classes: the component renders from it and
// the test pins it to the stylesheet's selectors, so a rename breaks loudly.
export const ROW_CLASS: Record<WorkspaceGitDiffLine["kind"], string> = {
  add: "diff-tab-added",
  remove: "diff-tab-removed",
  context: "diff-tab-context",
  header: "diff-tab-hunk",
};

const ROW_WORD: Record<WorkspaceGitDiffLine["kind"], string | null> = {
  add: "added",
  remove: "removed",
  context: null,
  header: null,
};

function formatNumber(value: number | null): string {
  return value === null ? "" : String(value);
}

function UnifiedRows({ lines }: { lines: readonly NumberedDiffLine[] }) {
  return (
    <>
      {lines.map((line) =>
        line.kind === "header" ? (
          <div className={ROW_CLASS.header} key={line.key}>
            <span className="diff-tab-text">{line.text}</span>
          </div>
        ) : (
          <div className={`diff-tab-line ${ROW_CLASS[line.kind]}`} key={line.key}>
            <span className="diff-tab-num">{formatNumber(line.oldNumber)}</span>
            <span className="diff-tab-num">{formatNumber(line.newNumber)}</span>
            <span className="diff-tab-marker" aria-hidden="true">
              {DIFF_LINE_MARKER[line.kind]}
            </span>
            {ROW_WORD[line.kind] !== null ? (
              <span className="diff-tab-visually-hidden">{ROW_WORD[line.kind]}</span>
            ) : null}
            <span className="diff-tab-text">{line.text}</span>
          </div>
        ),
      )}
    </>
  );
}

function SplitCell({ cell, number }: { cell: NumberedDiffLine | null; number: "old" | "new" }) {
  if (cell === null) return <div className="diff-tab-cell-empty" />;
  return (
    <div className={`diff-tab-cell ${ROW_CLASS[cell.kind]}`}>
      <span className="diff-tab-num">
        {formatNumber(number === "old" ? cell.oldNumber : cell.newNumber)}
      </span>
      <span className="diff-tab-marker" aria-hidden="true">
        {DIFF_LINE_MARKER[cell.kind]}
      </span>
      {ROW_WORD[cell.kind] !== null ? (
        <span className="diff-tab-visually-hidden">{ROW_WORD[cell.kind]}</span>
      ) : null}
      <span className="diff-tab-text">{cell.text}</span>
    </div>
  );
}

// A content row always carries a line, so one side's key is always defined.
function splitRowKey(row: SplitDiffRow): string {
  if (row.span) return row.header.key;
  const left = row.left?.key;
  const right = row.right?.key;
  if (left !== undefined && right !== undefined && left !== right) return `${left}+${right}`;
  return left ?? right ?? "empty";
}

function SplitRows({ rows }: { rows: readonly SplitDiffRow[] }) {
  return (
    <>
      {rows.map((row) =>
        row.span ? (
          <div className={ROW_CLASS.header} key={row.header.key}>
            <span>{row.header.text}</span>
          </div>
        ) : (
          <div className="diff-tab-split-row" key={splitRowKey(row)}>
            <SplitCell cell={row.left} number="old" />
            <SplitCell cell={row.right} number="new" />
          </div>
        ),
      )}
    </>
  );
}

export function DiffTab({
  path,
  diff,
}: {
  path: string;
  diff: ChangesReply<WorkspaceGitFileDiff>;
}) {
  const errorId = useId();
  const [mode, setMode] = useState<DiffTabMode>(() => rememberedMode);
  const chooseMode = (next: DiffTabMode): void => {
    rememberMode(next);
    setMode(next);
  };
  const slash = Math.max(path.lastIndexOf("/"), path.lastIndexOf("\\"));
  const base = slash < 0 ? path : path.slice(slash + 1);
  const dir = slash < 0 ? "" : path.slice(0, slash);
  const reply = diff.reply;
  const stats =
    reply === null ? (
      <span className="diff-tab-stats-word">{diff.failure !== null ? "error" : "…"}</span>
    ) : reply.status === "ok" ? (
      <>
        <span className="diff-tab-stats-add">+{reply.additions}</span>
        <span className="diff-tab-stats-del">−{reply.deletions}</span>
      </>
    ) : (
      // The status words DiffCard shows beside the same replies.
      <span className="diff-tab-stats-word">
        {reply.status === "binary"
          ? "binary"
          : reply.status === "too_large"
            ? "too large"
            : "error"}
      </span>
    );
  const numbered = useMemo(
    () => (reply !== null && reply.status === "ok" ? withDiffLineNumbers(reply.lines) : null),
    [reply],
  );
  const split = useMemo(
    () => (mode === "split" && numbered !== null ? toSplitRows(numbered) : null),
    [mode, numbered],
  );
  return (
    <div className="diff-tab">
      <div className="diff-tab-header">
        <FileGlyph />
        <span className="diff-tab-name" title={path}>
          {base}
        </span>
        {dir !== "" ? <span className="diff-tab-dir">{dir}</span> : null}
        <span className="diff-tab-stats">{stats}</span>
        {diff.failure !== null && reply !== null ? (
          // Polite: announced when the line appears or its reason changes.
          // Repeats stay silent — an identical failure schedules no render.
          <span className="diff-tab-refresh-failure" role="status">
            Couldn&apos;t refresh: {diff.failure.sentence}
          </span>
        ) : null}
        <div className="diff-tab-seg" role="group" aria-label="Diff layout">
          {(["unified", "split"] as const).map((candidate) => (
            <button
              key={candidate}
              type="button"
              className={`diff-tab-seg-button${
                mode === candidate ? " diff-tab-seg-button-is-on" : ""
              }`}
              aria-pressed={mode === candidate}
              onClick={() => chooseMode(candidate)}
            >
              {candidate === "unified" ? "Unified" : "Split"}
            </button>
          ))}
        </div>
      </div>
      <div className="diff-tab-body">
        {reply === null ? (
          diff.failure !== null ? (
            <div className="diff-tab-note diff-tab-note-error" role="alert">
              <ErrorText
                sentence={diff.failure.sentence}
                detail={diff.failure.detail}
                id={errorId}
              />
            </div>
          ) : (
            <div className="diff-tab-note" role="status">
              Loading diff…
            </div>
          )
        ) : reply.status === "binary" ? (
          <div className="diff-tab-note">This file is binary; there are no lines to show.</div>
        ) : reply.error !== null ? (
          <div className="diff-tab-note diff-tab-note-error" role="alert">
            {reply.error}
          </div>
        ) : reply.lines.length === 0 ? (
          <div className="diff-tab-note">This file has no uncommitted line changes.</div>
        ) : mode === "unified" ? (
          <UnifiedRows lines={numbered ?? []} />
        ) : (
          <SplitRows rows={split ?? []} />
        )}
      </div>
    </div>
  );
}
