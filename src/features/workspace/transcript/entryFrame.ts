import type { AgentChatItem } from "../../../lib/agentSession";

export function entryFrame(item: AgentChatItem) {
  const isSubagent =
    "parentToolUseId" in item &&
    typeof item.parentToolUseId === "string" &&
    item.parentToolUseId.length > 0;
  const measuredDepth =
    "spawnDepth" in item && typeof item.spawnDepth === "number" ? item.spawnDepth : null;
  const visibleDepth = measuredDepth === null ? null : Math.min(4, Math.max(0, measuredDepth));
  const className = `workspace-chat-entry workspace-chat-${item.role}${
    isSubagent ? " workspace-chat-subagent" : ""
  }${isSubagent && measuredDepth === null ? " workspace-chat-subagent-depth-unknown" : ""}`;
  const style =
    isSubagent && visibleDepth !== null
      ? { marginInlineStart: `${visibleDepth * 16}px` }
      : undefined;
  return { className, style, isSubagent, measuredDepth };
}
