import { useState, type ReactNode } from "react";

/**
 * The icon every agent line leads with: an agent message, a daemon notice and a
 * Devboule agent call share this one slot, so the lines read as one kind of event.
 */
export function AgentIcon() {
  return (
    <svg
      className="workspace-chat-agent-icon"
      aria-hidden="true"
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeLinecap="round"
      strokeLinejoin="round"
      strokeWidth="2"
      focusable="false"
    >
      <path d="M21 15a2 2 0 0 1-2 2H7l-4 4V5a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2z" />
    </svg>
  );
}

/**
 * One event in the transcript as one line: the summary always shows, and the
 * detail opens on the person's click, the way a tool call's output does. The
 * agent-to-agent cards and the daemon notices use it, so an event reads the same
 * whichever part of the system reported it.
 */
export function ActivityRow({
  testId,
  className,
  summary,
  title,
  detail,
}: {
  testId?: string;
  className?: string;
  summary: ReactNode;
  /** The tooltip for the line: the identifiers a reader may want, not shown. */
  title?: string;
  /** What opens on the click. */
  detail: ReactNode;
}) {
  const [opened, setOpened] = useState(false);
  return (
    <div
      className={`workspace-chat-entry workspace-chat-activity${className ? ` ${className}` : ""}`}
      data-testid={testId}
    >
      <details
        className="workspace-chat-activity-details"
        open={opened}
        onToggle={(event) => setOpened(event.currentTarget.open)}
      >
        <summary className="workspace-chat-activity-line" title={title}>
          <AgentIcon />
          {summary}
        </summary>
        {/* A closed disclosure still holds its children, so the detail mounts only once opened. */}
        {opened ? <div className="workspace-chat-activity-body">{detail}</div> : null}
      </details>
    </div>
  );
}
