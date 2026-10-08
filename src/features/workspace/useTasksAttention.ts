import {
  registeredSession,
  tasksNews,
  subscribeTasksNews,
  setTasksVisible,
} from "../../lib/agentSessionRegistry";
import {
  useCallback,
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
  useSyncExternalStore,
} from "react";
import { taskTransitions } from "../../lib/backgroundTasks";
import { lastSeenTaskState, rememberTaskState } from "./taskStateMemory";
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
 * with the list before it. The first comparison is against the list this session
 * last showed, so a finish that happened while the chat was unmounted still
 * counts, whether the fresh controller already holds its list at mount or the
 * attach reply brings it later. The news is kept with the source it came from,
 * and it is retired during render once the tab is in view or the source changes,
 * so no effect has to set state.
 *
 * A registered chat's news lives in the registry, whose one observer keeps it
 * while the chat is unmounted; this hook observes only the other sources, so
 * one news item is never acknowledged by two observers.
 */
export function useTasksAttention(
  pane: { sessionId: string; source: BackgroundTaskSource } | null,
  visible: boolean,
): boolean {
  const source = pane?.source ?? null;
  const sessionId = pane?.sessionId ?? null;
  const registered = registeredSession(source);
  const subscribe = useCallback(
    (listener: () => void) => subscribeTasksNews(source, listener),
    [source],
  );
  const retainedNews = useSyncExternalStore(subscribe, () => tasksNews(source));
  useLayoutEffect(() => {
    setTasksVisible(source, visible);
    return () => setTasksVisible(source, false);
  }, [source, visible]);
  const [news, setNews] = useState<News>({ source: null, unseen: false });
  if (news.unseen && (visible || news.source !== source)) setNews({ source, unseen: false });
  const visibleRef = useRef(visible);
  useEffect(() => {
    visibleRef.current = visible;
  }, [visible]);
  useEffect(() => {
    if (source === null || sessionId === null || registered) return undefined;
    // Read before the first observation writes over it.
    let previous = lastSeenTaskState(sessionId);
    const observe = () => {
      const next = source.getTaskState();
      if (next === null) return;
      rememberTaskState(sessionId, next);
      const settled = taskTransitions(previous, next).some(settledOnItsOwn);
      previous = next;
      if (settled && !visibleRef.current) setNews({ source, unseen: true });
    };
    observe();
    return source.subscribeTasks(observe);
  }, [source, sessionId, registered]);
  return (registered ? retainedNews : news.unseen && news.source === source) && !visible;
}
