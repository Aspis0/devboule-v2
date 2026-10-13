import type { AgentChatItem } from "../../../lib/agentSession";
import { devbouleActivity, devbouleToolName } from "../devbouleToolLine";
import { isCreationStartNote } from "./creatorNotes";
import { isFailedToolCall } from "./toolFailure";

/**
 * Agent-to-agent traffic the person does not read in this chat. The rows stay in
 * the journal and the Tasks panel; this only decides what the transcript draws.
 */

// Calls the orchestrator makes to answer or poll for its children's cards.
const PLUMBING_TOOLS = new Set(["answer_permission", "list_pending_permissions"]);

// A child's own status notice. Its needs-you state stays on the child's rail
// and tab, so the chat does not repeat it.
const CHILD_STATUS_NOTICES = new Set(["agent_input_required", "agent_quiet", "agent_idle_closed"]);

export function isAgentTraffic(item: AgentChatItem): boolean {
  switch (item.role) {
    case "system":
      // A running agent's row: the Tasks panel carries its state, the chat only its end.
      return (
        (item.task?.kind === "agent" && item.task.state === "running") ||
        isCreationStartNote(item.text)
      );
    case "tool": {
      const name = devbouleToolName(item.title);
      // The answer and listing rows stay out of the chat even when they fail: a
      // failure is the relay's own, and the person answers in the child's tab.
      if (PLUMBING_TOOLS.has(name)) return true;
      // A failed call is the person's to see, whatever it was for.
      if (isFailedToolCall(item)) return false;
      // A blocked agent is the one poll result the person needs; every other poll is noise.
      return name === "agent_activity" && devbouleActivity(item.output) !== "blocked";
    }
    // The relay of a child's card is what the orchestrator reads to answer it.
    // The person answers that card in the child's own tab.
    case "permission_request":
      return true;
    case "daemon_notice":
      return item.notice.recognized && CHILD_STATUS_NOTICES.has(item.notice.kind);
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
    if (devbouleToolName(item.title) !== "answer_permission") continue;
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
