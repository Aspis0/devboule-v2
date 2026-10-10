// The Tasks tab: the selected agent session's background tasks, one line each.
// It shows what the daemon's list says and acts only through the callbacks the
// workspace hands it.
import { memo, useCallback, useMemo, useRef, useState } from "react";
import { useConfirmAsk } from "../../components/ConfirmHost";
import { formatTaskDuration, taskStateWord, toolCountText } from "../../lib/backgroundTaskText";
import type { SessionKind, SessionTask } from "../../types/ipc";
import { groupTasks } from "./backgroundTaskGroups";
import { StripKindMark } from "./strip/StripKindMark";
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
  const [notice, setNotice] = useState<string | null>(null);
  // The ref refuses a second press before React has drawn the disabled state.
  const stoppingRef = useRef(new Set<string>());
  const [stopping, setStopping] = useState<ReadonlySet<string>>(new Set());
  const list = useBackgroundTaskState(tasks?.source ?? null);
  const groups = useMemo(() => groupTasks(list?.tasks ?? []), [list]);
  const now = useTaskClock(groups.running.length > 0);
  const omitted = list?.omitted ?? 0;

  // A child that cannot be opened says why, in the same line a stop reports through.
  const openAgent = useCallback(
    (childSessionId: string) => setNotice(tasks?.onOpenAgent(childSessionId) ?? null),
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
        setNotice(null);
        setNotice(await tasks.onStopAgent(childSessionId));
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
          childInfo={tasks?.childInfo}
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
          childInfo={tasks?.childInfo}
          onOpen={openAgent}
          onStop={stop}
        />
      ) : null}
      {omitted > 0 ? <p className="tasks-panel-more">+{omitted} more</p> : null}
      {notice === null ? null : (
        <p className="tasks-panel-error" role="alert">
          {notice}
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
  childInfo: AgentTasksContext["childInfo"] | undefined;
  onOpen: (childSessionId: string) => void;
  onStop: (task: SessionTask) => Promise<void>;
}

function TaskGroup({ label, tasks, now, stopping, childInfo, onOpen, onStop }: TaskGroupProps) {
  const childOf = (task: SessionTask) =>
    task.childSessionId === undefined ? undefined : childInfo?.(task.childSessionId);
  return (
    <section className="tasks-panel-group" aria-label={label}>
      <h3 className="tasks-panel-heading">{label}</h3>
      <ul className="tasks-panel-list">
        {tasks.map((task) => {
          const child = childOf(task);
          return (
            <TaskRow
              key={task.id}
              task={task}
              groupLabel={label}
              // Only a running row reads the clock; a settled one is not re-rendered by the tick.
              now={task.state === "running" ? now : null}
              stopping={task.childSessionId !== undefined && stopping.has(task.childSessionId)}
              childKind={child?.kind}
              childTitle={child?.title}
              childProvider={child?.provider}
              onOpen={onOpen}
              onStop={onStop}
            />
          );
        })}
      </ul>
    </section>
  );
}

interface TaskRowProps {
  task: SessionTask;
  /** The heading the row sits under: its state word shows only when it adds to that. */
  groupLabel: string;
  now: number | null;
  stopping: boolean;
  /** The child's provider kind and roster title, as scalars so a settled row's memo holds. */
  childKind: SessionKind | undefined;
  childTitle: string | undefined;
  /** The provider id: a row names it on hover, even where no glyph exists for it. */
  childProvider: string | undefined;
  onOpen: (childSessionId: string) => void;
  onStop: (task: SessionTask) => Promise<void>;
}

const UNTITLED_LABEL: Record<SessionTask["kind"], string> = {
  agent: "Untitled subagent",
  command: "Untitled command",
};

/** The mark a row opens with: the child's provider, or the terminal glyph for a command. */
function rowMarkKind(task: SessionTask, childKind: SessionKind | undefined): SessionKind {
  if (task.kind === "command") return "terminal";
  return childKind ?? "acp";
}

const TaskRow = memo(function TaskRow({
  task,
  groupLabel,
  now,
  stopping,
  childKind,
  childTitle,
  childProvider,
  onOpen,
  onStop,
}: TaskRowProps) {
  const childSessionId = task.kind === "agent" ? task.childSessionId : undefined;
  const span =
    now !== null
      ? now - task.startedAtMs
      : task.endedAtMs === undefined
        ? null
        : task.endedAtMs - task.startedAtMs;
  const duration = span === null ? null : formatTaskDuration(span);
  // A row names its title whatever the daemon sent: the roster's title stands in for an empty one.
  const title =
    task.title.trim() !== "" ? task.title : childTitle?.trim() || UNTITLED_LABEL[task.kind];
  const word = taskStateWord(task.state);
  // The model and the tool count are the row's detail: they wait behind a hover.
  const detail = [
    childProvider,
    task.model,
    task.toolCallCount === undefined ? undefined : toolCountText(task.toolCallCount),
  ]
    .filter((part): part is string => part !== undefined)
    .join(" · ");
  return (
    <li className="tasks-panel-row" data-state={task.state} title={detail || undefined}>
      <span className="tasks-panel-mark">
        <StripKindMark kind={rowMarkKind(task, childKind)} />
      </span>
      {childSessionId === undefined ? (
        <span className="tasks-panel-title">{title}</span>
      ) : (
        <button
          type="button"
          className="tasks-panel-title tasks-panel-open"
          onClick={() => onOpen(childSessionId)}
        >
          {title}
        </button>
      )}
      {word === groupLabel ? null : <span className="tasks-panel-word">{word}</span>}
      {duration === null ? null : <span className="tasks-panel-meta">{duration}</span>}
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
