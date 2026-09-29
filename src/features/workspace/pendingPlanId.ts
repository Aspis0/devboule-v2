import type { PermissionRequest } from "../../types/ipc";

/** The one permission card a pane can render: Workspace's queue item as the
 * render reads it. */
export interface RenderedPermissionCard {
  request: PermissionRequest;
  resolution?: unknown;
}

/**
 * The timeline plan row that may stand down: the id of the card the pane
 * actually renders, and only while that card is an unanswered plan. A plan
 * whose card waits behind another card keeps its row — a hidden plan with no
 * card on screen is worse than a duplicate. An id that is not a non-empty
 * string never hides anything.
 */
export function pendingPlanId(card: RenderedPermissionCard | null): string | null {
  if (card === null || card.resolution !== undefined) return null;
  if (card.request.kind !== "plan") return null;
  const id = card.request.toolCallId;
  return typeof id === "string" && id !== "" ? id : null;
}
