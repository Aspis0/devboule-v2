// The app's copy of one session's background-task list: which daemon snapshot
// is news, and which tasks changed state between two lists.
import type { SessionTask, SessionTaskList } from "../types/ipc";

const KNOWN_KINDS: ReadonlySet<string> = new Set(["agent", "command"]);
const KNOWN_STATES: ReadonlySet<string> = new Set(["running", "finished", "failed", "cancelled"]);

/**
 * The tasks this build can name. A newer daemon may send a kind or a state this
 * build has no word for; such a task is dropped here, so it gets neither a row
 * nor a state word that would read "undefined".
 */
function knownTasks(tasks: readonly SessionTask[]): SessionTask[] {
  return tasks.filter((task) => KNOWN_KINDS.has(task.kind) && KNOWN_STATES.has(task.state));
}

export interface BackgroundTaskState {
  /** The daemon process that published the list. Null for an attach reply, which carries none. */
  epoch: string | null;
  /** Counts from 1 within an epoch; 0 for an attach reply. */
  revision: number;
  tasks: SessionTask[];
  omitted: number;
}

/**
 * A `tasks_snapshot` event, applied unless it is no newer than the stored list
 * of the same epoch. A new epoch is a daemon that restarted, so its list wins.
 */
export function acceptTaskSnapshot(
  current: BackgroundTaskState | null,
  snapshot: { epoch: string; revision: number; tasks: SessionTask[]; omitted: number },
): BackgroundTaskState | null {
  if (
    current !== null &&
    current.epoch === snapshot.epoch &&
    snapshot.revision <= current.revision
  ) {
    return null;
  }
  return {
    epoch: snapshot.epoch,
    revision: snapshot.revision,
    tasks: knownTasks(snapshot.tasks),
    omitted: snapshot.omitted,
  };
}

/**
 * The attach reply, applied only while nothing is stored. Every snapshot event
 * is newer than the reply, so a reply that arrives after one is dropped.
 */
export function acceptTaskReply(
  current: BackgroundTaskState | null,
  reply: SessionTaskList,
  epoch: string | null,
): BackgroundTaskState | null {
  if (current !== null) return null;
  return { epoch, revision: 0, tasks: knownTasks(reply.tasks), omitted: reply.omitted };
}

/**
 * The tasks of `next` whose (id, state) pair the previous list did not hold:
 * each is a transition worth a row. With no previous list the first one is a
 * baseline and shows nothing: a task already running when the view attached is
 * not a change seen live. A new epoch is also a baseline, since a restarted
 * daemon's list is not news, and so is a list whose epoch is unknown when the
 * previous one had a known epoch: that cannot be told apart from a restart.
 */
export function taskTransitions(
  prev: BackgroundTaskState | null,
  next: BackgroundTaskState,
): SessionTask[] {
  if (prev === null) return [];
  if (prev.epoch !== null && prev.epoch !== next.epoch) return [];
  const held = new Set(prev.tasks.map((task) => `${task.id}\n${task.state}`));
  return next.tasks.filter((task) => !held.has(`${task.id}\n${task.state}`));
}

/** The shown rows in the running state; rows past the publish cap are not known here. */
export function runningTaskCount(list: BackgroundTaskState | null): number {
  return list?.tasks.filter((task) => task.state === "running").length ?? 0;
}
