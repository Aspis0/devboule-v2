// What the one "who is working" line above the composer says: the open agent's
// children that wait, work or failed, and its running commands. Pure: the line
// and the list it opens read the same rows and the same clocks.
import type { SessionTask } from "../../types/ipc";
import type { SubagentRow } from "./subagentRows";

export type LineSegment =
  | { tone: "attention"; name: string; word: string }
  | {
      tone: "live";
      name: string;
      word: string;
      /** The longest running turn's clock; absent while someone else's wait is up. */
      clock: string | null;
      /** Commands run beside the agents, as `1 command`. */
      commands: string | null;
    }
  | { tone: "danger"; name: string; word: string };

export interface WhoIsWorking {
  segments: LineSegment[];
  /** Only commands run: the line opens the Tasks tab rather than the list. */
  commandsOnly: boolean;
}

/** When each running turn began, read from the open agent's task list. */
export interface TaskClocks {
  agentStarts: ReadonlyMap<string, number>;
  commandStarts: readonly number[];
}

export interface WhoIsWorkingInput {
  rows: readonly SubagentRow[];
  /** Children waiting on the person. */
  waitingIds: ReadonlySet<string>;
  /** Failed children whose tab has opened: the line stops naming them. */
  seenFailedIds: ReadonlySet<string>;
  clocks: TaskClocks;
  now: number;
}

/** `mm:ss`, and `h:mm:ss` from an hour on. */
export function lineClock(elapsedMs: number): string {
  const total = Math.max(0, Math.floor(elapsedMs / 1000));
  const hours = Math.floor(total / 3600);
  const minutes = String(Math.floor((total % 3600) / 60)).padStart(2, "0");
  const seconds = String(total % 60).padStart(2, "0");
  return hours > 0 ? `${hours}:${minutes}:${seconds}` : `${minutes}:${seconds}`;
}

function shortId(id: string): string {
  return id.length > 16 ? `${id.slice(0, 12)}…` : id;
}

/** A row's name: its title, or its id shortened when it has none. */
export function subagentTitle(title: string | null, id: string): string {
  return title?.trim() ? title : shortId(id);
}

/** Reads the running turns out of a task list once; every row's clock comes from the result. */
export function taskClocks(tasks: readonly SessionTask[]): TaskClocks {
  const agentStarts = new Map<string, number>();
  const commandStarts: number[] = [];
  for (const task of tasks) {
    if (task.state !== "running") continue;
    if (task.kind === "agent") agentStarts.set(task.childSessionId ?? task.id, task.startedAtMs);
    else commandStarts.push(task.startedAtMs);
  }
  return { agentStarts, commandStarts };
}

/** The clock a row shows: how long its running turn has gone on, when the task list knows its start. */
export function rowClock(clocks: TaskClocks, row: SubagentRow, now: number): string | null {
  const start = clocks.agentStarts.get(row.id);
  return start === undefined ? null : lineClock(now - start);
}

function commandsText(count: number): string {
  return `${count} ${count === 1 ? "command" : "commands"}`;
}

/** One row's name, or the count of them when several share a state. */
function nameOf(rows: readonly SubagentRow[]): string {
  const [only] = rows;
  return rows.length === 1 && only !== undefined
    ? subagentTitle(only.title, only.id)
    : `${rows.length} agents`;
}

/**
 * The line's segments in the order the person needs them: a wait, then the
 * work, then an unseen failure. None means the line is not drawn.
 */
export function whoIsWorking({
  rows,
  waitingIds,
  seenFailedIds,
  clocks,
  now,
}: WhoIsWorkingInput): WhoIsWorking {
  const waiting: SubagentRow[] = [];
  const running: SubagentRow[] = [];
  const failed: SubagentRow[] = [];
  for (const row of rows) {
    if (row.kind === "child" && waitingIds.has(row.id)) waiting.push(row);
    else if (row.status === "running") running.push(row);
    else if (row.kind === "child" && row.status === "failed" && !seenFailedIds.has(row.id)) {
      failed.push(row);
    }
  }
  const commandCount = clocks.commandStarts.length;
  const runningStarts = running.flatMap((row) => {
    const start = clocks.agentStarts.get(row.id);
    return start === undefined ? [] : [start];
  });

  const segments: LineSegment[] = [];
  if (waiting.length > 0) {
    const word = waiting.length === 1 ? "needs you" : "need you";
    segments.push({ tone: "attention", name: nameOf(waiting), word });
  }
  if (running.length > 0) {
    segments.push({
      tone: "live",
      name: nameOf(running),
      word: "working",
      clock:
        waiting.length === 0 && runningStarts.length > 0
          ? lineClock(now - Math.min(...runningStarts))
          : null,
      commands: commandCount > 0 ? commandsText(commandCount) : null,
    });
  } else if (commandCount > 0) {
    segments.push({
      tone: "live",
      name: commandsText(commandCount),
      word: "running",
      clock: lineClock(now - Math.min(...clocks.commandStarts)),
      commands: null,
    });
  }
  if (failed.length > 0) {
    segments.push({ tone: "danger", name: nameOf(failed), word: "failed" });
  }
  const commandsOnly =
    waiting.length === 0 && running.length === 0 && failed.length === 0 && commandCount > 0;
  return { segments, commandsOnly };
}
