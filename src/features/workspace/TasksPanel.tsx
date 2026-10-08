// The Tasks tab: the selected agent session's background tasks, one line each.
// It shows what the daemon's list says and acts only through the callbacks the
// workspace hands it.
import { memo, useCallback, useMemo, useRef, useState } from "react";
import { useConfirmAsk } from "../../components/ConfirmHost";
import { formatTaskDuration, taskStateWord, toolCountText } from "../../lib/backgroundTaskText";
import type { SessionTask } from "../../types/ipc";
import { groupTasks } from "./backgroundTaskGroups";
import type { AgentTasksContext } from "./sidePanelRegistry";
import { useBackgroundTaskState } from "./useBackgroundTaskState";
import { useTaskClock } from "./useTaskClock";
import "./TasksPanel.css";

const STOP_MESSAGE = "The agent stops where it is. Its transcript stays in History.";

interface TasksPanelProps {
  tasks: AgentTasksContext | null;
}

export function TasksPanel({ tasks }: TasksPanelProps) {
  const askConfirm = useConfirmAsk();
  const [stopSentence, setStopSentence] = useState<string | null>(null);
  // The ref refuses a second press before React has drawn the disabled state.
  const stoppingRef = useRef(new Set<string>());
  const [stopping, setStopping] = useState<ReadonlySet<string>>(new Set());
  const list = useBackgroundTaskState(tasks?.source ?? null);
  const groups = useMemo(() => groupTasks(list?.tasks ?? []), [list]);
  const now = useTaskClock(groups.running.length > 0);
  const omitted = list?.omitted ?? 0;

  const openAgent = useCallback(
    (childSessionId: string) => tasks?.onOpenAgent(childSessionId),
    [tasks],
  );

  const stop = useCallback(
    async (task: SessionTask): Promise<void> => {
      const childSessionId = task.childSessionId;
      if (tasks === null || childSessionId === undefined) return;
      if (stoppingRef.current.has(childSessionId)) return;
      stoppingRef.current.add(childSessionId);
      setStopping(new Set(stoppingRef.current));
      try {
        const confirmed = await askConfirm({
          title: `Stop ${task.title}?`,
          message: STOP_MESSAGE,
          confirmLabel: "Stop",
          cancelLabel: "Keep it running",
        });
        if (!confirmed) return;
        setStopSentence(null);
        setStopSentence(await tasks.onStopAgent(childSessionId));
      } finally {
        stoppingRef.current.delete(childSessionId);
        setStopping(new Set(stoppingRef.current));
      }
    },
    [askConfirm, tasks],
  );

  const empty = groups.running.length === 0 && groups.finished.length === 0;
  return (
    <div className="tasks-panel" data-testid="tasks-panel">
      {empty && omitted === 0 ? <p className="tasks-panel-empty">No background tasks</p> : null}
      {groups.running.length > 0 ? (
        <TaskGroup
          label="Running"
          tasks={groups.running}
          now={now}
          stopping={stopping}
          onOpen={openAgent}
          onStop={stop}
        />
      ) : null}
      {groups.finished.length > 0 ? (
        <TaskGroup
          label="Finished"
          tasks={groups.finished}
          now={now}
          stopping={stopping}
          onOpen={openAgent}
          onStop={stop}
        />
      ) : null}
      {omitted > 0 ? <p className="tasks-panel-more">+{omitted} more</p> : null}
      {stopSentence === null ? null : (
        <p className="tasks-panel-error" role="alert">
          {stopSentence}
        </p>
      )}
    </div>
  );
}

interface TaskGroupProps {
  label: string;
  tasks: SessionTask[];
  now: number;
  stopping: ReadonlySet<string>;
  onOpen: (childSessionId: string) => void;
  onStop: (task: SessionTask) => Promise<void>;
}

function TaskGroup({ label, tasks, now, stopping, onOpen, onStop }: TaskGroupProps) {
  return (
    <section className="tasks-panel-group" aria-label={label}>
      <h3 className="tasks-panel-heading">{label}</h3>
      <ul className="tasks-panel-list">
        {tasks.map((task) => (
          <TaskRow
            key={task.id}
            task={task}
            // Only a running row reads the clock; a settled one is not re-rendered by the tick.
            now={task.state === "running" ? now : null}
            stopping={task.childSessionId !== undefined && stopping.has(task.childSessionId)}
            onOpen={onOpen}
            onStop={onStop}
          />
        ))}
      </ul>
    </section>
  );
}

interface TaskRowProps {
  task: SessionTask;
  now: number | null;
  stopping: boolean;
  onOpen: (childSessionId: string) => void;
  onStop: (task: SessionTask) => Promise<void>;
}

const TaskRow = memo(function TaskRow({ task, now, stopping, onOpen, onStop }: TaskRowProps) {
  const childSessionId = task.kind === "agent" ? task.childSessionId : undefined;
  const duration =
    now !== null
      ? formatTaskDuration(now - task.startedAtMs)
      : task.endedAtMs === undefined
        ? null
        : formatTaskDuration(task.endedAtMs - task.startedAtMs);
  const details = [
    task.model,
    task.toolCallCount === undefined ? undefined : toolCountText(task.toolCallCount),
  ].filter((detail): detail is string => detail !== undefined);
  return (
    <li className="tasks-panel-row" data-state={task.state}>
      {childSessionId === undefined ? (
        <span className="tasks-panel-title">{task.title}</span>
      ) : (
        <button
          type="button"
          className="tasks-panel-title tasks-panel-open"
          onClick={() => onOpen(childSessionId)}
        >
          {task.title}
        </button>
      )}
      <span className="tasks-panel-word">{taskStateWord(task.state)}</span>
      {duration === null ? null : <span className="tasks-panel-meta">{duration}</span>}
      {details.map((detail) => (
        <span key={detail} className="tasks-panel-meta">
          {detail}
        </span>
      ))}
      {task.state === "running" && childSessionId !== undefined ? (
        <button
          type="button"
          className="tasks-panel-stop"
          aria-label={`Stop ${task.title}`}
          disabled={stopping}
          onClick={() => void onStop(task)}
        >
          Stop
        </button>
      ) : null}
    </li>
  );
});
