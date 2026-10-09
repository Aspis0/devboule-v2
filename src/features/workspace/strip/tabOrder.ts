// Why: the strip used to be two lists glued together — sessions, then tool
// tabs — so a browser tab could never sit beside an agent. This keeps one
// ordered list of tab ids per workspace across every kind. The remembered order
// is applied over what the workspace composes, so a tab the order does not name
// yet lands at the end, where it always did. A recovered session is a new id
// whose predecessor is the id the order remembers: it takes that place.

/** One chip as the drop reads it: its id and the horizontal box it occupies. */
export interface TabSlot {
  id: string;
  left: number;
  right: number;
}

/**
 * The remembered order over the composed tabs. A tab is ranked by its own id;
 * a tab whose id is not remembered takes the place of its predecessor when the
 * predecessor is no longer open, and only the first such tab claims it.
 */
export function orderStripTabs<T extends { id: string }>(
  tabs: T[],
  order: readonly string[],
  predecessorOf: (tab: T) => string | null = () => null,
): T[] {
  if (order.length === 0) return tabs;
  const rank = new Map(order.map((id, index) => [id, index]));
  const open = new Set(tabs.map((tab) => tab.id));
  const claimed = new Set<number>();
  const ranked: Array<{ tab: T; rank: number }> = [];
  const pending: T[] = [];
  for (const tab of tabs) {
    const own = rank.get(tab.id);
    if (own === undefined) {
      pending.push(tab);
      continue;
    }
    claimed.add(own);
    ranked.push({ tab, rank: own });
  }
  const unranked: T[] = [];
  for (const tab of pending) {
    const predecessor = predecessorOf(tab);
    const place = predecessor === null || open.has(predecessor) ? undefined : rank.get(predecessor);
    if (place === undefined || claimed.has(place)) {
      unranked.push(tab);
      continue;
    }
    claimed.add(place);
    ranked.push({ tab, rank: place });
  }
  ranked.sort((left, right) => left.rank - right.rank);
  return [...ranked.map((entry) => entry.tab), ...unranked];
}

/** The same ids with `movedId` taken out and put back at `index`, clamped to
 * the list. The index counts the list without the moved id. */
export function moveTabId(ids: readonly string[], movedId: string, index: number): string[] {
  const rest = ids.filter((id) => id !== movedId);
  const at = Math.min(Math.max(index, 0), rest.length);
  return [...rest.slice(0, at), movedId, ...rest.slice(at)];
}

/** How many chips other than the dragged one lie left of the pointer. Chips are
 * read by their middle, so the drop lands on the side the pointer is on. */
export function insertionIndex(slots: readonly TabSlot[], x: number, movedId: string): number {
  let index = 0;
  for (const slot of slots) {
    if (slot.id === movedId) continue;
    if ((slot.left + slot.right) / 2 < x) index += 1;
  }
  return index;
}

/** The x of the gap a dragged chip would land in, given the index insertionIndex
 * chose: the edge of the chip before it, or the first chip's left edge at zero. */
export function insertionEdge(
  slots: readonly TabSlot[],
  movedId: string,
  index: number,
): number | null {
  const rest = slots.filter((slot) => slot.id !== movedId);
  if (rest.length === 0) return null;
  if (index <= 0) return rest[0]!.left;
  return rest[Math.min(index, rest.length) - 1]!.right;
}
