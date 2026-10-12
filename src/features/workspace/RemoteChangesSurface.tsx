import { memo, useMemo, useState } from "react";
import { ErrorText } from "../../components/ErrorText";
import type { WorkspaceKey } from "./hosts/hostIdentity";
import { useWorkspaceDaemon } from "./workspaceDaemon";
import { REMOTE_PANELS_MIN_DIALECT } from "./RemoteFilesSurface";
import { StaleDaemonNote } from "./WorkspaceFileTab";
import { ChangesTreeView } from "./ChangesTreeView";
import { useRemoteWorkspaceChanges } from "./useRemoteWorkspaceChanges";
import { AgentChanges } from "./agentChanges/AgentChanges";
import { remoteAgentChangeReads } from "./agentChanges/agentChangeReads";
import "./panel/changes.css";

/** Oldest local daemon dialect that takes the paired-host agent-edit requests.
 * Mirrors `REMOTE_AGENT_EDIT_READS_MIN_VERSION` in `devboule-protocol`: a rebase
 * that moves that number moves this one too. */
export const REMOTE_AGENT_EDIT_READS_MIN_DIALECT = 43;

/**
 * The Changes panel for a paired host's workspace: the host's own
 * working-tree status over the held peer link, read-only. A row click
 * opens the file tab (which edits over the same link) instead of a
 * diff — no stage, unstage, discard, commit or history rides this road.
 * A refusal keeps the remote's own sentence.
 */
export const RemoteChangesSurface = memo(function RemoteChangesSurface({
  workspaceKey,
  deviceId,
  workspaceId,
  onOpenFile,
}: {
  workspaceKey: WorkspaceKey;
  deviceId: string;
  workspaceId: string;
  onOpenFile?: (workspaceKey: WorkspaceKey, path: string) => void;
}) {
  const daemon = useWorkspaceDaemon();
  const version = daemon.protocolVersion;
  const known = version !== null;
  const stale = version !== null && version < REMOTE_PANELS_MIN_DIALECT;
  const gated = !known || stale;
  const { reply, failure, refresh } = useRemoteWorkspaceChanges(
    gated ? null : deviceId,
    gated ? null : workspaceId,
  );
  const [selection, setSelection] = useState<string | null>(null);
  // A local daemon below the dialect cannot send the agent-edit requests.
  const agentDialect = version !== null && version >= REMOTE_AGENT_EDIT_READS_MIN_DIALECT;
  const agentReads = useMemo(
    () => remoteAgentChangeReads(deviceId, workspaceId),
    [deviceId, workspaceId],
  );

  const openPath = (path: string): void => {
    setSelection(path);
    if (onOpenFile !== undefined) onOpenFile(workspaceKey, path);
  };

  if (gated) {
    return (
      <div tabIndex={-1} role="region" aria-label="Changes" className="workspace-changes">
        {stale ? (
          <StaleDaemonNote what="see this workspace" />
        ) : (
          <div className="workspace-changes-state" role="status">
            Loading changes…
          </div>
        )}
      </div>
    );
  }

  return (
    <div tabIndex={-1} role="region" aria-label="Changes" className="workspace-changes">
      <div>
        <button
          type="button"
          className="workspace-changes-refresh"
          aria-label="Refresh"
          title="Refresh"
          onClick={() => refresh()}
        >
          <svg width="12" height="12" viewBox="0 0 24 24" fill="none" aria-hidden="true">
            <path
              d="M21 12a9 9 0 1 1-2.64-6.36"
              stroke="currentColor"
              strokeWidth="2"
              strokeLinecap="round"
            />
            <path
              d="M21 3v6h-6"
              stroke="currentColor"
              strokeWidth="2"
              strokeLinecap="round"
              strokeLinejoin="round"
            />
          </svg>
        </button>
      </div>
      {failure !== null ? (
        <div className="workspace-changes-error" role="alert">
          <ErrorText
            sentence={failure.sentence}
            detail={failure.detail}
            id="changes-status-error"
          />
        </div>
      ) : null}
      {reply === null ? (
        failure === null ? (
          <div className="workspace-changes-state" role="status">
            Loading changes…
          </div>
        ) : null
      ) : !reply.isGit ? (
        // A folder with no repository on the host: its agents' edits, read from the
        // host's journal, stand in for a clean tree, which this folder never has.
        <>
          {reply.gitMissing === true ? (
            <div className="workspace-changes-state">
              git is not installed: install Git for Windows, or put git on PATH.
            </div>
          ) : null}
          {reply.error === null ? (
            agentDialect ? (
              <AgentChanges reads={agentReads} onRefresh={refresh} />
            ) : (
              <StaleDaemonNote what="see this folder's agent edits" />
            )
          ) : (
            <div className="workspace-changes-error" role="alert">
              {reply.error}
            </div>
          )}
        </>
      ) : reply.rows.length === 0 ? (
        <div className="workspace-changes-state">No uncommitted changes.</div>
      ) : (
        <ChangesTreeView
          rows={reply.rows}
          inexact={false}
          selection={selection}
          onSelect={openPath}
          onStage={() => undefined}
          onUnstage={() => undefined}
          onDiscard={() => undefined}
          menuPath={null}
          onToggleMenu={() => undefined}
          onCloseMenu={() => undefined}
          acting={false}
          onOpenFile={onOpenFile}
          workspaceKey={workspaceKey}
          readOnly
        />
      )}
    </div>
  );
});
