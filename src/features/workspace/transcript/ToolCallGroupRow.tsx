import { useCallback, useState, type SyntheticEvent } from "react";
import type { ToolCallGroup } from "../../../lib/toolCallGroups";
import {
  INTERRUPTED_TOOL_CLASS,
  INTERRUPTED_TOOL_COPY,
  isToolRunningStatus,
} from "../interruptedTool";
import { entryFrame } from "./entryFrame";
import { ToolRow } from "./ToolRow";

export function ToolCallGroupRow({
  group,
  transcriptEnded,
}: {
  group: ToolCallGroup;
  transcriptEnded: boolean;
}) {
  const [open, setOpen] = useState(false);
  const onToggle = useCallback((event: SyntheticEvent<HTMLDetailsElement>) => {
    setOpen(event.currentTarget.open);
  }, []);
  const first = group.items[0];
  if (first === undefined) return null;
  const frame = entryFrame(first);
  const anyRunning = group.items.some((item) => isToolRunningStatus(item.status));
  const interrupted = transcriptEnded && anyRunning;
  const running = anyRunning && !transcriptEnded;
  const failures = group.items.filter((item) => item.status.toLowerCase() === "failed");
  const failed = failures.length > 0;
  // Closed, the failed calls stand outside the group; they are mounted once.
  const bodyItems =
    open || !failed ? group.items : group.items.filter((item) => !failures.includes(item));
  const callCount = `${group.items.length} tool calls`;
  const className = `${frame.className} workspace-chat-tool-group${running ? " is-running" : ""}${failed ? " is-failed" : ""}${interrupted ? ` ${INTERRUPTED_TOOL_CLASS}` : ""}`;
  return (
    <>
      <details className={className} open={open} style={frame.style} onToggle={onToggle}>
        <summary className="workspace-chat-tool-group-summary" aria-expanded={open}>
          <span className="workspace-chat-tool-group-count">{callCount}</span>
          <span className="workspace-chat-tool-group-summary-text">{group.summary}</span>
          {interrupted ? (
            <span className="workspace-chat-tool-interrupted">{INTERRUPTED_TOOL_COPY}</span>
          ) : null}
          {failed ? (
            <span className="workspace-chat-tool-failed">
              <span aria-hidden="true">✗ </span>
              failed
            </span>
          ) : null}
          {running ? (
            <span className="workspace-chat-tool-running" role="img" aria-label="Running" />
          ) : null}
        </summary>
        <div className="workspace-chat-tool-group-body">
          {bodyItems.map((item) => (
            <ToolRow key={item.id} item={item} transcriptEnded={transcriptEnded} />
          ))}
        </div>
      </details>
      {/* A failure is never behind a click: while the group is closed its failed
          calls stand under the group line, line and excerpt both. */}
      {open || !failed ? null : (
        <div className="workspace-chat-tool-group-failures">
          {failures.map((item) => (
            <ToolRow key={item.id} item={item} transcriptEnded={transcriptEnded} />
          ))}
        </div>
      )}
    </>
  );
}
