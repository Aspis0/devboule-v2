import { useId, useState } from "react";
import { useCopyFeedback } from "../../../lib/useCopyFeedback";
import { OUTPUT_RENDER_CAP, outputLineKind } from "./toolOutputView";

interface ToolOutputProps {
  /** The whole output. */
  lines: readonly string[];
  /** What shows before the expander opens the rest. */
  collapsed: readonly string[];
  /** `diff` colours added and removed lines; `failure` is the boxed excerpt. */
  tone: "plain" | "diff" | "failure";
}

/** A tool's output clipped to a few lines, with a button that opens the rest. */
export function ToolOutput({ lines, collapsed, tone }: ToolOutputProps) {
  const [all, setAll] = useState(false);
  const linesId = useId();
  const copy = useCopyFeedback({ resetAfterMs: 1500 });
  const hidden = lines.length - collapsed.length;
  const shown = all ? lines.slice(0, OUTPUT_RENDER_CAP) : collapsed;
  const unmounted = all ? Math.max(0, lines.length - OUTPUT_RENDER_CAP) : 0;
  const copyState = copy.stateFor("output");
  return (
    <div className={`workspace-chat-tool-output is-${tone}`}>
      {/* A box that can scroll has to take the keyboard, or its lines are out of reach. */}
      <div
        id={linesId}
        className="workspace-chat-tool-output-lines"
        {...(tone === "diff" || all
          ? { tabIndex: 0, role: "region", "aria-label": "Tool output" }
          : {})}
      >
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
      {unmounted > 0 ? (
        <div className="workspace-chat-tool-output-cap">
          {`${unmounted} more lines not shown · `}
          <button
            type="button"
            className="workspace-chat-tool-more"
            onClick={() => void copy.copy("output", lines.join("\n"))}
          >
            {copyState === "copied"
              ? "Copied"
              : copyState === "failed"
                ? "Copy failed"
                : "Copy all output"}
          </button>
        </div>
      ) : null}
      {hidden > 0 ? (
        <button
          type="button"
          className="workspace-chat-tool-more"
          aria-expanded={all}
          aria-controls={linesId}
          onClick={() => setAll((open) => !open)}
        >
          {all ? (
            "Show less"
          ) : (
            <>
              {`+${hidden} ${hidden === 1 ? "line" : "lines"}`}
              <span className="sr-only"> of output</span>
            </>
          )}
        </button>
      ) : null}
    </div>
  );
}
