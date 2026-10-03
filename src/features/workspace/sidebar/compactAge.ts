/**
 * How long ago a workspace's last output was, in the vocabulary a row's first
 * line has room for: "now", "4m", "3h", "2d", then the wider units rather
 * than a three-digit day count.
 *
 * The input is the roster's `elapsedMs`, a DURATION counted since the last
 * observed output — not an instant — so it is read directly and never
 * subtracted from a clock. Null in, null out: a recovered row has no runtime to
 * have reported a silence, which is unknown, not "a long time ago".
 */
const MINUTE_MS = 60_000;

export function compactAge(elapsedMs: number | null): string | null {
  if (elapsedMs === null || !Number.isFinite(elapsedMs)) return null;
  const elapsed = Math.max(0, elapsedMs);
  if (elapsed < MINUTE_MS) return "now";

  const minutes = Math.floor(elapsed / MINUTE_MS);
  if (minutes < 60) return `${minutes}m`;

  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `${hours}h`;

  const days = Math.floor(hours / 24);
  if (days < 7) return `${days}d`;

  const weeks = Math.floor(days / 7);
  if (weeks < 5) return `${weeks}w`;

  const months = Math.floor(days / 30);
  if (months < 12) return `${months}mo`;
  return `${Math.floor(days / 365)}y`;
}
