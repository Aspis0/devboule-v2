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
  const preview = text
    .split(/\r?\n/)
    .map((line) => line.trim())
    .find((line) => line.length > 0);

  return (
    <div className={className} style={style}>
      <button
        aria-controls={bodyId}
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
        {isStreaming ? <span className="workspace-chat-thought-status">Thinking…</span> : null}
        {preview !== undefined ? (
          <span className="workspace-chat-thought-preview">{preview}</span>
        ) : null}
      </button>
      <div
        className="workspace-chat-copy workspace-chat-thought-body"
        hidden={!expanded}
        id={bodyId}
      >
        {text}
      </div>
    </div>
  );
}
