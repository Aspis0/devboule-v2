// Why: one place decides what each close entry actually closes — the strip's
// visible order, the anchor tab exclusive for left/right/others, and a
// multi-selection intersected with what is on screen. The lists are the
// composed strip (sessions plus tool tabs), sliced by id; the caller
// partitions the answer by what a close means for each kind.

export type TabCloseAction = "close" | "left" | "right" | "others";

export function sessionsForTabAction<T extends { id: string }>(
  action: TabCloseAction,
  sessions: readonly T[],
  anchorId: string,
): T[] {
  const index = sessions.findIndex((session) => session.id === anchorId);
  if (index === -1) return [];
  if (action === "left") return [...sessions.slice(0, index)];
  if (action === "right") return [...sessions.slice(index + 1)];
  if (action === "others") return sessions.filter((session) => session.id !== anchorId);
  return [sessions[index]];
}

export function sessionsForSelection<T extends { id: string }>(
  selection: ReadonlySet<string>,
  sessions: readonly T[],
): T[] {
  return sessions.filter((session) => selection.has(session.id));
}
