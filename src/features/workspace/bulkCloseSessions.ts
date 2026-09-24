// Why: one place decides what each close entry actually closes — the strip's
// visible order, the anchor tab exclusive for left/right/others (Paseo's
// slicing in workspace-screen.tsx), and a multi-selection intersected with
// what is on screen.

import type { Session } from "../../types/ipc";

export type TabCloseAction = "close" | "left" | "right" | "others";

export function sessionsForTabAction(
  action: TabCloseAction,
  sessions: readonly Session[],
  anchorId: string,
): Session[] {
  const index = sessions.findIndex((session) => session.id === anchorId);
  if (index === -1) return [];
  if (action === "left") return [...sessions.slice(0, index)];
  if (action === "right") return [...sessions.slice(index + 1)];
  if (action === "others") return sessions.filter((session) => session.id !== anchorId);
  return [sessions[index]];
}

export function sessionsForSelection(
  selection: ReadonlySet<string>,
  sessions: readonly Session[],
): Session[] {
  return sessions.filter((session) => selection.has(session.id));
}
