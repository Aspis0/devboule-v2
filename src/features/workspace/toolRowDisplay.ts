import { browserToolName, isBrowserToolRow } from "../../lib/browserToolName";
import type { AgentChatItem } from "../../lib/agentSession";
import { linkTarget } from "../../lib/externalUrl";
import { carriesCredentials } from "../../lib/urlCredentials";

export type ToolItem = Extract<AgentChatItem, { role: "tool" }>;

export interface ToolRowModel {
  displayName: string;
  summary?: string;
  /** Set only for a fetch whose title is an http(s) URL: where the row links. */
  linkUrl?: string;
}

// Maps, not object literals: `kind` is provider-controlled and must not be
// able to read prototype members (`__proto__`, `constructor`, `toString`).
const DISPLAY_NAMES = new Map<string, string>([
  ["plan", "Plan"],
  ["execute", "Ran"],
  ["read", "Read"],
  ["edit", "Edited"],
  ["delete", "Deleted"],
  ["search", "Searched"],
  ["fetch", "Fetched"],
  ["think", "Task"],
  // An answered model question: the label names it, the summary (the
  // question itself, as the title) stays visible without a click.
  ["question", "Question"],
]);

// A call still running reads in the present; the past tense is for a finished one.
const RUNNING_NAMES = new Map<string, string>([
  ["execute", "Running"],
  ["read", "Reading"],
  ["edit", "Editing"],
  ["delete", "Deleting"],
  ["search", "Searching"],
  ["fetch", "Fetching"],
]);

// Not a cost bound — the split pass is linear: this is the INPUT length past
// which a name is shown as sent instead of split. The function never
// truncates; the row's CSS ellipsis bounds the label.
const MAX_HUMANIZED_LENGTH = 128;

/** Turn a tool name into a display label. `MAX_HUMANIZED_LENGTH` (128) caps
 * the INPUT: a longer name, or one containing `:`, `.`, `/` or `__`, comes
 * back trimmed and as sent — nothing is ever truncated, so only the CSS
 * ellipsis bounds what the row shows, and a name that goes through the split
 * can come out longer than it went in. Within the cap one linear pass over
 * code points: a word starts after a separator/whitespace run, after a
 * lowercase/number → uppercase step (`\p{Ll}`/`\p{N}` → `\p{Lu}`), or before
 * the last uppercase of an uppercase run that a lowercase follows; letters
 * that are neither upper- nor lowercase (CJK, titlecase) give no boundary.
 * Each word is case-mapped only when that keeps its code-point count,
 * otherwise its letters stay as sent, and only the first code point is
 * uppercased. */
export function humanizeToolName(name: string): string {
  const trimmed = name.trim();
  if (!trimmed) return name;
  if (trimmed.length > MAX_HUMANIZED_LENGTH) return trimmed;
  if (/[:./]/.test(trimmed) || trimmed.includes("__")) return trimmed;
  const words: string[] = [];
  const characters = Array.from(trimmed);
  let start = 0;
  for (let i = 0; i < characters.length; i += 1) {
    const character = characters[i]!;
    // `.`, `:`, `/` and `__` never reach the loop: they return as sent above.
    if (/[\s_-]/.test(character)) {
      if (i > start) words.push(characters.slice(start, i).join(""));
      start = i + 1;
      continue;
    }
    if (i === 0) continue;
    const previous = characters[i - 1]!;
    const next = characters[i + 1];
    const breaksWord =
      (/[\p{Ll}\p{N}]/u.test(previous) && /\p{Lu}/u.test(character)) ||
      (/\p{Lu}/u.test(previous) &&
        /\p{Lu}/u.test(character) &&
        next !== undefined &&
        /\p{Ll}/u.test(next));
    if (breaksWord) {
      words.push(characters.slice(start, i).join(""));
      start = i;
    }
  }
  words.push(characters.slice(start).join(""));
  return words
    .filter((word) => word.length > 0)
    .map((word, index) => {
      const isAcronym = word.length > 1 && word === word.toUpperCase();
      const mapped = isAcronym ? word : word.toLowerCase();
      const cased = sameCodePointCount(mapped, word) ? mapped : word;
      const capitalized =
        index === 0 ? cased.replace(/^./u, (character) => character.toUpperCase()) : cased;
      return sameCodePointCount(capitalized, cased) ? capitalized : cased;
    })
    .join(" ");
}

/** A case map may not change a word's code-point count: `ß` → `SS` or
 * `İ` → `i` + combining dot would no longer name the tool, so such words keep
 * their letters as sent. */
function sameCodePointCount(mapped: string, original: string): boolean {
  return Array.from(mapped).length === Array.from(original).length;
}

function isBareName(value: string): boolean {
  return value.length > 0 && !/[\s/:.]/.test(value);
}

function thinkDisplayName(subagentType?: string): string {
  const trimmed = subagentType?.trim();
  if (trimmed) return trimmed.charAt(0).toUpperCase() + trimmed.slice(1);
  return "Task";
}

/**
 * A fetch title is a link only when the system-browser command opens it. A URL
 * carrying a username or password keeps its host as the summary and gets no
 * `href`, so credentials never reach the DOM; any other title is shown as sent.
 */
function fetchDisplay(title: string): { summary?: string; linkUrl?: string } {
  if (title.length === 0) return {};
  if (!/^https?:\/\//i.test(title)) return { summary: title };
  const target = linkTarget(title);
  if (target !== null) return { summary: target.host, linkUrl: target.href };
  if (carriesCredentials(title)) {
    const url = parsedUrl(title);
    return url !== null && url.host.length > 0 ? { summary: url.host } : {};
  }
  return { summary: title };
}

/** The URL a title states, or null when no parser accepts it. */
function parsedUrl(value: string): URL | null {
  try {
    return new URL(value);
  } catch {
    return null;
  }
}

/**
 * A browser row's own line, or nothing when the row carries only the tool's
 * name: the daemon titles the call with the argument it was given (`click e33`),
 * and a provider that sends the bare name gives a reader nothing to add to the
 * family's label.
 */
function browserSummary(title: string): string | undefined {
  const trimmed = title.trim();
  return browserToolName(trimmed) === null && trimmed.length > 0 ? trimmed : undefined;
}

export function toolRowDisplay(item: ToolItem, running = false): ToolRowModel {
  const kind = item.kind?.trim().toLowerCase();
  if (isBrowserToolRow(kind, item.title)) {
    const summary = browserSummary(item.title);
    return {
      displayName: "Browser",
      ...(summary === undefined ? {} : { summary }),
    };
  }
  const label =
    kind === undefined
      ? undefined
      : ((running ? RUNNING_NAMES.get(kind) : undefined) ?? DISPLAY_NAMES.get(kind));
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
  let linkUrl: string | undefined;
  if (!fromBareName && kind !== "plan") {
    if (kind === "read" || kind === "edit" || kind === "delete") {
      const first = item.locations?.[0]?.path;
      summary =
        first !== undefined && first.length > 0
          ? first
          : item.title.length > 0
            ? item.title
            : undefined;
    } else if (kind === "fetch") {
      const fetch = fetchDisplay(item.title);
      summary = fetch.summary;
      linkUrl = fetch.linkUrl;
    } else {
      summary = item.title.length > 0 ? item.title : undefined;
    }
  }
  return {
    displayName,
    ...(summary === undefined ? {} : { summary }),
    ...(linkUrl === undefined ? {} : { linkUrl }),
  };
}
