import { useEffect, useRef, useState } from "react";
import { taskTransitions, type BackgroundTaskState } from "../../lib/backgroundTasks";

export interface PaneTasks {
  sessionId: string;
  list: BackgroundTaskState | null;
}

/** A task that settled on its own: a stop the person asked for is not news. */
function settledOnItsOwn(task: { state: string }): boolean {
  return task.state === "finished" || task.state === "failed";
}

/**
 * Whether the Tasks tab has news to show: a task finished or failed while the
 * tab was out of view, and the tab has not been in view since. The list is
 * compared with the one before it for the same session. The first list is a
 * baseline, not news, so the attach reply never lights the tab.
 */
export function useTasksAttention(paneTasks: PaneTasks | null, tasksVisible: boolean): boolean {
  const [unseen, setUnseen] = useState(false);
  const previous = useRef<PaneTasks | null>(null);
  useEffect(() => {
    const before = previous.current;
    previous.current = paneTasks;
    if (tasksVisible) {
      setUnseen(false);
      return;
    }
    if (paneTasks === null || paneTasks.list === null) return;
    if (before === null || before.sessionId !== paneTasks.sessionId || before.list === null) return;
    if (taskTransitions(before.list, paneTasks.list).some(settledOnItsOwn)) setUnseen(true);
  }, [paneTasks, tasksVisible]);
  return unseen;
}
