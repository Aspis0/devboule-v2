import { runningTaskCount } from "../../lib/backgroundTasks";
import "./BackgroundTasksPill.css";
import { useBackgroundTaskState, type BackgroundTaskSource } from "./useBackgroundTaskState";

interface BackgroundTasksPillProps {
  source: BackgroundTaskSource | null;
  /** Opens the side panel's Tasks tab. */
  onOpen: () => void;
}

/** The running-task count above the composer. Nothing runs, nothing is drawn. */
export function BackgroundTasksPill({ source, onOpen }: BackgroundTasksPillProps) {
  const runningCount = runningTaskCount(useBackgroundTaskState(source));
  if (runningCount === 0) return null;
  return (
    <button
      type="button"
      className="background-tasks-pill"
      data-testid="background-tasks-pill"
      onClick={onOpen}
    >
      {runningCount} running {runningCount === 1 ? "task" : "tasks"}
    </button>
  );
}
