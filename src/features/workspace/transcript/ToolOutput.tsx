import { useId, useState } from "react";
import { outputLineKind } from "./toolOutputView";

interface ToolOutputProps {
  lines: readonly string[];
  /** How many lines show before the expander. */
  preview: number;
  /** `diff` colours added and removed lines; `failure` is the boxed excerpt. */
  tone: "plain" | "diff" | "failure";
}

/** A tool's output clipped to a few lines, with a button that opens the rest. */
export function ToolOutput({ lines, preview, tone }: ToolOutputProps) {
  const [all, setAll] = useState(false);
  const linesId = useId();
  const hidden = lines.length - preview;
  const shown = all || hidden <= 0 ? lines : lines.slice(0, preview);
  return (
    <div className={`workspace-chat-tool-output is-${tone}`}>
      <div id={linesId} className="workspace-chat-tool-output-lines">
        {shown.map((line, index) => (
          <div
            key={index}
            className={`workspace-chat-tool-output-line${
              tone === "diff" ? ` is-${outputLineKind(line)}` : ""
            }`}
          >
            {line}
          </div>
        ))}
      </div>
      {hidden > 0 ? (
        <button
          type="button"
          className="workspace-chat-tool-more"
          aria-expanded={all}
          aria-controls={linesId}
          onClick={() => setAll((open) => !open)}
        >
          {all ? "Show less" : `+${hidden} lines`}
        </button>
      ) : null}
    </div>
  );
}
