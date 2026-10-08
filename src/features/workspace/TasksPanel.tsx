// The Tasks tab: the selected agent session's background tasks, one line each.
// It shows what the daemon's list says and acts only through the callbacks the
// workspace hands it.
import { useState } from "react";
import { useConfirmAsk } from "../../components/ConfirmHost";
import { formatTaskDuration, taskStateWord, toolCountText } from "../../lib/backgroundTaskText";
import type { SessionTask } from "../../types/ipc";
import { groupTasks } from "./backgroundTaskGroups";
import type { AgentTasksContext } from "./sidePanelRegistry";
import { useTaskClock } from "./useTaskClock";
import "./TasksPanel.css";

const STOP_MESSAGE = "The agent stops where it is. Its transcript stays in History.";

interface TasksPanelProps {
  tasks: AgentTasksContext | null;
}

export function TasksPanel({ tasks }: TasksPanelProps) {
  const askConfirm = useConfirmAsk();
  const [stopSentence, setStopSentence] = useState<string | null>(null);
  const list = tasks?.list ?? null;
  const groups = groupTasks(list?.tasks ?? []);
  const now = useTaskClock(groups.running.length > 0);
  const omitted = list?.omitted ?? 0;

  async function stop(task: SessionTask): Promise<void> {
    if (tasks === null || task.childSessionId === undefined) return;
    const confirmed = await askConfirm({
      title: `Stop ${task.title}?`,
      message: STOP_MESSAGE,
      confirmLabel: "Stop",
      cancelLabel: "Keep it running",
    });
    if (!confirmed) return;
    setStopSentence(null);
    setStopSentence(await tasks.onStopAgent(task.childSessionId));
  }

  const empty = groups.running.length === 0 && groups.finished.length === 0;
  return (
    <div className="tasks-panel" data-testid="tasks-panel">
      {empty && omitted === 0 ? <p className="tasks-panel-empty">No background tasks</p> : null}
      {groups.running.length > 0 ? (
        <TaskGroup
          label="Running"
          tasks={groups.running}
          now={now}
          onOpen={(childSessionId) => tasks?.onOpenAgent(childSessionId)}
          onStop={stop}
        />
      ) : null}
      {groups.finished.length > 0 ? (
        <TaskGroup
          label="Finished"
          tasks={groups.finished}
          now={now}
          onOpen={(childSessionId) => tasks?.onOpenAgent(childSessionId)}
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
  onOpen: (childSessionId: string) => void;
  onStop: (task: SessionTask) => Promise<void>;
}

function TaskGroup({ label, tasks, now, onOpen, onStop }: TaskGroupProps) {
  return (
    <section className="tasks-panel-group" aria-label={label}>
      <h3 className="tasks-panel-heading">{label}</h3>
      <ul className="tasks-panel-list">
        {tasks.map((task) => (
          <TaskRow key={task.id} task={task} now={now} onOpen={onOpen} onStop={onStop} />
        ))}
      </ul>
    </section>
  );
}

interface TaskRowProps {
  task: SessionTask;
  now: number;
  onOpen: (childSessionId: string) => void;
  onStop: (task: SessionTask) => Promise<void>;
}

function TaskRow({ task, now, onOpen, onStop }: TaskRowProps) {
  const childSessionId = task.kind === "agent" ? task.childSessionId : undefined;
  const duration =
    task.state === "running"
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
          onClick={() => void onStop(task)}
        >
          Stop
        </button>
      ) : null}
    </li>
  );
}
