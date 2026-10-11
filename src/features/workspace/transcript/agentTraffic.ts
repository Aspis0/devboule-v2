import type { AgentChatItem } from "../../../lib/agentSession";
import { devbouleToolName } from "../devbouleToolLine";

/**
 * Agent-to-agent traffic the person does not read in this chat. The rows stay in
 * the journal and the Tasks panel; this only decides what the transcript draws.
 */

// The daemon's notes to a creator when a child it made starts or fails to
// (creation/run.rs). They are written for the agent, not for the person.
const CREATOR_WAKE_PREFIXES = [
  "The person approved the agent creation:",
  "The agent creation did not start:",
];

// A running agent's task row (backgroundTaskText.ts); the Tasks panel carries its state.
const RUNNING_AGENT_PREFIX = "Running agent ";

// Calls the orchestrator makes to answer or poll for its children's cards.
const PLUMBING_TOOLS = new Set(["answer_permissions", "list_pending_permissions"]);

// The same test entryFrame uses for the sub-agent frame: a null id is the parent's own row.
function isChildOwned(item: AgentChatItem): boolean {
  return (
    "parentToolUseId" in item &&
    typeof item.parentToolUseId === "string" &&
    item.parentToolUseId.length > 0
  );
}

export function isAgentTraffic(item: AgentChatItem): boolean {
  // A sub-agent's text and tool calls are read in the child's own tab.
  if (isChildOwned(item)) return true;
  switch (item.role) {
    case "system":
      return (
        item.text.startsWith(RUNNING_AGENT_PREFIX) ||
        CREATOR_WAKE_PREFIXES.some((prefix) => item.text.startsWith(prefix))
      );
    case "tool":
      return PLUMBING_TOOLS.has(devbouleToolName(item.title));
    case "permission_request":
      return true;
    default:
      return false;
  }
}
