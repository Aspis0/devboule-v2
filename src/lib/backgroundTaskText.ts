// The words of a background task's transcript row and of its duration. Both
// are fixed at the moment the row is written: a row is a record of a change.
import type { SessionTask } from "../types/ipc";

/** `Ns` under a minute, `Nm Ns` under an hour, `Nh Nm` after that. */
export function formatTaskDuration(milliseconds: number): string {
  const seconds = Math.max(0, Math.floor(milliseconds / 1000));
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
    parts.push(`took ${formatTaskDuration(task.endedAtMs - task.startedAtMs)}`);
  }
  return parts.join(" · ");
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
