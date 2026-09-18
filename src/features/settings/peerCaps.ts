// Next capability array for a toggle, preserving names the table never heard of.
import type { Cap } from "../../types/ipc";

export function nextCaps(
  current: readonly Cap[],
  toggled: Cap,
  next: boolean,
  order: readonly Cap[],
): Cap[] {
  const held = new Set<Cap>(current);
  if (next) held.add(toggled);
  else held.delete(toggled);
  const known = order.filter((candidate) => held.has(candidate));
  const unknown = current.filter((candidate) => !order.includes(candidate) && held.has(candidate));
  if (next && !order.includes(toggled) && !current.includes(toggled)) unknown.push(toggled);
  return [...known, ...unknown];
}
