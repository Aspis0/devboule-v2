import { formatTaskDuration } from "../../lib/backgroundTaskText";
import type { ToolChatItem } from "../../lib/toolCallGroups";
import type { ToolPhase } from "./toolRowDisplay";

/**
 * The one line a Devboule agent tool reads as: its verb, the agent it touched and
 * what that came to. The raw result stays behind the line's click. A call's
 * title is the tool's name, optionally with its Claude or MCP qualifier, and any
 * words after the name are the agent's name when the result names none.
 */

type Family = "activity" | "send" | "create" | "cancel" | "stop";

const FAMILY_BY_NAME = new Map<string, Family>([
  ["agent_activity", "activity"],
  ["send_message", "send"],
  ["create_agent", "create"],
  ["cancel_agent", "cancel"],
  ["stop_agent", "stop"],
]);

const VERBS: Record<Family, { done: string; running: string }> = {
  activity: { done: "Checked", running: "Checking" },
  send: { done: "Sent a message to", running: "Sending a message to" },
  create: { done: "Created", running: "Creating" },
  cancel: { done: "Canceled", running: "Canceling" },
  stop: { done: "Stopped", running: "Stopping" },
};

const QUALIFIER = /^(?:mcp__devboule__)?(?:devboule_)?/;

/** The agent's name when the result gives none: "an agent" reads as a person would say it. */
const UNNAMED_AGENT = "an agent";

const NAME_KEYS = ["displayName", "title", "name", "childName"] as const;
// The stop result's run time, from the child's start to the stop (`runMs`).
const DURATION_KEYS = ["runMs"] as const;
const ACTIVITY_STATES = new Set(["idle", "working", "blocked", "unknown"]);

type Result = Record<string, unknown>;

function parseResult(output: string): Result | null {
  try {
    const value: unknown = JSON.parse(output);
    return typeof value === "object" && value !== null && !Array.isArray(value)
      ? (value as Result)
      : null;
  } catch {
    return null;
  }
}

function resultName(result: Result | null): string | undefined {
  if (result === null) return undefined;
  for (const key of NAME_KEYS) {
    const value = result[key];
    if (typeof value === "string" && value.trim() !== "") return value.trim();
  }
  return undefined;
}

function resultDuration(result: Result | null): string | null {
  if (result === null) return null;
  for (const key of DURATION_KEYS) {
    const value = result[key];
    if (typeof value === "number" && Number.isFinite(value)) {
      return formatTaskDuration(value);
    }
  }
  return null;
}

function resultActivity(result: Result | null): string | undefined {
  const value = result?.activity;
  return typeof value === "string" && ACTIVITY_STATES.has(value) ? value : undefined;
}

/** The tool's name, and the words after it in the title. */
function splitTitle(title: string): { name: string; rest: string } {
  const bare = title.trim().replace(QUALIFIER, "");
  const space = bare.search(/\s/);
  if (space < 0) return { name: bare, rest: "" };
  return { name: bare.slice(0, space), rest: bare.slice(space).trim() };
}

/** The bare name of a call, with its MCP qualifier and any words after it dropped. */
export function devbouleToolName(title: string): string {
  return splitTitle(title).name;
}

/** The line for a Devboule agent tool, or undefined for any other call. */
export function devbouleToolLine(
  item: ToolChatItem,
  phase: ToolPhase = "done",
): string | undefined {
  const { name, rest } = splitTitle(item.title);
  const family = FAMILY_BY_NAME.get(name);
  if (family === undefined) return undefined;
  const result = parseResult(item.output);
  const agent = resultName(result) ?? (rest === "" ? UNNAMED_AGENT : rest);
  const verb = VERBS[family][phase === "running" ? "running" : "done"];
  if (family === "activity") {
    const activity = resultActivity(result);
    return activity === undefined ? `${verb} ${agent}` : `${verb} ${agent}: ${activity}`;
  }
  if (family === "stop") {
    const took = resultDuration(result);
    return took === null ? `${verb} ${agent}` : `${verb} ${agent} (${took})`;
  }
  return `${verb} ${agent}`;
}
