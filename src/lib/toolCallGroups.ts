import type { AgentChatItem } from "./agentSession";

export type ToolChatItem = Extract<AgentChatItem, { role: "tool" }>;

/**
 * A run of consecutive groupable tool calls, rendered as one collapsible row.
 *
 * Grouping follows Paseo's `prepareGroupedHistory`
 * (`packages/app/src/tool-calls/detail-level/grouping.ts:115-148`): consecutive
 * groupable tools accumulate into a pending run and any other item flushes it.
 * The id is the first item's id, so it stays stable while streaming appends
 * tools to the end of the run.
 */
export interface ToolCallGroup {
  kind: "tool-group";
  id: string;
  items: ToolChatItem[];
  summary: string;
}

export function isToolCallGroup(entry: AgentChatItem | ToolCallGroup): entry is ToolCallGroup {
  // A group has no `role`, which every chat item carries: a wire item cannot
  // forge this discriminant even with `kind: "tool-group"`.
  return !("role" in entry) && entry.kind === "tool-group";
}

/**
 * Paseo's `isGroupableToolCall` (`grouping.ts:85-90`) excludes the `plan`
 * detail type and the `speak` tool name. Here `kind === "plan"` is the plan
 * exclusion; `speak` has no equivalent — no tool kind maps to it
 * (`toolRowDisplay.ts` knows `execute/read/edit/delete/search/fetch/think`),
 * so only `plan` is excluded.
 */
export function isGroupableToolCall(item: AgentChatItem): item is ToolChatItem {
  if (item.role !== "tool") return false;
  return item.kind?.trim().toLowerCase() !== "plan";
}

/**
 * Batch consecutive groupable tools into runs. A run never mixes depths: a
 * tool joins the pending run only when its `parentToolUseId` and `spawnDepth`
 * equal the run's first item's, so parent-level and subagent rows never share
 * a collapsed wrapper. A run of one stays a plain item: unlike Paseo's
 * overview level (which wraps even single calls in a group object — see
 * `projection.test.ts` "builds a loading aggregate for a one-call run"),
 * this surface has no detail-level switch, so a one-item group would only
 * add a useless collapsed wrapper around a single row.
 */
export function groupToolCalls(items: AgentChatItem[]): Array<AgentChatItem | ToolCallGroup> {
  const output: Array<AgentChatItem | ToolCallGroup> = [];
  let pending: ToolChatItem[] = [];
  let pendingKey: string | null = null;
  const flush = () => {
    if (pending.length === 0) return;
    if (pending.length === 1) {
      const single = pending[0];
      if (single !== undefined) output.push(single);
    } else {
      const first = pending[0];
      if (first !== undefined) {
        const run = [...pending];
        output.push({
          kind: "tool-group",
          id: first.id,
          items: run,
          summary: summarizeToolCallGroup(run),
        });
      }
    }
    pending = [];
    pendingKey = null;
  };
  for (const item of items) {
    if (isGroupableToolCall(item)) {
      // `?? ""` mirrors the surface: an absent parent or depth is the
      // parent level, and two absences are equal.
      const key = `${item.parentToolUseId ?? ""}:${item.spawnDepth ?? ""}`;
      if (pendingKey !== null && pendingKey !== key) flush();
      pending.push(item);
      pendingKey = key;
      continue;
    }
    flush();
    output.push(item);
  }
  flush();
  return output;
}

export interface ToolCallGroupSummary {
  editedFileCount: number;
  commandCount: number;
  readFileCount: number;
  searchCount: number;
  otherToolCount: number;
}

/**
 * Count a run the way Paseo's `buildOverviewGroup`
 * (`packages/app/src/tool-calls/detail-level/overview/model.ts:31-79`) does:
 * edited and read files dedupe by path, everything else counts calls. The
 * `paseoCalls` bucket has no equivalent here and is omitted; `fetch` falls
 * into `otherToolCount` exactly as Paseo's unbucketed `fetch` detail type
 * does. Paseo keys by `detail.filePath`, which its protocol always provides
 * for edit/read details, and has no unkeyed branch and no summary fallback
 * (`buildOverviewGroup` returns the six counts unconditionally); a
 * location-less edit/read here cannot name a file, so it counts as another
 * tool instead of guessing by title.
 */
export function countToolCallGroup(items: readonly ToolChatItem[]): ToolCallGroupSummary {
  const editedFiles = new Set<string>();
  const readFiles = new Set<string>();
  let commandCount = 0;
  let searchCount = 0;
  let otherToolCount = 0;
  for (const item of items) {
    const kind = item.kind?.trim().toLowerCase() ?? "";
    const filePath = item.locations?.[0]?.path;
    if ((kind === "edit" || kind === "delete") && filePath !== undefined) {
      editedFiles.add(filePath);
    } else if (kind === "read" && filePath !== undefined) {
      readFiles.add(filePath);
    } else if (kind === "execute") {
      commandCount += 1;
    } else if (kind === "search") {
      searchCount += 1;
    } else {
      otherToolCount += 1;
    }
  }
  return {
    editedFileCount: editedFiles.size,
    commandCount,
    readFileCount: readFiles.size,
    searchCount,
    otherToolCount,
  };
}

// English strings copied from Paseo's locale
// (`packages/app/src/i18n/resources/en.ts:1853-1879`, `toolCallGroup.*`).
function pluralize(count: number, one: string, other: string): string {
  return count === 1 ? one.replace("{{count}}", "1") : other.replace("{{count}}", String(count));
}

/**
 * Join summary parts the way Paseo's `useOverviewSummary` + `joinSummaryParts`
 * (`packages/app/src/tool-calls/detail-level/overview/view.tsx:24-60`) do:
 * two parts join with "and", three or more use ", " with "and" before the
 * last, and the first character is uppercased.
 */
function joinSummaryParts(parts: string[], conjunction: string): string {
  if (parts.length === 0) return "";
  let joined = parts[0] ?? "";
  if (parts.length === 2) {
    joined = `${parts[0]} ${conjunction} ${parts[1]}`;
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
    [summary.otherToolCount, "used {{count}} other tool", "used {{count}} other tools"],
  ] as const;
  for (const [count, one, other] of entries) {
    if (count > 0) parts.push(pluralize(count, one, other));
  }
  // Every item lands in exactly one bucket, so a group (always 2+ items)
  // always yields at least one part. There is no `"N tool calls"`
  // fallback — Paseo has none either.
  return joinSummaryParts(parts, "and");
}
