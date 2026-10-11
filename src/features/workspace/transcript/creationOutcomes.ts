import type { AgentChatItem } from "../../../lib/agentSession";
import type { ToolChatItem } from "../../../lib/toolCallGroups";
import { devbouleToolName } from "../devbouleToolLine";
import { creationFailureReason } from "./creatorNotes";
import { isFailedToolCall } from "./toolFailure";

/**
 * A creation the daemon reports as not started is one outcome: the call's own row
 * or the note, never both. The note answers the nearest creation call before it in
 * the same turn. A failed call already shows its failure, so the note goes; a call
 * that completed is the one that reads as started, so the call goes.
 */
export function pairCreationOutcomes(items: AgentChatItem[]): AgentChatItem[] {
  const dropped = new Set<AgentChatItem>();
  let calls: ToolChatItem[] = [];
  for (const item of items) {
    if (item.role === "user") {
      calls = [];
    } else if (item.role === "tool" && devbouleToolName(item.title) === "create_agent") {
      calls.push(item);
    } else if (item.role === "system" && creationFailureReason(item.text) !== null) {
      const call = calls.pop();
      if (call === undefined) continue;
      dropped.add(isFailedToolCall(call) ? item : call);
    }
  }
  return items.filter((item) => !dropped.has(item));
}
