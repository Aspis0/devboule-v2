// Why: the strip used to be two lists glued together — sessions, then tool
// tabs — so a browser tab could never sit beside an agent. This keeps one
// ordered list of tab ids per workspace across every kind. The remembered order
// is applied over what the workspace composes, so a tab the order does not name
// yet (or a restored session with a new id) lands at the end, where it always did.

/** One chip as the drop reads it: its id and the horizontal box it occupies. */
export interface TabSlot {
  id: string;
  left: number;
  right: number;
}

export function orderStripTabs<T extends { id: string }>(tabs: T[], order: readonly string[]): T[] {
  if (order.length === 0) return tabs;
  const rank = new Map(order.map((id, index) => [id, index]));
  const ranked = tabs.filter((tab) => rank.has(tab.id));
  const unranked = tabs.filter((tab) => !rank.has(tab.id));
  ranked.sort((left, right) => (rank.get(left.id) ?? 0) - (rank.get(right.id) ?? 0));
  return [...ranked, ...unranked];
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
