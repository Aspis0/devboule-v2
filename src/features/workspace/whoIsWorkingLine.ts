// What the one "who is working" line above the composer says: the open agent's
// children that wait, work or failed, and its running commands. Pure: the line
// and the list it opens read the same rows.
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

export interface WhoIsWorkingInput {
  rows: readonly SubagentRow[];
  /** Children waiting on the person. */
  waitingIds: ReadonlySet<string>;
  /** Failed children whose tab was opened from the list: the line stops naming them. */
  seenFailedIds: ReadonlySet<string>;
  tasks: readonly SessionTask[];
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

/** The start of each running agent turn, by the child session it runs in. */
function agentStarts(tasks: readonly SessionTask[]): ReadonlyMap<string, number> {
  const starts = new Map<string, number>();
  for (const task of tasks) {
    if (task.kind === "agent" && task.state === "running") {
      starts.set(task.childSessionId ?? task.id, task.startedAtMs);
    }
  }
  return starts;
}

/** The clock a row shows: how long its running turn has gone on, when the task list knows its start. */
export function rowClock(
  row: SubagentRow,
  tasks: readonly SessionTask[],
  now: number,
): string | null {
  const start = agentStarts(tasks).get(row.id);
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
 * The line's segments in the order the person needs them: a wait first, then
 * the work, then an unseen failure. Empty when nothing waits, works or failed
 * unseen, so the line is not drawn.
 */
export function whoIsWorkingSegments({
  rows,
  waitingIds,
  seenFailedIds,
  tasks,
  now,
}: WhoIsWorkingInput): LineSegment[] {
  const waiting = rows.filter((row) => row.kind === "child" && waitingIds.has(row.id));
  const running = rows.filter((row) => row.status === "running" && !waiting.includes(row));
  const failed = rows.filter(
    (row) => row.kind === "child" && row.status === "failed" && !seenFailedIds.has(row.id),
  );
  const commands = tasks.filter((task) => task.kind === "command" && task.state === "running");
  const starts = agentStarts(tasks);
  const runningStarts = running.flatMap((row) => {
    const start = starts.get(row.id);
    return start === undefined ? [] : [start];
  });
  const commandStarts = commands.map((task) => task.startedAtMs);

  const segments: LineSegment[] = [];
  if (waiting.length > 0) {
    segments.push({ tone: "attention", name: nameOf(waiting), word: "needs you" });
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
      commands: commands.length > 0 ? commandsText(commands.length) : null,
    });
  } else if (commands.length > 0) {
    segments.push({
      tone: "live",
      name: commandsText(commands.length),
      word: "running",
      clock: lineClock(now - Math.min(...commandStarts)),
      commands: null,
    });
  }
  if (failed.length > 0) {
    segments.push({ tone: "danger", name: nameOf(failed), word: "failed" });
  }
  return segments;
}
