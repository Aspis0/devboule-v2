import { useId, useState, type CSSProperties } from "react";

interface ThoughtRowProps {
  label: string;
  text: string;
  isStreaming: boolean;
  className: string;
  style?: CSSProperties;
}

export function ThoughtRow({ label, text, isStreaming, className, style }: ThoughtRowProps) {
  const [expanded, setExpanded] = useState(false);
  const bodyId = useId();
  const lines = text
    .split(/\r?\n/)
    .map((line) => line.trim())
    .filter((line) => line.length > 0);
  const [preview] = lines;
  const status = isStreaming ? (
    <span className="workspace-chat-thought-status">Thinking…</span>
  ) : null;

  // A single line has nothing to fold away, so the row carries the text itself.
  if (lines.length <= 1) {
    return (
      <div className={className} style={style}>
        <div className="workspace-chat-thought-line">
          <span className="workspace-chat-thought-label">{label}</span>
          {status}
          {preview !== undefined ? (
            <span className="workspace-chat-thought-text">{preview}</span>
          ) : null}
        </div>
      </div>
    );
  }

  return (
    <div className={className} style={style}>
      <button
        aria-controls={expanded ? bodyId : undefined}
        aria-expanded={expanded}
        className="workspace-chat-thought-trigger"
        onClick={() => setExpanded((value) => !value)}
        type="button"
      >
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
          <path d="m9 18 6-6-6-6" />
        </svg>
        <span className="workspace-chat-thought-label">{label}</span>
        {status}
        {expanded ? null : <span className="workspace-chat-thought-preview">{preview}</span>}
      </button>
      {expanded ? (
        <div className="workspace-chat-copy workspace-chat-thought-body" id={bodyId}>
          {text}
        </div>
      ) : null}
    </div>
  );
}
