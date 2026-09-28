import type { SessionKind } from "../../types/ipc";
import { sessionTitle } from "../workspace/workspaceSessions";

export interface HistoryDayGroup<T> {
  key: string;
  label: string;
  entries: T[];
}

export interface HistorySearchFields {
  /** Identity, read by `sessionTitle` so the search matches the shown name. */
  id?: string | null;
  title?: string | null;
  kind?: SessionKind | null;
  displayName?: string | null;
  workspace?: string | null;
  branch?: string | null;
  project?: string | null;
  host?: string | null;
}

interface TimestampedEntry {
  updatedAtMs?: number | null;
}

export function groupByDay<T extends TimestampedEntry>(
  entries: readonly T[] | null | undefined,
  now: number,
): HistoryDayGroup<T>[] {
  const todayKey = localDayKey(now);
  const yesterday = new Date(now);
  if (todayKey !== null && Number.isFinite(now)) yesterday.setDate(yesterday.getDate() - 1);
  const yesterdayKey = todayKey === null ? null : localDayKey(yesterday.getTime());
  const sortedEntries = [...(entries ?? [])].sort(
    (first, second) => timestampForSort(second) - timestampForSort(first),
  );
  const groups = new Map<string, HistoryDayGroup<T>>();

  for (const entry of sortedEntries) {
    const key = localDayKey(entry.updatedAtMs) ?? "unknown";
    let group = groups.get(key);
    if (!group) {
      group = {
        key,
        label:
          key === todayKey
            ? "Today"
            : key === yesterdayKey
              ? "Yesterday"
              : dateLabel(entry.updatedAtMs),
        entries: [],
      };
      groups.set(key, group);
    }
    group.entries.push(entry);
  }

  return [...groups.values()];
}

export function historyRowMatches(
  row: HistorySearchFields | null | undefined,
  query: string | null | undefined,
): boolean {
  const normalizedQuery = query?.trim().toLowerCase() ?? "";
  if (!normalizedQuery) return true;
  if (!row) return false;

  // The name the row shows, from the one function that decides it — the search
  // may not be narrower than what the user has just read. The title stays in
  // the haystack beside it: a row whose display name covers the title is still
  // the same session, and dropping the title would take away a word that used
  // to find it.
  const shownName = sessionTitle({
    id: row.id ?? "",
    title: row.title ?? "",
    kind: row.kind ?? "terminal",
    displayName: row.displayName ?? undefined,
  });

  return [shownName, row.title, row.workspace, row.branch, row.project, row.host].some(
    (value) => typeof value === "string" && value.toLowerCase().includes(normalizedQuery),
  );
}

function timestampForSort(entry: TimestampedEntry): number {
  return typeof entry.updatedAtMs === "number" && Number.isFinite(entry.updatedAtMs)
    ? entry.updatedAtMs
    : Number.NEGATIVE_INFINITY;
}

function localDayKey(timestamp: number | null | undefined): string | null {
  if (typeof timestamp !== "number" || !Number.isFinite(timestamp)) return null;
  const date = new Date(timestamp);
  if (Number.isNaN(date.getTime())) return null;
  const year = date.getFullYear();
  const month = String(date.getMonth() + 1).padStart(2, "0");
  const day = String(date.getDate()).padStart(2, "0");
  return `${year}-${month}-${day}`;
}

function dateLabel(timestamp: number | null | undefined): string {
  if (typeof timestamp !== "number" || !Number.isFinite(timestamp)) return "Unknown date";
  const date = new Date(timestamp);
  if (Number.isNaN(date.getTime())) return "Unknown date";
  const months = [
    "Jan",
    "Feb",
    "Mar",
    "Apr",
    "May",
    "Jun",
    "Jul",
    "Aug",
    "Sep",
    "Oct",
    "Nov",
    "Dec",
  ];
  return `${date.getDate()} ${months[date.getMonth()]} ${date.getFullYear()}`;
}
