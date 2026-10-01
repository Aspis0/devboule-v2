import { useState } from "react";
import { RECOVERED_SESSION_UNAVAILABLE, type AgentChatItem } from "../../../lib/agentSession";
import { groupToolCalls, isToolCallGroup, type ToolCallGroup } from "../../../lib/toolCallGroups";

function deriveEntries(
  items: AgentChatItem[],
  recoveredAttach: boolean,
  pendingPlanToolCallId: string | null,
  previous: readonly (AgentChatItem | ToolCallGroup)[],
) {
  const visibleItems = recoveredAttach
    ? items.filter((item) => item.role !== "error" || item.text !== RECOVERED_SESSION_UNAVAILABLE)
    : items;
  const grouped = groupToolCalls(visibleItems, previous);
  if (pendingPlanToolCallId === null) return grouped;
  // Plans never join groups; hide only the row whose approval card is on screen.
  return grouped.filter((entry) => {
    if (isToolCallGroup(entry)) return true;
    return !(
      entry.role === "tool" &&
      entry.kind === "plan" &&
      entry.toolCallId === pendingPlanToolCallId
    );
  });
}

export function useTranscriptEntries(
  items: AgentChatItem[],
  recoveredAttach: boolean,
  pendingPlanToolCallId: string | null,
) {
  const [snapshot, setSnapshot] = useState(() => ({
    items,
    recoveredAttach,
    pendingPlanToolCallId,
    entries: deriveEntries(items, recoveredAttach, pendingPlanToolCallId, []),
  }));
  if (
    snapshot.items !== items ||
    snapshot.recoveredAttach !== recoveredAttach ||
    snapshot.pendingPlanToolCallId !== pendingPlanToolCallId
  ) {
    const next = {
      items,
      recoveredAttach,
      pendingPlanToolCallId,
      entries: deriveEntries(items, recoveredAttach, pendingPlanToolCallId, snapshot.entries),
    };
    // React retries this list before its children render; abandoned renders leave state intact.
    setSnapshot(next);
    return next.entries;
  }
  return snapshot.entries;
}
