export interface HistoryDayGroup<T> {
  key: string;
  label: string;
  entries: T[];
}

export interface HistorySearchFields {
  title?: string | null;
  workspace?: string | null;
  branch?: string | null;
  project?: string | null;
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

export function relativeTime(updatedAtMs: number | null | undefined, now: number): string {
  if (typeof updatedAtMs !== "number" || !Number.isFinite(updatedAtMs) || !Number.isFinite(now)) {
    return "—";
  }
  const elapsedMs = Math.max(0, now - updatedAtMs);
  if (elapsedMs < 60_000) return "just now";

  const minutes = Math.floor(elapsedMs / 60_000);
  if (minutes < 60) return `${minutes}m ago`;

  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `${hours}h ago`;

  const days = Math.floor(hours / 24);
  if (days < 7) return `${days}d ago`;
  const weeks = Math.floor(days / 7);
  if (weeks < 5) return `${weeks}w ago`;
  const months = Math.floor(days / 30);
  if (months < 12) return `${months}mo ago`;
  return `${Math.floor(days / 365)}y ago`;
}

export function historyRowMatches(
  row: HistorySearchFields | null | undefined,
  query: string | null | undefined,
): boolean {
  const normalizedQuery = query?.trim().toLowerCase() ?? "";
  if (!normalizedQuery) return true;
  if (!row) return false;

  return [row.title, row.workspace, row.branch, row.project].some(
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
