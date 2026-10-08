// The Tasks tab's order: the running rows, then the settled ones, each newest
// first. The daemon's list is in publish order, which is not what a person
// scans for.
import type { SessionTask } from "../../types/ipc";

export interface TaskGroups {
  running: SessionTask[];
  finished: SessionTask[];
}

/** When a settled row is judged: its end, or its start for a row the daemon sent without one. */
function settledAt(task: SessionTask): number {
  return task.endedAtMs ?? task.startedAtMs;
}

export function groupTasks(tasks: readonly SessionTask[]): TaskGroups {
  const running = tasks
    .filter((task) => task.state === "running")
    .sort((a, b) => b.startedAtMs - a.startedAtMs);
  const finished = tasks
    .filter((task) => task.state !== "running")
    .sort((a, b) => settledAt(b) - settledAt(a));
  return { running, finished };
}
