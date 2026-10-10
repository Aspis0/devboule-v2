import { useCallback, useMemo, useState, type SyntheticEvent } from "react";
import type { ToolCallGroup } from "../../../lib/toolCallGroups";
import {
  INTERRUPTED_TOOL_CLASS,
  INTERRUPTED_TOOL_COPY,
  isToolRunningStatus,
} from "../interruptedTool";
import { entryFrame } from "./entryFrame";
import type { OutputExpansion } from "./ToolOutput";
import { ToolRow } from "./ToolRow";

export function ToolCallGroupRow({
  group,
  transcriptEnded,
}: {
  group: ToolCallGroup;
  transcriptEnded: boolean;
}) {
  // A group is one closed line, like Paseo's: the person opens it to see its calls.
  const [open, setOpen] = useState(false);
  // A call's "show all" state lives here, so a re-render of the group keeps it.
  const [expandedIds, setExpandedIds] = useState<ReadonlySet<string>>(() => new Set());
  // A call keeps one expansion object for as long as its flag holds, so a
  // re-render of the group hands unchanged rows the same prop and their memo hits.
  const [expansionCache] = useState(() => new Map<string, OutputExpansion>());
  const expansions = useMemo(() => {
    const next = new Map<string, OutputExpansion>();
    for (const item of group.items) {
      const expanded = expandedIds.has(item.id);
      const cached = expansionCache.get(item.id);
      const expansion =
        cached !== undefined && cached.expanded === expanded
          ? cached
          : {
              expanded,
              setExpanded: (value: boolean) =>
                setExpandedIds((current) => {
                  if (current.has(item.id) === value) return current;
                  const updated = new Set(current);
                  if (value) updated.add(item.id);
                  else updated.delete(item.id);
                  return updated;
                }),
            };
      expansionCache.set(item.id, expansion);
      next.set(item.id, expansion);
    }
    return next;
  }, [group.items, expandedIds, expansionCache]);
  const onToggle = useCallback((event: SyntheticEvent<HTMLDetailsElement>) => {
    setOpen(event.currentTarget.open);
  }, []);
  const first = group.items[0];
  if (first === undefined) return null;
  const frame = entryFrame(first);
  const anyRunning = group.items.some((item) => isToolRunningStatus(item.status));
  const interrupted = transcriptEnded && anyRunning;
  const running = anyRunning && !transcriptEnded;
  const failedCount = group.items.filter((item) => item.status.toLowerCase() === "failed").length;
  const failed = failedCount > 0;
  const className = `${frame.className} workspace-chat-tool-group${running ? " is-running" : ""}${failed ? " is-failed" : ""}${interrupted ? ` ${INTERRUPTED_TOOL_CLASS}` : ""}`;
  return (
    <details className={className} open={open} style={frame.style} onToggle={onToggle}>
      <summary className="workspace-chat-tool-group-summary" aria-expanded={open}>
        <span className="workspace-chat-tool-group-summary-text">{group.summary}</span>
        {failed ? (
          <span className="workspace-chat-tool-group-failed">{`${failedCount} failed`}</span>
        ) : null}
        {interrupted ? (
          <span className="workspace-chat-tool-interrupted">{INTERRUPTED_TOOL_COPY}</span>
        ) : null}
        {running ? (
          <span className="workspace-chat-tool-running" role="img" aria-label="Running" />
        ) : null}
      </summary>
      <div className="workspace-chat-tool-group-body">
        {group.items.map((item) => (
          <ToolRow
            key={item.id}
            item={item}
            transcriptEnded={transcriptEnded}
            expansion={expansions.get(item.id)}
          />
        ))}
      </div>
    </details>
  );
}
