import type { Session, SessionKind } from "../../../types/ipc";
import { isAgentKind } from "../../../types/ipc";
import { localWorkspaceKey, type WorkspaceKey } from "../hosts/hostIdentity";
import { sessionAttentionLabel, sessionNeedsApproval } from "../sessionAttention";
import { sessionTitle } from "../workspaceSessions";
import { compactAge } from "./compactAge";

/** One agent as the rail lists it under its workspace. */
export interface AgentRowView {
  id: string;
  kind: SessionKind;
  title: string;
  /** What it is doing, in a word: an ask for the person first, then its state. */
  word: string;
  /** True while the agent is stopped on the person's approval. */
  attention: boolean;
  /** Whether a turn is running: the row may then say the step it is on. */
  working: boolean;
  /** The roster has heard nothing from it for a while, though it may be working. */
  quiet: boolean;
  /** How long it has been quiet; null when the roster reports no silence. */
  age: string | null;
}

function wordFor(session: Session): string {
  const asked = sessionAttentionLabel(session);
  if (asked !== null) return asked;
  if (session.state.type === "recovered") return "recovered";
  if (session.state.type === "silent") return "quiet";
  if (session.activity === "working") return "working";
  if (session.activity === "blocked") return "blocked";
  return "idle";
}

/**
 * The top-level agents of each workspace, in roster order. A subagent (one an
 * agent created) is reached from its creator's pill, not listed here, and an
 * ended session belongs to History.
 */
export function buildAgentRows(
  sessions: readonly Session[],
): ReadonlyMap<WorkspaceKey, readonly AgentRowView[]> {
  const byWorkspace = new Map<WorkspaceKey, AgentRowView[]>();
  for (const session of sessions) {
    if (!isAgentKind(session.kind) || session.createdBy !== undefined) continue;
    if (session.state.type === "ended" || session.workspaceId === null) continue;
    const key = localWorkspaceKey(session.workspaceId);
    if (key === null) continue;
    const row: AgentRowView = {
      id: session.id,
      kind: session.kind,
      title: sessionTitle(session),
      word: wordFor(session),
      attention: sessionNeedsApproval(session),
      working: session.activity === "working",
      quiet: session.state.type === "silent",
      age: compactAge(session.elapsedMs ?? null),
    };
    const list = byWorkspace.get(key);
    if (list === undefined) byWorkspace.set(key, [row]);
    else list.push(row);
  }
  return byWorkspace;
}
