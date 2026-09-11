import type { AgentChatItem } from "../../lib/agentSession";

export type ToolItem = Extract<AgentChatItem, { role: "tool" }>;

export type ToolIconName = "terminal" | "eye" | "pencil" | "search" | "bot" | "wrench";

export interface ToolRowModel {
  displayName: string;
  summary?: string;
  icon: ToolIconName;
}

const DISPLAY_NAMES: Record<string, string> = {
  execute: "Shell",
  read: "Read",
  edit: "Edit",
  delete: "Edit",
  search: "Search",
  fetch: "Fetch",
  think: "Task",
};

const ICONS: Record<string, ToolIconName> = {
  execute: "terminal",
  read: "eye",
  edit: "pencil",
  delete: "pencil",
  search: "search",
  fetch: "search",
  think: "bot",
};

/** Names with `:`, `.`, `/` or `__` are kept as-is; otherwise `[-_.]`→space, capitalize first. */
export function humanizeToolName(name: string): string {
  const trimmed = name.trim();
  if (!trimmed) return name;
  if (/[:./]/.test(trimmed) || trimmed.includes("__")) return trimmed;
  return trimmed
    .replace(/[._-]+/g, " ")
    .split(" ")
    .filter((segment) => segment.length > 0)
    .join(" ")
    .toLowerCase()
    .replace(/^./, (character) => character.toUpperCase());
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
  const known = kind !== undefined && DISPLAY_NAMES[kind] !== undefined;
  const fromBareName = !known && isBareName(item.title);
  const displayName = known
    ? kind === "think"
      ? thinkDisplayName(item.subagentType)
      : DISPLAY_NAMES[kind as string]
    : fromBareName
      ? humanizeToolName(item.title)
      : "Tool";
  let summary: string | undefined;
  if (!fromBareName) {
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
    icon: (kind !== undefined && ICONS[kind]) || "wrench",
  };
}
