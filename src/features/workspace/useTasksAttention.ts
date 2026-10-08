import { useEffect, useRef, useState } from "react";
import { taskTransitions } from "../../lib/backgroundTasks";
import type { BackgroundTaskSource } from "./useBackgroundTaskState";

/** A task that settled on its own: a stop the person asked for is not news. */
function settledOnItsOwn(task: { state: string }): boolean {
  return task.state === "finished" || task.state === "failed";
}

interface News {
  source: BackgroundTaskSource | null;
  unseen: boolean;
}

/**
 * Whether the Tasks tab has news: a task finished or failed while the tab was
 * out of view, and the tab has not been in view since. Each change is compared
 * with the list before it, and the list the source held at subscribe time is the
 * baseline, so the attach reply never lights the tab. The news is kept with the
 * source it came from, and it is retired during render once the tab is in view or
 * the source changes, so no effect has to set state.
 */
export function useTasksAttention(source: BackgroundTaskSource | null, visible: boolean): boolean {
  const [news, setNews] = useState<News>({ source: null, unseen: false });
  if (news.unseen && (visible || news.source !== source)) setNews({ source, unseen: false });
  const visibleRef = useRef(visible);
  useEffect(() => {
    visibleRef.current = visible;
  }, [visible]);
  useEffect(() => {
    if (source === null) return undefined;
    let previous = source.getTaskState();
    return source.subscribeTasks(() => {
      const next = source.getTaskState();
      const settled = next !== null && taskTransitions(previous, next).some(settledOnItsOwn);
      previous = next;
      if (settled && !visibleRef.current) setNews({ source, unseen: true });
    });
  }, [source]);
  return news.unseen && news.source === source && !visible;
}
