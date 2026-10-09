import { browserToolName, isBrowserToolRow } from "../../lib/browserToolName";

/**
 * The verb a tool line reads as, for the daemon's own tools. A terminal tool
 * arrives as `devboule_<name>` (or with Claude's `mcp__devboule__` qualifier);
 * a browser call's title is its verb and its target, `click e33`.
 */

const TERMINAL_VERBS = new Map<string, string>([
  ["create_terminal", "Create terminal"],
  ["send_terminal_keys", "Send keys to terminal"],
  ["capture_terminal", "Capture terminal"],
  ["list_terminals", "List terminals"],
  ["kill_terminal", "Close terminal"],
]);

const QUALIFIERS = /^(?:mcp__devboule__)?(?:devboule_)?/;

/** The line's verb for a terminal tool, or undefined for any other name. */
export function terminalToolVerb(title: string): string | undefined {
  return TERMINAL_VERBS.get(title.trim().replace(QUALIFIERS, ""));
}

// The browser lane's commands, spelled as the daemon titles them: the tool name
// with its underscores as spaces. A two-word command is matched before its own
// first word, so `new tab` is not read as `new` with a target.
const BROWSER_VERBS = [
  "act",
  "check",
  "click",
  "click at",
  "close tab",
  "console logs",
  "export cookies",
  "fill",
  "fill login",
  "find",
  "hover",
  "list tabs",
  "navigate",
  "new tab",
  "press",
  "read text",
  "screenshot",
  "screenshot all",
  "scroll",
  "select",
  "snapshot",
  "type",
  "wait for",
].sort((left, right) => right.length - left.length);

/** A browser title read as its verb and target, or null when it names no known verb. */
export function browserVerb(title: string): { verb: string; target?: string } | null {
  const trimmed = title.trim();
  const named = BROWSER_VERBS.find((verb) => trimmed === verb || trimmed.startsWith(`${verb} `));
  if (named === undefined) return null;
  const target = trimmed.slice(named.length).trim();
  const verb = named.charAt(0).toUpperCase() + named.slice(1);
  return target.length > 0 ? { verb, target } : { verb };
}

/**
 * Whether a row keeps its output out of the transcript: every browser call, and
 * a terminal capture. Their text is for the agent; the person reads a screenshot.
 */
export function rowShowsNoOutput(kind: string | undefined, title: string): boolean {
  return isBrowserToolRow(kind, title) || terminalToolVerb(title) === "Capture terminal";
}

/** A browser screenshot, bare or with its verb and target: the one call that shows a picture. */
export function isScreenshotRow(kind: string | undefined, title: string): boolean {
  if (!isBrowserToolRow(kind, title)) return false;
  const named = browserToolName(title.trim()) ?? title.trim();
  return /^(?:browser_)?screenshot(?:[ _]all)?(?:\s|$)/.test(named);
}
