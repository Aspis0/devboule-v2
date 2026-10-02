// What one parent's subagent pill lists: the children it created, read from
// the roster, and the provider's own task rows, kept apart by kind.
import type {
  AgentSubagent,
  AgentSubagentStatus,
  AgentSubagentStatusCounts,
} from "../../lib/agentSession";
import type { AgentActivityState, Session, SessionState } from "../../types/ipc";
import { sessionTitle } from "./workspaceSessions";

export interface SubagentRow {
  /** A row's identity is kind and id: a task id and a session id never name the same thing. */
  kind: "child" | "task";
  id: string;
  title: string | null;
  status: AgentSubagentStatus;
  /** The roster generation a child had; a task has none. */
  generation: number | null;
}

type RosterChild = Pick<
  Session,
  "id" | "kind" | "title" | "displayName" | "createdBy" | "activity"
> & { state?: SessionState };

// Paseo's finished set for the same action: completed | failed | canceled.
const ARCHIVABLE_STATUSES: ReadonlySet<AgentSubagentStatus> = new Set([
  "finished",
  "failed",
  "stopped",
]);

// `state` is the process, `activity` is the turn: a live or silent process is
// finished only when the daemon says its turn is idle, never because it is up.
function childStatus(
  state: SessionState | undefined,
  activity: AgentActivityState | undefined,
): AgentSubagentStatus {
  switch (state?.type) {
    case "live":
    case "silent":
      if (activity === "working" || activity === "blocked") return "running";
      return activity === "idle" ? "finished" : "unknown";
    case "ended":
      return state.code === 0 ? "finished" : "failed";
    case "recovered":
      return "stopped";
    case undefined:
      return "unknown";
  }
}

export function childRow(session: RosterChild): SubagentRow {
  return {
    kind: "child",
    id: session.id,
    title: sessionTitle(session),
    status: childStatus(session.state, session.activity),
    generation: session.state?.generation ?? null,
  };
}

/** Only a created child can close: a provider task has no session of its own. */
export function isArchivable(row: Pick<SubagentRow, "kind" | "status">): boolean {
  return row.kind === "child" && ARCHIVABLE_STATUSES.has(row.status);
}

/** The parent's children in roster order, then its provider tasks in theirs. A row
 * naming itself as creator is not its own child: Confirm would close the parent. */
export function deriveSubagentRows(
  parentId: string,
  roster: ReadonlyArray<RosterChild> | undefined,
  tasks: readonly AgentSubagent[],
): SubagentRow[] {
  const children = (roster ?? []).filter(
    (session) => session.createdBy === parentId && session.id !== parentId,
  );
  return [
    ...children.map(childRow),
    ...tasks.map((task): SubagentRow => ({
      kind: "task",
      id: task.id,
      title: task.title,
      status: task.status,
      generation: null,
    })),
  ];
}

export function countSubagentStatuses(rows: readonly SubagentRow[]): AgentSubagentStatusCounts {
  const counts: AgentSubagentStatusCounts = {
    running: 0,
    finished: 0,
    failed: 0,
    stopped: 0,
    unknown: 0,
  };
  for (const row of rows) counts[row.status] += 1;
  return counts;
}
