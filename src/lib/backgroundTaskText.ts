// The words of a background task's transcript row and of its duration. Both
// are fixed at the moment the row is written: a row is a record of a change.
import type { SessionTask } from "../types/ipc";

/**
 * `Ns` under a minute, `Nm Ns` under an hour, `Nh Nm` after that. A span that
 * ran backwards (clock skew, or a daemon bug) has no honest duration, so it is
 * null and the caller shows none, rather than a 0s that reads as an instant task.
 */
export function formatTaskDuration(milliseconds: number): string | null {
  if (milliseconds < 0) return null;
  const seconds = Math.floor(milliseconds / 1000);
  if (seconds < 60) return `${seconds}s`;
  const minutes = Math.floor(seconds / 60);
  if (minutes < 60) return `${minutes}m ${seconds % 60}s`;
  return `${Math.floor(minutes / 60)}h ${minutes % 60}m`;
}

const SETTLED_WORD: Record<Exclude<SessionTask["state"], "running">, string> = {
  finished: "finished",
  failed: "failed",
  cancelled: "stopped",
};

/** `1 tool`, `4 tools`: the count a running agent row names. */
export function toolCountText(count: number): string {
  return `${count} ${count === 1 ? "tool" : "tools"}`;
}

/** One transcript row for a task's state. The details a task lacks are left out, not blanked. */
export function taskRowText(task: SessionTask): string {
  if (task.state === "running") {
    if (task.kind === "command") return `Running ${task.title}`;
    const details: string[] = [];
    if (task.model !== undefined) details.push(task.model);
    if (task.toolCallCount !== undefined) details.push(toolCountText(task.toolCallCount));
    return [`Running agent ${task.title}`, ...details].join(" · ");
  }
  const noun = task.kind === "agent" ? "agent" : "command";
  const parts = [`Background ${noun} ${SETTLED_WORD[task.state]}`, task.title];
  if (task.endedAtMs !== undefined) {
    const took = formatTaskDuration(task.endedAtMs - task.startedAtMs);
    if (took !== null) parts.push(`took ${took}`);
  }
  return parts.join(" · ");
}

/**
 * A task's row as facts. The transcript draws the row from these, so it never
 * reads its own words back, and a click knows which child to open.
 */
export interface TaskRowFacts {
  kind: SessionTask["kind"];
  state: SessionTask["state"];
  title: string;
  /** The child session an agent task opens. */
  childSessionId?: string;
  /** How long the task ran, once it has ended. */
  durationMs?: number;
}

export function taskRowFacts(task: SessionTask): TaskRowFacts {
  const durationMs = task.endedAtMs === undefined ? undefined : task.endedAtMs - task.startedAtMs;
  return {
    kind: task.kind,
    state: task.state,
    title: task.title,
    ...(task.childSessionId === undefined ? {} : { childSessionId: task.childSessionId }),
    ...(durationMs === undefined || durationMs < 0 ? {} : { durationMs }),
  };
}

/** A run's duration with its zero unit dropped: `7m`, `7m 12s`, `45s`. */
export function compactTaskDuration(milliseconds: number): string | null {
  return formatTaskDuration(milliseconds)?.replace(/\s0[sm]$/, "") ?? null;
}

/** The quiet line a settled agent task reads as: `Explore auth finished · 7m`. */
export function agentTaskLineText(facts: TaskRowFacts): string {
  if (facts.state === "cancelled") return `${facts.title} stopped`;
  const took = facts.durationMs === undefined ? null : compactTaskDuration(facts.durationMs);
  return took === null
    ? `${facts.title} ${facts.state}`
    : `${facts.title} ${facts.state} · ${took}`;
}

/** The word a task's row carries beside its title. */
export function taskStateWord(state: SessionTask["state"]): string {
  switch (state) {
    case "running":
      return "Running";
    case "finished":
      return "Finished";
    case "failed":
      return "Failed";
    case "cancelled":
      return "Stopped";
  }
}
