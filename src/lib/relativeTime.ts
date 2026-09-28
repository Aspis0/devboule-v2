/**
 * How long ago one instant was, in the coarsest unit that fits — the
 * wording shared by the history rows and the Changes panel's commit list.
 * Moved out of the history feature so a commit row does not have to import
 * the session roster to print its own time. DevicesPanel keeps its own copy
 * with different thresholds: declared duplication, not shared.
 */
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
