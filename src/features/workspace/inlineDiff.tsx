import type { ReactNode } from "react";
import type { WorkspaceGitDiffLine } from "../../types/ipc";
import { splitPath } from "./agentChanges/agentChangesView";
import { DIFF_LINE_MARKER, DIFF_ROW_WORD } from "./diffMarker";
import "./inlineDiff.css";

/**
 * A file's diff under a Changes list: a hairline, a two-line head, then the
 * rows. No card. The number column stays blank when the rows carry no numbers.
 */
export function InlineDiff({
  path,
  counts,
  meta,
  children,
}: {
  path: string;
  counts: ReactNode;
  /** The head's second line; null when there is no author to name. */
  meta: string | null;
  children: ReactNode;
}) {
  const { name, dir } = splitPath(path);
  return (
    <div className="workspace-changes-diff">
      <div className="workspace-changes-diff-head">
        <div className="workspace-changes-diff-title" title={path}>
          <span className="workspace-changes-diff-name">{name}</span>
          <span className="workspace-changes-diff-dir">{dir}</span>
          {counts}
        </div>
        {meta === null ? null : <p className="workspace-changes-diff-meta">{meta}</p>}
      </div>
      <div className="workspace-changes-diff-body">{children}</div>
    </div>
  );
}

export function DiffRows({ lines }: { lines: readonly WorkspaceGitDiffLine[] }) {
  return lines.map((line, index) =>
    line.kind === "header" ? (
      <div className="workspace-changes-diff-hunk" key={index}>
        {line.text}
      </div>
    ) : (
      <div
        className={`workspace-changes-diff-row workspace-changes-diff-row-${line.kind}`}
        key={index}
      >
        <span className="workspace-changes-diff-num" aria-hidden="true" />
        <span className="workspace-changes-diff-mark" aria-hidden="true">
          {DIFF_LINE_MARKER[line.kind]}
        </span>
        {DIFF_ROW_WORD[line.kind] !== null ? (
          <span className="sr-only">{DIFF_ROW_WORD[line.kind]}</span>
        ) : null}
        <span className="workspace-changes-diff-text">{line.text}</span>
      </div>
    ),
  );
}

export function DiffNote({
  children,
  alert = false,
}: {
  children: ReactNode;
  /** A refusal, announced as one. */
  alert?: boolean;
}) {
  return (
    <div
      className={`workspace-changes-diff-note${alert ? " workspace-changes-diff-note-error" : ""}`}
      role={alert ? "alert" : undefined}
    >
      {children}
    </div>
  );
}
