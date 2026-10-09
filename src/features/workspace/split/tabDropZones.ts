// Why: the drop zones are copied from Paseo's `split-drop-zone.tsx` — a centred
// 40% square wins, then 15% edge bands, then the nearest edge — with the four
// edges collapsed onto the two this layout can make. The numbers are that
// file's: EDGE_RATIO 0.15, CENTER_RATIO 0.4.

/** What the pane below may hold: one browser tool tab's id, which is what the
 * split record stores and what `splitPaneStorage.ts` will read back. Nothing
 * else can go below — a conversation has no lower pane to render, and a diff or
 * a file tab's record does not survive a restart. */
export const LOWER_PANE_TAB_PREFIX = "tool:browser:";

export function tabCanGoBelow(tabId: string): boolean {
  return tabId.startsWith(LOWER_PANE_TAB_PREFIX);
}

/** A band along one edge of the pane, as a share of its width or height. */
export const EDGE_RATIO = 0.15;

/** The centred square that means "where it already is", as a share of each axis. */
export const CENTER_RATIO = 0.4;

/**
 * Where a tab dropped over the workspace centre would land.
 *
 * `left` and `right` are not a split this layout can make — it has one
 * horizontal divider — so a vertical edge resolves to the nearer horizontal
 * band, which is the split that edge was reaching across to.
 */
export type DropZone = "center" | "top" | "bottom";

/** What the outcome of a drop is, decided before anything moves. */
export type DropOutcome =
  /** The dragged tab goes into the pane below. */
  | { kind: "split-down" }
  /** The dragged tab becomes the pane above and the tab that was there moves down. */
  | { kind: "split-up" }
  /** The dragged tab leaves the pane below and the workspace is one pane again. */
  | { kind: "merge" }
  /** The dragged tab moves to the place on the tab row the pointer is over. */
  | { kind: "reorder" }
  /** A plain selection: the tab is in front, nothing structural changes. */
  | { kind: "select" };

/** The centre a drop is read against. `x` and `y` are pointer coordinates
 * relative to its own box. */
export interface DropArea {
  width: number;
  height: number;
  x: number;
  y: number;
}

/** The rule as copied: the centred square first, then the edge bands in the
 * order left, right, top, bottom, then the nearest edge by distance. */
export function resolveDropZone(area: DropArea): DropZone {
  const insetX = area.width * ((1 - CENTER_RATIO) / 2);
  const insetY = area.height * ((1 - CENTER_RATIO) / 2);
  const insideCenterX = area.x >= insetX && area.x <= area.width - insetX;
  const insideCenterY = area.y >= insetY && area.y <= area.height - insetY;
  if (insideCenterX && insideCenterY) return "center";

  const bandX = area.width * EDGE_RATIO;
  const bandY = area.height * EDGE_RATIO;
  const edge =
    area.x <= bandX
      ? "left"
      : area.x >= area.width - bandX
        ? "right"
        : area.y <= bandY
          ? "top"
          : area.y >= area.height - bandY
            ? "bottom"
            : nearestEdge(area);
  // The nearest edge, resolved to the split this layout can make.
  if (edge === "top" || edge === "bottom") return edge;
  return area.y <= area.height / 2 ? "top" : "bottom";
}

function nearestEdge(area: DropArea): "left" | "right" | "top" | "bottom" {
  const distances = [
    { edge: "left" as const, distance: area.x },
    { edge: "right" as const, distance: area.width - area.x },
    { edge: "top" as const, distance: area.y },
    { edge: "bottom" as const, distance: area.height - area.y },
  ];
  distances.sort((left, right) => left.distance - right.distance);
  return distances[0]?.edge ?? "bottom";
}

/** What the layout looked like when the pointer was let go. */
export interface DropContext {
  /** `strip` is a drop on the tab row itself, which is where a tab goes back to. */
  zone: DropZone | "strip";
  draggedTabId: string;
  /** The tab in the pane below, or null while the workspace is one pane. */
  lowerTabId: string | null;
  /** The tab the pane above shows, or null while it shows the empty state. */
  upperTabId: string | null;
  /** Whether the pane above's tab is one the pane below can take: a browser tab
   * is, a conversation is not, because this model's lower pane holds a page. */
  upperCanMoveBelow: boolean;
}

/**
 * What a drop does.
 *
 * Two drops are refused by this layout rather than by the rule's zones: a tab
 * the pane below cannot hold goes nowhere but the front, and a drop on the top
 * edge that would have to move a conversation downward is a selection too. Both
 * say no by doing the smaller thing, so nothing is destroyed and no record is
 * written that the layout cannot honour.
 */
export function resolveDropOutcome(context: DropContext): DropOutcome {
  const { draggedTabId, lowerTabId, upperTabId, upperCanMoveBelow, zone } = context;
  if (zone === "strip") {
    return lowerTabId === draggedTabId ? { kind: "merge" } : { kind: "reorder" };
  }
  if (zone === "bottom") {
    // A tab the pane below cannot show is a selection, not a split that would
    // be merged away again a frame later.
    return tabCanGoBelow(draggedTabId) ? { kind: "split-down" } : { kind: "select" };
  }
  if (zone === "center") return { kind: "select" };
  if (lowerTabId === draggedTabId) return { kind: "merge" };
  // Only a page can take the pane above, so only a page may be the one dropped there.
  if (
    upperCanMoveBelow &&
    upperTabId !== null &&
    upperTabId !== draggedTabId &&
    tabCanGoBelow(draggedTabId)
  ) {
    return { kind: "split-up" };
  }
  return { kind: "select" };
}
