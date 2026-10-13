import { isBrowserToolRow } from "./browserToolName";
import type { ToolChatItem } from "./toolCallGroups";

export interface ToolCallGroupSummary {
  editedFileCount: number;
  commandCount: number;
  readFileCount: number;
  searchCount: number;
  browserCallCount: number;
  otherToolCount: number;
}

/**
 * Count a run: edited and read files dedupe by path over every location a row
 * names, so one batch row counts each file it touched once; everything else
 * counts calls. A `fetch` is not one of the counted kinds here, so it lands in
 * `otherToolCount`. A row that names no file cannot be counted by file, so it
 * counts as another tool instead of guessing by title. A browser call is one
 * kind of its own: the row shows as one, and a run that hides twenty browser
 * calls behind "other tools" says nothing about what the agent did.
 */
export function countToolCallGroup(items: readonly ToolChatItem[]): ToolCallGroupSummary {
  const editedFiles = new Set<string>();
  const readFiles = new Set<string>();
  let commandCount = 0;
  let searchCount = 0;
  let browserCallCount = 0;
  let otherToolCount = 0;
  for (const item of items) {
    const kind = item.kind?.trim().toLowerCase() ?? "";
    if (kind === "edit" || kind === "delete" || kind === "read") {
      const files = kind === "read" ? readFiles : editedFiles;
      let named = false;
      for (const location of item.locations ?? []) {
        if (!location.path) continue;
        files.add(location.path);
        named = true;
      }
      if (!named) otherToolCount += 1;
    } else if (kind === "execute") {
      commandCount += 1;
    } else if (kind === "search") {
      searchCount += 1;
    } else if (kind === "browser" || isBrowserToolRow(kind, item.title)) {
      // `isBrowserToolRow` answers the row for `other` and for no kind, so a
      // row journaled before the daemon learned Claude's spelling counts here
      // too. `kind` was already checked and is none of the named kinds.
      browserCallCount += 1;
    } else {
      otherToolCount += 1;
    }
  }
  return {
    editedFileCount: editedFiles.size,
    commandCount,
    readFileCount: readFiles.size,
    searchCount,
    browserCallCount,
    otherToolCount,
  };
}

// Summary strings copied from Paseo's locale (`packages/app/src/i18n/resources/en.ts:1853-1879`).
function pluralize(count: number, one: string, other: string): string {
  return count === 1 ? one.replace("{{count}}", "1") : other.replace("{{count}}", String(count));
}

/**
 * Join summary parts with ", ", and "and" before the last one from three on:
 * "Edited 2 files, ran 8 commands" and "Edited 1 file, ran 2 commands, and read
 * 1 file". The first character is uppercased.
 */
function joinSummaryParts(parts: string[], conjunction: string): string {
  if (parts.length === 0) return "";
  let joined = parts[0] ?? "";
  if (parts.length === 2) {
    joined = `${parts[0]}, ${parts[1]}`;
  } else if (parts.length > 2) {
    joined = `${parts.slice(0, -1).join(", ")}, ${conjunction} ${parts.at(-1)}`;
  }
  const first = joined[0];
  return first ? `${first.toLocaleUpperCase()}${joined.slice(1)}` : joined;
}

export function summarizeToolCallGroup(items: readonly ToolChatItem[]): string {
  const summary = countToolCallGroup(items);
  const parts: string[] = [];
  const entries = [
    [summary.editedFileCount, "edited {{count}} file", "edited {{count}} files"],
    [summary.commandCount, "ran {{count}} command", "ran {{count}} commands"],
    [summary.readFileCount, "read {{count}} file", "read {{count}} files"],
    [summary.searchCount, "searched {{count}} time", "searched {{count}} times"],
    [summary.browserCallCount, "used {{count}} browser tool", "used {{count}} browser tools"],
    [summary.otherToolCount, "used {{count}} tool", "used {{count}} tools"],
  ] as const;
  for (const [count, one, other] of entries) {
    if (count > 0) parts.push(pluralize(count, one, other));
  }
  // Every item lands in exactly one bucket, so a group (always 2+ items)
  // always yields at least one part. There is no `"N tool calls"` fallback.
  return joinSummaryParts(parts, "and");
}
