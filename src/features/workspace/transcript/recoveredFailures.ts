import type { AgentChatItem } from "../../../lib/agentSession";
import { isFailedToolCall } from "./toolFailure";

type ToolItem = Extract<AgentChatItem, { role: "tool" }>;

/**
 * A failed call that the same operation later completed, in the same turn, is not
 * drawn: the agent recovered, and the person needs the result only. The same
 * operation is the same title and the same target. A call that names no target
 * (a Devboule call whose event carries no arguments) has no operation key, so
 * nothing can recover it and it stays drawn.
 */
export function dropRecoveredFailures(items: AgentChatItem[]): AgentChatItem[] {
  const completed = new Set<string>();
  const kept: AgentChatItem[] = [];
  for (let index = items.length - 1; index >= 0; index -= 1) {
    const item = items[index];
    if (item.role === "user") completed.clear();
    if (item.role !== "tool") {
      kept.push(item);
      continue;
    }
    const operation = operationOf(item);
    if (isFailedToolCall(item)) {
      if (operation === null || !completed.has(operation)) kept.push(item);
      continue;
    }
    if (operation !== null && item.status.toLowerCase() === "completed") completed.add(operation);
    kept.push(item);
  }
  return kept.reverse();
}

/** The title and the target a call acted on, or null when the call names no target. */
function operationOf(item: ToolItem): string | null {
  const target =
    item.command ??
    (item.locations ?? []).map((location) => `${location.path}:${location.line ?? ""}`).join("\n");
  return target === "" ? null : JSON.stringify([item.title, target]);
}
