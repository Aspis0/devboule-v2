import { useId, useState } from "react";
import { useCopyFeedback } from "../../../lib/useCopyFeedback";
import { OUTPUT_RENDER_CAP, copyText, outputLineKind } from "./toolOutputView";

/** The "show all" state of an output whose row a group may remount. */
export interface OutputExpansion {
  expanded: boolean;
  setExpanded: (expanded: boolean) => void;
}

interface ToolOutputProps {
  /** The whole output. */
  lines: readonly string[];
  /** What shows before the expander opens the rest. */
  collapsed: readonly string[];
  /** `diff` colours added and removed lines; `failure` is the boxed excerpt. */
  tone: "plain" | "diff" | "failure";
  /** Absent, the output keeps its own state. */
  expansion?: OutputExpansion;
}

/** A tool's output clipped to a few lines, with a button that opens the rest. */
export function ToolOutput({ lines, collapsed, tone, expansion }: ToolOutputProps) {
  const [ownAll, setOwnAll] = useState(false);
  const all = expansion === undefined ? ownAll : expansion.expanded;
  const toggleAll = () => {
    if (expansion === undefined) setOwnAll((open) => !open);
    else expansion.setExpanded(!expansion.expanded);
  };
  const linesId = useId();
  const copy = useCopyFeedback({ resetAfterMs: 1500 });
  const [copyTruncated, setCopyTruncated] = useState(false);
  const hidden = lines.length - collapsed.length;
  const visible = all ? lines : collapsed;
  const shown = visible.slice(0, OUTPUT_RENDER_CAP);
  const unmounted = Math.max(0, visible.length - OUTPUT_RENDER_CAP);
  const copyState = copy.stateFor("output");
  return (
    <div className={`workspace-chat-tool-output is-${tone}`}>
      {/* A box that can scroll has to take the keyboard, or its lines are out of reach. */}
      <div
        id={linesId}
        className="workspace-chat-tool-output-lines"
        {...(tone !== "failure" || all
          ? { tabIndex: 0, role: "region", "aria-label": "Tool output" }
          : {})}
      >
        {shown.map((line, index) => (
          <div
            // The index keeps blank lines distinct; the text remounts a line whose content changed.
            key={`${index}:${line.slice(0, 32)}`}
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
            onClick={() => {
              const { text, truncated } = copyText(lines);
              setCopyTruncated(truncated);
              void copy.copy("output", text);
            }}
          >
            {copyState === "copied"
              ? copyTruncated
                ? "Copied (truncated)"
                : "Copied"
              : copyState === "failed"
                ? "Copy failed"
                : "Copy output"}
          </button>
        </div>
      ) : null}
      {hidden > 0 ? (
        <button
          type="button"
          className="workspace-chat-tool-more"
          aria-expanded={all}
          aria-controls={linesId}
          onClick={toggleAll}
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
