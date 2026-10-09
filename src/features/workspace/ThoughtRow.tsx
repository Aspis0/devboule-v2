import { useId, useState, type CSSProperties } from "react";

interface ThoughtRowProps {
  label: string;
  text: string;
  isStreaming: boolean;
  className: string;
  style?: CSSProperties;
}

/**
 * A thought is one quiet row until the person opens it. The text appears once,
 * inside the box, so a collapsed row never repeats what the box will show.
 */
export function ThoughtRow({ label, text, isStreaming, className, style }: ThoughtRowProps) {
  const [expanded, setExpanded] = useState(false);
  const bodyId = useId();
  const status = isStreaming ? <span className="workspace-chat-thought-status">…</span> : null;
  const icon = (
    <svg
      aria-hidden="true"
      className="workspace-chat-thought-icon"
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeLinecap="round"
      strokeLinejoin="round"
      strokeWidth="2"
    >
      <path d="M9 18h6" />
      <path d="M10 22h4" />
      <path d="M12 2a7 7 0 0 0-4 12.7c.6.5 1 1.2 1 2V17h6v-.3c0-.8.4-1.5 1-2A7 7 0 0 0 12 2z" />
    </svg>
  );

  if (text.trim().length === 0) {
    return (
      <div className={className} style={style}>
        <div className="workspace-chat-thought-line">
          {icon}
          <span className="workspace-chat-thought-label">{label}</span>
          {status}
        </div>
      </div>
    );
  }

  return (
    <div className={`${className}${expanded ? " is-expanded" : ""}`} style={style}>
      <button
        aria-controls={expanded ? bodyId : undefined}
        aria-expanded={expanded}
        className="workspace-chat-thought-trigger"
        onClick={() => setExpanded((value) => !value)}
        type="button"
      >
        {icon}
        <span className="workspace-chat-thought-label">{label}</span>
        {status}
        {expanded ? (
          <svg
            aria-hidden="true"
            className="workspace-chat-thought-chevron"
            viewBox="0 0 24 24"
            fill="none"
            stroke="currentColor"
            strokeLinecap="round"
            strokeLinejoin="round"
            strokeWidth="2"
          >
            <path d="m6 9 6 6 6-6" />
          </svg>
        ) : null}
      </button>
      {expanded ? (
        <div className="workspace-chat-thought-body" id={bodyId}>
          {text}
        </div>
      ) : null}
    </div>
  );
}
