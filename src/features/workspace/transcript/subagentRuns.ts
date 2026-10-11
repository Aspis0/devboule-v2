import type { AgentChatItem } from "../../../lib/agentSession";

/**
 * Each sub-agent run becomes one quiet line, at the place of its spawning call.
 * The run's items are read from the line's click, so the person can still read
 * what a sub-agent said when it has no tab of its own.
 */

/** The spawning call a child item belongs to, or null for a row of the parent. */
function parentOf(item: AgentChatItem): string | null {
  return "parentToolUseId" in item &&
    typeof item.parentToolUseId === "string" &&
    item.parentToolUseId.length > 0
    ? item.parentToolUseId
    : null;
}

interface Run {
  description: string | null;
  texts: string[];
}

export function foldSubagentRuns(items: AgentChatItem[]): AgentChatItem[] {
  const runs = new Map<string, Run>();
  for (const item of items) {
    const id = parentOf(item);
    if (id === null) continue;
    const run = runs.get(id) ?? { description: null, texts: [] };
    runs.set(id, run);
    if ((item.role === "assistant" || item.role === "thought") && item.text.trim() !== "") {
      run.texts.push(item.text);
    }
  }
  for (const item of items) {
    if (item.role !== "tool" || parentOf(item) !== null) continue;
    const run = runs.get(item.toolCallId);
    if (run !== undefined && item.title.trim() !== "") run.description = item.title.trim();
  }

  const drawn = new Set<string>();
  const folded: AgentChatItem[] = [];
  for (const item of items) {
    const id =
      parentOf(item) ??
      (item.role === "tool" && runs.has(item.toolCallId) ? item.toolCallId : null);
    if (id === null) {
      folded.push(item);
      continue;
    }
    if (drawn.has(id)) continue;
    drawn.add(id);
    const run = runs.get(id) ?? { description: null, texts: [] };
    folded.push({
      id: `subagent-${id}`,
      role: "system",
      severity: "info",
      text: run.description === null ? "Subagent" : `Subagent · ${run.description}`,
      subagent: { texts: run.texts },
    });
  }
  return folded;
}
