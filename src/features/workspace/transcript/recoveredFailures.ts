import type { AgentChatItem } from "../../../lib/agentSession";
import { isFailedToolCall } from "./toolFailure";

/**
 * A failed call that the same call later completed, in the same turn, is not
 * drawn: the agent recovered, and the person needs the result only. A call is
 * the same when its title is, exactly, so `npm test` never hides `npm run lint`.
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
    if (isFailedToolCall(item)) {
      if (!completed.has(item.title)) kept.push(item);
      continue;
    }
    if (item.status.toLowerCase() === "completed") completed.add(item.title);
    kept.push(item);
  }
  return kept.reverse();
}
