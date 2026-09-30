import {
  memo,
  useEffect,
  useId,
  useRef,
  useState,
  type KeyboardEvent as ReactKeyboardEvent,
} from "react";
import type { AgentTaskItem } from "../../lib/agentSession";
import "./AgentTaskPill.css";

interface AgentTaskPillProps {
  items: readonly AgentTaskItem[];
}

type StatusKind = AgentTaskItem["status"] | "unknown";

/** What a mark says to a screen reader: the mark itself draws, never speaks. */
const STATUS_WORD: Record<StatusKind, string> = {
  completed: "Done",
  in_progress: "In progress",
  pending: "Pending",
  unknown: "Unknown",
};

/**
 * The wire is unvalidated JS: a newer daemon can send a status this build has
 * no word or mark for, and `Object.hasOwn` keeps prototype keys ("toString")
 * out of the lookup.
 */
function statusKind(status: string): StatusKind {
  return Object.hasOwn(STATUS_WORD, status) ? (status as StatusKind) : "unknown";
}

/**
 * The agent's plan checklist pinned above the composer: a collapsed
 * "N of M" head naming what the agent is on, expanding to one row per item
 * with a status mark. The marks are status, not checkboxes — only the head
 * is a control, and an empty list renders nothing at all.
 */
export const AgentTaskPill = memo(function AgentTaskPill({ items }: AgentTaskPillProps) {
  const [open, setOpen] = useState(false);
  const headRef = useRef<HTMLButtonElement>(null);
  const listId = useId();

  // A cleared checklist must not carry its open state into the next plan: the
  // new one arrives collapsed.
  useEffect(() => {
    if (items.length === 0) setOpen(false);
  }, [items]);

  if (items.length === 0) return null;

  const completed = items.filter((item) => item.status === "completed").length;
  const running = items.filter((item) => item.status === "in_progress");
  const pending = items.filter((item) => item.status === "pending");
  const allDone = completed === items.length;
  // The head copy tells the truth about where the agent is: the step it runs,
  // "next" only when nothing runs; with nothing left at all the count carries
  // the spec's collapsed copy instead of a label.
  const current: { text: string; title?: string } | null =
    running.length > 0
      ? { text: running[0].activeForm || running[0].text, title: running[0].text }
      : pending.length > 0
        ? { text: `next: ${pending[0].text}`, title: pending[0].text }
        : null;
  const count = `${completed} of ${items.length}${allDone ? " done" : ""}`;
  const moreRunning = running.length > 1 ? `+${running.length - 1} more running` : null;

  function headKeyDown(event: ReactKeyboardEvent<HTMLDivElement>): void {
    if (event.key !== "Escape" || !open) return;
    // This Escape closed the pill; nothing above it should react to it too.
    event.stopPropagation();
    setOpen(false);
    headRef.current?.focus();
  }

  return (
    <div className="agent-task-pill" data-testid="agent-task-pill" onKeyDown={headKeyDown}>
      <button
        ref={headRef}
        type="button"
        className="agent-task-pill-head"
        aria-expanded={open}
        aria-controls={listId}
        onClick={() => setOpen((wasOpen) => !wasOpen)}
        data-testid="agent-task-pill-toggle"
      >
        {/* Names the control without stealing the visible words from it. */}
        <span className="agent-task-pill-sr">Plan checklist,</span>
        <svg
          className="agent-task-pill-icon"
          viewBox="0 0 24 24"
          fill="none"
          stroke="currentColor"
          strokeWidth="2"
          strokeLinecap="round"
          strokeLinejoin="round"
          aria-hidden="true"
        >
          <path d="m3 17 2 2 4-4" />
          <path d="m3 7 2 2 4-4" />
          <path d="M13 6h8" />
          <path d="M13 12h8" />
          <path d="M13 18h8" />
        </svg>
        <span className="agent-task-pill-count">{count}</span>
        <span className="agent-task-pill-bar" aria-hidden="true">
          <span
            className="agent-task-pill-bar-fill"
            style={{ width: `${Math.round((completed / items.length) * 100)}%` }}
          />
        </span>
        {current === null ? null : (
          <span className="agent-task-pill-current" title={current.title}>
            {current.text}
          </span>
        )}
        {moreRunning === null ? null : <span className="agent-task-pill-more">{moreRunning}</span>}
        <svg
          className="agent-task-pill-chevron"
          viewBox="0 0 24 24"
          fill="none"
          stroke="currentColor"
          strokeWidth="2"
          strokeLinecap="round"
          strokeLinejoin="round"
          aria-hidden="true"
        >
          <path d="m6 9 6 6 6-6" />
        </svg>
      </button>
      <ul
        id={listId}
        className="agent-task-pill-list"
        hidden={!open}
        data-testid="agent-task-pill-list"
      >
        {items.map((item, index) => {
          const kind = statusKind(item.status);
          return (
            <li key={`${index}-${item.id ?? "task"}`} className="agent-task-row" data-status={kind}>
              <span
                className={`agent-task-mark agent-task-mark--${kind}${
                  kind === "in_progress" ? " dot-pulse" : ""
                }`}
                aria-hidden="true"
              />
              <span className="agent-task-row-status">{STATUS_WORD[kind]}</span>
              <span className="agent-task-row-text" title={item.text}>
                {item.text}
              </span>
            </li>
          );
        })}
      </ul>
    </div>
  );
});
