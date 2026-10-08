import "./BackgroundTasksPill.css";

interface BackgroundTasksPillProps {
  runningCount: number;
  /** Opens the side panel's Tasks tab. */
  onOpen: () => void;
}

/** The running-task count above the composer. Nothing runs, nothing is drawn. */
export function BackgroundTasksPill({ runningCount, onOpen }: BackgroundTasksPillProps) {
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
