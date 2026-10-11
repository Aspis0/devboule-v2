import { useState } from "react";
import { RECOVERED_SESSION_UNAVAILABLE, type AgentChatItem } from "../../../lib/agentSession";
import { groupToolCalls, isToolCallGroup, type ToolCallGroup } from "../../../lib/toolCallGroups";

/** One quiet line per event group: the daemon announces a compaction twice
 * (start, then finish), and only the finish earns a row. Exact strings — a
 * reworded daemon degrades to two lines, never to a dropped event. */
const COMPACT_PAIRS: ReadonlyArray<readonly [string, string]> = [
  ["Compacting the context.", "Context compacted."],
  ["Context manually compacted", "Compacted"],
];

/**
 * A start notice drops out when its finish comes later in the same turn: a tool
 * call or an answer can land between the two, and the start must not then stay
 * behind as a spinner that never ends. A user message ends the turn, so a
 * finish after one leaves its start where it was.
 */
export function collapseCompactNotices(items: AgentChatItem[]): AgentChatItem[] {
  const dropped = new Set<number>();
  items.forEach((item, index) => {
    if (item.role !== "system") return;
    const pair = COMPACT_PAIRS.find(([start]) => item.text === start);
    if (pair === undefined) return;
    for (let later = index + 1; later < items.length; later += 1) {
      const next = items[later];
      if (next.role === "user") return;
      if (next.role === "system" && next.text === pair[1]) {
        dropped.add(index);
        return;
      }
    }
  });
  return items.filter((_, index) => !dropped.has(index));
}

/**
 * Plans never draw a row: an answered plan lives in the composer's icon, and a
 * plan still waiting is the card on screen. Plans never join a group either.
 */
function deriveEntries(
  items: AgentChatItem[],
  recoveredAttach: boolean,
  previous: readonly (AgentChatItem | ToolCallGroup)[],
) {
  const visibleItems = recoveredAttach
    ? items.filter((item) => item.role !== "error" || item.text !== RECOVERED_SESSION_UNAVAILABLE)
    : items;
  return groupToolCalls(collapseCompactNotices(visibleItems), previous).filter(
    (entry) => isToolCallGroup(entry) || !(entry.role === "tool" && entry.kind === "plan"),
  );
}

export function useTranscriptEntries(items: AgentChatItem[], recoveredAttach: boolean) {
  const [snapshot, setSnapshot] = useState(() => ({
    items,
    recoveredAttach,
    entries: deriveEntries(items, recoveredAttach, []),
  }));
  if (snapshot.items !== items || snapshot.recoveredAttach !== recoveredAttach) {
    const next = {
      items,
      recoveredAttach,
      entries: deriveEntries(items, recoveredAttach, snapshot.entries),
    };
    // React retries this list before its children render; abandoned renders leave state intact.
    setSnapshot(next);
    return next.entries;
  }
  return snapshot.entries;
}
