import {
  remoteHostAgentDiff,
  remoteHostAgentEdits,
  workspaceAgentDiff,
  workspaceAgentEdits,
} from "../../../lib/tauri";
import { parseWorkspaceKey, type WorkspaceKey } from "../hosts/hostIdentity";
import type { WorkspaceAgentEdits, WorkspaceAgentFileDiff } from "../../../types/ipc";

/**
 * Where the agent-edit reads come from: a folder on this machine, or a folder on
 * a paired host over the held link. `id` names the source, so a reply read for
 * one source is never shown under another.
 */
export interface AgentChangeReads {
  id: string;
  edits: () => Promise<WorkspaceAgentEdits>;
  diff: (path: string) => Promise<WorkspaceAgentFileDiff>;
}

export function localAgentChangeReads(workspaceKey: WorkspaceKey): AgentChangeReads {
  const workspaceId = parseWorkspaceKey(workspaceKey).workspaceId;
  return {
    id: `local\u0000${workspaceKey}`,
    edits: () => workspaceAgentEdits(workspaceId),
    diff: (path) => workspaceAgentDiff(workspaceId, path),
  };
}

export function remoteAgentChangeReads(deviceId: string, workspaceId: string): AgentChangeReads {
  return {
    id: `remote\u0000${deviceId}\u0000${workspaceId}`,
    edits: () => remoteHostAgentEdits(deviceId, workspaceId),
    diff: (path) => remoteHostAgentDiff(deviceId, workspaceId, path),
  };
}
