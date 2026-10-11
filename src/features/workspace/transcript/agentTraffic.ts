import type { AgentChatItem } from "../../../lib/agentSession";
import { devbouleActivity, devbouleToolName } from "../devbouleToolLine";
import { isFailedToolCall } from "./toolFailure";

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
      // A running agent's row: the Tasks panel carries its state, the chat only its end.
      return (
        (item.task?.kind === "agent" && item.task.state === "running") ||
        CREATOR_WAKE_PREFIXES.some((prefix) => item.text.startsWith(prefix))
      );
    case "tool": {
      // A failed call is the person's to see, whatever it was for.
      if (isFailedToolCall(item)) return false;
      const name = devbouleToolName(item.title);
      // A blocked agent is the one poll result the person needs; every other poll is noise.
      if (name === "agent_activity") return devbouleActivity(item.output) !== "blocked";
      return PLUMBING_TOOLS.has(name);
    }
    case "daemon_notice":
      return item.notice.recognized && item.notice.kind === "agent_idle_closed";
    default:
      return false;
  }
}

/** The card an answer names when the answer went through: `{state: "answered", cardId}`. */
function cardAnsweredBy(output: string): string | null {
  try {
    const value: unknown = JSON.parse(output);
    if (typeof value !== "object" || value === null) return null;
    const { state, cardId } = value as { state?: unknown; cardId?: unknown };
    return state === "answered" && typeof cardId === "string" ? cardId : null;
  } catch {
    return null;
  }
}

/**
 * A child's permission card that an agent answered is traffic: the answer closed it
 * and the person has nothing to do. A card no successful answer names stays, since
 * the person may still have to act on it.
 */
export function dropAgentAnsweredCards(items: AgentChatItem[]): AgentChatItem[] {
  const answered = new Set<string>();
  for (const item of items) {
    if (item.role !== "tool" || isFailedToolCall(item)) continue;
    if (devbouleToolName(item.title) !== "answer_permissions") continue;
    const cardId = cardAnsweredBy(item.output);
    if (cardId !== null) answered.add(cardId);
  }
  return items.filter((item) => !(item.role === "permission_request" && answered.has(item.cardId)));
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
    if (item.role !== "system" || item.task?.kind !== "agent") continue;
    if (item.task.state !== "running") settledByTurn[settledByTurn.length - 1].add(item.task.title);
  }
  let turn = 0;
  return items.filter((item) => {
    if (item.role === "user") turn += 1;
    if (item.role !== "daemon_notice" || !item.notice.recognized) return true;
    if (item.notice.kind !== "agent_finished" || item.notice.childName === null) return true;
    return !settledByTurn[turn].has(item.notice.childName);
  });
}
