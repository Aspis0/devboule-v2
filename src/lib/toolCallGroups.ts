import type { AgentChatItem } from "./agentSession";
import * as toolCallGroupSummary from "./toolCallGroupSummary";

export type ToolChatItem = Extract<AgentChatItem, { role: "tool" }>;

/**
 * A run of consecutive groupable tool calls, rendered as one collapsible row.
 *
 * Grouping: consecutive groupable tools accumulate into a pending run and
 * any other item flushes it. The id is the first item's id, so it stays
 * stable while streaming appends tools to the end of the run.
 */
export interface ToolCallGroup {
  kind: "tool-group";
  id: string;
  items: ToolChatItem[];
  summary: string;
}

export function isToolCallGroup(entry: AgentChatItem | ToolCallGroup): entry is ToolCallGroup {
  // A group has no `role`, which every chat item carries: a wire item cannot
  // forge this discriminant even with `kind: "tool-group"`.
  return !("role" in entry) && entry.kind === "tool-group";
}

/**
 * Excludes the `plan` detail type and the `question` tool name, so the
 * transcript's only copy of a question and its answer is never buried in a
 * collapsed group. The `question` exclusion is deliberate, not an oversight.
 */
export function isGroupableToolCall(item: AgentChatItem): item is ToolChatItem {
  if (item.role !== "tool") return false;
  const kind = item.kind?.trim().toLowerCase();
  return kind !== "plan" && kind !== "question";
}

/**
 * Batch consecutive groupable tools into runs. A run never mixes depths: a
 * tool joins the pending run only when its `parentToolUseId` and `spawnDepth`
 * equal the run's first item's, so parent-level and subagent rows never share
 * a collapsed wrapper. A run of one stays a plain item: this surface has no
 * detail-level switch, so a one-item group would only add a useless wrapper
 * around a single row — collapsing one row saves nothing.
 */
export function groupToolCalls(
  items: AgentChatItem[],
  previous: readonly (AgentChatItem | ToolCallGroup)[] = [],
): Array<AgentChatItem | ToolCallGroup> {
  const previousGroups = new Map(
    previous.filter(isToolCallGroup).map((group) => [group.id, group]),
  );
  const output: Array<AgentChatItem | ToolCallGroup> = [];
  let pending: ToolChatItem[] = [];
  let pendingKey: string | null = null;
  const flush = () => {
    if (pending.length === 0) return;
    if (pending.length === 1) {
      const single = pending[0];
      if (single !== undefined) output.push(single);
    } else {
      const first = pending[0];
      if (first !== undefined) {
        const run = [...pending];
        const old = previousGroups.get(first.id);
        if (
          old !== undefined &&
          old.items.length === run.length &&
          run.every((item, index) => item === old.items[index])
        ) {
          output.push(old);
        } else {
          output.push({
            kind: "tool-group",
            id: first.id,
            items: run,
            summary: toolCallGroupSummary.summarizeToolCallGroup(run),
          });
        }
      }
    }
    pending = [];
    pendingKey = null;
  };
  for (const item of items) {
    if (isGroupableToolCall(item)) {
      // `?? ""` mirrors the surface: an absent parent or depth is the
      // parent level, and two absences are equal.
      const key = `${item.parentToolUseId ?? ""}:${item.spawnDepth ?? ""}`;
      if (pendingKey !== null && pendingKey !== key) flush();
      pending.push(item);
      pendingKey = key;
      continue;
    }
    flush();
    output.push(item);
  }
  flush();
  return output;
}
