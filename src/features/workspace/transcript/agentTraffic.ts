import type { AgentChatItem } from "../../../lib/agentSession";
import { parseSettledAgentRow } from "../../../lib/backgroundTaskText";
import { devbouleActivity, devbouleToolName } from "../devbouleToolLine";

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
    case "tool": {
      const name = devbouleToolName(item.title);
      // A blocked agent is the one poll result the person needs; every other poll is noise.
      if (name === "agent_activity") return devbouleActivity(item.output) !== "blocked";
      return PLUMBING_TOOLS.has(name);
    }
    case "permission_request":
      return true;
    case "daemon_notice":
      return item.notice.recognized && item.notice.kind === "agent_idle_closed";
    default:
      return false;
  }
}

/**
 * A finish notice drops out when its turn already draws the same child's settled
 * task row: that row carries the finish. The order of the two is not fixed, so
 * the turn is the unit, not the position.
 */
export function dropDuplicateFinishNotices(items: AgentChatItem[]): AgentChatItem[] {
  const settledByTurn: Set<string>[] = [new Set()];
  for (const item of items) {
    if (item.role === "user") settledByTurn.push(new Set());
    if (item.role !== "system") continue;
    const row = parseSettledAgentRow(item.text);
    if (row !== null) settledByTurn[settledByTurn.length - 1].add(row.title);
  }
  let turn = 0;
  return items.filter((item) => {
    if (item.role === "user") turn += 1;
    if (item.role !== "daemon_notice" || !item.notice.recognized) return true;
    if (item.notice.kind !== "agent_finished" || item.notice.childName === null) return true;
    return !settledByTurn[turn].has(item.notice.childName);
  });
}
