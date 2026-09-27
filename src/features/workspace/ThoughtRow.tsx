import { useId, useState, type CSSProperties, type KeyboardEvent } from "react";

interface ThoughtRowProps {
  text: string;
  isStreaming: boolean;
  className: string;
  style?: CSSProperties;
}

export function ThoughtRow({ text, isStreaming, className, style }: ThoughtRowProps) {
  const [expanded, setExpanded] = useState(false);
  const bodyId = useId();
  const preview = text.split(/\r?\n/, 1)[0] ?? "";
  const toggle = () => setExpanded((value) => !value);

  return (
    <div className={className} style={style}>
      <button
        aria-controls={bodyId}
        aria-expanded={expanded}
        className="workspace-chat-thought-trigger"
        onClick={toggle}
        onKeyDown={(event: KeyboardEvent<HTMLButtonElement>) => {
          if ((event.key === "Enter" || event.key === " ") && !event.repeat) {
            event.preventDefault();
            toggle();
          }
        }}
        type="button"
      >
        <span className="workspace-chat-thought-label">
          {isStreaming ? "Thinking…" : "Thought"}
        </span>
        {preview ? <span className="workspace-chat-thought-preview">{preview}</span> : null}
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
