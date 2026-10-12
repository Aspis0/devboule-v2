// What the who-is-working line and its list read for one parent: the children
// it created, read from the roster, and the provider's own task rows, kept
// apart by kind.
import type {
  AgentSubagent,
  AgentSubagentStatus,
  AgentSubagentStatusCounts,
} from "../../lib/agentSession";
import type {
  AgentActivityState,
  Session,
  SessionKind,
  SessionState,
  SessionTask,
} from "../../types/ipc";
import type { BackgroundTaskState } from "../../lib/backgroundTasks";
import { sessionTitle } from "./workspaceSessions";

export interface SubagentRow {
  /** A row's identity is kind and id: a task id and a session id never name the same thing. */
  kind: "child" | "task";
  id: string;
  title: string | null;
  status: AgentSubagentStatus;
  /** The roster generation a child had; a task has none. */
  generation: number | null;
  /** The provider a child runs under, for its mark; a task has none. */
  provider?: SessionKind;
  /** When a child's running turn began, from the roster. */
  turnStartedAtMs?: number;
}

type RosterChild = Pick<
  Session,
  "id" | "kind" | "title" | "displayName" | "createdBy" | "activity" | "turnStartedAtMs"
> & { state?: SessionState };

// Paseo's finished set for the same action: completed | failed | canceled.
const ARCHIVABLE_STATUSES: ReadonlySet<AgentSubagentStatus> = new Set([
  "finished",
  "failed",
  "stopped",
]);

/** A child's exit that ended its process cleanly with a non-zero code: the shell
 * convention range up to 123. 124 and up are timeouts, signals and shell errors. */
function isCleanExit(code: number): boolean {
  return code >= 1 && code <= 123;
}

// `state` is the process, `activity` is the turn: a live or silent process is
// finished only when the daemon says its turn is idle, never because it is up.
// An ended process has no turn left to read, so its exit decides, and a stale
// turn status cannot outrank it.
function childStatus(
  state: SessionState | undefined,
  activity: AgentActivityState | undefined,
  taskState: SessionTask["state"] | undefined,
): AgentSubagentStatus {
  if (state?.type === "ended") {
    // The roster reads a killed child's exit as a crash; the task list knows it was stopped.
    if (taskState === "cancelled") return "stopped";
    if (taskState === "failed") return "failed";
    // An exit code alone is no verdict. Exit 0 finished; a clean non-zero exit
    // closed without a verdict; no code, a signal, or a timeout-range code is a crash.
    if (state.code === 0) return "finished";
    return state.code !== null && isCleanExit(state.code) ? "stopped" : "failed";
  }
  if (state?.type === "recovered") return "stopped";
  if (state === undefined) return "unknown";
  if (activity === "working" || activity === "blocked") return "running";
  return activity === "idle" ? "finished" : "unknown";
}

export function childRow(session: RosterChild, taskState?: SessionTask["state"]): SubagentRow {
  return {
    kind: "child",
    id: session.id,
    title: sessionTitle(session),
    status: childStatus(session.state, session.activity, taskState),
    generation: session.state?.generation ?? null,
    provider: session.kind,
    turnStartedAtMs: session.turnStartedAtMs,
  };
}

/** The task list's state for each child session it names; a task with no child session has none. */
export function childTaskStates(
  list: BackgroundTaskState | null,
): ReadonlyMap<string, SessionTask["state"]> {
  const states = new Map<string, SessionTask["state"]>();
  for (const task of list?.tasks ?? []) {
    if (task.childSessionId !== undefined) states.set(task.childSessionId, task.state);
  }
  return states;
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
  /** The daemon's task-list state per child session, when the list has one. */
  taskStates?: ReadonlyMap<string, SessionTask["state"]>,
): SubagentRow[] {
  const children = (roster ?? []).filter(
    (session) => session.createdBy === parentId && session.id !== parentId,
  );
  return [
    ...children.map((session) => childRow(session, taskStates?.get(session.id))),
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
