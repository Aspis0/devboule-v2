import { formatTaskDuration } from "../../lib/backgroundTaskText";
import type { ToolChatItem } from "../../lib/toolCallGroups";
import type { ToolPhase } from "./toolRowDisplay";

/**
 * The one line a Devboule agent tool reads as: its verb, the agent it touched and
 * what that came to. The raw result stays behind the line's click, except a
 * created agent's, which the chat never shows. A call's title is the tool's name,
 * optionally with its Claude or MCP qualifier, and any words after the name are
 * the agent's name when the result names none.
 */

type Family = "activity" | "send" | "create" | "cancel" | "stop";

const FAMILY_BY_NAME = new Map<string, Family>([
  ["agent_activity", "activity"],
  ["send_message", "send"],
  ["create_agent", "create"],
  ["cancel_agent", "cancel"],
  ["stop_agent", "stop"],
]);

// A call that did not finish reads as what it did not do; a cancelled call is never a success.
const VERBS: Record<Family, { done: string; running: string; failed: string }> = {
  activity: { done: "Checked", running: "Checking", failed: "Couldn't check" },
  send: {
    done: "Sent a message to",
    running: "Sending a message to",
    failed: "Couldn't send a message to",
  },
  create: { done: "Started", running: "Starting", failed: "Couldn't start" },
  cancel: { done: "Canceled", running: "Canceling", failed: "Couldn't cancel" },
  stop: { done: "Stopped", running: "Stopping", failed: "Couldn't stop" },
};

const CANCELLED_SUBJECT: Record<Family, string> = {
  activity: "check of",
  send: "message to",
  create: "start of",
  cancel: "cancel of",
  stop: "stop of",
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

/** The activity a check call's result reports, or undefined when it reports none. */
export function devbouleActivity(output: string): string | undefined {
  return resultActivity(parseResult(output));
}

/** The child session a call's result names: what a click on the line opens. */
export function devbouleChildSession(output: string): string | undefined {
  const value = parseResult(output)?.sessionId;
  return typeof value === "string" && value !== "" ? value : undefined;
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
  const named = resultName(result) ?? (rest === "" ? undefined : rest);
  if (phase === "cancelled")
    return `Cancelled · ${CANCELLED_SUBJECT[family]} ${named ?? UNNAMED_AGENT}`;
  if (family === "create") {
    if (phase === "failed") return "Couldn't start the agent";
    const head = `${phase === "running" ? "Starting" : "Started"} agent`;
    return named === undefined ? head : `${head} · ${named}`;
  }
  const agent = named ?? UNNAMED_AGENT;
  const verb = VERBS[family][phase === "running" || phase === "failed" ? phase : "done"];
  if (family === "activity") {
    const activity = resultActivity(result);
    if (activity === "blocked") return `${agent} is blocked`;
    return activity === undefined ? `${verb} ${agent}` : `${verb} ${agent}: ${activity}`;
  }
  if (family === "stop") {
    const took = resultDuration(result);
    return took === null ? `${verb} ${agent}` : `${verb} ${agent} · ${took}`;
  }
  return `${verb} ${agent}`;
}

/** Whether a Devboule call's raw result may be opened behind its line. A created agent's result is never shown. */
export function devbouleShowsOutput(title: string): boolean {
  return FAMILY_BY_NAME.get(splitTitle(title).name) !== "create";
}
