import type { SessionKind } from "../../types/ipc";
import { relativeTime } from "../../lib/relativeTime";
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
}

interface TimestampedEntry {
  updatedAtMs?: number | null;
  /** An open session with no timestamp yet: grouped with today, sorted first. */
  groupWithToday?: boolean;
}

export function groupByDay<T extends TimestampedEntry>(
  entries: readonly T[] | null | undefined,
  now: number,
): HistoryDayGroup<T>[] {
  const todayKey = localDayKey(now);
  const yesterday = new Date(now);
  if (todayKey !== null && Number.isFinite(now)) yesterday.setDate(yesterday.getDate() - 1);
  const yesterdayKey = todayKey === null ? null : localDayKey(yesterday.getTime());
  // Timestamp-less live rows rank first explicitly; the fallback avoids the
  // NaN that an Infinity-minus-Infinity comparator would produce, so their
  // relative order stays the insertion order (stable sort) by construction.
  const sortedEntries = [...(entries ?? [])].sort(
    (first, second) =>
      rankForSort(first) - rankForSort(second) ||
      timestampForSort(second) - timestampForSort(first) ||
      0,
  );
  const groups = new Map<string, HistoryDayGroup<T>>();

  for (const entry of sortedEntries) {
    const key =
      localDayKey(entry.updatedAtMs) ??
      (entry.groupWithToday && todayKey !== null ? todayKey : "unknown");
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

  // The name the row shows, from the one function that decides it, plus the
  // stored fields the list can hold while open: id and title for a renamed
  // row, kind so the show-all list answers "terminal", workspace, branch
  // and project for the meta line. None of these is painted, so a query can
  // match where the eye sees nothing — that is the price of searching rows
  // whose fields may still be loading.
  const shownName = sessionTitle({
    id: row.id ?? "",
    title: row.title ?? "",
    kind: row.kind ?? "terminal",
    displayName: row.displayName ?? undefined,
  });

  return [shownName, row.id, row.title, row.kind, row.workspace, row.branch, row.project].some(
    (value) => typeof value === "string" && value.toLowerCase().includes(normalizedQuery),
  );
}

export function historyRelativeTime(
  timestamp: number | null | undefined,
  now: number,
): string | null {
  if (
    typeof timestamp !== "number" ||
    !Number.isFinite(timestamp) ||
    !Number.isFinite(now) ||
    Number.isNaN(new Date(timestamp).getTime()) ||
    Number.isNaN(new Date(now).getTime())
  ) {
    return null;
  }

  const days = calendarDayNumber(now) - calendarDayNumber(timestamp);
  if (days <= 0) return relativeTime(timestamp, now);
  if (days < 7) return `${days}d ago`;
  const weeks = Math.floor(days / 7);
  if (weeks < 5) return `${weeks}w ago`;
  const months = Math.floor(days / 30);
  if (months < 12) return `${months}mo ago`;
  return `${Math.floor(days / 365)}y ago`;
}

function rankForSort(entry: TimestampedEntry): number {
  return entry.groupWithToday && typeof entry.updatedAtMs !== "number" ? 0 : 1;
}

function timestampForSort(entry: TimestampedEntry): number {
  if (typeof entry.updatedAtMs === "number" && Number.isFinite(entry.updatedAtMs))
    return entry.updatedAtMs;
  return Number.NEGATIVE_INFINITY;
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

function calendarDayNumber(timestamp: number): number {
  const date = new Date(timestamp);
  const utcDay = new Date(0);
  utcDay.setUTCFullYear(date.getFullYear(), date.getMonth(), date.getDate());
  utcDay.setUTCHours(0, 0, 0, 0);
  return utcDay.getTime() / 86_400_000;
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
