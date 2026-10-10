/**
 * The thinking options a live session's picker offers. The daemon takes Claude's
 * `max` only as a launch flag, so a live change to it is refused: for Claude the
 * live picker lists `max` only as the level the session already has. A creation
 * form is not live and keeps the full list.
 */
export function liveEffortOptions<T extends { id: string }>(
  efforts: readonly T[],
  providerId: string | null | undefined,
  currentId: string | null | undefined,
): T[] {
  if (providerId !== "claude") return [...efforts];
  return efforts.filter((effort) => effort.id !== "max" || effort.id === currentId);
}
