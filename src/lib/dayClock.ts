// One home for the local day/clock label the turn rail and the session
// overview both print: the clock when the instant falls on the given day,
// the date (year included) beside the time otherwise.

/** Constructed once at module scope: building an Intl formatter resolves
 * locale data, which no per-render call should pay. */
const CLOCK_TIME = new Intl.DateTimeFormat(undefined, {
  hour: "2-digit",
  minute: "2-digit",
});
const DATE_AND_TIME = new Intl.DateTimeFormat(undefined, {
  year: "numeric",
  month: "short",
  day: "numeric",
  hour: "2-digit",
  minute: "2-digit",
});

/** The local day an instant falls on, `YYYY-MM-DD` — the key "today"
 * is compared against. */
export function dayKey(ms: number): string {
  const date = new Date(ms);
  const month = String(date.getMonth() + 1).padStart(2, "0");
  const day = String(date.getDate()).padStart(2, "0");
  return `${date.getFullYear()}-${month}-${day}`;
}

/** The clock for one instant against one day key: time when it falls on
 * that day, date beside time otherwise. The repo's rule for a nullable ms
 * (relativeTime.ts, historyGrouping.ts): null, NaN and infinity are "no
 * time", never a 1970 date on the card. */
export function formatDayClock(atMs: number | null | undefined, today: string): string | null {
  if (typeof atMs !== "number" || !Number.isFinite(atMs)) return null;
  const at = new Date(atMs);
  // Finite but out of the Date range formats as an Invalid Date and throws inside Intl.
  if (Number.isNaN(at.getTime())) return null;
  return dayKey(at.getTime()) === today ? CLOCK_TIME.format(at) : DATE_AND_TIME.format(at);
}
