// A task list a test sets by hand, with the lane a session controller has.
import type { BackgroundTaskState } from "../../lib/backgroundTasks";
import type { BackgroundTaskSource } from "./useBackgroundTaskState";

export interface FakeTaskSource extends BackgroundTaskSource {
  set(list: BackgroundTaskState | null): void;
}

export function fakeTaskSource(list: BackgroundTaskState | null = null): FakeTaskSource {
  let state = list;
  const listeners = new Set<() => void>();
  return {
    getTaskState: () => state,
    subscribeTasks(listener) {
      listeners.add(listener);
      return () => {
        listeners.delete(listener);
      };
    },
    set(next) {
      state = next;
      for (const listener of [...listeners]) listener();
    },
  };
}
