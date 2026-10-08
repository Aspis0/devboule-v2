import { useCallback, useSyncExternalStore } from "react";
import type { AgentSession } from "../../lib/agentSession";
import type { BackgroundTaskState } from "../../lib/backgroundTasks";

/** What a consumer of the task list reads: a session controller's task lane. */
export type BackgroundTaskSource = Pick<AgentSession, "getTaskState" | "subscribeTasks">;

const NO_LISTENER = () => undefined;

/** The source's list. The reader re-renders when the list changes, and nothing else. */
export function useBackgroundTaskState(
  source: BackgroundTaskSource | null,
): BackgroundTaskState | null {
  const subscribe = useCallback(
    (listener: () => void) => source?.subscribeTasks(listener) ?? NO_LISTENER,
    [source],
  );
  const read = useCallback(() => source?.getTaskState() ?? null, [source]);
  return useSyncExternalStore(subscribe, read);
}
