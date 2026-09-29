import type { AgentChatItem } from "../../lib/agentSession";

export type ToolItem = Extract<AgentChatItem, { role: "tool" }>;

export type ToolIconName = "terminal" | "eye" | "pencil" | "search" | "bot" | "wrench";

export interface ToolRowModel {
  displayName: string;
  summary?: string;
  icon: ToolIconName;
}

// Maps, not object literals: `kind` is provider-controlled and must not be
// able to read prototype members (`__proto__`, `constructor`, `toString`).
const DISPLAY_NAMES = new Map<string, string>([
  ["plan", "Plan"],
  ["execute", "Shell"],
  ["read", "Read"],
  ["edit", "Edit"],
  ["delete", "Edit"],
  ["search", "Search"],
  ["fetch", "Fetch"],
  ["think", "Task"],
  // An answered model question: the label names it, the summary (the
  // question itself, as the title) stays visible without a click.
  ["question", "Question"],
]);

const ICONS = new Map<string, ToolIconName>([
  ["plan", "eye"],
  ["execute", "terminal"],
  ["read", "eye"],
  ["edit", "pencil"],
  ["delete", "pencil"],
  ["search", "search"],
  ["fetch", "search"],
  ["think", "bot"],
  ["question", "bot"],
]);

// Past this cap a name is shown as sent: provider strings are uncapped, and
// the split pass is not worth its cost on them.
const MAX_HUMANIZED_LENGTH = 128;

/** Names with `:`, `.`, `/` or `__` are kept as sent. Otherwise, one linear
 * split rule: a word starts after a separator/whitespace run, after a
 * lower/digit → uppercase step, or before the last uppercase of an uppercase
 * run that a lowercase follows. */
export function humanizeToolName(name: string): string {
  const trimmed = name.trim();
  if (!trimmed) return name;
  if (trimmed.length > MAX_HUMANIZED_LENGTH) return trimmed;
  if (/[:./]/.test(trimmed) || trimmed.includes("__")) return trimmed;
  const words: string[] = [];
  let start = 0;
  for (let i = 0; i < trimmed.length; i += 1) {
    const character = trimmed[i]!;
    if (/[\s._-]/.test(character)) {
      if (i > start) words.push(trimmed.slice(start, i));
      start = i + 1;
      continue;
    }
    if (i === 0) continue;
    const previous = trimmed[i - 1]!;
    const next = trimmed[i + 1];
    const breaksWord =
      (/[a-z0-9]/.test(previous) && /[A-Z]/.test(character)) ||
      (/[A-Z]/.test(previous) &&
        /[A-Z]/.test(character) &&
        next !== undefined &&
        /[a-z]/.test(next));
    if (breaksWord) {
      words.push(trimmed.slice(start, i));
      start = i;
    }
  }
  words.push(trimmed.slice(start));
  return words
    .filter((word) => word.length > 0)
    .map((word, index) => {
      const isAcronym = word.length > 1 && word === word.toUpperCase();
      const cased = isAcronym ? word : word.toLowerCase();
      return index === 0 ? cased.replace(/^./, (character) => character.toUpperCase()) : cased;
    })
    .join(" ");
}

function isBareName(value: string): boolean {
  return value.length > 0 && !/[\s/:.]/.test(value);
}

function thinkDisplayName(subagentType?: string): string {
  const trimmed = subagentType?.trim();
  if (trimmed) return trimmed.charAt(0).toUpperCase() + trimmed.slice(1);
  return "Task";
}

export function toolRowDisplay(item: ToolItem): ToolRowModel {
  const kind = item.kind?.trim().toLowerCase();
  const label = kind !== undefined ? DISPLAY_NAMES.get(kind) : undefined;
  const fromBareName = label === undefined && isBareName(item.title);
  const displayName =
    label !== undefined
      ? kind === "think"
        ? thinkDisplayName(item.subagentType)
        : label
      : fromBareName
        ? // A separators-only title humanizes to ""; the raw title beats an
          // empty label.
          humanizeToolName(item.title) || item.title
        : "Tool";
  let summary: string | undefined;
  if (!fromBareName && kind !== "plan") {
    if (kind === "read" || kind === "edit" || kind === "delete") {
      const first = item.locations?.[0]?.path;
      summary =
        first !== undefined && first.length > 0
          ? first
          : item.title.length > 0
            ? item.title
            : undefined;
    } else {
      summary = item.title.length > 0 ? item.title : undefined;
    }
  }
  return {
    displayName,
    ...(summary === undefined ? {} : { summary }),
    icon: (kind !== undefined ? ICONS.get(kind) : undefined) ?? "wrench",
  };
}
