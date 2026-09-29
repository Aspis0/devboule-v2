// Why: what a tool tab's own menu offers. A tool tab has no session behind
// it, so its menu is close-only — never rename, never delete.

import { buildToolTabCloseEntries, type TabMenuEntry } from "./tabCloseMenu";
import type { StripTab } from "./toolTabs";

/** The close entries for a tool anchor, or null when the anchor is not one. */
export function toolTabMenuEntries(
  tabs: readonly StripTab[],
  anchorId: string,
): TabMenuEntry[] | null {
  const index = tabs.findIndex((tab) => tab.id === anchorId);
  if (index === -1) return null;
  if (tabs[index]?.type !== "tool") return null;
  return buildToolTabCloseEntries(index, tabs.length);
}
