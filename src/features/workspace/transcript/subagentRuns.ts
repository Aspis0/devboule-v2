import type { AgentChatItem } from "../../../lib/agentSession";
import type { ToolChatItem } from "../../../lib/toolCallGroups";
import { devbouleResultName } from "../devbouleToolLine";
import { toolRowDisplay } from "../toolRowDisplay";
import { dropRecoveredFailures } from "./recoveredFailures";
import { isFailedToolCall } from "./toolFailure";

/**
 * Each sub-agent run becomes one quiet line, at the place of its spawning call.
 * The run's words and its failed calls are read from the line's click, so the
 * person can still see what a sub-agent did when it has no tab of its own.
 */

/** The spawning call a child item belongs to, or null for a row of the parent. */
function parentOf(item: AgentChatItem): string | null {
  return "parentToolUseId" in item &&
    typeof item.parentToolUseId === "string" &&
    item.parentToolUseId.length > 0
    ? item.parentToolUseId
    : null;
}

// A Devboule call's title is its tool's name; the agent it made is named by its result.
const DEVBOULE_CALL = /^(?:mcp__devboule__|devboule_)/;

/** The name a spawning call gives its run, or null when it names none. */
function spawnName(spawn: ToolChatItem): string | null {
  const title = spawn.title.trim();
  if (DEVBOULE_CALL.test(title)) return devbouleResultName(spawn.output) ?? null;
  return title === "" ? null : title;
}

/** The line a failed nested call reads as: what it was doing, in the verb its tool row gives. */
function failureLine(item: ToolChatItem): string {
  const model = toolRowDisplay(item, "failed");
  const target = model.summary === undefined ? "" : ` ${model.summary}`;
  return `Couldn't ${model.displayName.toLowerCase()}${target}`;
}

interface Run {
  description: string | null;
  items: AgentChatItem[];
}

export function foldSubagentRuns(items: AgentChatItem[]): AgentChatItem[] {
  const runs = new Map<string, Run>();
  for (const item of items) {
    const id = parentOf(item);
    if (id === null) continue;
    const run = runs.get(id) ?? { description: null, items: [] };
    runs.set(id, run);
    run.items.push(item);
  }
  for (const item of items) {
    if (item.role !== "tool" || parentOf(item) !== null) continue;
    const run = runs.get(item.toolCallId);
    if (run !== undefined) run.description = spawnName(item);
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
    const run = runs.get(id) ?? { description: null, items: [] };
    folded.push({
      id: `subagent-${id}`,
      role: "system",
      severity: "info",
      text: run.description === null ? "Subagent" : `Subagent · ${run.description}`,
      subagent: {
        texts: run.items.flatMap((child) =>
          (child.role === "assistant" || child.role === "thought") && child.text.trim() !== ""
            ? [child.text]
            : [],
        ),
        failures: dropRecoveredFailures(run.items).flatMap((child) =>
          child.role === "tool" && isFailedToolCall(child) ? [failureLine(child)] : [],
        ),
      },
    });
  }
  return folded;
}
