// The Changes panel of a folder that is not a git repository: the files this
// workspace's agents edited, their totals, and the chosen file's diff.
import { ErrorText } from "../../../components/ErrorText";
import type { WorkspaceAgentEditFile } from "../../../types/ipc";
import type { WorkspaceKey } from "../hosts/hostIdentity";
import { ChangeCounts } from "../ChangeCounts";
import { FileIcon } from "../ChangesTreeView";
import { RefreshButton } from "../RefreshButton";
import { DiffNote, DiffRows, InlineDiff } from "../inlineDiff";
import { useRevealedPath, type RevealRequest } from "../useRevealedPath";
import { useTaskClock } from "../useTaskClock";
import { editedAgo, filesText, splitPath } from "./agentChangesView";
import { useAgentChanges } from "./useAgentChanges";
import "./agentChanges.css";

export const AGENT_CHANGES_NOTE = "Not a git folder — showing what agents edited here.";

interface AgentChangesProps {
  workspaceKey: WorkspaceKey;
  /** The panel's own status read: a folder can become a repository while this panel is open. */
  onRefresh: () => void;
  /** A file a transcript row asked to show. */
  revealPath?: RevealRequest | null;
}

export function AgentChanges({ workspaceKey, onRefresh, revealPath }: AgentChangesProps) {
  const { edits, files, selection, select, diff, refresh } = useAgentChanges(workspaceKey, true);
  useRevealedPath(revealPath, select);
  const refreshAll = () => {
    onRefresh();
    refresh();
  };
  // The clock the "N min ago" lines count from; it ticks with the panel's own re-renders.
  const now = useTaskClock(true);
  const reply = edits.reply;
  const selected = files.find((file) => file.path === selection) ?? null;

  if (reply === null) {
    return edits.failure === null ? (
      <div className="workspace-changes-state" role="status">
        Loading changes…
      </div>
    ) : (
      <div className="workspace-changes-error" role="alert">
        <ErrorText
          sentence={edits.failure.sentence}
          detail={edits.failure.detail}
          id="agent-changes-error"
        />
      </div>
    );
  }

  return (
    <div className="workspace-agent-changes">
      <div className="workspace-agent-changes-head">
        <span>{files.length === 0 ? "No changes by agents yet" : filesText(files.length)}</span>
        {files.length === 0 ? null : (
          <span className="workspace-agent-changes-total">
            <ChangeCounts additions={reply.totals.additions} deletions={reply.totals.deletions} />
          </span>
        )}
        <RefreshButton onRefresh={refreshAll} iconSize={14} />
      </div>
      {reply.error === null ? null : <DiffNote alert>{reply.error}</DiffNote>}
      {edits.failure === null ? null : (
        <div className="workspace-changes-error" role="alert">
          <ErrorText
            sentence={edits.failure.sentence}
            detail={edits.failure.detail}
            id="agent-changes-error"
          />
        </div>
      )}
      {files.length === 0 ? null : (
        <ul className="workspace-agent-changes-list">
          {files.map((file) => {
            const { name, dir } = splitPath(file.path);
            const isSelected = file.path === selection;
            return (
              <li
                key={file.path}
                className={`workspace-agent-changes-row${isSelected ? " is-selected" : ""}`}
              >
                <button
                  type="button"
                  className="workspace-agent-changes-select"
                  aria-current={isSelected ? "true" : undefined}
                  title={file.path}
                  onClick={() => select(file.path)}
                >
                  {FileIcon}
                  <span className="workspace-agent-changes-name">{name}</span>
                  <span className="workspace-agent-changes-dir">{dir}</span>
                </button>
                <ChangeCounts additions={file.additions} deletions={file.deletions} />
              </li>
            );
          })}
        </ul>
      )}
      {selected === null ? null : (
        <InlineDiff
          path={selected.path}
          counts={<ChangeCounts additions={selected.additions} deletions={selected.deletions} />}
          meta={editMeta(selected, now)}
        >
          {diff.reply === null ? (
            diff.failure === null ? (
              <DiffNote>Loading diff…</DiffNote>
            ) : (
              <DiffNote alert>
                <ErrorText
                  sentence={diff.failure.sentence}
                  detail={diff.failure.detail}
                  id="agent-diff-error"
                />
              </DiffNote>
            )
          ) : diff.reply.status === "error" ? (
            <DiffNote alert>{diff.reply.error}</DiffNote>
          ) : (
            <>
              {diff.reply.status === "truncated" ? (
                <DiffNote>Part of this diff is not kept.</DiffNote>
              ) : null}
              <DiffRows lines={diff.reply.lines} />
            </>
          )}
        </InlineDiff>
      )}
      <p className="workspace-agent-changes-note">{AGENT_CHANGES_NOTE}</p>
    </div>
  );
}

/** `cia · 10 min ago · 1 edit`: the author, the newest edit's age, the edit count. */
function editMeta(file: WorkspaceAgentEditFile, now: number): string {
  const edits = file.calls.length === 1 ? "1 edit" : `${file.calls.length} edits`;
  return [file.lastAgent, editedAgo(file.lastAtMs, now), edits]
    .filter((part) => part !== null && part !== "")
    .join(" · ");
}
